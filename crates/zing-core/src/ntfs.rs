//! NTFS 主檔案表（MFT）記錄的解析。
//!
//! 只取資料夾大小索引需要的欄位：使用中／資料夾旗標、所屬的基本記錄、檔名（上層資料夾與名稱）、
//! 未命名資料流的邏輯大小、檔案屬性、重新解析標籤，以及 $MFT 自己的資料區段（用來循序讀取整個 MFT）。
//! 讀取磁碟區需要系統管理員權限，由 zing-index 服務負責；這裡只處理位元組，方便以構造的記錄測試。

/// 根資料夾的記錄編號
pub const ROOT: u64 = 5;
/// 檔案參照的低 48 位元是記錄編號，高 16 位元是序號
pub const FRN_MASK: u64 = 0x0000_ffff_ffff_ffff;
/// 系統保留的記錄（$MFT、$LogFile、$Extend…）；根資料夾除外，不列入索引
pub const FIRST_USER_RECORD: u64 = 16;

pub const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// 重新解析標籤的「名稱代理」位元：junction、符號連結等指向別處的資料夾
pub const REPARSE_TAG_NAME_SURROGATE: u32 = 0x2000_0000;

const ATTR_STANDARD_INFORMATION: u32 = 0x10;
const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_REPARSE_POINT: u32 = 0xc0;
const ATTR_END: u32 = 0xffff_ffff;

const NAMESPACE_DOS: u8 = 2;

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// 還原每個磁區最後兩個位元組（更新序列）。`verify` 為 true 時先確認磁區結尾都等於更新序列號碼，
/// 不符（寫入到一半的記錄）回傳 false 且不修改。從磁碟直接讀取的記錄要驗證；
/// FSCTL_GET_NTFS_FILE_RECORD 傳回的記錄可能已還原，不驗證直接寫回（寫回的值相同，重複套用無害）。
pub fn apply_fixup(rec: &mut [u8], sector: usize, verify: bool) -> bool {
    let (Some(off), Some(count)) = (u16_at(rec, 4), u16_at(rec, 6)) else {
        return false;
    };
    let (off, count) = (off as usize, count as usize);
    if count < 2 || sector < 2 || off + count * 2 > rec.len() || (count - 1) * sector > rec.len() {
        return false;
    }
    if verify {
        let usn = [rec[off], rec[off + 1]];
        if (1..count).any(|i| rec[i * sector - 2..i * sector] != usn) {
            return false;
        }
    }
    for i in 1..count {
        let end = i * sector - 2;
        rec[end] = rec[off + i * 2];
        rec[end + 1] = rec[off + i * 2 + 1];
    }
    true
}

/// 一個檔名：上層資料夾的記錄編號與名稱
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name {
    pub parent: u64,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub frn: u64,
    pub dir: bool,
    /// 延伸記錄所屬的基本記錄編號；基本記錄為 0
    pub base: u64,
    /// 非 DOS 命名空間的檔名；有硬連結時會有多個
    pub names: Vec<Name>,
    /// 未命名資料流的邏輯大小（常駐，或含第 0 個 VCN 的非常駐區段才有）
    pub size: Option<u64>,
    /// 標準資訊中的檔案屬性
    pub attrs: Option<u32>,
    /// 標準資訊中的修改時間（FILETIME）
    pub modified: Option<u64>,
    /// 重新解析標籤；沒有或無法讀取（非常駐）時為 0
    pub reparse_tag: u32,
    /// 常駐屬性清單中，存放檔名或資料流的其他記錄
    pub extensions: Vec<u64>,
}

impl Record {
    /// 指向別處的資料夾（junction、符號連結）：它的子項不在這個位置，無法從索引得知
    pub fn surrogate(&self) -> bool {
        let reparse = self.attrs.is_some_and(|a| a & FILE_ATTRIBUTE_REPARSE_POINT != 0) || self.reparse_tag != 0;
        reparse && (self.reparse_tag == 0 || self.reparse_tag & REPARSE_TAG_NAME_SURROGATE != 0)
    }

    pub fn hidden(&self) -> bool {
        self.attrs.is_some_and(|a| a & FILE_ATTRIBUTE_HIDDEN != 0)
    }

    /// 併入延伸記錄的檔名與資料流大小（屬性清單讓它們存放在其他記錄）
    pub fn merge(&mut self, ext: &Record) {
        if self.size.is_none() {
            self.size = ext.size;
        }
        for n in &ext.names {
            if !self.names.iter().any(|m| m.parent == n.parent) {
                self.names.push(n.clone());
            }
        }
    }
}

/// 屬性的範圍與（常駐時的）值
struct Attr<'a> {
    ty: u32,
    raw: &'a [u8],
    named: bool,
    value: Option<&'a [u8]>,
}

fn attributes(rec: &[u8]) -> impl Iterator<Item = Attr<'_>> {
    let used = u32_at(rec, 24).map_or(0, |u| (u as usize).min(rec.len()));
    let mut off = u16_at(rec, 20).map_or(usize::MAX, |o| o as usize);
    std::iter::from_fn(move || {
        let ty = u32_at(rec, off)?;
        let len = u32_at(rec, off + 4)? as usize;
        if ty == ATTR_END || len < 16 || off + len > used {
            return None;
        }
        let raw = &rec[off..off + len];
        off += len;
        let nonresident = raw[8] != 0;
        let value = if nonresident {
            None
        } else {
            let vlen = u32_at(raw, 16)? as usize;
            let voff = u16_at(raw, 20)? as usize;
            raw.get(voff..voff.checked_add(vlen)?)
        };
        Some(Attr { ty, raw, named: raw[9] != 0, value })
    })
}

/// 解析一筆已還原更新序列的記錄；不是有效或未使用中的記錄回傳 None
pub fn parse_record(rec: &[u8], frn: u64) -> Option<Record> {
    if rec.get(0..4)? != b"FILE" {
        return None;
    }
    let flags = u16_at(rec, 22)?;
    if flags & 1 == 0 {
        return None;
    }
    let dir = flags & 2 != 0;
    let mut r = Record { frn, dir, base: u64_at(rec, 32)? & FRN_MASK, ..Default::default() };
    for a in attributes(rec) {
        match a.ty {
            ATTR_STANDARD_INFORMATION => {
                r.attrs = a.value.and_then(|v| u32_at(v, 32));
                r.modified = a.value.and_then(|v| u64_at(v, 8));
            }
            ATTR_FILE_NAME => {
                let Some(v) = a.value else { continue };
                let (Some(parent), Some(&len), Some(&ns)) = (u64_at(v, 0), v.get(64), v.get(65)) else {
                    continue;
                };
                if ns == NAMESPACE_DOS {
                    continue;
                }
                let parent = parent & FRN_MASK;
                let Some(bytes) = v.get(66..66 + len as usize * 2) else { continue };
                let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
                let name = String::from_utf16_lossy(&units);
                if !r.names.iter().any(|n| n.parent == parent) {
                    r.names.push(Name { parent, name });
                }
            }
            ATTR_DATA if !a.named => match a.value {
                Some(v) => r.size = Some(v.len() as u64),
                None => {
                    if u64_at(a.raw, 16) == Some(0) {
                        r.size = u64_at(a.raw, 48);
                    }
                }
            },
            ATTR_REPARSE_POINT => r.reparse_tag = a.value.and_then(|v| u32_at(v, 0)).unwrap_or(0),
            ATTR_ATTRIBUTE_LIST => {
                let Some(v) = a.value else { continue };
                let mut o = 0;
                while let (Some(ty), Some(len)) = (u32_at(v, o), u16_at(v, o + 4)) {
                    if len < 26 {
                        break;
                    }
                    if let Some(at) = u64_at(v, o + 16) {
                        let at = at & FRN_MASK;
                        if (ty == ATTR_FILE_NAME || ty == ATTR_DATA) && at != frn && !r.extensions.contains(&at) {
                            r.extensions.push(at);
                        }
                    }
                    o += len as usize;
                }
            }
            _ => {}
        }
    }
    Some(r)
}

/// 非常駐資料流的一段連續叢集；`lcn` 為 None 是稀疏（未配置）區段
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub lcn: Option<u64>,
    pub clusters: u64,
}

/// 解碼資料區段清單（每段：標頭位元組的低 4 位元是長度的位元組數、高 4 位元是起點差值的位元組數）
pub fn decode_runs(b: &[u8]) -> Option<Vec<Run>> {
    let mut out = Vec::new();
    let (mut i, mut lcn) = (0usize, 0i64);
    while let Some(&h) = b.get(i) {
        if h == 0 {
            break;
        }
        let (ls, os) = ((h & 0xf) as usize, (h >> 4) as usize);
        i += 1;
        if ls == 0 || ls > 8 || os > 8 || i + ls + os > b.len() {
            return None;
        }
        let mut len = 0u64;
        for k in 0..ls {
            len |= (b[i + k] as u64) << (8 * k);
        }
        i += ls;
        if os == 0 {
            out.push(Run { lcn: None, clusters: len });
        } else {
            let mut delta = 0i64;
            for k in 0..os {
                delta |= (b[i + k] as i64) << (8 * k);
            }
            // 依最高位元做符號延伸
            let shift = 64 - 8 * os as u32;
            delta = (delta << shift) >> shift;
            lcn = lcn.checked_add(delta).filter(|&l| l >= 0)?;
            out.push(Run { lcn: Some(lcn as u64), clusters: len });
        }
        i += os;
    }
    Some(out)
}

/// 未命名資料流的非常駐區段：(起始 VCN, 資料區段)。用於讀取 $MFT 自己的位置。
pub fn data_runs(rec: &[u8]) -> Vec<(u64, Vec<Run>)> {
    attributes(rec)
        .filter(|a| a.ty == ATTR_DATA && !a.named && a.value.is_none())
        .filter_map(|a| {
            let start = u64_at(a.raw, 16)?;
            let off = u16_at(a.raw, 32)? as usize;
            Some((start, decode_runs(a.raw.get(off..)?)?))
        })
        .collect()
}

/// 測試與基準測試用：構造 MFT 記錄
pub mod build {
    use super::*;

    pub const RECORD: usize = 1024;
    pub const SECTOR: usize = 512;

    pub struct Builder {
        frn: u64,
        flags: u16,
        base: u64,
        attrs: Vec<Vec<u8>>,
    }

    fn header(ty: u32, nonresident: bool, name_len: u8, len: usize) -> Vec<u8> {
        let mut a = vec![0u8; len];
        a[0..4].copy_from_slice(&ty.to_le_bytes());
        a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        a[8] = nonresident as u8;
        a[9] = name_len;
        a
    }

    fn resident(ty: u32, value: &[u8]) -> Vec<u8> {
        let len = (24 + value.len()).div_ceil(8) * 8;
        let mut a = header(ty, false, 0, len);
        a[16..20].copy_from_slice(&(value.len() as u32).to_le_bytes());
        a[20..22].copy_from_slice(&24u16.to_le_bytes());
        a[24..24 + value.len()].copy_from_slice(value);
        a
    }

    impl Builder {
        pub fn file(frn: u64) -> Self {
            Builder { frn, flags: 1, base: 0, attrs: Vec::new() }
        }

        pub fn dir(frn: u64) -> Self {
            Builder { frn, flags: 3, base: 0, attrs: Vec::new() }
        }

        /// 延伸記錄（屬於 `base`）
        pub fn extension(frn: u64, base: u64) -> Self {
            Builder { frn, flags: 1, base, attrs: Vec::new() }
        }

        pub fn deleted(mut self) -> Self {
            self.flags &= !1;
            self
        }

        pub fn attrs(self, attrs: u32) -> Self {
            self.info(attrs, 0)
        }

        /// 標準資訊：檔案屬性與修改時間（FILETIME）
        pub fn info(mut self, attrs: u32, modified: u64) -> Self {
            let mut v = vec![0u8; 48];
            v[8..16].copy_from_slice(&modified.to_le_bytes());
            v[32..36].copy_from_slice(&attrs.to_le_bytes());
            self.attrs.push(resident(ATTR_STANDARD_INFORMATION, &v));
            self
        }

        pub fn name(mut self, parent: u64, name: &str, namespace: u8) -> Self {
            let units: Vec<u16> = name.encode_utf16().collect();
            let mut v = vec![0u8; 66 + units.len() * 2];
            // 序號放在高 16 位元，解析時要去掉
            v[0..8].copy_from_slice(&(parent | (7 << 48)).to_le_bytes());
            v[64] = units.len() as u8;
            v[65] = namespace;
            for (i, u) in units.iter().enumerate() {
                v[66 + i * 2..68 + i * 2].copy_from_slice(&u.to_le_bytes());
            }
            self.attrs.push(resident(ATTR_FILE_NAME, &v));
            self
        }

        pub fn data(mut self, bytes: &[u8]) -> Self {
            self.attrs.push(resident(ATTR_DATA, bytes));
            self
        }

        /// 非常駐的未命名資料流；`size` 只在 `start_vcn` 為 0 時有意義
        pub fn data_nonresident(mut self, start_vcn: u64, size: u64, runs: &[u8]) -> Self {
            let len = (64 + runs.len() + 1).div_ceil(8) * 8;
            let mut a = header(ATTR_DATA, true, 0, len);
            a[16..24].copy_from_slice(&start_vcn.to_le_bytes());
            a[32..34].copy_from_slice(&64u16.to_le_bytes());
            a[40..48].copy_from_slice(&size.next_multiple_of(4096).to_le_bytes());
            a[48..56].copy_from_slice(&size.to_le_bytes());
            a[56..64].copy_from_slice(&size.to_le_bytes());
            a[64..64 + runs.len()].copy_from_slice(runs);
            self.attrs.push(a);
            self
        }

        /// 具名資料流（替代資料流），不計入大小
        pub fn named_data(mut self, bytes: &[u8]) -> Self {
            let mut a = resident(ATTR_DATA, bytes);
            a[9] = 1;
            self.attrs.push(a);
            self
        }

        pub fn reparse(mut self, tag: u32) -> Self {
            let mut v = vec![0u8; 8];
            v[0..4].copy_from_slice(&tag.to_le_bytes());
            self.attrs.push(resident(ATTR_REPARSE_POINT, &v));
            self
        }

        /// 屬性清單：每項 (屬性類型, 所在記錄)
        pub fn attribute_list(mut self, entries: &[(u32, u64)]) -> Self {
            let mut v = Vec::new();
            for &(ty, at) in entries {
                let mut e = vec![0u8; 32];
                e[0..4].copy_from_slice(&ty.to_le_bytes());
                e[4..6].copy_from_slice(&32u16.to_le_bytes());
                e[16..24].copy_from_slice(&at.to_le_bytes());
                v.extend(e);
            }
            self.attrs.push(resident(ATTR_ATTRIBUTE_LIST, &v));
            self
        }

        /// 輸出套用了更新序列的記錄（與磁碟上的格式相同）
        pub fn bytes(&self) -> Vec<u8> {
            let mut r = vec![0u8; RECORD];
            r[0..4].copy_from_slice(b"FILE");
            let sectors = RECORD / SECTOR;
            r[4..6].copy_from_slice(&48u16.to_le_bytes());
            r[6..8].copy_from_slice(&((sectors + 1) as u16).to_le_bytes());
            r[20..22].copy_from_slice(&56u16.to_le_bytes());
            r[22..24].copy_from_slice(&self.flags.to_le_bytes());
            r[32..40].copy_from_slice(&(self.base | if self.base != 0 { 3 << 48 } else { 0 }).to_le_bytes());
            r[44..48].copy_from_slice(&(self.frn as u32).to_le_bytes());
            let mut off = 56;
            for a in &self.attrs {
                r[off..off + a.len()].copy_from_slice(a);
                off += a.len();
            }
            r[off..off + 4].copy_from_slice(&ATTR_END.to_le_bytes());
            off += 8;
            r[24..28].copy_from_slice(&(off as u32).to_le_bytes());
            r[28..32].copy_from_slice(&(RECORD as u32).to_le_bytes());
            // 更新序列：磁區最後兩個位元組搬到陣列，換成序列號碼
            let usn = [0x34u8, 0x12];
            r[48..50].copy_from_slice(&usn);
            for i in 1..=sectors {
                let end = i * SECTOR - 2;
                let (a, b) = (r[end], r[end + 1]);
                r[48 + i * 2] = a;
                r[49 + i * 2] = b;
                r[end] = usn[0];
                r[end + 1] = usn[1];
            }
            r
        }

        /// 構造、還原並解析
        pub fn parse(&self) -> Option<Record> {
            let mut b = self.bytes();
            assert!(apply_fixup(&mut b, SECTOR, true));
            parse_record(&b, self.frn)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    #[test]
    fn file_with_resident_data_and_names() {
        let r = Builder::file(40)
            .attrs(0x20)
            .name(30, "LONGNA~1.TXT", NAMESPACE_DOS)
            .name(30, "long name.txt", 1)
            .data(&[7u8; 300])
            .named_data(&[1u8; 50])
            .parse()
            .unwrap();
        assert!(!r.dir);
        assert_eq!(r.size, Some(300));
        assert_eq!(r.attrs, Some(0x20));
        // DOS 名稱略過
        assert_eq!(r.names, vec![Name { parent: 30, name: "long name.txt".into() }]);
        assert!(!r.surrogate() && !r.hidden());
    }

    #[test]
    fn directory_names_and_hard_links() {
        let d = Builder::dir(30).attrs(0x12).name(ROOT, "資料夾 α", 3).parse().unwrap();
        assert!(d.dir && d.hidden());
        assert_eq!(d.size, None);
        assert_eq!(d.names, vec![Name { parent: ROOT, name: "資料夾 α".into() }]);
        let f = Builder::file(41).name(30, "a", 1).name(31, "b", 0).name(30, "c", 0).data(b"x").parse().unwrap();
        // 硬連結：每個上層資料夾一次
        assert_eq!(f.names.iter().map(|n| n.parent).collect::<Vec<_>>(), vec![30, 31]);
    }

    #[test]
    fn nonresident_size_only_from_first_extent() {
        let r = Builder::file(42).name(30, "big", 1).data_nonresident(0, 5_000_000_000, &[0x11, 0x10, 0x20, 0]).parse().unwrap();
        assert_eq!(r.size, Some(5_000_000_000));
        let ext = Builder::extension(43, 42).data_nonresident(16, 999, &[0x11, 0x10, 0x40, 0]).parse().unwrap();
        assert_eq!((ext.base, ext.size), (42, None));
    }

    #[test]
    fn extension_records_and_merge() {
        let mut base = Builder::file(50).attribute_list(&[(0x10, 50), (0x30, 51), (0x80, 52), (0x80, 52)]).parse().unwrap();
        assert_eq!(base.extensions, vec![51, 52]);
        assert_eq!((base.size, base.names.len()), (None, 0));
        base.merge(&Builder::extension(51, 50).name(30, "x", 1).parse().unwrap());
        base.merge(&Builder::extension(52, 50).data_nonresident(0, 12345, &[0x11, 1, 1, 0]).parse().unwrap());
        assert_eq!((base.size, base.names[0].parent), (Some(12345), 30));
    }

    #[test]
    fn reparse_points() {
        let junction = Builder::dir(60).attrs(0x410).reparse(0xa000_0003).name(ROOT, "j", 1).parse().unwrap();
        assert!(junction.surrogate());
        // OneDrive 的雲端資料夾不是名稱代理，子項仍在原處
        let cloud = Builder::dir(61).attrs(0x410).reparse(0x9000_601a).name(ROOT, "c", 1).parse().unwrap();
        assert!(!cloud.surrogate());
        // 標籤在非常駐屬性中讀不到時保守視為代理
        assert!(Builder::dir(62).attrs(0x410).parse().unwrap().surrogate());
    }

    #[test]
    fn rejects_bad_records() {
        assert_eq!(Builder::file(70).deleted().parse(), None);
        let mut torn = Builder::file(71).data(b"x").bytes();
        torn[SECTOR - 2] ^= 0xff; // 寫入到一半：磁區結尾不等於序列號碼
        assert!(!apply_fixup(&mut torn, SECTOR, true));
        assert_eq!(parse_record(&[0u8; 1024], 1), None);
        // 屬性長度超出記錄時停止，不 panic
        let mut b = Builder::file(72).data(b"abc").bytes();
        assert!(apply_fixup(&mut b, SECTOR, true));
        b[56 + 4..56 + 8].copy_from_slice(&5000u32.to_le_bytes());
        assert_eq!(parse_record(&b, 72).unwrap().size, None);
    }

    #[test]
    fn fixup_without_verify_is_idempotent() {
        let mut b = Builder::file(80).data(&[9u8; 600]).bytes();
        assert!(apply_fixup(&mut b, SECTOR, true));
        let once = b.clone();
        assert!(apply_fixup(&mut b, SECTOR, false));
        assert_eq!(b, once);
        assert_eq!(parse_record(&b, 80).unwrap().size, Some(600));
    }

    #[test]
    fn run_lists() {
        // 0x21：長度 1 位元組、起點 2 位元組；第二段起點為負的差值
        let runs = decode_runs(&[0x21, 0x10, 0x00, 0x01, 0x21, 0x08, 0x00, 0xff, 0x01, 0x05, 0x00]).unwrap();
        assert_eq!(
            runs,
            vec![
                Run { lcn: Some(0x100), clusters: 0x10 },
                Run { lcn: Some(0x100 - 0x100), clusters: 8 },
                Run { lcn: None, clusters: 5 },
            ]
        );
        assert_eq!(decode_runs(&[0x11, 0x01, 0x80, 0]), None); // 起點變成負數
        assert_eq!(decode_runs(&[0x21, 0x01]), None); // 截斷
        let mut rec = Builder::file(0).data_nonresident(0, 1 << 20, &[0x31, 0x40, 0x0c, 0x00, 0x00, 0]).bytes();
        apply_fixup(&mut rec, SECTOR, true);
        assert_eq!(data_runs(&rec), vec![(0, vec![Run { lcn: Some(12), clusters: 0x40 }])]);
    }
}
