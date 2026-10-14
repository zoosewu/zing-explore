//! 批次重新命名的規則與預覽：搜尋與取代（文字或規則運算式）、範圍（主檔名／副檔名／全部）、
//! 編號 `{n}`、大小寫轉換；檢查不合法的名稱與衝突。
//!
//! 編號語法：`{n}` 從 1 開始；`{n:3}` 補零到 3 位；`{n:3:10}` 補零到 3 位、從 10 開始。
//! 編號只計算符合搜尋條件的項目（沒有符合的不佔號碼）。
//! 規則運算式的取代可用 `$1`、`${name}` 引用群組（`regex` 語法，`\w`、`\d`、`\p{Han}` 等支援 Unicode）。
//! 搜尋為空白時，`取代為` 的內容（展開編號後）直接成為新的名稱（依範圍）。

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// 主檔名（不含副檔名）
    #[default]
    Stem,
    /// 副檔名（不含點）
    Ext,
    /// 整個名稱
    Full,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaseMode {
    #[default]
    Keep,
    Lower,
    Upper,
    /// 每個字的第一個字母大寫
    Title,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub scope: Scope,
    pub case: CaseMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    Empty,
    /// 含有不能用在檔名的字元
    BadChar(char),
    /// 結尾是空白或句點
    TrailingDotOrSpace,
    /// CON、NUL、COM1… 等保留名稱
    Reserved,
    TooLong,
    /// 與另一個要改的項目同名
    Duplicate,
    /// 與資料夾中其他（未選取的）項目同名
    Exists,
}

impl Problem {
    pub fn describe(&self) -> String {
        match self {
            Problem::Empty => "名稱是空的".into(),
            Problem::BadChar(c) => format!("不能包含「{c}」"),
            Problem::TrailingDotOrSpace => "結尾不能是空白或句點".into(),
            Problem::Reserved => "是系統保留的名稱".into(),
            Problem::TooLong => "名稱太長".into(),
            Problem::Duplicate => "與另一個項目的新名稱相同".into(),
            Problem::Exists => "資料夾中已有同名項目".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub old: String,
    pub new: String,
    pub problem: Option<Problem>,
}

impl Planned {
    pub fn changed(&self) -> bool {
        self.old != self.new
    }
}

/// 名稱拆成 (主檔名, 副檔名)；資料夾、以點開頭且只有一個點的名稱沒有副檔名
fn split(name: &str, is_dir: bool) -> (&str, Option<&str>) {
    if is_dir {
        return (name, None);
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

/// 展開 `{n}`、`{n:寬度}`、`{n:寬度:起始}`；`index` 從 0 起算
fn expand_counter(template: &str, index: usize) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find("{n") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let spec = &after[..end];
        let parts: Vec<&str> = spec.strip_prefix(':').map_or(Vec::new(), |s| s.split(':').collect());
        let valid = spec.is_empty() || (spec.starts_with(':') && parts.len() <= 2 && parts.iter().all(|p| p.parse::<u64>().is_ok()));
        if !valid {
            out.push_str("{n");
            rest = after;
            continue;
        }
        let width = parts.first().and_then(|p| p.parse::<usize>().ok()).unwrap_or(0).min(12);
        let start = parts.get(1).and_then(|p| p.parse::<u64>().ok()).unwrap_or(1);
        out.push_str(&format!("{:0width$}", start + index as u64, width = width));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn apply_case(s: &str, mode: CaseMode) -> String {
    match mode {
        CaseMode::Keep => s.to_string(),
        CaseMode::Lower => s.to_lowercase(),
        CaseMode::Upper => s.to_uppercase(),
        CaseMode::Title => {
            let mut out = String::with_capacity(s.len());
            let mut start = true;
            for c in s.chars() {
                if c.is_alphanumeric() {
                    if start {
                        out.extend(c.to_uppercase());
                    } else {
                        out.extend(c.to_lowercase());
                    }
                    start = false;
                } else {
                    out.push(c);
                    start = true;
                }
            }
            out
        }
    }
}

fn replace_literal(s: &str, find: &str, with: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        return s.replace(find, with);
    }
    let (ls, lf) = (s.to_lowercase(), find.to_lowercase());
    // 小寫後長度改變（少數字元）時退回區分大小寫，避免切錯位置
    if ls.len() != s.len() || lf.len() != find.len() {
        return s.replace(find, with);
    }
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (i, _) in ls.match_indices(&lf) {
        out.push_str(&s[last..i]);
        out.push_str(with);
        last = i + find.len();
    }
    out.push_str(&s[last..]);
    out
}

pub fn validate(name: &str) -> Option<Problem> {
    if name.is_empty() {
        return Some(Problem::Empty);
    }
    if let Some(c) = name.chars().find(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (*c as u32) < 32) {
        return Some(Problem::BadChar(c));
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Some(Problem::TrailingDotOrSpace);
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    if reserved {
        return Some(Problem::Reserved);
    }
    if name.encode_utf16().count() > 255 {
        return Some(Problem::TooLong);
    }
    None
}

/// 計算新名稱。`items`：(名稱, 是否資料夾)，依清單順序（編號照這個順序）；
/// `others`：資料夾中沒有要改名的其他項目。規則運算式有誤時回傳錯誤訊息。
pub fn plan(items: &[(String, bool)], rule: &Rule, others: &[String]) -> Result<Vec<Planned>, String> {
    let re = if rule.regex && !rule.find.is_empty() {
        let pattern = if rule.case_sensitive { rule.find.clone() } else { format!("(?i){}", rule.find) };
        Some(regex::Regex::new(&pattern).map_err(|e| format!("規則運算式有誤：{e}"))?)
    } else {
        None
    };
    let mut matched = 0usize;
    let mut out: Vec<Planned> = items
        .iter()
        .map(|(name, is_dir)| {
            let (stem, ext) = split(name, *is_dir);
            let target = match rule.scope {
                Scope::Stem => stem,
                Scope::Ext => ext.unwrap_or(""),
                Scope::Full => name.as_str(),
            };
            let hit = if rule.find.is_empty() {
                true
            } else if let Some(re) = &re {
                re.is_match(target)
            } else if rule.case_sensitive {
                target.contains(rule.find.as_str())
            } else {
                target.to_lowercase().contains(&rule.find.to_lowercase())
            };
            let with = expand_counter(&rule.replace, matched);
            if hit {
                matched += 1;
            }
            let replaced = if rule.find.is_empty() {
                if rule.replace.is_empty() { target.to_string() } else { with }
            } else if let Some(re) = &re {
                re.replace_all(target, with.as_str()).into_owned()
            } else {
                replace_literal(target, &rule.find, &with, rule.case_sensitive)
            };
            let part = apply_case(&replaced, rule.case);
            let new = match rule.scope {
                Scope::Stem => match ext {
                    Some(e) => format!("{part}.{e}"),
                    None => part,
                },
                // 原本沒有副檔名的不加
                Scope::Ext => match ext {
                    Some(_) if part.is_empty() => stem.to_string(),
                    Some(_) => format!("{stem}.{part}"),
                    None => name.clone(),
                },
                Scope::Full => part,
            };
            Planned { old: name.clone(), problem: None, new }
        })
        .collect();

    let mut count: HashMap<String, usize> = HashMap::new();
    for p in &out {
        *count.entry(p.new.to_lowercase()).or_default() += 1;
    }
    let others: std::collections::HashSet<String> = others.iter().map(|s| s.to_lowercase()).collect();
    for p in &mut out {
        if !p.changed() {
            continue;
        }
        p.problem = validate(&p.new).or_else(|| {
            let key = p.new.to_lowercase();
            if count[&key] > 1 {
                Some(Problem::Duplicate)
            } else if others.contains(&key) {
                Some(Problem::Exists)
            } else {
                None
            }
        });
    }
    Ok(out)
}

/// 有新名稱等於另一個項目的舊名稱（例如 a→b、b→a）時，必須先全部改成暫時名稱再改成新名稱；
/// 只差大小寫的改名（a.txt → A.txt）不算
pub fn needs_two_phase(pairs: &[(String, String)]) -> bool {
    let olds: std::collections::HashSet<String> = pairs.iter().map(|(o, _)| o.to_lowercase()).collect();
    pairs.iter().any(|(o, n)| !o.eq_ignore_ascii_case(n) && olds.contains(&n.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[&str]) -> Vec<(String, bool)> {
        names.iter().map(|n| (n.to_string(), false)).collect()
    }

    fn news(p: &[Planned]) -> Vec<&str> {
        p.iter().map(|x| x.new.as_str()).collect()
    }

    #[test]
    fn literal_replace_in_stem_only() {
        let r = Rule { find: "img".into(), replace: "相片".into(), ..Default::default() };
        let p = plan(&files(&["IMG_001.jpg", "img.img"]), &r, &[]).unwrap();
        // 預設不區分大小寫，只改主檔名
        assert_eq!(news(&p), vec!["相片_001.jpg", "相片.img"]);
        let r = Rule { case_sensitive: true, ..r };
        assert_eq!(news(&plan(&files(&["IMG_001.jpg"]), &r, &[]).unwrap()), vec!["IMG_001.jpg"]);
    }

    #[test]
    fn regex_groups_and_counter() {
        let r = Rule { find: r"^(\d{4})(\d{2})(\d{2})".into(), replace: "$1-$2-$3 #{n:2}".into(), regex: true, ..Default::default() };
        let p = plan(&files(&["20261014_a.png", "20261015_b.png", "x.png"]), &r, &[]).unwrap();
        assert_eq!(news(&p), vec!["2026-10-14 #01_a.png", "2026-10-15 #02_b.png", "x.png"]);
        let bad = Rule { find: "(".into(), regex: true, ..Default::default() };
        assert!(plan(&files(&["a"]), &bad, &[]).is_err());
    }

    #[test]
    fn empty_find_sets_whole_part() {
        let r = Rule { replace: "旅行 {n:3:10}".into(), ..Default::default() };
        let mut items = files(&["a.jpg", "b.JPG"]);
        items.push(("資料夾.v2".into(), true));
        let p = plan(&items, &r, &[]).unwrap();
        assert_eq!(news(&p), vec!["旅行 010.jpg", "旅行 011.JPG", "旅行 012"]);
    }

    #[test]
    fn extension_scope_and_case() {
        let r = Rule { scope: Scope::Ext, case: CaseMode::Lower, ..Default::default() };
        let p = plan(&files(&["A.JPG", "README", "B.Png"]), &r, &[]).unwrap();
        assert_eq!(news(&p), vec!["A.jpg", "README", "B.png"]);
        let r = Rule { scope: Scope::Full, case: CaseMode::Title, ..Default::default() };
        assert_eq!(news(&plan(&files(&["hello WORLD-x.txt"]), &r, &[]).unwrap()), vec!["Hello World-X.Txt"]);
    }

    #[test]
    fn detects_problems() {
        let r = Rule { replace: "same".into(), ..Default::default() };
        let p = plan(&files(&["a.txt", "b.txt"]), &r, &[]).unwrap();
        assert!(p.iter().all(|x| x.problem == Some(Problem::Duplicate)));
        let r = Rule { find: "a".into(), replace: "c".into(), ..Default::default() };
        let p = plan(&files(&["a.txt"]), &r, &["C.TXT".into()]).unwrap();
        assert_eq!(p[0].problem, Some(Problem::Exists));
        assert_eq!(validate("CON.txt"), Some(Problem::Reserved));
        assert_eq!(validate("COM0"), None);
        assert_eq!(validate("a?b"), Some(Problem::BadChar('?')));
        assert_eq!(validate("a."), Some(Problem::TrailingDotOrSpace));
        assert_eq!(validate(""), Some(Problem::Empty));
        // 沒有改變的不檢查
        let p = plan(&files(&["CON.txt"]), &Rule::default(), &[]).unwrap();
        assert_eq!(p[0].problem, None);
    }

    #[test]
    fn two_phase_only_for_overlaps() {
        let p = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        assert!(needs_two_phase(&p(&[("a", "b"), ("b", "a")])));
        assert!(needs_two_phase(&p(&[("1", "2"), ("2", "3")])));
        assert!(!needs_two_phase(&p(&[("a.txt", "A.txt"), ("b", "c")])));
    }

    #[test]
    fn regex_is_unicode_aware() {
        // \w、\d 包含中文與全形數字；(?i) 對非英文字母也有效
        let r = Rule { find: r"^(\w+)\s*(\d+)$".into(), replace: "$2-$1".into(), regex: true, ..Default::default() };
        assert_eq!(news(&plan(&files(&["旅行相片 ３.jpg"]), &r, &[]).unwrap()), vec!["３-旅行相片.jpg"]);
        let r = Rule { find: r"\p{Han}+".into(), replace: "X".into(), regex: true, ..Default::default() };
        assert_eq!(news(&plan(&files(&["a相片b.txt"]), &r, &[]).unwrap()), vec!["aXb.txt"]);
        let r = Rule { find: "ä".into(), replace: "ae".into(), regex: true, ..Default::default() };
        assert_eq!(news(&plan(&files(&["Äpfel.txt"]), &r, &[]).unwrap()), vec!["aepfel.txt"]);
    }

    #[test]
    fn counter_skips_unmatched() {
        let r = Rule { find: r"^p(\d)$".into(), replace: "q{n:2}".into(), regex: true, ..Default::default() };
        let p = plan(&files(&["a.txt", "p1.txt", "b.txt", "p2.txt"]), &r, &[]).unwrap();
        assert_eq!(news(&p), vec!["a.txt", "q01.txt", "b.txt", "q02.txt"]);
    }

    #[test]
    fn counter_syntax() {
        assert_eq!(expand_counter("x{n}y", 0), "x1y");
        assert_eq!(expand_counter("{n:4}", 6), "0007");
        assert_eq!(expand_counter("{n:2:0}-{n}", 3), "03-4");
        assert_eq!(expand_counter("{name}{n:x}", 0), "{name}{n:x}");
    }
}
