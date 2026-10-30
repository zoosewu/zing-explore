//! 快速鍵字串解析，例如 `Ctrl+Shift+P`、`Alt+Up`、`F2`、`Ctrl+WheelUp`；以及快速鍵生效的情境（`Scope`）。

use std::fmt;

/// 快速鍵生效的情境：按下按鍵時先找目前情境的綁定（由具體到一般），最後才找全域的。
/// 設定檔中以 `[[key]]` 的 `when` 指定（省略為全域）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    #[default]
    Global,
    /// 焦點在檔案清單
    List,
    /// 焦點在檔案清單，而且正在篩選（篩選字串不為空）：優先於 `List`
    Filter,
    /// 焦點在樹狀列
    Tree,
    /// 下拉選單、命令面板、命令列（F10）、右側面板、歡迎畫面這類「上下選一個」的介面（`ui.*` 命令）
    Popup,
    /// 全磁碟搜尋的面板：優先於 `Popup`
    Search,
}

impl Scope {
    pub const ALL: [Scope; 6] = [Scope::Global, Scope::List, Scope::Filter, Scope::Tree, Scope::Popup, Scope::Search];

    /// 設定檔中的寫法（全域為空字串）
    pub fn name(self) -> &'static str {
        match self {
            Scope::Global => "",
            Scope::List => "list",
            Scope::Filter => "filter",
            Scope::Tree => "tree",
            Scope::Popup => "popup",
            Scope::Search => "search",
        }
    }

    pub fn parse(s: &str) -> Option<Scope> {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("global") {
            return Some(Scope::Global);
        }
        Scope::ALL.into_iter().find(|w| w.name().eq_ignore_ascii_case(s))
    }

    /// 顯示名稱（原文，顯示時 `i18n::tr`）
    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => tr_key!("全域"),
            Scope::List => tr_key!("清單"),
            Scope::Filter => tr_key!("篩選中"),
            Scope::Tree => tr_key!("樹狀列"),
            Scope::Popup => tr_key!("彈出清單"),
            Scope::Search => tr_key!("全磁碟搜尋"),
        }
    }

    /// 命令所屬的情境：快速鍵編輯器在這個命令上錄下的按鍵屬於這個情境
    pub fn of_command(script: &str) -> Scope {
        let name = script.split_whitespace().next().unwrap_or("");
        match name {
            "filter.clear" | "filter.backspace" => Scope::Filter,
            "search.sort" | "search.openitem" => Scope::Search,
            // tree.toggle（顯示／隱藏樹狀列）是全域的
            "tree.up" | "tree.down" | "tree.expand" | "tree.collapse" | "tree.open" | "tree.leave" | "tree.moveup" | "tree.movedown" => {
                Scope::Tree
            }
            _ if name.starts_with("cursor.") => Scope::List,
            _ if name.starts_with("ui.") => Scope::Popup,
            _ => Scope::Global,
        }
    }

    /// 按下按鍵時依序查找的情境（`self` 是目前最具體的情境），最後是全域
    pub fn chain(self) -> &'static [Scope] {
        match self {
            Scope::Global => &[Scope::Global],
            Scope::List => &[Scope::List, Scope::Global],
            Scope::Filter => &[Scope::Filter, Scope::List, Scope::Global],
            Scope::Tree => &[Scope::Tree, Scope::Global],
            Scope::Popup => &[Scope::Popup],
            Scope::Search => &[Scope::Search, Scope::Popup],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Win32 虛擬鍵碼（滾輪是 `WHEEL_UP`／`WHEEL_DOWN`）
    pub vk: u16,
    /// 生效的情境：同一個按鍵在不同情境是不同的快速鍵
    pub when: Scope,
}

/// 滾輪往上、往下（未使用的虛擬鍵碼），由視窗層轉成按鍵事件
pub const WHEEL_UP: u16 = 0x0E;
pub const WHEEL_DOWN: u16 = 0x0F;

/// 有名稱的按鍵（顯示用第一個名稱，解析時不分大小寫）
const NAMED: &[(&str, u16)] = &[
    ("Backspace", 0x08),
    ("Back", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Return", 0x0D),
    ("Esc", 0x1B),
    ("Escape", 0x1B),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PgUp", 0x21),
    ("PageDown", 0x22),
    ("PgDn", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Insert", 0x2D),
    ("Ins", 0x2D),
    ("Delete", 0x2E),
    ("Del", 0x2E),
    ("Apps", 0x5D),
    ("Menu", 0x5D),
    ("BrowserBack", 0xA6),
    ("BrowserForward", 0xA7),
    ("Semicolon", 0xBA),
    ("Equals", 0xBB),
    ("Plus", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Backtick", 0xC0),
    ("[", 0xDB),
    ("Backslash", 0xDC),
    ("]", 0xDD),
    ("Quote", 0xDE),
    // 滑鼠側鍵（VK_XBUTTON1/2）與滾輪，由視窗層轉成按鍵事件
    ("MouseBack", 0x05),
    ("MouseForward", 0x06),
    ("WheelUp", WHEEL_UP),
    ("WheelDown", WHEEL_DOWN),
];

impl KeyChord {
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut chord = KeyChord::default();
        for part in s.split('+').map(str::trim) {
            let lower = part.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" => chord.alt = true,
                "" => return Err(tr!("快速鍵格式錯誤：{s}", s)),
                _ => {
                    if chord.vk != 0 {
                        return Err(tr!("快速鍵只能有一個主鍵：{s}", s));
                    }
                    chord.vk = key_code(&lower).ok_or_else(|| tr!("未知按鍵「{part}」：{s}", part, s))?;
                }
            }
        }
        if chord.vk == 0 {
            return Err(tr!("快速鍵缺少主鍵：{s}", s));
        }
        Ok(chord)
    }

    /// 同一個按鍵在另一個情境
    pub fn in_scope(self, when: Scope) -> KeyChord {
        KeyChord { when, ..self }
    }

    /// 沒有 Ctrl、Alt 的可列印字元鍵（字母、數字、空白、標點）：篩選中、輸入框裡這些按鍵是文字
    pub fn is_printable(self) -> bool {
        !self.ctrl && !self.alt && matches!(self.vk, 0x20 | 0x30..=0x39 | 0x41..=0x5A | 0xBA..=0xC0 | 0xDB..=0xDE)
    }
}

fn key_code(name: &str) -> Option<u16> {
    let b = name.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_alphanumeric()) {
        return Some(b[0].to_ascii_uppercase() as u16);
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
        && (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    NAMED.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|&(_, v)| v)
}

/// 按鍵的寫法（不含修飾鍵與情境）
pub fn key_name(vk: u16) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        vk => match NAMED.iter().find(|&&(_, v)| v == vk) {
            Some((name, _)) => name.to_string(),
            None => format!("0x{vk:02X}"),
        },
    }
}

impl fmt::Display for KeyChord {
    /// 設定檔中的寫法（不含情境，情境另外寫在 `when`）
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        f.write_str(&key_name(self.vk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chords() {
        let k = KeyChord::parse("Ctrl+Shift+P").unwrap();
        assert!(k.ctrl && k.shift && !k.alt);
        assert_eq!(k.vk, b'P' as u16);
        assert_eq!(k.when, Scope::Global);
        assert_eq!(KeyChord::parse("F2").unwrap().vk, 0x71);
        assert_eq!(KeyChord::parse("alt+up").unwrap().vk, 0x26);
        assert_eq!(KeyChord::parse("Delete").unwrap().to_string(), "Delete");
        assert_eq!(KeyChord::parse("ctrl+l").unwrap().to_string(), "Ctrl+L");
        assert_eq!(KeyChord::parse("pgdn").unwrap().to_string(), "PageDown");
        assert_eq!(KeyChord::parse("mouseback").unwrap().to_string(), "MouseBack");
        assert_eq!(KeyChord::parse("Ctrl+WheelUp").unwrap().vk, WHEEL_UP);
        assert!(KeyChord::parse("Ctrl+").is_err());
        assert!(KeyChord::parse("Ctrl+A+B").is_err());
        assert!(KeyChord::parse("Hyper+A").is_err());
    }

    #[test]
    fn scopes() {
        for s in Scope::ALL {
            assert_eq!(Scope::parse(s.name()), Some(s));
            assert_eq!(*s.chain().first().unwrap(), s, "最具體的在前面");
        }
        assert_eq!(Scope::parse("LIST"), Some(Scope::List));
        assert_eq!(Scope::parse("global"), Some(Scope::Global));
        assert_eq!(Scope::parse("nowhere"), None);
        assert_eq!(Scope::of_command("cursor.down --extend"), Scope::List);
        assert_eq!(Scope::of_command("tree.expand"), Scope::Tree);
        assert_eq!(Scope::of_command("tree.toggle"), Scope::Global);
        assert_eq!(Scope::of_command("ui.accept"), Scope::Popup);
        assert_eq!(Scope::of_command("filter.backspace"), Scope::Filter);
        assert_eq!(Scope::of_command("search.sort --by=size"), Scope::Search);
        assert_eq!(Scope::of_command("search.open"), Scope::Global);
        assert_eq!(Scope::of_command("nav.up"), Scope::Global);
        // 彈出清單不往全域找：選單開著時按全域的快速鍵不該執行命令
        assert!(!Scope::Popup.chain().contains(&Scope::Global));
        let k = KeyChord::parse("Up").unwrap();
        assert_ne!(k, k.in_scope(Scope::Tree));
        assert!(KeyChord::parse("J").unwrap().is_printable());
        assert!(KeyChord::parse("Shift+Slash").unwrap().is_printable());
        assert!(!KeyChord::parse("Ctrl+J").unwrap().is_printable());
        assert!(!KeyChord::parse("Down").unwrap().is_printable());
    }
}
