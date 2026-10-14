//! 一個 NTFS 磁碟區的資料夾大小索引。
//!
//! 以記錄編號為索引的陣列（上層資料夾、大小、旗標），加上資料夾的名稱、子資料夾清單與遞迴加總。
//! 由 MFT 的全部記錄建立（`add` 後 `finish`），之後依變更日誌逐筆以 `update` 套用最新狀態，
//! 差值沿上層資料夾鏈傳遞到根資料夾。
//!
//! 大小規則：邏輯大小（未命名資料流）；有硬連結的檔案只算一次（算在第一個檔名的資料夾）；
//! 隱藏與系統檔案一律計入；替代資料流不計；系統保留記錄（$MFT、$Extend…）不列入。

use std::collections::HashMap;
use std::io::{self, Read, Write};

use crate::ntfs::{self, Record};

const NONE: u32 = u32::MAX;

const USED: u8 = 1;
const DIR: u8 = 2;
const HIDDEN: u8 = 4;
const SURROGATE: u8 = 8;
/// 大小已由資料流設定（延伸記錄合併時不覆蓋）
const SIZED: u8 = 16;

/// 遞迴加總：位元組、檔案數、子資料夾數
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
}

impl Totals {
    fn add(&mut self, o: Totals, sign: i64) {
        let f = |a: u64, b: u64| a.saturating_add_signed((b as i64).saturating_mul(sign));
        self.bytes = f(self.bytes, o.bytes);
        self.files = f(self.files, o.files);
        self.dirs = f(self.dirs, o.dirs);
    }
}

#[derive(Default)]
struct Dir {
    name: Box<str>,
    children: Vec<u32>,
    total: Totals,
    /// 這個資料夾底下最後一次變動時的 epoch
    touched: u64,
}

/// 一個子資料夾的資訊
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub name: String,
    pub totals: Totals,
    /// 有子資料夾；None 表示指向別處（junction 等），無法從索引得知
    pub subdirs: Option<bool>,
    /// 有非隱藏的子資料夾
    pub visible_subdirs: Option<bool>,
    pub hidden: bool,
}

pub struct VolumeIndex {
    parent: Vec<u32>,
    size: Vec<u64>,
    flags: Vec<u8>,
    dirs: HashMap<u32, Dir>,
    epoch: u64,
}

fn excluded(frn: u64) -> bool {
    (frn < ntfs::FIRST_USER_RECORD && frn != ntfs::ROOT) || frn >= NONE as u64
}

impl VolumeIndex {
    /// `records` 是 MFT 記錄數（預先配置）；`epoch` 每次啟動應不同（例如目前時間），
    /// 讓重新啟動後的 `touched` 不會與舊值相同
    pub fn new(records: usize, epoch: u64) -> Self {
        VolumeIndex {
            parent: vec![NONE; records],
            size: vec![0; records],
            flags: vec![0; records],
            dirs: HashMap::new(),
            epoch,
        }
    }

    fn ensure(&mut self, f: usize) {
        if f >= self.flags.len() {
            let n = (f + 1).max(self.flags.len() + self.flags.len() / 8);
            self.parent.resize(n, NONE);
            self.size.resize(n, 0);
            self.flags.resize(n, 0);
        }
    }

    /// 建立階段：加入一筆記錄（基本或延伸記錄，順序不拘）。全部加入後呼叫 `finish`。
    pub fn add(&mut self, r: &Record) {
        let target = if r.base != 0 { r.base } else { r.frn };
        if excluded(target) {
            return;
        }
        let t = target as usize;
        self.ensure(t);
        if r.base == 0 {
            let mut fl = self.flags[t] | USED;
            if r.dir {
                fl |= DIR;
            }
            if r.hidden() {
                fl |= HIDDEN;
            }
            if r.dir && r.surrogate() {
                fl |= SURROGATE;
            }
            self.flags[t] = fl;
        }
        if let Some(s) = r.size
            && self.flags[t] & SIZED == 0
        {
            self.size[t] = s;
            self.flags[t] |= SIZED;
        }
        if self.parent[t] == NONE
            && let Some(n) = r.names.first()
            && !excluded(n.parent)
        {
            self.parent[t] = n.parent as u32;
        }
        if r.dir {
            let d = self.dirs.entry(target as u32).or_default();
            if d.name.is_empty()
                && let Some(n) = r.names.first()
            {
                d.name = n.name.as_str().into();
            }
        }
    }

    /// 由陣列重建子資料夾清單與遞迴加總（建立完成、載入快照後呼叫）
    pub fn finish(&mut self) {
        let root = ntfs::ROOT as u32;
        self.dirs.retain(|&f, _| self.flags[f as usize] & (USED | DIR) == USED | DIR);
        for d in self.dirs.values_mut() {
            d.children.clear();
            d.total = Totals::default();
            d.touched = self.epoch;
        }
        for f in 0..self.flags.len() {
            if self.flags[f] & USED == 0 || f as u32 == root {
                continue;
            }
            let p = self.parent[f];
            let Some(pd) = self.dirs.get_mut(&p) else { continue };
            if self.flags[f] & DIR != 0 {
                pd.children.push(f as u32);
            } else {
                pd.total.bytes += self.size[f];
                pd.total.files += 1;
            }
        }
        // 由根往下的廣度優先順序，反向處理即可先算完子資料夾；`seen` 防止損毀資料造成的迴圈
        let mut order = Vec::with_capacity(self.dirs.len());
        let mut seen = vec![false; self.flags.len()];
        if self.dirs.contains_key(&root) {
            order.push(root);
            seen[root as usize] = true;
        }
        let mut i = 0;
        while i < order.len() {
            for &c in &self.dirs[&order[i]].children {
                if !std::mem::replace(&mut seen[c as usize], true) {
                    order.push(c);
                }
            }
            i += 1;
        }
        for &d in order.iter().rev() {
            let mut sum = Totals::default();
            for c in &self.dirs[&d].children {
                if let Some(cd) = self.dirs.get(c) {
                    sum.add(cd.total, 1);
                    sum.dirs += 1;
                }
            }
            self.dirs.get_mut(&d).unwrap().total.add(sum, 1);
        }
    }

    /// 這個項目自己對上層的貢獻
    fn contribution(&self, f: usize) -> Totals {
        if self.flags[f] & DIR != 0 {
            let mut t = self.dirs.get(&(f as u32)).map(|d| d.total).unwrap_or_default();
            t.dirs += 1;
            t
        } else {
            Totals { bytes: self.size[f], files: 1, dirs: 0 }
        }
    }

    /// 從 `start` 往上把差值加到每一層資料夾
    fn propagate(&mut self, start: u32, delta: Totals, sign: i64) {
        let mut p = start;
        for _ in 0..4096 {
            let Some(d) = self.dirs.get_mut(&p) else { return };
            d.total.add(delta, sign);
            d.touched = self.epoch;
            if p == ntfs::ROOT as u32 {
                return;
            }
            p = self.parent[p as usize];
        }
    }

    /// 套用一個項目的最新狀態（變更日誌事件後重新讀取的記錄，已併入延伸記錄）；None 表示已刪除
    pub fn update(&mut self, frn: u64, rec: Option<&Record>) {
        if excluded(frn) {
            return;
        }
        let f = frn as usize;
        self.ensure(f);
        self.epoch += 1;
        let old_parent = self.parent[f];
        if self.flags[f] & USED != 0 {
            let c = self.contribution(f);
            if let Some(pd) = self.dirs.get_mut(&old_parent) {
                pd.children.retain(|&x| x as usize != f);
            }
            self.propagate(old_parent, c, -1);
        }
        let Some(r) = rec else {
            self.flags[f] = 0;
            self.parent[f] = NONE;
            self.size[f] = 0;
            self.dirs.remove(&(f as u32));
            return;
        };
        let mut fl = USED;
        if r.dir {
            fl |= DIR;
            if r.surrogate() {
                fl |= SURROGATE;
            }
        }
        if r.hidden() {
            fl |= HIDDEN;
        }
        self.flags[f] = fl;
        self.size[f] = if r.dir { 0 } else { r.size.unwrap_or(0) };
        // 硬連結：原本的資料夾仍是它的檔名之一時維持不變，避免來回搬動
        let parent = if r.names.iter().any(|n| n.parent == old_parent as u64) {
            old_parent
        } else {
            r.names.first().filter(|n| !excluded(n.parent)).map_or(NONE, |n| n.parent as u32)
        };
        self.parent[f] = parent;
        if r.dir {
            let epoch = self.epoch;
            let d = self.dirs.entry(f as u32).or_default();
            if let Some(n) = r.names.iter().find(|n| n.parent == parent as u64) {
                d.name = n.name.as_str().into();
            }
            d.touched = epoch;
        } else {
            self.dirs.remove(&(f as u32));
        }
        if f as u64 == ntfs::ROOT {
            return;
        }
        if let Some(pd) = self.dirs.get_mut(&parent)
            && r.dir
        {
            pd.children.push(f as u32);
        }
        let c = self.contribution(f);
        self.propagate(parent, c, 1);
    }

    /// 索引中有這個資料夾（系統保留記錄視為已知，不需要補讀）
    pub fn has_dir(&self, frn: u64) -> bool {
        excluded(frn) || u32::try_from(frn).is_ok_and(|f| self.dirs.contains_key(&f))
    }

    /// 資料夾的遞迴加總與最後變動的 epoch
    pub fn dir(&self, frn: u64) -> Option<(Totals, u64)> {
        let d = self.dirs.get(&u32::try_from(frn).ok()?)?;
        Some((d.total, d.touched))
    }

    /// 子資料夾清單（依名稱排序前的任意順序）
    pub fn children(&self, frn: u64) -> Option<Vec<Child>> {
        let d = self.dirs.get(&u32::try_from(frn).ok()?)?;
        Some(
            d.children
                .iter()
                .filter_map(|&c| {
                    let cd = self.dirs.get(&c)?;
                    let fl = self.flags[c as usize];
                    let known = fl & SURROGATE == 0;
                    let visible = cd.children.iter().any(|&g| self.flags[g as usize] & HIDDEN == 0);
                    Some(Child {
                        name: cd.name.to_string(),
                        totals: cd.total,
                        subdirs: known.then_some(!cd.children.is_empty()),
                        visible_subdirs: known.then_some(visible),
                        hidden: fl & HIDDEN != 0,
                    })
                })
                .collect(),
        )
    }

    pub fn records(&self) -> usize {
        self.flags.len()
    }

    pub fn dir_count(&self) -> usize {
        self.dirs.len()
    }

    /// 估計的記憶體用量（位元組）
    pub fn memory(&self) -> usize {
        let arrays = self.flags.capacity() * (4 + 8 + 1);
        let dirs: usize = self
            .dirs
            .values()
            .map(|d| std::mem::size_of::<(u32, Dir)>() + 8 + d.name.len() + d.children.capacity() * 4)
            .sum();
        arrays + dirs
    }

    // ------------------------------------------------------------ 快照

    const MAGIC: &'static [u8; 4] = b"ZIDX";
    const VERSION: u32 = 1;

    /// 寫出快照；`journal` 是變更日誌的 (識別碼, 下一個 USN)，下次載入後從那裡接續
    pub fn save(&self, w: &mut impl Write, journal: (u64, i64)) -> io::Result<()> {
        w.write_all(Self::MAGIC)?;
        w.write_all(&Self::VERSION.to_le_bytes())?;
        w.write_all(&journal.0.to_le_bytes())?;
        w.write_all(&journal.1.to_le_bytes())?;
        w.write_all(&(self.flags.len() as u64).to_le_bytes())?;
        let mut buf = Vec::with_capacity(1 << 16);
        let flush = |buf: &mut Vec<u8>, w: &mut dyn Write| -> io::Result<()> {
            w.write_all(buf)?;
            buf.clear();
            Ok(())
        };
        for chunk in self.parent.chunks(4096) {
            chunk.iter().for_each(|v| buf.extend_from_slice(&v.to_le_bytes()));
            flush(&mut buf, w)?;
        }
        for chunk in self.size.chunks(4096) {
            chunk.iter().for_each(|v| buf.extend_from_slice(&v.to_le_bytes()));
            flush(&mut buf, w)?;
        }
        w.write_all(&self.flags)?;
        w.write_all(&(self.dirs.len() as u64).to_le_bytes())?;
        for (&f, d) in &self.dirs {
            buf.extend_from_slice(&f.to_le_bytes());
            let name = d.name.as_bytes();
            let len = name.len().min(u16::MAX as usize);
            buf.extend_from_slice(&(len as u16).to_le_bytes());
            buf.extend_from_slice(&name[..len]);
            if buf.len() > 60_000 {
                flush(&mut buf, w)?;
            }
        }
        flush(&mut buf, w)
    }

    /// 讀入快照，回傳索引與變更日誌位置。記錄數超過 `max_records` 視為損毀（避免依檔頭配置過多記憶體）
    pub fn load(r: &mut impl Read, epoch: u64, max_records: usize) -> io::Result<(Self, (u64, i64))> {
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "索引快照格式不符");
        let mut b8 = [0u8; 8];
        let mut b4 = [0u8; 4];
        r.read_exact(&mut b4)?;
        if &b4 != Self::MAGIC {
            return Err(bad());
        }
        r.read_exact(&mut b4)?;
        if u32::from_le_bytes(b4) != Self::VERSION {
            return Err(bad());
        }
        r.read_exact(&mut b8)?;
        let id = u64::from_le_bytes(b8);
        r.read_exact(&mut b8)?;
        let usn = i64::from_le_bytes(b8);
        r.read_exact(&mut b8)?;
        let n = u64::from_le_bytes(b8) as usize;
        if n > NONE as usize || n > max_records {
            return Err(bad());
        }
        let mut raw = vec![0u8; n * 4];
        r.read_exact(&mut raw)?;
        let parent = raw.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
        let mut raw = vec![0u8; n * 8];
        r.read_exact(&mut raw)?;
        let size = raw.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect();
        let mut flags = vec![0u8; n];
        r.read_exact(&mut flags)?;
        r.read_exact(&mut b8)?;
        let count = u64::from_le_bytes(b8) as usize;
        if count > n {
            return Err(bad());
        }
        let mut dirs = HashMap::with_capacity(count);
        let mut b2 = [0u8; 2];
        for _ in 0..count {
            r.read_exact(&mut b4)?;
            r.read_exact(&mut b2)?;
            let mut name = vec![0u8; u16::from_le_bytes(b2) as usize];
            r.read_exact(&mut name)?;
            let name = String::from_utf8(name).map_err(|_| bad())?;
            dirs.insert(u32::from_le_bytes(b4), Dir { name: name.into(), ..Default::default() });
        }
        let mut idx = VolumeIndex { parent, size, flags, dirs, epoch };
        if idx.dirs.keys().any(|&f| f as usize >= n) {
            return Err(bad());
        }
        idx.finish();
        Ok((idx, (id, usn)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ntfs::build::Builder;
    use crate::ntfs::ROOT;

    fn rec(b: Builder) -> Record {
        b.parse().unwrap()
    }

    /// 根 ─ a(20) ─ a1(21) ─ f 100、f 50
    ///          └ f 7
    ///   ├ h(22, 隱藏) ─ f 1000（硬連結也在 a1）
    ///   └ j(23, junction)
    fn sample() -> VolumeIndex {
        let mut v = VolumeIndex::new(64, 1000);
        for r in [
            Builder::dir(ROOT).name(ROOT, ".", 1),
            Builder::file(0).name(ROOT, "$MFT", 1).data(&[0; 99]),
            Builder::dir(11).name(ROOT, "$Extend", 1),
            Builder::file(30).name(11, "$UsnJrnl", 1).data(&[0; 77]),
            Builder::dir(20).name(ROOT, "a", 1),
            Builder::dir(21).name(20, "a1", 1),
            Builder::file(31).name(21, "x", 1).data(&[0; 100]),
            Builder::file(32).name(21, "y", 1).data(&[0; 50]),
            Builder::file(33).name(20, "z", 1).data(&[0; 7]),
            Builder::dir(22).attrs(2).name(ROOT, "h", 1),
            Builder::file(34).name(22, "big", 1).name(21, "link", 1).data_nonresident(0, 1000, &[0x11, 1, 1, 0]),
            Builder::dir(23).attrs(0x410).reparse(0xa000_0003).name(ROOT, "j", 1),
            // 延伸記錄先於基本記錄出現
            Builder::extension(36, 35).data_nonresident(0, 5, &[0x11, 1, 1, 0]),
            Builder::file(35).attribute_list(&[(0x80, 36)]).name(20, "ext", 1),
            Builder::file(37).name(21, "gone", 1).data(b"x").deleted(),
        ] {
            if let Some(r) = r.parse() {
                v.add(&r);
            }
        }
        v.finish();
        v
    }

    fn names(v: &VolumeIndex, frn: u64) -> Vec<String> {
        let mut n: Vec<String> = v.children(frn).unwrap().into_iter().map(|c| c.name).collect();
        n.sort();
        n
    }

    #[test]
    fn builds_totals() {
        let v = sample();
        // 系統保留記錄與 $Extend 底下不計；硬連結只算在第一個檔名（h）；延伸記錄的大小併入
        assert_eq!(v.dir(21).unwrap().0, Totals { bytes: 150, files: 2, dirs: 0 });
        assert_eq!(v.dir(20).unwrap().0, Totals { bytes: 162, files: 4, dirs: 1 });
        assert_eq!(v.dir(22).unwrap().0, Totals { bytes: 1000, files: 1, dirs: 0 });
        assert_eq!(v.dir(ROOT).unwrap().0, Totals { bytes: 1162, files: 5, dirs: 4 });
        assert_eq!(names(&v, ROOT), vec!["a", "h", "j"]);
        let kids = v.children(ROOT).unwrap();
        let get = |n: &str| kids.iter().find(|c| c.name == n).unwrap().clone();
        assert_eq!((get("a").subdirs, get("a").visible_subdirs), (Some(true), Some(true)));
        assert!(get("h").hidden);
        assert_eq!(get("h").subdirs, Some(false));
        assert_eq!(get("j").subdirs, None);
        assert_eq!(v.dir(11), None);
    }

    #[test]
    fn updates_propagate() {
        let mut v = sample();
        let before = v.dir(ROOT).unwrap().1;
        // 檔案變大
        v.update(31, Some(&rec(Builder::file(31).name(21, "x", 1).data(&[0; 400]))));
        assert_eq!(v.dir(21).unwrap().0.bytes, 450);
        assert_eq!(v.dir(ROOT).unwrap().0.bytes, 1462);
        assert!(v.dir(ROOT).unwrap().1 > before);
        // 新增檔案、刪除檔案
        v.update(40, Some(&rec(Builder::file(40).name(20, "new", 1).data(&[0; 10]))));
        v.update(33, None);
        assert_eq!(v.dir(20).unwrap().0, Totals { bytes: 465, files: 4, dirs: 1 });
        // 搬移整個資料夾 a1 到 h 底下：加總跟著搬
        v.update(21, Some(&rec(Builder::dir(21).name(22, "moved", 1))));
        assert_eq!(v.dir(20).unwrap().0, Totals { bytes: 15, files: 2, dirs: 0 });
        assert_eq!(v.dir(22).unwrap().0, Totals { bytes: 1450, files: 3, dirs: 1 });
        assert_eq!(v.dir(ROOT).unwrap().0, Totals { bytes: 1465, files: 5, dirs: 4 });
        assert_eq!(names(&v, 22), vec!["moved"]);
        assert!(names(&v, 20).is_empty());
        // 新資料夾、刪除空資料夾
        v.update(41, Some(&rec(Builder::dir(41).name(ROOT, "n", 1))));
        assert_eq!(names(&v, ROOT), vec!["a", "h", "j", "n"]);
        v.update(41, None);
        assert_eq!(v.dir(ROOT).unwrap().0, Totals { bytes: 1465, files: 5, dirs: 4 });
        // 系統保留記錄的事件忽略
        v.update(3, Some(&rec(Builder::file(3).name(ROOT, "$Volume", 1).data(&[0; 5]))));
        assert_eq!(v.dir(ROOT).unwrap().0.files, 5);
    }

    #[test]
    fn hard_link_parent_is_stable() {
        let mut v = sample();
        // 硬連結的另一個檔名排在前面時仍留在原本的資料夾
        v.update(34, Some(&rec(Builder::file(34).name(21, "link", 1).name(22, "big", 1).data(&[0; 8]))));
        assert_eq!(v.dir(22).unwrap().0.bytes, 8);
        assert_eq!(v.dir(21).unwrap().0.bytes, 150);
        // 原本的檔名刪除後改算到剩下的
        v.update(34, Some(&rec(Builder::file(34).name(21, "link", 1).data(&[0; 8]))));
        assert_eq!((v.dir(22).unwrap().0.bytes, v.dir(21).unwrap().0.bytes), (0, 158));
    }

    #[test]
    fn grows_beyond_initial_capacity() {
        let mut v = sample();
        v.update(5000, Some(&rec(Builder::file(5000).name(20, "far", 1).data(&[0; 3]))));
        assert_eq!(v.dir(20).unwrap().0.bytes, 165);
        assert!(v.records() > 5000);
    }

    #[test]
    fn snapshot_roundtrip() {
        let v = sample();
        let mut buf = Vec::new();
        v.save(&mut buf, (77, 123_456)).unwrap();
        let (w, journal) = VolumeIndex::load(&mut buf.as_slice(), 5, 1000).unwrap();
        assert_eq!(journal, (77, 123_456));
        for f in [ROOT, 20, 21, 22, 23] {
            assert_eq!(w.dir(f).unwrap().0, v.dir(f).unwrap().0);
        }
        assert_eq!(names(&w, ROOT), names(&v, ROOT));
        assert_eq!(w.children(ROOT).unwrap().iter().find(|c| c.name == "j").unwrap().subdirs, None);
        assert!(VolumeIndex::load(&mut &buf[..buf.len() - 3], 5, 1000).is_err());
        assert!(VolumeIndex::load(&mut &b"nope"[..], 5, 1000).is_err());
        // 檔頭的記錄數超過上限：不配置、直接拒絕
        assert!(VolumeIndex::load(&mut buf.as_slice(), 5, 10).is_err());
    }
}
