//! 壓縮檔當資料夾瀏覽：路徑寫成 `D:\下載\專案.zip\src\main.rs`（與檔案總管相同）。
//!
//! zip 自己讀結尾的中央目錄（不解壓縮、不加相依套件，十萬個項目也很快）；其他格式（7z、rar、tar…）由 zing-app 經
//! Shell 的命名空間列舉（Windows 內建的壓縮資料夾）。開啟、複製、拖出裡面的項目都交給 Shell（它認得這種路徑）。
//! 唯讀：不能在壓縮檔裡新增、刪除、改名。

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::enumerate::EnumError;
use crate::listing::{ATTR_DIRECTORY, Listing};

/// Shell 能當資料夾瀏覽的壓縮檔（Windows 11 的壓縮資料夾支援的格式）
const EXTS: &[&str] = &[".zip", ".7z", ".rar", ".tar", ".tgz", ".tar.gz", ".tbz2", ".tar.bz2", ".txz", ".tar.xz", ".tar.zst", ".cab"];

/// 檔名是可以進入瀏覽的壓縮檔（只看名稱）
pub fn is_archive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    EXTS.iter().any(|e| lower.len() > e.len() && lower.ends_with(e))
}

pub fn is_zip(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".zip")
}

/// 路徑在壓縮檔裡（含壓縮檔本身）時，可能的（壓縮檔, 裡面的路徑）分法，由外到內。只看名稱：名稱像壓縮檔的
/// 資料夾也會列出，呼叫端要確認壓縮檔的部分是檔案（`D:\a.zip` 是資料夾、裡面還有 `b.zip` 時，第二個才對）
pub fn splits(path: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut start = 0;
    std::iter::from_fn(move || {
        while start < path.len() {
            let end = path[start..].find('\\').map_or(path.len(), |i| start + i);
            let name = &path[start..end];
            start = end + 1;
            // 第一段是磁碟機（C:）或 UNC 的伺服器名稱，不會是壓縮檔
            if end > name.len() && is_archive_name(name) {
                let inner = path.get(end + 1..).unwrap_or("");
                return Some((&path[..end], inner.trim_end_matches('\\')));
            }
        }
        None
    })
}

/// 路徑在壓縮檔裡（只看名稱）：唯讀、不監看
pub fn is_inside(path: &str) -> bool {
    splits(path).next().is_some()
}

/// zip 的一個項目（路徑以 `\` 分隔，沒有開頭或結尾的分隔符號）
#[derive(Debug, Clone, PartialEq)]
pub struct ZipEntry {
    pub path: String,
    pub size: u64,
    /// FILETIME（UTC）
    pub modified: u64,
    pub dir: bool,
}

/// zip 的中央目錄
#[derive(Debug, Default)]
pub struct ZipIndex {
    pub entries: Vec<ZipEntry>,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

fn corrupt() -> EnumError {
    EnumError::Other(tr!("壓縮檔損毀或不是 zip 格式").to_string())
}

impl ZipIndex {
    /// 讀取 zip 檔的中央目錄
    pub fn read(path: &str) -> Result<Self, EnumError> {
        crate::guard::io("讀取 zip 的目錄");
        let mut f = std::fs::File::open(crate::path::long_form(path)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => EnumError::NotFound,
            std::io::ErrorKind::PermissionDenied => EnumError::AccessDenied,
            _ => EnumError::Other(e.to_string()),
        })?;
        Self::parse(&mut f)
    }

    pub fn parse<R: Read + Seek>(r: &mut R) -> Result<Self, EnumError> {
        let io = |e: std::io::Error| EnumError::Other(e.to_string());
        let len = r.seek(SeekFrom::End(0)).map_err(io)?;
        // 結尾記錄（22 位元組）＋最長 65535 的註解
        let tail_len = len.min(22 + 65535) as usize;
        let tail_start = len - tail_len as u64;
        let mut tail = vec![0u8; tail_len];
        r.seek(SeekFrom::Start(tail_start)).map_err(io)?;
        r.read_exact(&mut tail).map_err(io)?;
        let eocd = (0..tail_len.saturating_sub(21))
            .rev()
            .find(|&i| u32_at(&tail, i) == 0x0605_4b50 && i + 22 + u16_at(&tail, i + 20) as usize <= tail_len)
            .ok_or_else(corrupt)?;
        let mut count = u16_at(&tail, eocd + 10) as u64;
        let mut cd_size = u32_at(&tail, eocd + 12) as u64;
        let mut cd_offset = u32_at(&tail, eocd + 16) as u64;
        // ZIP64：結尾記錄前面是 ZIP64 結尾記錄的位置
        if (count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF) && eocd >= 20 && u32_at(&tail, eocd - 20) == 0x0706_4b50 {
            let at = u64_at(&tail, eocd - 20 + 8);
            let mut rec = [0u8; 56];
            r.seek(SeekFrom::Start(at)).map_err(io)?;
            r.read_exact(&mut rec).map_err(io)?;
            if u32_at(&rec, 0) != 0x0606_4b50 {
                return Err(corrupt());
            }
            count = u64_at(&rec, 32);
            cd_size = u64_at(&rec, 40);
            cd_offset = u64_at(&rec, 48);
        }
        if cd_size > len {
            return Err(corrupt());
        }
        let mut cd = vec![0u8; cd_size as usize];
        r.seek(SeekFrom::Start(cd_offset)).map_err(io)?;
        let mut ok = r.read_exact(&mut cd).is_ok() && cd.len() >= 4 && u32_at(&cd, 0) == 0x0201_4b50;
        if !ok && cd_size > 0 {
            // 前面接了其他資料（自解壓縮檔）：中央目錄緊接在結尾記錄之前
            let at = (tail_start + eocd as u64).checked_sub(cd_size).ok_or_else(corrupt)?;
            r.seek(SeekFrom::Start(at)).map_err(io)?;
            ok = r.read_exact(&mut cd).is_ok() && u32_at(&cd, 0) == 0x0201_4b50;
        }
        if !ok && cd_size > 0 {
            return Err(corrupt());
        }
        let mut entries = Vec::with_capacity(count.min(1 << 20) as usize);
        let mut i = 0;
        while i + 46 <= cd.len() && u32_at(&cd, i) == 0x0201_4b50 {
            let flags = u16_at(&cd, i + 8);
            let (time, date) = (u16_at(&cd, i + 12), u16_at(&cd, i + 14));
            let mut size = u32_at(&cd, i + 24) as u64;
            let (name_len, extra_len, comment_len) = (u16_at(&cd, i + 28) as usize, u16_at(&cd, i + 30) as usize, u16_at(&cd, i + 32) as usize);
            let ext_attr = u32_at(&cd, i + 38);
            let name_at = i + 46;
            let end = name_at + name_len + extra_len + comment_len;
            if end > cd.len() {
                return Err(corrupt());
            }
            let raw = &cd[name_at..name_at + name_len];
            let mut name = if flags & 0x800 != 0 { String::from_utf8_lossy(raw).into_owned() } else { oem_decode(raw) };
            let mut modified = None;
            let mut e = name_at + name_len;
            let extra_end = e + extra_len;
            while e + 4 <= extra_end {
                let (id, n) = (u16_at(&cd, e), u16_at(&cd, e + 2) as usize);
                let body = &cd[e + 4..(e + 4 + n).min(extra_end)];
                match id {
                    // ZIP64：值為 0xFFFFFFFF 的欄位依序放在這裡，第一個是原始大小
                    0x0001 if size == 0xFFFF_FFFF && body.len() >= 8 => size = u64_at(body, 0),
                    // NTFS：修改時間（UTC 的 FILETIME）
                    0x000a if body.len() >= 32 && u16_at(body, 4) == 1 => modified = Some(u64_at(body, 8)),
                    // 延伸時間戳記：修改時間（Unix 秒）
                    0x5455 if body.len() >= 5 && body[0] & 1 != 0 && modified.is_none() => {
                        modified = Some((u32_at(body, 1) as u64 + 11_644_473_600) * 10_000_000)
                    }
                    // Info-ZIP 的 Unicode 路徑
                    0x7075 if body.len() > 5 && body[0] == 1 => name = String::from_utf8_lossy(&body[5..]).into_owned(),
                    _ => {}
                }
                e += 4 + n;
            }
            let dir = name.ends_with('/') || name.ends_with('\\') || ext_attr & 0x10 != 0;
            if let Some(path) = clean(&name) {
                entries.push(ZipEntry { path, size: if dir { 0 } else { size }, modified: modified.unwrap_or_else(|| dos_time(date, time)), dir });
            }
            i = end;
        }
        Ok(Self { entries })
    }

    /// 列出 `inner`（壓縮檔裡的資料夾，`""` 是最上層）直接包含的項目；只列出檔案所在的資料夾也補上
    pub fn list(&self, inner: &str, dirs_only: bool, cancel: &AtomicBool) -> Result<Listing, EnumError> {
        let inner = inner.trim_matches('\\');
        let mut out = Listing::default();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut found = inner.is_empty();
        for (k, e) in self.entries.iter().enumerate() {
            if k % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                return Err(EnumError::Cancelled);
            }
            let rest = if inner.is_empty() {
                e.path.as_str()
            } else {
                match e.path.get(..inner.len()) {
                    Some(head) if head.eq_ignore_ascii_case(inner) && e.path.len() == inner.len() => {
                        found |= e.dir;
                        continue;
                    }
                    Some(head) if head.eq_ignore_ascii_case(inner) && e.path.as_bytes()[inner.len()] == b'\\' => &e.path[inner.len() + 1..],
                    _ => continue,
                }
            };
            found = true;
            let (name, deeper) = match rest.find('\\') {
                Some(i) => (&rest[..i], true),
                None => (rest, false),
            };
            let is_dir = deeper || e.dir;
            if (dirs_only && !is_dir) || !seen.insert(name.to_lowercase()) {
                continue;
            }
            if is_dir {
                // 只由更深的項目推得的資料夾沒有時間；明確的資料夾項目稍後出現時不重複
                let modified = if deeper { 0 } else { e.modified };
                out.push_str(name, ATTR_DIRECTORY, 0, modified);
            } else {
                out.push_str(name, 0, e.size, e.modified);
            }
        }
        if !found {
            return Err(EnumError::NotFound);
        }
        Ok(out)
    }
}

/// 項目名稱 → 以 `\` 分隔的路徑；去掉 `.`、空的段落，含 `..` 的不列出
fn clean(name: &str) -> Option<String> {
    let mut parts = Vec::new();
    for p in name.split(['/', '\\']) {
        match p {
            "" | "." => {}
            ".." => return None,
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("\\"))
}

/// 沒有 UTF-8 標記的名稱：與檔案總管相同，以 OEM 字碼頁解讀
fn oem_decode(raw: &[u8]) -> String {
    if raw.is_ascii() {
        return String::from_utf8_lossy(raw).into_owned();
    }
    use windows::Win32::Globalization::{CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), raw, None);
        let mut w = vec![0u16; n.max(0) as usize];
        MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), raw, Some(&mut w));
        String::from_utf16_lossy(&w)
    }
}

/// DOS 的日期時間（當地時間）→ FILETIME（UTC）
fn dos_time(date: u16, time: u16) -> u64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
    let local = SYSTEMTIME {
        wYear: 1980 + (date >> 9),
        wMonth: ((date >> 5) & 0xF).max(1),
        wDay: (date & 0x1F).max(1),
        wHour: time >> 11,
        wMinute: (time >> 5) & 0x3F,
        wSecond: (time & 0x1F) * 2,
        ..Default::default()
    };
    let (mut utc, mut ft) = (SYSTEMTIME::default(), FILETIME::default());
    unsafe {
        if TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).is_err() || SystemTimeToFileTime(&utc, &mut ft).is_err() {
            return 0;
        }
    }
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Cursor;

    /// 測試用：只有中央目錄與結尾記錄的 zip（列出不需要檔案內容）。(名稱, 大小, 是否 UTF-8, 額外欄位)
    pub fn zip_bytes(items: &[(&[u8], u32, bool, &[u8])], prefix: &[u8]) -> Vec<u8> {
        let mut out = prefix.to_vec();
        // 自解壓縮檔的位移不含前面的資料
        let cd_offset = (out.len() - prefix.len()) as u32;
        for (name, size, utf8, extra) in items {
            out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            out.extend_from_slice(&[20, 0, 20, 0]);
            out.extend_from_slice(&(if *utf8 { 0x800u16 } else { 0 }).to_le_bytes());
            out.extend_from_slice(&[0, 0]); // 方法
            out.extend_from_slice(&0x6000u16.to_le_bytes()); // 12:00:00
            out.extend_from_slice(&((44 << 9 | 3 << 5 | 15) as u16).to_le_bytes()); // 2024-03-15
            out.extend_from_slice(&[0; 4]); // CRC
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&(extra.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0; 2 + 2 + 2 + 4 + 4]);
            out.extend_from_slice(name);
            out.extend_from_slice(extra);
        }
        let cd_size = (out.len() - prefix.len()) as u32 - cd_offset;
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(items.len() as u16).to_le_bytes());
        out.extend_from_slice(&(items.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(b"abc");
        out
    }

    fn names(l: &Listing) -> Vec<(String, bool)> {
        let mut v: Vec<(String, bool)> = (0..l.len()).map(|i| (l.name_string(i), l.entries[i].is_dir())).collect();
        v.sort();
        v
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn archive_names_and_splits() {
        assert!(is_archive_name("專案.ZIP"));
        assert!(is_archive_name("a.tar.gz"));
        assert!(!is_archive_name("a.gz"));
        assert!(!is_archive_name(".zip"));
        assert!(!is_archive_name("a.docx"));
        assert_eq!(splits(r"D:\下載\專案.zip\src\a.rs").collect::<Vec<_>>(), [(r"D:\下載\專案.zip", r"src\a.rs")]);
        assert_eq!(splits(r"D:\a.zip").collect::<Vec<_>>(), [(r"D:\a.zip", "")]);
        assert_eq!(splits(r"D:\a.zip\b.7z\c").collect::<Vec<_>>(), [(r"D:\a.zip", r"b.7z\c"), (r"D:\a.zip\b.7z", "c")]);
        assert!(!is_inside(r"D:\下載\a.txt"));
        assert!(!is_inside(""));
    }

    #[test]
    fn lists_folders_including_implied_ones() {
        let z = zip_bytes(
            &[(b"readme.txt", 5, false, &[]), (b"src/", 0, false, &[]), (b"src/main.rs", 7, false, &[]), (b"docs/a/b.md", 3, false, &[]), (b"./x/../evil", 1, false, &[])],
            &[],
        );
        let idx = ZipIndex::parse(&mut Cursor::new(z)).unwrap();
        let top = idx.list("", false, &never()).unwrap();
        assert_eq!(names(&top), [("docs".into(), true), ("readme.txt".into(), false), ("src".into(), true)]);
        let src = idx.list("SRC", false, &never()).unwrap();
        assert_eq!(names(&src), [("main.rs".into(), false)]);
        assert_eq!(src.entries[0].size, 7);
        assert_ne!(src.entries[0].modified, 0, "DOS 時間換成 FILETIME");
        assert_eq!(names(&idx.list("docs", false, &never()).unwrap()), [("a".into(), true)]);
        assert_eq!(names(&idx.list("", true, &never()).unwrap()), [("docs".into(), true), ("src".into(), true)]);
        assert_eq!(idx.list("nope", false, &never()).unwrap_err(), EnumError::NotFound);
        // 明確的空資料夾也找得到
        let z = zip_bytes(&[(b"empty/", 0, false, &[])], &[]);
        let idx = ZipIndex::parse(&mut Cursor::new(z)).unwrap();
        assert!(idx.list("empty", false, &never()).unwrap().is_empty());
    }

    #[test]
    fn utf8_names_ntfs_times_and_sfx_prefix() {
        let mut ntfs = vec![0u8; 4];
        ntfs.extend_from_slice(&1u16.to_le_bytes());
        ntfs.extend_from_slice(&24u16.to_le_bytes());
        ntfs.extend_from_slice(&133_000_000_000_000_000u64.to_le_bytes());
        ntfs.extend_from_slice(&[0; 16]);
        let mut extra = 0x000au16.to_le_bytes().to_vec();
        extra.extend_from_slice(&(ntfs.len() as u16).to_le_bytes());
        extra.extend_from_slice(&ntfs);
        let z = zip_bytes(&[("資料夾/報告.txt".as_bytes(), 9, true, &extra)], b"MZ self-extracting stub");
        let idx = ZipIndex::parse(&mut Cursor::new(z)).unwrap();
        assert_eq!(idx.entries[0].path, r"資料夾\報告.txt");
        assert_eq!(idx.entries[0].modified, 133_000_000_000_000_000);
    }

    #[test]
    fn not_a_zip_is_an_error() {
        assert!(ZipIndex::parse(&mut Cursor::new(b"hello world, definitely not a zip file".to_vec())).is_err());
        assert!(ZipIndex::parse(&mut Cursor::new(Vec::new())).is_err());
    }
}
