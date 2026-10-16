//! 全磁碟搜尋的名稱比對（索引中的名稱是 UTF-8）。
//!
//! 查詢以空白分隔成多個詞，每個詞都要符合（AND）；雙引號包住的部分可以含空白。
//! 含 `*` 或 `?` 的詞以萬用字元比對整個名稱，其他的詞是子字串。
//! 大小寫：只有 ASCII 字母不分大小寫（逐位元組比對，兩百萬個名稱約數十毫秒）；其他文字須完全相同。

#[derive(Clone, Debug, PartialEq, Eq)]
enum Term {
    Sub(Vec<u8>),
    Glob(Vec<u8>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameQuery {
    terms: Vec<Term>,
}

impl NameQuery {
    pub fn parse(q: &str) -> Self {
        let mut terms = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut push = |s: &mut String| {
            if !s.is_empty() {
                let b = s.to_ascii_lowercase().into_bytes();
                // 只有 * 的詞符合一切，不必比對
                if !b.iter().all(|&c| c == b'*') {
                    let glob = b.iter().any(|&c| c == b'*' || c == b'?');
                    terms.push(if glob { Term::Glob(b) } else { Term::Sub(b) });
                }
            }
            s.clear();
        };
        for c in q.chars() {
            match c {
                '"' => quoted = !quoted,
                c if c.is_whitespace() && !quoted => push(&mut cur),
                c => cur.push(c),
            }
        }
        push(&mut cur);
        NameQuery { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn matches(&self, name: &[u8]) -> bool {
        self.terms.iter().all(|t| match t {
            Term::Sub(n) => contains(name, n),
            Term::Glob(p) => glob(p, name),
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn m(q: &str, name: &str) -> bool {
        NameQuery::parse(q).matches(name.as_bytes())
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
}
