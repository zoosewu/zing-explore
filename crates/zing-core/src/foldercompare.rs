//! 資料夾比對與同步（`folder.compare`）：比對兩個資料夾（含子資料夾），列出有差異的項目，依方向產生同步計畫。
//!
//! 比對只看名稱（不分大小寫）、大小、修改時間（容許 2 秒：FAT、exFAT 只記到 2 秒）；需要時再比對內容
//! （`same_content`，大小相同、時間不同的檔案）。只在一邊的資料夾整個算一個差異，不再往下列。
//! 列舉由呼叫端提供（zing-app 在背景執行緒、經壓縮檔與一般資料夾共用的列舉），這裡不碰 Win32，可以單元測試。

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

/// 資料夾裡的一個項目
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// FILETIME（UTC）
    pub modified: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    OnlyLeft,
    OnlyRight,
    LeftNewer,
    RightNewer,
    /// 修改時間相同、大小不同（或比對內容後不同）：不知道哪個新，雙向同步時略過
    Different,
    /// 一邊是檔案、一邊是資料夾：不同步
    TypeMismatch,
}

/// 一個差異：`rel` 是相對於比對的資料夾的路徑（以 `\` 分隔）
#[derive(Clone, Debug, PartialEq)]
pub struct Diff {
    pub rel: String,
    pub kind: Kind,
    pub left: Option<Item>,
    pub right: Option<Item>,
}

impl Diff {
    pub fn dir(&self) -> bool {
        self.left.as_ref().or(self.right.as_ref()).is_some_and(|i| i.dir)
    }
}

/// FILETIME 的 2 秒
const TOLERANCE: u64 = 2 * 10_000_000;

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}\\{name}") }
}

/// 比對。`list(rel, side)` 列出那一邊的 `rel` 資料夾（`""` 是最上層）；`progress` 收到目前比對過的項目數
pub fn compare(
    mut list: impl FnMut(&str, Side) -> Result<Vec<Item>, String>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize),
) -> Result<Vec<Diff>, String> {
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    let mut seen = 0usize;
    while let Some(rel) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(String::new());
        }
        let mut left = list(&rel, Side::Left)?;
        let mut right = list(&rel, Side::Right)?;
        seen += left.len() + right.len();
        progress(seen);
        let key = |i: &Item| i.name.to_lowercase();
        left.sort_by_key(key);
        right.sort_by_key(key);
        let (mut a, mut b) = (left.into_iter().peekable(), right.into_iter().peekable());
        let mut subdirs = Vec::new();
        loop {
            let order = match (a.peek(), b.peek()) {
                (None, None) => break,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(l), Some(r)) => key(l).cmp(&key(r)),
            };
            match order {
                std::cmp::Ordering::Less => {
                    let l = a.next().unwrap();
                    out.push(Diff { rel: join(&rel, &l.name), kind: Kind::OnlyLeft, left: Some(l), right: None });
                }
                std::cmp::Ordering::Greater => {
                    let r = b.next().unwrap();
                    out.push(Diff { rel: join(&rel, &r.name), kind: Kind::OnlyRight, left: None, right: Some(r) });
                }
                std::cmp::Ordering::Equal => {
                    let (l, r) = (a.next().unwrap(), b.next().unwrap());
                    let path = join(&rel, &l.name);
                    let kind = match (l.dir, r.dir) {
                        (true, true) => {
                            subdirs.push(path);
                            continue;
                        }
                        (false, false) => match file_kind(&l, &r) {
                            Some(k) => k,
                            None => continue,
                        },
                        _ => Kind::TypeMismatch,
                    };
                    out.push(Diff { rel: path, kind, left: Some(l), right: Some(r) });
                }
            }
        }
        // 依名稱順序往下（堆疊後進先出，所以反過來放）
        stack.extend(subdirs.into_iter().rev());
    }
    out.sort_by_key(|d| d.rel.to_lowercase());
    Ok(out)
}

/// 兩邊都是檔案：相同時回傳 None
fn file_kind(l: &Item, r: &Item) -> Option<Kind> {
    let same_time = l.modified.abs_diff(r.modified) <= TOLERANCE;
    match (same_time, l.size == r.size) {
        (true, true) => None,
        (true, false) => Some(Kind::Different),
        (false, _) if l.modified > r.modified => Some(Kind::LeftNewer),
        (false, _) => Some(Kind::RightNewer),
    }
}

/// 逐位元組比對兩個檔案的內容（大小相同時才需要）；讀不到就當作不同
pub fn same_content(a: &str, b: &str, cancel: &AtomicBool) -> bool {
    crate::guard::io("比對檔案內容");
    let open = |p: &str| std::fs::File::open(crate::path::long_form(p)).map(std::io::BufReader::new);
    let (Ok(mut fa), Ok(mut fb)) = (open(a), open(b)) else { return false };
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let Ok(n) = fill(&mut fa, &mut ba) else { return false };
        let Ok(m) = fill(&mut fb, &mut bb) else { return false };
        if n != m || ba[..n] != bb[..m] {
            return false;
        }
        if n == 0 {
            return true;
        }
    }
}

/// 讀滿緩衝區（或到檔案結尾）
fn fill(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
    /// 各自補上缺少的、較新的蓋掉較舊的
    Both,
}

/// 同步計畫：`copies` 是（來源那一邊, 相對路徑），複製到另一邊同一個位置；`removes` 是（哪一邊, 相對路徑），
/// 送進資源回收筒（被覆蓋的舊檔先移走，復原時才找得回來）
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub copies: Vec<(Side, String)>,
    pub removes: Vec<(Side, String)>,
    /// 其中被覆蓋的數目（`removes` 裡同時要複製回來的）
    pub overwrites: usize,
    /// 不同步的差異（方向相反、無法判斷新舊、類型不同）
    pub skipped: usize,
}

/// 依方向產生同步計畫。單向時只複製較新的與缺少的；`mirror`（只用於單向）讓目的地與來源完全相同：
/// 目的地較新的也覆蓋、只在目的地的刪除
pub fn plan(diffs: &[Diff], dir: Direction, mirror: bool) -> Plan {
    let mut p = Plan::default();
    let copy = |p: &mut Plan, from: Side, d: &Diff, overwrite: bool| {
        if overwrite {
            let to = if from == Side::Left { Side::Right } else { Side::Left };
            p.removes.push((to, d.rel.clone()));
            p.overwrites += 1;
        }
        p.copies.push((from, d.rel.clone()));
    };
    for d in diffs {
        use Kind::*;
        match (dir, d.kind) {
            (Direction::LeftToRight | Direction::Both, OnlyLeft) => copy(&mut p, Side::Left, d, false),
            (Direction::RightToLeft | Direction::Both, OnlyRight) => copy(&mut p, Side::Right, d, false),
            (Direction::LeftToRight | Direction::Both, LeftNewer) => copy(&mut p, Side::Left, d, true),
            (Direction::RightToLeft | Direction::Both, RightNewer) => copy(&mut p, Side::Right, d, true),
            (Direction::LeftToRight, RightNewer | Different) if mirror => copy(&mut p, Side::Left, d, true),
            (Direction::RightToLeft, LeftNewer | Different) if mirror => copy(&mut p, Side::Right, d, true),
            (Direction::LeftToRight, OnlyRight) if mirror => p.removes.push((Side::Right, d.rel.clone())),
            (Direction::RightToLeft, OnlyLeft) if mirror => p.removes.push((Side::Left, d.rel.clone())),
            // 單向時大小不同、時間相同：無法判斷哪個新，以來源為準
            (Direction::LeftToRight, Different) => copy(&mut p, Side::Left, d, true),
            (Direction::RightToLeft, Different) => copy(&mut p, Side::Right, d, true),
            _ => p.skipped += 1,
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn f(name: &str, size: u64, secs: u64) -> Item {
        Item { name: name.into(), dir: false, size, modified: secs * 10_000_000 }
    }
    fn d(name: &str) -> Item {
        Item { name: name.into(), dir: true, size: 0, modified: 0 }
    }

    fn run(left: HashMap<&str, Vec<Item>>, right: HashMap<&str, Vec<Item>>) -> Vec<(String, Kind)> {
        let never = AtomicBool::new(false);
        let list = |rel: &str, side: Side| {
            let m = if side == Side::Left { &left } else { &right };
            // 檔案系統不分大小寫
            Ok(m.iter().find(|(k, _)| k.eq_ignore_ascii_case(rel)).map(|(_, v)| v.clone()).unwrap_or_default())
        };
        compare(list, &never, |_| {}).unwrap().into_iter().map(|d| (d.rel, d.kind)).collect()
    }

    #[test]
    fn finds_differences_recursively() {
        let left = HashMap::from([
            ("", vec![f("same.txt", 5, 100), f("new.txt", 5, 200), f("only-left.txt", 1, 1), d("Sub"), d("onlydir"), f("mix", 1, 1)]),
            ("Sub", vec![f("a.txt", 3, 100), f("b.txt", 3, 100)]),
            ("onlydir", vec![f("deep.txt", 1, 1)]),
        ]);
        let right = HashMap::from([
            ("", vec![f("SAME.txt", 5, 101), f("new.txt", 5, 100), f("only-right.txt", 1, 1), d("sub"), d("mix")]),
            ("sub", vec![f("a.txt", 4, 100), f("b.txt", 3, 300)]),
        ]);
        assert_eq!(
            run(left, right),
            [
                ("mix".to_string(), Kind::TypeMismatch),
                ("new.txt".to_string(), Kind::LeftNewer),
                ("only-left.txt".to_string(), Kind::OnlyLeft),
                ("only-right.txt".to_string(), Kind::OnlyRight),
                ("onlydir".to_string(), Kind::OnlyLeft),
                (r"Sub\a.txt".to_string(), Kind::Different),
                (r"Sub\b.txt".to_string(), Kind::RightNewer),
            ]
        );
    }

    #[test]
    fn list_errors_stop_the_comparison() {
        let never = AtomicBool::new(false);
        let r = compare(|_, side| if side == Side::Right { Err("找不到".into()) } else { Ok(Vec::new()) }, &never, |_| {});
        assert_eq!(r, Err("找不到".to_string()));
    }

    fn diff(rel: &str, kind: Kind) -> Diff {
        Diff { rel: rel.into(), kind, left: None, right: None }
    }

    #[test]
    fn plans_follow_the_direction() {
        use Kind::*;
        let diffs = [diff("ol", OnlyLeft), diff("or", OnlyRight), diff("ln", LeftNewer), diff("rn", RightNewer), diff("df", Different), diff("tm", TypeMismatch)];
        let p = plan(&diffs, Direction::LeftToRight, false);
        assert_eq!(p.copies, [(Side::Left, "ol".into()), (Side::Left, "ln".into()), (Side::Left, "df".into())]);
        assert_eq!(p.removes, [(Side::Right, "ln".into()), (Side::Right, "df".into())]);
        assert_eq!((p.overwrites, p.skipped), (2, 3));
        let p = plan(&diffs, Direction::LeftToRight, true);
        assert_eq!(p.copies.len(), 4, "鏡像：目的地較新的也覆蓋");
        assert!(p.removes.contains(&(Side::Right, "or".into())), "鏡像：只在目的地的刪除");
        assert_eq!(p.skipped, 1);
        let p = plan(&diffs, Direction::Both, true);
        assert_eq!(p.copies, [(Side::Left, "ol".into()), (Side::Right, "or".into()), (Side::Left, "ln".into()), (Side::Right, "rn".into())]);
        assert_eq!(p.removes, [(Side::Right, "ln".into()), (Side::Left, "rn".into())]);
        assert_eq!(p.skipped, 2, "雙向：無法判斷新舊、類型不同的略過；鏡像不適用");
        let p = plan(&diffs, Direction::RightToLeft, false);
        assert_eq!(p.copies, [(Side::Right, "or".into()), (Side::Right, "rn".into()), (Side::Right, "df".into())]);
    }

    #[test]
    fn content_comparison() {
        let dir = crate::testdir::dir().join("zing-foldercompare");
        let _ = std::fs::create_dir_all(&dir);
        let (a, b, c) = (dir.join("a.bin"), dir.join("b.bin"), dir.join("c.bin"));
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut other = big.clone();
        other[150_000] ^= 1;
        std::fs::write(&a, &big).unwrap();
        std::fs::write(&b, &big).unwrap();
        std::fs::write(&c, &other).unwrap();
        let never = AtomicBool::new(false);
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        assert!(same_content(&s(&a), &s(&b), &never));
        assert!(!same_content(&s(&a), &s(&c), &never));
        assert!(!same_content(&s(&a), &s(&dir.join("missing")), &never));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
