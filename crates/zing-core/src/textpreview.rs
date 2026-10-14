//! 文字預覽：判斷檔案開頭是不是文字、解碼、切成行。
//!
//! 只看檔案開頭（呼叫端讀取前 256 KB）。編碼依序判斷：BOM（UTF-8、UTF-16 LE/BE）、合法的 UTF-8、
//! 其餘交給呼叫端提供的系統 ANSI 字碼頁轉換（繁體中文 Windows 為 Big5）。

/// 最多顯示的行數與每行字元數
pub const MAX_LINES: usize = 4000;
pub const MAX_LINE_CHARS: usize = 1000;

/// 一定當成文字的副檔名（小寫）
const TEXT_EXTS: &[&str] = &[
    "txt", "md", "log", "ini", "cfg", "conf", "toml", "json", "jsonc", "yaml", "yml", "xml", "csv", "tsv", "sql", "rs",
    "c", "h", "cpp", "hpp", "cc", "cs", "java", "kt", "go", "py", "rb", "js", "mjs", "ts", "tsx", "jsx", "css", "scss",
    "html", "htm", "vue", "lua", "sh", "ps1", "psm1", "bat", "cmd", "reg", "gitignore", "editorconfig", "svg", "srt",
];

/// 檔案開頭看起來是文字嗎：有 BOM，或副檔名在清單中，或沒有 NUL 且是合法的 UTF-8（容許結尾截斷）
pub fn looks_like_text(ext: &str, head: &[u8]) -> bool {
    if head.starts_with(&[0xEF, 0xBB, 0xBF]) || head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return true;
    }
    if TEXT_EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
        return !head.contains(&0);
    }
    !head.is_empty() && !head.contains(&0) && utf8_prefix(head).is_some()
}

/// 合法的 UTF-8；只有結尾被截斷的多位元組字元時，去掉它
fn utf8_prefix(b: &[u8]) -> Option<&str> {
    match std::str::from_utf8(b) {
        Ok(s) => Some(s),
        Err(e) if e.error_len().is_none() && b.len() - e.valid_up_to() < 4 => {
            std::str::from_utf8(&b[..e.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

fn utf16(b: &[u8], big_endian: bool) -> String {
    let units: Vec<u16> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| if big_endian { u16::from_be_bytes(*c) } else { u16::from_le_bytes(*c) })
        .collect();
    String::from_utf16_lossy(&units)
}

/// 解碼；`ansi` 把系統字碼頁的位元組轉成字串
pub fn decode(b: &[u8], ansi: impl Fn(&[u8]) -> String) -> String {
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, false);
    }
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, true);
    }
    match utf8_prefix(b) {
        Some(s) => s.to_string(),
        None => ansi(b),
    }
}

/// 切成行：去掉換行字元、Tab 換成 4 個空白、限制行數與每行長度。回傳 (行, 是否截斷)
pub fn lines(text: &str) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    for line in text.split('\n') {
        if out.len() == MAX_LINES {
            return (out, true);
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut s = String::with_capacity(line.len().min(MAX_LINE_CHARS * 3));
        for (n, ch) in line.chars().enumerate() {
            if n == MAX_LINE_CHARS {
                s.push('…');
                break;
            }
            match ch {
                '\t' => s.push_str("    "),
                c if c.is_control() => s.push(' '),
                c => s.push(c),
            }
        }
        out.push(s);
    }
    // 檔案以換行結尾時不多出一行空白
    if out.last().is_some_and(|l| l.is_empty()) && out.len() > 1 {
        out.pop();
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_ansi(_: &[u8]) -> String {
        "ANSI".into()
    }

    #[test]
    fn detects_text() {
        assert!(looks_like_text("txt", "報告".as_bytes()));
        assert!(looks_like_text("", b"plain\nascii"));
        assert!(!looks_like_text("png", &[0x89, b'P', b'N', b'G', 0, 0]));
        assert!(!looks_like_text("bin", &[0xff, 0xfe - 1, 0x80, 0x81]));
        assert!(looks_like_text("dat", &[0xFF, 0xFE, b'a', 0]));
        // UTF-8 在 256 KB 處被截斷的多位元組字元
        let mut b = "中文".as_bytes().to_vec();
        b.pop();
        assert!(looks_like_text("", &b));
        assert_eq!(decode(&b, no_ansi), "中");
    }

    #[test]
    fn decodes_encodings() {
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'h', b'i'], no_ansi), "hi");
        assert_eq!(decode(&[0xFF, 0xFE, 0x2D, 0x4E], no_ansi), "中");
        assert_eq!(decode(&[0xFE, 0xFF, 0x4E, 0x2D], no_ansi), "中");
        assert_eq!(decode(&[0xA4, 0xA4], no_ansi), "ANSI"); // Big5 的「中」
    }

    #[test]
    fn splits_lines() {
        let (l, cut) = lines("a\r\n\tb\x07\n");
        assert_eq!(l, vec!["a".to_string(), "    b ".to_string()]);
        assert!(!cut);
        let many = "x\n".repeat(MAX_LINES + 5);
        let (l, cut) = lines(&many);
        assert_eq!(l.len(), MAX_LINES);
        assert!(cut);
        let long = "y".repeat(MAX_LINE_CHARS + 10);
        assert_eq!(lines(&long).0[0].chars().count(), MAX_LINE_CHARS + 1);
    }
}
