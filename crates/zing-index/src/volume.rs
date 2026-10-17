//! 磁碟區的原始存取：循序讀取 MFT、讀取單筆記錄、變更日誌（USN Journal）。都需要系統管理員權限。

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_JOURNAL_DELETE_IN_PROGRESS, ERROR_JOURNAL_ENTRY_DELETED, ERROR_JOURNAL_NOT_ACTIVE, GENERIC_READ,
    HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetVolumeInformationW, OPEN_EXISTING, ReadFile,
};
use windows::Win32::System::IO::{DeviceIoControl, OVERLAPPED};
use windows::Win32::System::Ioctl::{
    CREATE_USN_JOURNAL_DATA, FSCTL_CREATE_USN_JOURNAL, FSCTL_GET_NTFS_FILE_RECORD, FSCTL_GET_NTFS_VOLUME_DATA,
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, NTFS_FILE_RECORD_INPUT_BUFFER, NTFS_VOLUME_DATA_BUFFER,
    READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
};
use windows::core::PCWSTR;
use zing_core::ntfs::{self, Record, Run};
use zing_core::text::wide_z;
use zing_core::volindex::VolumeIndex;

pub struct Volume {
    handle: HANDLE,
    pub cluster: u64,
    pub record: usize,
    sector: usize,
    mft_valid: u64,
    mft_lcn: u64,
}

// 磁碟區控制代碼可在執行緒間使用；讀取以 OVERLAPPED 指定位移，不共用檔案指標
unsafe impl Send for Volume {}
unsafe impl Sync for Volume {}

impl Drop for Volume {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// 變更日誌的位置
#[derive(Clone, Copy, Debug)]
pub struct Journal {
    pub id: u64,
    pub first: i64,
    pub next: i64,
}

#[derive(Debug)]
pub enum JournalError {
    /// 日誌被刪除、重建或已覆蓋要讀的位置：需要重新掃描
    Reset,
    Other(String),
}

fn err(e: windows::core::Error) -> String {
    format!("{}（0x{:08X}）", e.message(), e.code().0 as u32)
}

/// 磁碟區的檔案系統名稱與序號
pub fn volume_info(root: &str) -> Option<(String, u32)> {
    let mut fs = [0u16; 32];
    let mut serial = 0u32;
    let w = wide_z(root);
    unsafe { GetVolumeInformationW(PCWSTR(w.as_ptr()), None, Some(&mut serial), None, None, Some(&mut fs)) }.ok()?;
    let len = fs.iter().position(|&c| c == 0).unwrap_or(fs.len());
    Some((String::from_utf16_lossy(&fs[..len]), serial))
}

impl Volume {
    /// 開啟磁碟區（`letter` 如 'C'）；不是 NTFS 時回傳錯誤
    pub fn open(letter: char) -> Result<Volume, String> {
        let root = format!("{letter}:\\");
        let (fs, _) = volume_info(&root).ok_or(tr!("無法讀取磁碟區資訊"))?;
        if fs != "NTFS" {
            return Err(tr!("檔案系統是 {fs}，不是 NTFS", fs));
        }
        let path = wide_z(&format!(r"\\.\{letter}:"));
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(err)?;
        let mut v = Volume { handle, cluster: 0, record: 0, sector: 0, mft_valid: 0, mft_lcn: 0 };
        let mut data = NTFS_VOLUME_DATA_BUFFER::default();
        unsafe {
            DeviceIoControl(
                handle,
                FSCTL_GET_NTFS_VOLUME_DATA,
                None,
                0,
                Some(&mut data as *mut _ as *mut _),
                size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
                None,
                None,
            )
        }
        .map_err(err)?;
        v.cluster = data.BytesPerCluster as u64;
        v.record = data.BytesPerFileRecordSegment as usize;
        v.sector = data.BytesPerSector as usize;
        v.mft_valid = data.MftValidDataLength as u64;
        v.mft_lcn = data.MftStartLcn as u64;
        if v.cluster == 0 || v.record < 256 || v.sector < 256 || !v.record.is_multiple_of(v.sector) {
            return Err(tr!("磁碟區參數不合理").into());
        }
        Ok(v)
    }

    /// MFT 的記錄數
    pub fn record_count(&self) -> usize {
        (self.mft_valid / self.record as u64) as usize
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        let mut ov = OVERLAPPED::default();
        ov.Anonymous.Anonymous.Offset = offset as u32;
        ov.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        let mut n = 0u32;
        unsafe { ReadFile(self.handle, Some(buf), Some(&mut n), Some(&mut ov)) }.map_err(err)?;
        if n as usize != buf.len() {
            return Err(tr!("讀取不完整（{n} / {total}）", n, total = buf.len()));
        }
        Ok(())
    }

    /// 以 FSCTL_GET_NTFS_FILE_RECORD 讀取一筆記錄（已還原更新序列）；記錄未使用時回傳 None
    pub fn get_record(&self, frn: u64) -> Option<Vec<u8>> {
        let input = NTFS_FILE_RECORD_INPUT_BUFFER { FileReferenceNumber: frn as i64 };
        let mut out = vec![0u8; 16 + self.record];
        let mut n = 0u32;
        unsafe {
            DeviceIoControl(
                self.handle,
                FSCTL_GET_NTFS_FILE_RECORD,
                Some(&input as *const _ as *const _),
                size_of::<NTFS_FILE_RECORD_INPUT_BUFFER>() as u32,
                Some(out.as_mut_ptr() as *mut _),
                out.len() as u32,
                Some(&mut n),
                None,
            )
        }
        .ok()?;
        // 要求的記錄未使用時，會傳回編號較小的最近一筆使用中記錄
        let got = u64::from_le_bytes(out[0..8].try_into().ok()?) & ntfs::FRN_MASK;
        if got != frn {
            return None;
        }
        let len = u32::from_le_bytes(out[8..12].try_into().ok()?) as usize;
        let mut rec = out.get(12..12 + len)?.to_vec();
        ntfs::apply_fixup(&mut rec, self.sector, false);
        Some(rec)
    }

    /// 讀取一個項目目前的狀態（併入延伸記錄）；已刪除時回傳 None
    pub fn read_full(&self, frn: u64) -> Option<Record> {
        let mut r = ntfs::parse_record(&self.get_record(frn)?, frn)?;
        if r.base != 0 {
            return None;
        }
        for e in r.extensions.clone() {
            if let Some(x) = self.get_record(e).and_then(|b| ntfs::parse_record(&b, e)) {
                r.merge(&x);
            }
        }
        Some(r)
    }

    /// $MFT 自己的資料區段（依 VCN 順序）
    fn mft_runs(&self) -> Result<Vec<Run>, String> {
        let mut rec = vec![0u8; self.record];
        self.read_at(self.mft_lcn * self.cluster, &mut rec)?;
        if !ntfs::apply_fixup(&mut rec, self.sector, true) {
            return Err(tr!("$MFT 記錄損毀").into());
        }
        let mut pieces = ntfs::data_runs(&rec);
        let needed = self.mft_valid.div_ceil(self.cluster);
        let covered: u64 = pieces.iter().flat_map(|p| &p.1).map(|r| r.clusters).sum();
        if covered < needed {
            // $MFT 非常零碎：其餘區段放在屬性清單指向的延伸記錄
            let base = ntfs::parse_record(&rec, 0).ok_or(tr!("$MFT 記錄無法解析"))?;
            for e in base.extensions {
                if let Some(b) = self.get_record(e) {
                    pieces.extend(ntfs::data_runs(&b));
                }
            }
        }
        pieces.sort_by_key(|p| p.0);
        let runs: Vec<Run> = pieces.into_iter().flat_map(|p| p.1).collect();
        if runs.iter().map(|r| r.clusters).sum::<u64>() < needed {
            return Err(tr!("找不到完整的 $MFT 位置").into());
        }
        Ok(runs)
    }

    /// 循序讀取整個 MFT 建立索引；`progress` 收到 0–100。取消時回傳錯誤。
    pub fn scan(&self, epoch: u64, cancel: &AtomicBool, mut progress: impl FnMut(u8)) -> Result<VolumeIndex, String> {
        let runs = self.mft_runs()?;
        let total = self.record_count();
        let mut idx = VolumeIndex::new(total, epoch);
        let chunk_clusters = ((8u64 << 20) / self.cluster).max(1);
        let mut buf = vec![0u8; (chunk_clusters * self.cluster) as usize];
        // 叢集比記錄小時，一筆記錄可能跨兩個區段：未滿一筆的位元組留到下一段
        let mut carry: Vec<u8> = Vec::new();
        let mut frn = 0u64;
        let mut last_pct = u8::MAX;
        'runs: for run in runs {
            let mut lcn = run.lcn;
            let mut left = run.clusters;
            while left > 0 {
                if frn as usize >= total {
                    break 'runs;
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err(tr!("已取消").into());
                }
                let n = left.min(chunk_clusters);
                let bytes = (n * self.cluster) as usize;
                match lcn {
                    Some(l) => self.read_at(l * self.cluster, &mut buf[..bytes])?,
                    // 稀疏區段（理論上不會出現在 $MFT）：當作空白記錄
                    None => buf[..bytes].fill(0),
                }
                carry.extend_from_slice(&buf[..bytes]);
                let whole = carry.len() / self.record * self.record;
                for rec in carry[..whole].chunks_exact_mut(self.record) {
                    if frn as usize >= total {
                        break;
                    }
                    if ntfs::apply_fixup(rec, self.sector, true)
                        && let Some(r) = ntfs::parse_record(rec, frn)
                    {
                        idx.add(&r);
                    }
                    frn += 1;
                }
                carry.drain(..whole);
                left -= n;
                lcn = lcn.map(|l| l + n);
                let pct = (frn * 100 / total.max(1) as u64).min(99) as u8;
                if pct != last_pct {
                    last_pct = pct;
                    progress(pct);
                }
            }
        }
        idx.finish();
        progress(100);
        Ok(idx)
    }

    /// 查詢變更日誌；沒有啟用時建立（與 Everything 相同：32 MB，每次擴充 8 MB）
    pub fn journal(&self) -> Result<Journal, String> {
        let mut data = USN_JOURNAL_DATA_V0::default();
        let query = |data: &mut USN_JOURNAL_DATA_V0| unsafe {
            DeviceIoControl(
                self.handle,
                FSCTL_QUERY_USN_JOURNAL,
                None,
                0,
                Some(data as *mut _ as *mut _),
                size_of::<USN_JOURNAL_DATA_V0>() as u32,
                None,
                None,
            )
        };
        if let Err(e) = query(&mut data) {
            if e.code() != ERROR_JOURNAL_NOT_ACTIVE.to_hresult() {
                return Err(err(e));
            }
            let create = CREATE_USN_JOURNAL_DATA { MaximumSize: 32 << 20, AllocationDelta: 8 << 20 };
            unsafe {
                DeviceIoControl(
                    self.handle,
                    FSCTL_CREATE_USN_JOURNAL,
                    Some(&create as *const _ as *const _),
                    size_of::<CREATE_USN_JOURNAL_DATA>() as u32,
                    None,
                    0,
                    None,
                    None,
                )
            }
            .map_err(err)?;
            query(&mut data).map_err(err)?;
        }
        Ok(Journal { id: data.UsnJournalID, first: data.FirstUsn, next: data.NextUsn })
    }

    /// 從 `start` 讀取變更日誌，依序把變動的記錄編號放進 `out`（最多約 `limit` 筆），回傳下一個位置
    pub fn read_journal(&self, id: u64, start: i64, out: &mut Vec<u64>, limit: usize) -> Result<i64, JournalError> {
        let mut buf = vec![0u8; 256 << 10];
        let mut usn = start;
        while out.len() < limit {
            let input = READ_USN_JOURNAL_DATA_V0 {
                StartUsn: usn,
                ReasonMask: u32::MAX,
                ReturnOnlyOnClose: 0,
                Timeout: 0,
                BytesToWaitFor: 0,
                UsnJournalID: id,
            };
            let mut n = 0u32;
            let r = unsafe {
                DeviceIoControl(
                    self.handle,
                    FSCTL_READ_USN_JOURNAL,
                    Some(&input as *const _ as *const _),
                    size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                    Some(buf.as_mut_ptr() as *mut _),
                    buf.len() as u32,
                    Some(&mut n),
                    None,
                )
            };
            if let Err(e) = r {
                let c = e.code();
                if c == ERROR_JOURNAL_ENTRY_DELETED.to_hresult()
                    || c == ERROR_JOURNAL_NOT_ACTIVE.to_hresult()
                    || c == ERROR_JOURNAL_DELETE_IN_PROGRESS.to_hresult()
                {
                    return Err(JournalError::Reset);
                }
                return Err(JournalError::Other(err(e)));
            }
            let n = n as usize;
            if n < 8 {
                break;
            }
            let next = i64::from_le_bytes(buf[0..8].try_into().unwrap());
            let mut o = 8;
            while o + 16 <= n {
                let len = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) as usize;
                if len < 16 || o + len > n {
                    break;
                }
                let major = u16::from_le_bytes(buf[o + 4..o + 6].try_into().unwrap());
                // NTFS 預設回傳第 2 版記錄（64 位元檔案參照）
                if major == 2 {
                    out.push(u64::from_le_bytes(buf[o + 8..o + 16].try_into().unwrap()) & ntfs::FRN_MASK);
                }
                o += len;
            }
            if next == usn {
                break;
            }
            usn = next;
            if n == 8 {
                break;
            }
        }
        Ok(usn)
    }
}
