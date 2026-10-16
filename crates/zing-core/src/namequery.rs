//! 全磁碟搜尋的查詢語法與比對（索引中的名稱是 UTF-8）。
//!
//! 查詢以空白分隔成多個詞，每個詞都要符合（AND）；雙引號包住的部分可以含空白，`!` 開頭表示排除。
//! - 一般的詞比對名稱：含 `*` 或 `?` 的詞以萬用字元比對整個名稱，其他的詞是子字串。
//! - 含 `\` 的詞改為比對完整路徑（例如 `專案\src`、`c:\users\*.txt`）。
//! - 篩選：`ext:jpg;png`（副檔名，只對檔案）、`size:>10mb`、`size:1mb..1gb`、`dm:today`、`dm:2024-05`、
//!   `dm:>2024-05-01`、`dm:7d`（最近 7 天）、`file:`、`folder:`。
//! - 排序：`sort:name`、`path`、`ext`（預設遞增）、`size`、`date`（預設遞減），可加 `-asc`／`-desc`。
//!
//! 大小寫：不分大小寫（Unicode 小寫對應）。查詢只含 ASCII 與中日韓文字時名稱逐位元組比對（ASCII 字母轉小寫；
//! 其他文字轉成小寫後也不會變成查詢中的字元，只有 K（克耳文符號）、İ 這類轉成 ASCII 的少數字元例外）；
//! 查詢含有大小寫之分的非 ASCII 字母（é、Σ、Ａ…）時，名稱先轉成小寫再比對。

/// 名稱或路徑的比對方式（已轉成小寫）
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pat {
    Sub(Vec<u8>),
    Glob(Vec<u8>),
}

impl Pat {
    fn bytes(&self) -> &[u8] {
        match self {
            Pat::Sub(b) | Pat::Glob(b) => b,
        }
    }

    fn new(folded: Vec<u8>) -> Self {
        if folded.iter().any(|&c| c == b'*' || c == b'?') { Pat::Glob(folded) } else { Pat::Sub(folded) }
    }

    /// `s` 必須已轉成小寫（ASCII 字母可以是大寫）
    fn test(&self, s: &[u8]) -> bool {
        match self {
            Pat::Sub(n) => contains(s, n),
            Pat::Glob(p) => glob(p, s),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Cond {
    Name(Pat),
    Path(Pat),
    /// 副檔名（小寫，不含點）其中之一；資料夾不符合
    Ext(Vec<Vec<u8>>),
    /// 大小範圍（含兩端）
    Size(u64, u64),
    /// 修改時間範圍（Unix 秒，含起點、不含終點）
    Date(i64, i64),
    File,
    Folder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Path,
    Ext,
    Size,
    Date,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub desc: bool,
}

/// 比對時需要的項目資訊
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub dir: bool,
    /// 檔案的大小；資料夾為遞迴加總
    pub size: u64,
    /// 修改時間（Unix 秒）；0 表示不知道
    pub mtime: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameQuery {
    conds: Vec<(Cond, bool)>,
    pub sort: Option<Sort>,
    /// 無法解讀的詞（例如 `size:abc`）；這些詞被忽略
    pub errors: Vec<String>,
    /// 名稱要先轉成小寫：有含「有大小寫之分的非 ASCII 字母」的名稱或路徑條件。只含 ASCII 與中日韓文字的條件
    /// 直接比對原本的名稱即可（這些字元轉成小寫後不變，名稱中其他字元轉換與否不影響是否符合）
    fold_names: bool,
}

/// 預先比對過的資料夾路徑（見 `NameQuery::dir_match`）
pub struct DirMatch {
    folded: Box<[u8]>,
    /// 第 i 個路徑子字串條件已出現在資料夾路徑中
    hits: u64,
}

impl DirMatch {
    /// 轉成小寫的資料夾路徑
    pub fn folded(&self) -> &[u8] {
        &self.folded
    }
}

/// 比對時重複使用的緩衝區（轉成小寫的名稱與路徑）
#[derive(Default)]
pub struct Scratch {
    name: Vec<u8>,
    path: Vec<u8>,
}

impl NameQuery {
    /// 以目前的時間與時區解讀（`dm:today` 等）
    pub fn parse(q: &str) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        Self::parse_at(q, now, local_offset(now))
    }

    /// `now` 是 Unix 秒，`offset` 是當地時間與 UTC 的差（秒）
    pub fn parse_at(q: &str, now: i64, offset: i64) -> Self {
        let mut out = NameQuery::default();
        for word in split(q) {
            let (neg, w) = match word.strip_prefix('!') {
                Some(rest) if !rest.is_empty() => (true, rest),
                _ => (false, word.as_str()),
            };
            match out.filter(w, now, offset) {
                Some(Ok(Some(c))) => out.conds.push((c, neg)),
                Some(Ok(None)) => {}
                Some(Err(())) => out.errors.push(word.clone()),
                None => {
                    let folded = fold(w.as_bytes());
                    // 只有 * 的詞符合一切，不必比對
                    if folded.iter().all(|&c| c == b'*') {
                        continue;
                    }
                    let c = if w.contains('\\') { Cond::Path(Pat::new(folded)) } else { Cond::Name(Pat::new(folded)) };
                    out.conds.push((c, neg));
                }
            }
        }
        out.fold_names = out.conds.iter().any(|(c, _)| match c {
            Cond::Name(p) | Cond::Path(p) => !caseless(p.bytes()),
            Cond::Ext(exts) => exts.iter().any(|e| !caseless(e)),
            _ => false,
        });
        out
    }

    /// `key:value` 形式的篩選；不是篩選時回傳 None，無法解讀時 Err，`sort:` 回傳 Ok(None)
    fn filter(&mut self, w: &str, now: i64, offset: i64) -> Option<Result<Option<Cond>, ()>> {
        let (key, value) = w.split_once(':')?;
        let value = value.trim();
        Some(match key.to_ascii_lowercase().as_str() {
            "ext" => {
                let exts: Vec<Vec<u8>> = value
                    .split([';', ','])
                    .map(|e| fold(e.trim().trim_start_matches('.').as_bytes()))
                    .filter(|e| !e.is_empty())
                    .collect();
                if exts.is_empty() { Err(()) } else { Ok(Some(Cond::Ext(exts))) }
            }
            "size" => parse_size(value).map(|(a, b)| Some(Cond::Size(a, b))).ok_or(()),
            "dm" | "date" => parse_date(value, now, offset).map(|(a, b)| Some(Cond::Date(a, b))).ok_or(()),
            "file" | "files" if value.is_empty() => Ok(Some(Cond::File)),
            "folder" | "folders" if value.is_empty() => Ok(Some(Cond::Folder)),
            "sort" => match parse_sort(value) {
                Some(s) => {
                    self.sort = Some(s);
                    Ok(None)
                }
                None => Err(()),
            },
            // 磁碟代號（`c:\…`）等：當作一般的詞
            _ => return None,
        })
    }

    /// 沒有任何條件（只有 `sort:` 也算）
    pub fn is_empty(&self) -> bool {
        self.conds.is_empty()
    }

    /// 有比對完整路徑的條件：呼叫者需要提供路徑給 `matches_path`
    pub fn needs_path(&self) -> bool {
        self.conds.iter().any(|(c, _)| matches!(c, Cond::Path(_)))
    }

    /// 有篩選大小的條件（資料夾的大小要另外查表）
    pub fn needs_size(&self) -> bool {
        self.conds.iter().any(|(c, _)| matches!(c, Cond::Size(..)))
    }

    /// 除了路徑以外的條件
    pub fn matches_meta(&self, s: &mut Scratch, name: &[u8], m: Meta) -> bool {
        // 只有用到名稱的條件才轉小寫（例如只篩選大小時不必）
        let n = if self.fold_names { fold_into(name, &mut s.name) } else { name };
        for (c, neg) in &self.conds {
            let ok = match c {
                Cond::Path(_) => continue,
                Cond::Name(p) => p.test(n),
                Cond::Ext(exts) => {
                    !m.dir && {
                        let ext = n.iter().rposition(|&b| b == b'.').map_or(&[][..], |i| &n[i + 1..]);
                        exts.iter().any(|e| e.len() == ext.len() && e.iter().zip(ext).all(|(a, b)| *a == b.to_ascii_lowercase()))
                    }
                }
                Cond::Size(lo, hi) => (*lo..=*hi).contains(&m.size),
                Cond::Date(lo, hi) => m.mtime != 0 && (*lo..*hi).contains(&(m.mtime as i64)),
                Cond::File => !m.dir,
                Cond::Folder => m.dir,
            };
            if ok == *neg {
                return false;
            }
        }
        true
    }

    /// 比對完整路徑的條件（`needs_path` 為 false 時一律符合）
    pub fn matches_path(&self, s: &mut Scratch, path: &[u8]) -> bool {
        if !self.needs_path() {
            return true;
        }
        let path = fold_into(path, &mut s.path);
        for (c, neg) in &self.conds {
            let Cond::Path(p) = c else { continue };
            let ok = p.test(path);
            if ok == *neg {
                return false;
            }
        }
        true
    }

    /// 資料夾的路徑（已轉成小寫）：先比對一次，`matches_in_dir` 再比對其中的項目
    pub fn dir_match(&self, folded: Vec<u8>) -> DirMatch {
        let mut hits = 0u64;
        for (i, p) in self.path_pats().enumerate().take(64) {
            if let Pat::Sub(n) = p
                && contains(&folded, n)
            {
                hits |= 1 << i;
            }
        }
        DirMatch { folded: folded.into(), hits }
    }

    fn path_pats(&self) -> impl Iterator<Item = &Pat> {
        self.conds.iter().filter_map(|(c, _)| if let Cond::Path(p) = c { Some(p) } else { None })
    }

    /// 等同 `matches_path(資料夾路徑 + "\" + name)`。子字串只需檢查跨到名稱的部分（資料夾路徑的最後
    /// 「長度 - 1」個位元組接上名稱），完全在資料夾路徑裡的已由 `dir_match` 算好
    pub fn matches_in_dir(&self, s: &mut Scratch, dir: &DirMatch, name: &[u8]) -> bool {
        let Scratch { name: nbuf, path } = s;
        let name = if self.fold_names { fold_into(name, nbuf) } else { name };
        let mut i = 0;
        for (c, neg) in &self.conds {
            let Cond::Path(p) = c else { continue };
            let ok = match p {
                Pat::Sub(_) if i < 64 && dir.hits >> i & 1 == 1 => true,
                Pat::Sub(n) if i < 64 => {
                    let t = dir.folded.len().saturating_sub(n.len() - 1);
                    path.clear();
                    path.extend_from_slice(&dir.folded[t..]);
                    path.push(b'\\');
                    path.extend_from_slice(name);
                    contains(path, n)
                }
                _ => {
                    path.clear();
                    path.extend_from_slice(&dir.folded);
                    path.push(b'\\');
                    path.extend_from_slice(name);
                    p.test(path)
                }
            };
            i += 1;
            if ok == *neg {
                return false;
            }
        }
        true
    }

    /// 所有條件（測試、冒煙測試用）
    pub fn matches(&self, name: &str, path: &str, m: Meta) -> bool {
        let mut s = Scratch::default();
        self.matches_meta(&mut s, name.as_bytes(), m) && self.matches_path(&mut s, path.as_bytes())
    }
}

/// 以空白分隔；雙引號包住的部分可以含空白
fn split(q: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in q.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// 轉成小寫（Unicode）
pub fn fold(s: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    fold_into(s, &mut v).to_ascii_lowercase()
}

/// 除了 ASCII 字母以外沒有需要轉換的字元：ASCII，以及沒有大小寫的 U+3000–U+9FFF（中日文、注音、假名）、
/// U+B000–U+DFFF（韓文音節）。逐位元組判斷，不必解碼
fn caseless(s: &[u8]) -> bool {
    if s.is_ascii() {
        return true;
    }
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            0..0x80 => i += 1,
            0xE3..=0xE9 | 0xEB..=0xED => i += 3,
            _ => return false,
        }
    }
    true
}

/// 不需要轉換時直接回傳 `s`（比對時 ASCII 字母逐位元組轉小寫）；否則把小寫寫進 `buf`
fn fold_into<'a>(s: &'a [u8], buf: &'a mut Vec<u8>) -> &'a [u8] {
    if caseless(s) {
        return s;
    }
    buf.clear();
    for c in String::from_utf8_lossy(s).chars() {
        if c.is_ascii() {
            buf.push(c.to_ascii_lowercase() as u8);
        } else {
            for l in c.to_lowercase() {
                let mut b = [0u8; 4];
                buf.extend_from_slice(l.encode_utf8(&mut b).as_bytes());
            }
        }
    }
    buf
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.len() > hay.len() {
        return false;
    }
    let first = needle[0];
    'outer: for start in 0..=hay.len() - needle.len() {
        if hay[start].to_ascii_lowercase() != first {
            continue;
        }
        for k in 1..needle.len() {
            if hay[start + k].to_ascii_lowercase() != needle[k] {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

/// UTF-8 字元的位元組數（由第一個位元組判斷）
fn char_len(b: u8) -> usize {
    match b {
        0xF0.. => 4,
        0xE0.. => 3,
        0xC0.. => 2,
        _ => 1,
    }
}

/// 迭代式萬用字元比對：`*` 任意長度、`?` 一個字元（不是一個位元組）
fn glob(pat: &[u8], name: &[u8]) -> bool {
    let (mut p, mut n) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while n < name.len() {
        if p < pat.len() && pat[p] == b'?' {
            p += 1;
            n += char_len(name[n]);
        } else if p < pat.len() && pat[p] == name[n].to_ascii_lowercase() {
            p += 1;
            n += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = p;
            mark = n;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            mark += char_len(name[mark]);
            n = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

// ---------------------------------------------------------------- 篩選的值

/// `10mb`、`1.5g`、`512`（位元組）
fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let mul = match unit.trim() {
        "" | "b" => 1u64,
        "k" | "kb" => 1 << 10,
        "m" | "mb" => 1 << 20,
        "g" | "gb" => 1 << 30,
        "t" | "tb" => 1 << 40,
        _ => return None,
    };
    Some((n * mul as f64).round() as u64)
}

/// `>10mb`、`>=1g`、`<1k`、`<=5`、`1mb..1gb`、`=0`、`0`（相等）；回傳含兩端的範圍
fn parse_size(v: &str) -> Option<(u64, u64)> {
    if let Some((a, b)) = v.split_once("..") {
        let lo = if a.is_empty() { 0 } else { parse_bytes(a)? };
        let hi = if b.is_empty() { u64::MAX } else { parse_bytes(b)? };
        return (lo <= hi).then_some((lo, hi));
    }
    let (op, rest) = split_op(v);
    let n = parse_bytes(rest)?;
    Some(match op {
        ">" => (n.checked_add(1)?, u64::MAX),
        ">=" => (n, u64::MAX),
        "<" => (0, n.checked_sub(1)?),
        "<=" => (0, n),
        _ => (n, n),
    })
}

fn split_op(v: &str) -> (&str, &str) {
    for op in [">=", "<=", ">", "<", "="] {
        if let Some(rest) = v.strip_prefix(op) {
            return (op, rest);
        }
    }
    ("", v)
}

/// 公曆日期到 1970-01-01 的天數
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `2024`、`2024-05`、`2024-05-01`（也接受 `/`、`.`）：當地時間的 [開始, 結束)，以 1970 起的天數表示
fn parse_day_range(s: &str) -> Option<(i64, i64)> {
    let parts: Vec<i64> = s.split(['-', '/', '.']).map(|p| p.parse().ok()).collect::<Option<_>>()?;
    match parts[..] {
        [y] if (1601..=9999).contains(&y) => Some((days_from_civil(y, 1, 1), days_from_civil(y + 1, 1, 1))),
        [y, m] if (1..=12).contains(&m) => {
            let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
            Some((days_from_civil(y, m, 1), days_from_civil(ny, nm, 1)))
        }
        [y, m, d] if (1..=12).contains(&m) && (1..=31).contains(&d) => {
            let s = days_from_civil(y, m, d);
            Some((s, s + 1))
        }
        _ => None,
    }
}

/// `today`、`yesterday`、`7d`（最近 7 天）、`12h`、日期（見 `parse_day_range`），可加 `>`、`>=`、`<`、`<=`，
/// 或 `a..b`；回傳 Unix 秒的 [開始, 結束)
fn parse_date(v: &str, now: i64, offset: i64) -> Option<(i64, i64)> {
    const DAY: i64 = 86_400;
    let today = (now + offset).div_euclid(DAY);
    // 當地的某一天 → UTC 的秒
    let at = |day: i64| day * DAY - offset;
    let lower = v.to_ascii_lowercase();
    let v = lower.as_str();
    let days = |s: &str| -> Option<(i64, i64)> {
        match s {
            "today" => Some((today, today + 1)),
            "yesterday" => Some((today - 1, today)),
            _ => parse_day_range(s),
        }
    };
    if let Some(n) = v.strip_suffix('d').and_then(|n| n.parse::<i64>().ok()) {
        return Some((now - n * DAY, i64::MAX));
    }
    if let Some(n) = v.strip_suffix('h').and_then(|n| n.parse::<i64>().ok()) {
        return Some((now - n * 3600, i64::MAX));
    }
    if let Some((a, b)) = v.split_once("..") {
        let lo = if a.is_empty() { i64::MIN } else { at(days(a)?.0) };
        let hi = if b.is_empty() { i64::MAX } else { at(days(b)?.1) };
        return (lo < hi).then_some((lo, hi));
    }
    let (op, rest) = split_op(v);
    let (s, e) = days(rest)?;
    Some(match op {
        ">" => (at(e), i64::MAX),
        ">=" => (at(s), i64::MAX),
        "<" => (i64::MIN, at(s)),
        "<=" => (i64::MIN, at(e)),
        _ => (at(s), at(e)),
    })
}

fn parse_sort(v: &str) -> Option<Sort> {
    let v = v.to_ascii_lowercase();
    let (k, dir) = match v.rsplit_once('-') {
        Some((k, d @ ("asc" | "desc"))) => (k.to_string(), Some(d == "desc")),
        _ => (v.clone(), None),
    };
    let key = match k.as_str() {
        "name" => SortKey::Name,
        "path" => SortKey::Path,
        "ext" | "type" => SortKey::Ext,
        "size" => SortKey::Size,
        "date" | "dm" => SortKey::Date,
        _ => return None,
    };
    let desc = dir.unwrap_or(matches!(key, SortKey::Size | SortKey::Date));
    Some(Sort { key, desc })
}

/// 當地時間與 UTC 的差（秒），依 `now` 當時的日光節約時間規則
fn local_offset(now: i64) -> i64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime};
    let ft = ((now + 11_644_473_600) * 10_000_000) as u64;
    let ft = FILETIME { dwLowDateTime: ft as u32, dwHighDateTime: (ft >> 32) as u32 };
    let (mut utc, mut local, mut back) = (SYSTEMTIME::default(), SYSTEMTIME::default(), FILETIME::default());
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
            || SystemTimeToFileTime(&local, &mut back).is_err()
        {
            return 0;
        }
    }
    let v = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as i64;
    (v(back) - v(ft)) / 10_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: Meta = Meta { dir: false, size: 5, mtime: 0 };

    fn m(q: &str, name: &str) -> bool {
        NameQuery::parse_at(q, 0, 0).matches(name, "", FILE)
    }

    #[test]
    fn terms_and_case() {
        assert!(m("report", "Annual REPORT 2024.pdf"));
        assert!(m("rep 2024", "Annual REPORT 2024.pdf"));
        assert!(!m("rep 2023", "Annual REPORT 2024.pdf"));
        assert!(m("\"annual report\"", "Annual REPORT 2024.pdf"));
        assert!(!m("\"annual  report\"", "Annual REPORT 2024.pdf"));
        assert!(m("報告", "年度報告.docx"));
        assert!(!m("報告", "年度報表.docx"));
        assert!(NameQuery::parse("   ").is_empty());
        assert!(NameQuery::parse("*").is_empty());
        assert!(NameQuery::parse("sort:size").is_empty());
    }

    #[test]
    fn unicode_case() {
        assert!(m("ÉTÉ", "photos été.jpg"));
        assert!(m("straße", "STRASSE straße.txt"));
        assert!(m("ΣΟΦΙΑ", "σοφια.txt"));
        assert!(m("ｒｅａｄｍｅ", "ＲＥＡＤＭＥ.txt"));
        assert!(m("*.JPG", "相片.jpg"));
        assert!(m("ПРИВЕТ*", "привет мир.txt"));
    }

    #[test]
    fn wildcards() {
        assert!(m("*.jpg", "IMG_0001.JPG"));
        assert!(!m("*.jpg", "IMG_0001.JPG.bak"));
        assert!(m("img_????.jpg", "IMG_0001.jpg"));
        // ? 是一個字元：中文佔 3 個位元組
        assert!(m("相?.png", "相片.png"));
        assert!(!m("相?.png", "相片片.png"));
        assert!(m("*片*", "相片片.png"));
        assert!(m("*.jpg 2024", "2024 旅行.jpg"));
    }

    #[test]
    fn negation() {
        assert!(m("report !draft", "report.pdf"));
        assert!(!m("report !draft", "report draft.pdf"));
        assert!(!m("!*.tmp", "a.tmp"));
        assert!(m("!", "!"));
    }

    #[test]
    fn paths() {
        let q = NameQuery::parse_at(r"src\ *.rs", 0, 0);
        assert!(q.needs_path());
        assert!(q.matches("main.rs", r"E:\code\Src\main.rs", FILE));
        assert!(!q.matches("main.rs", r"E:\code\lib\main.rs", FILE));
        assert!(!q.matches("main.txt", r"E:\code\src\main.txt", FILE));
        let q = NameQuery::parse_at(r"c:\users\*\桌面\*", 0, 0);
        assert!(q.matches("a.txt", r"C:\Users\me\桌面\a.txt", FILE));
        assert!(!q.matches("a.txt", r"D:\Users\me\桌面\a.txt", FILE));
        assert!(!NameQuery::parse("abc").needs_path());
        // 預先比對資料夾：結果與比對完整路徑相同（條件跨越資料夾與名稱、完全在資料夾裡、萬用字元、排除）
        for qs in [r"de\ma", r"code\", r"\src\main", r"e:\code\*\main.*", r"!lib\", r"x\y"] {
            let q = NameQuery::parse_at(qs, 0, 0);
            for (dir, name) in [(r"E:\code\Src", "main.rs"), (r"E:\code\lib", "Main.RS"), (r"E:\Code", "x")] {
                let mut s = Scratch::default();
                let dm = q.dir_match(fold(dir.as_bytes()));
                let full = format!("{dir}\\{name}");
                assert_eq!(q.matches_in_dir(&mut s, &dm, name.as_bytes()), q.matches_path(&mut s, full.as_bytes()), "{qs} {full}");
            }
        }
        let q = NameQuery::parse_at(r"!\node_modules\ index.js", 0, 0);
        assert!(q.matches("index.js", r"C:\app\index.js", FILE));
        assert!(!q.matches("index.js", r"C:\app\node_modules\x\index.js", FILE));
    }

    #[test]
    fn filters() {
        let dir = Meta { dir: true, size: 1 << 30, mtime: 0 };
        let file = |size| Meta { dir: false, size, mtime: 0 };
        let q = |s| NameQuery::parse_at(s, 0, 0);
        assert!(q("ext:jpg;PNG").matches("a.Png", "", file(1)));
        assert!(!q("ext:jpg").matches("a.jpeg", "", file(1)));
        assert!(!q("ext:jpg").matches("x.jpg", "", dir));
        assert!(q("ext:.txt,md").matches("README.MD", "", file(1)));
        assert!(q("size:>10mb").matches("a", "", file(11 << 20)));
        assert!(!q("size:>10mb").matches("a", "", file(10 << 20)));
        assert!(q("size:<=1k").matches("a", "", file(1024)));
        assert!(q("size:1.5kb").matches("a", "", file(1536)));
        assert!(q("size:1mb..1gb").matches("a", "", dir));
        assert!(!q("size:1mb..1gb").matches("a", "", file(5)));
        assert!(q("size:0").matches("a", "", file(0)));
        assert!(q("folder:").matches("a", "", dir));
        assert!(!q("folder:").matches("a", "", file(1)));
        assert!(q("file: !ext:tmp").matches("a.txt", "", file(1)));
        assert!(!q("file: !ext:tmp").matches("a.tmp", "", file(1)));
        let bad = q("size:abc dm:13月 sort:color ext: 報告");
        assert_eq!(bad.errors, ["size:abc", "dm:13月", "sort:color", "ext:"]);
        assert!(bad.matches("報告.doc", "", file(1)));
    }

    #[test]
    fn dates() {
        // 2024-05-10 12:00 UTC，時區 +8
        let now = days_from_civil(2024, 5, 10) * 86_400 + 12 * 3600;
        let tz = 8 * 3600;
        let at = |y, mo, d, h: i64| (days_from_civil(y, mo, d) * 86_400 + h * 3600 - tz) as u32;
        let ok = |q: &str, t: u32| NameQuery::parse_at(q, now, tz).matches("a", "", Meta { dir: false, size: 1, mtime: t });
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        // 當地 2024-05-10 20:00
        assert!(ok("dm:today", at(2024, 5, 10, 0)));
        assert!(ok("dm:today", at(2024, 5, 10, 23)));
        assert!(!ok("dm:today", at(2024, 5, 9, 23)));
        assert!(ok("dm:yesterday", at(2024, 5, 9, 1)));
        assert!(ok("dm:2024-05", at(2024, 5, 31, 23)));
        assert!(!ok("dm:2024-05", at(2024, 6, 1, 0)));
        assert!(ok("dm:2024", at(2024, 1, 1, 0)));
        assert!(ok("dm:>2024-05-01", at(2024, 5, 2, 0)));
        assert!(!ok("dm:>2024-05-01", at(2024, 5, 1, 23)));
        assert!(ok("dm:<=2024-05-01", at(2024, 5, 1, 23)));
        assert!(ok("dm:2024-01..2024-03", at(2024, 3, 31, 12)));
        assert!(!ok("dm:2024-01..2024-03", at(2024, 4, 1, 0)));
        assert!(ok("dm:7d", (now - 6 * 86_400) as u32));
        assert!(!ok("dm:7d", (now - 8 * 86_400) as u32));
        assert!(ok("dm:2024/5/9", at(2024, 5, 9, 5)));
        // 不知道修改時間的項目不符合日期篩選
        assert!(!ok("dm:2024", 0));
    }

    #[test]
    fn sorting() {
        let s = |q| NameQuery::parse_at(q, 0, 0).sort;
        assert_eq!(s("a"), None);
        assert_eq!(s("a sort:size"), Some(Sort { key: SortKey::Size, desc: true }));
        assert_eq!(s("sort:name"), Some(Sort { key: SortKey::Name, desc: false }));
        assert_eq!(s("sort:date-asc"), Some(Sort { key: SortKey::Date, desc: false }));
        assert_eq!(s("sort:Path-Desc"), Some(Sort { key: SortKey::Path, desc: true }));
    }
}
