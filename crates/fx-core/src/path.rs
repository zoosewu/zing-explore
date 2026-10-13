//! 路徑工具。UI 層以 `String` 保存路徑；空字串代表「此電腦」（磁碟機清單）。

/// 去除多餘的結尾反斜線（磁碟根目錄 `C:\` 保留），並把 `/` 轉成 `\`。
pub fn normalize(p: &str) -> String {
    let mut s = p.trim().trim_matches('"').replace('/', "\\");
    if s.len() == 2 && s.ends_with(':') {
        s.push('\\');
    }
    while s.len() > 3 && s.ends_with('\\') && !s.ends_with(":\\") {
        s.pop();
    }
    s
}

pub fn is_root(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() == 3 && b[1] == b':' && b[2] == b'\\'
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return name.to_string();
    }
    if dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}\\{name}")
    }
}

/// 上一層。`C:\` 的上一層是「此電腦」（空字串）；「此電腦」沒有上一層。
pub fn parent(p: &str) -> Option<String> {
    if p.is_empty() {
        return None;
    }
    if is_root(p) {
        return Some(String::new());
    }
    let trimmed = p.trim_end_matches('\\');
    let idx = trimmed.rfind('\\')?;
    let parent = &trimmed[..idx];
    if parent.len() == 2 && parent.ends_with(':') {
        return Some(format!("{parent}\\"));
    }
    if parent.is_empty() || parent == "\\" {
        // UNC 路徑（\\server\share）的頂層
        return Some(String::new());
    }
    Some(parent.to_string())
}

/// 最後一段名稱；根目錄回傳 `C:`，「此電腦」回傳空字串。
pub fn file_name(p: &str) -> &str {
    let t = p.trim_end_matches('\\');
    t.rsplit('\\').next().unwrap_or(t)
}

/// 麵包屑：每段的顯示名稱與完整路徑。第一段固定為「此電腦」。
pub fn segments(p: &str) -> Vec<(String, String)> {
    let mut out = vec![("此電腦".to_string(), String::new())];
    if p.is_empty() {
        return out;
    }
    let mut chain = Vec::new();
    let mut cur = Some(p.to_string());
    while let Some(c) = cur {
        if c.is_empty() {
            break;
        }
        cur = parent(&c);
        chain.push(c);
    }
    for full in chain.into_iter().rev() {
        out.push((file_name(&full).to_string(), full));
    }
    out
}

/// 超過 MAX_PATH 時加上 `\\?\` 前綴，讓 Win32 API 接受長路徑。
pub fn long_form(p: &str) -> String {
    if p.len() < 240 || p.starts_with("\\\\?\\") {
        return p.to_string();
    }
    if let Some(unc) = p.strip_prefix("\\\\") {
        format!("\\\\?\\UNC\\{unc}")
    } else {
        format!("\\\\?\\{p}")
    }
}

pub fn to_wide_z(p: &str) -> Vec<u16> {
    crate::text::wide_z(p)
}

/// `a` 是否為 `b` 本身或其上層
pub fn is_ancestor_or_self(a: &str, b: &str) -> bool {
    if a.is_empty() {
        return true;
    }
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    b == a || (b.starts_with(&a) && (a.ends_with('\\') || b[a.len()..].starts_with('\\')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(normalize("c:"), "c:\\");
        assert_eq!(normalize("C:/Users/x/"), "C:\\Users\\x");
        assert_eq!(normalize("C:\\"), "C:\\");
        assert_eq!(parent("C:\\Users"), Some("C:\\".into()));
        assert_eq!(parent("C:\\"), Some(String::new()));
        assert_eq!(parent(""), None);
        assert_eq!(parent("C:\\a\\b"), Some("C:\\a".into()));
        assert_eq!(join("C:\\", "x"), "C:\\x");
        assert_eq!(join("C:\\a", "x"), "C:\\a\\x");
        assert_eq!(file_name("C:\\a\\bc"), "bc");
        assert_eq!(file_name("C:\\"), "C:");
        let s = segments("C:\\a\\b");
        let names: Vec<_> = s.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["此電腦", "C:", "a", "b"]);
        assert_eq!(s[2].1, "C:\\a");
        assert!(is_ancestor_or_self("C:\\a", "c:\\A\\b"));
        assert!(!is_ancestor_or_self("C:\\a", "C:\\ab"));
        assert!(is_ancestor_or_self("C:\\", "C:\\ab"));
    }
}
