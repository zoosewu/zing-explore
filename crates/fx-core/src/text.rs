//! UTF-16 文字工具：不分大小寫比較、自然排序、篩選比對。
//! 全部直接在 UTF-16 切片上運作，避免排序/篩選時轉換字串。

use std::cmp::Ordering;

/// 單一 UTF-16 單位的小寫折疊。ASCII 走快速路徑；代理字元原樣回傳。
#[inline]
pub fn fold(c: u16) -> u32 {
    if c < 0x80 {
        let b = c as u8;
        return b.to_ascii_lowercase() as u32;
    }
    // 中日韓文字等沒有大小寫的區段直接回傳（排除其中少數有大小寫的區段）
    if c >= 0x2E80 && !matches!(c, 0xA640..=0xA7FF | 0xAB70..=0xABBF | 0xFF21..=0xFF3A) {
        return c as u32;
    }
    match char::from_u32(c as u32) {
        Some(ch) => {
            let mut lower = ch.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) => l as u32,
                _ => c as u32,
            }
        }
        None => c as u32,
    }
}

#[inline]
fn is_digit(c: u16) -> bool {
    (b'0' as u16..=b'9' as u16).contains(&c)
}

pub fn eq_ignore_case(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| x == y || fold(x) == fold(y))
}

/// 自然排序（類檔案總管）：數字段以數值比較，其餘不分大小寫。
pub fn natural_cmp(a: &[u16], b: &[u16]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (ca, cb) = (a[i], b[j]);
        if is_digit(ca) && is_digit(cb) {
            let si = i;
            let sj = j;
            while i < a.len() && is_digit(a[i]) {
                i += 1;
            }
            while j < b.len() && is_digit(b[j]) {
                j += 1;
            }
            // 去掉前導零後，位數多者較大，位數相同則逐位比較
            let ra = trim_zeros(&a[si..i]);
            let rb = trim_zeros(&b[sj..j]);
            let ord = ra.len().cmp(&rb.len()).then_with(|| ra.cmp(rb));
            if ord != Ordering::Equal {
                return ord;
            }
            continue;
        }
        if ca != cb {
            let ord = fold(ca).cmp(&fold(cb));
            if ord != Ordering::Equal {
                return ord;
            }
        }
        i += 1;
        j += 1;
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

fn trim_zeros(s: &[u16]) -> &[u16] {
    let n = s.iter().take_while(|&&c| c == b'0' as u16).count();
    &s[n.min(s.len().saturating_sub(1))..]
}

/// 已折疊的篩選樣式。含 `*` 或 `?` 時以萬用字元比對整個檔名，否則為子字串比對。
#[derive(Clone, Debug, Default)]
pub struct Pattern {
    folded: Vec<u32>,
    glob: bool,
}

impl Pattern {
    pub fn new(pattern: &str) -> Self {
        let folded: Vec<u32> = pattern.encode_utf16().map(fold).collect();
        let glob = folded.iter().any(|&c| c == '*' as u32 || c == '?' as u32);
        Self { folded, glob }
    }

    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }

    pub fn matches(&self, name: &[u16]) -> bool {
        if self.folded.is_empty() {
            return true;
        }
        if self.glob {
            glob_match(&self.folded, name)
        } else {
            contains_folded(name, &self.folded)
        }
    }
}

fn contains_folded(hay: &[u16], needle: &[u32]) -> bool {
    if needle.len() > hay.len() {
        return false;
    }
    let first = needle[0];
    'outer: for start in 0..=hay.len() - needle.len() {
        if fold(hay[start]) != first {
            continue;
        }
        for (k, &n) in needle.iter().enumerate().skip(1) {
            if fold(hay[start + k]) != n {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

/// 迭代式萬用字元比對（`*` 任意長度、`?` 單一字元），O(n·m) 最壞情況、無遞迴。
fn glob_match(pat: &[u32], name: &[u16]) -> bool {
    let (mut p, mut n) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while n < name.len() {
        if p < pat.len() && (pat[p] == '?' as u32 || pat[p] == fold(name[n])) {
            p += 1;
            n += 1;
        } else if p < pat.len() && pat[p] == '*' as u32 {
            star = p;
            mark = n;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            mark += 1;
            n = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == '*' as u32 {
        p += 1;
    }
    p == pat.len()
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nc(a: &str, b: &str) -> Ordering {
        natural_cmp(&wide(a), &wide(b))
    }

    #[test]
    fn natural_order() {
        assert_eq!(nc("file2", "file10"), Ordering::Less);
        assert_eq!(nc("File2", "file2"), Ordering::Less); // 大小寫僅作為最終決勝
        assert_eq!(nc("a", "B"), Ordering::Less);
        assert_ne!(nc("img007", "img7"), Ordering::Equal); // 數值相同時仍有確定順序
        assert_eq!(nc("x", "x1"), Ordering::Less);
        assert_eq!(nc("報告2", "報告10"), Ordering::Less);
    }

    #[test]
    fn patterns() {
        let p = Pattern::new("RS");
        assert!(p.matches(&wide("main.rs")));
        assert!(!p.matches(&wide("main.c")));
        let g = Pattern::new("*.rs");
        assert!(g.matches(&wide("Main.RS")));
        assert!(!g.matches(&wide("main.rsx")));
        assert!(Pattern::new("a?c").matches(&wide("abc")));
        assert!(Pattern::new("文件").matches(&wide("我的文件.docx")));
    }
}
