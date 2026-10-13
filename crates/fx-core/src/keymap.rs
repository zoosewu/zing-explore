//! 快速鍵字串解析，例如 `Ctrl+Shift+P`、`Alt+Up`、`F2`。

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Win32 虛擬鍵碼
    pub vk: u16,
}

const NAMED: &[(&str, u16)] = &[
    ("backspace", 0x08),
    ("back", 0x08),
    ("tab", 0x09),
    ("enter", 0x0D),
    ("return", 0x0D),
    ("esc", 0x1B),
    ("escape", 0x1B),
    ("space", 0x20),
    ("pageup", 0x21),
    ("pgup", 0x21),
    ("pagedown", 0x22),
    ("pgdn", 0x22),
    ("end", 0x23),
    ("home", 0x24),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
    ("insert", 0x2D),
    ("ins", 0x2D),
    ("delete", 0x2E),
    ("del", 0x2E),
    ("apps", 0x5D),
    ("menu", 0x5D),
    ("browserback", 0xA6),
    ("browserforward", 0xA7),
    ("semicolon", 0xBA),
    ("equals", 0xBB),
    ("plus", 0xBB),
    ("comma", 0xBC),
    ("minus", 0xBD),
    ("period", 0xBE),
    ("slash", 0xBF),
    ("backtick", 0xC0),
    ("[", 0xDB),
    ("backslash", 0xDC),
    ("]", 0xDD),
    ("quote", 0xDE),
    // 滑鼠側鍵（VK_XBUTTON1/2），由視窗層轉成按鍵事件
    ("mouseback", 0x05),
    ("mouseforward", 0x06),
];

impl KeyChord {
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut chord = KeyChord { ctrl: false, shift: false, alt: false, vk: 0 };
        for part in s.split('+').map(str::trim) {
            let lower = part.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" => chord.alt = true,
                "" => return Err(format!("快速鍵格式錯誤：{s}")),
                _ => {
                    if chord.vk != 0 {
                        return Err(format!("快速鍵只能有一個主鍵：{s}"));
                    }
                    chord.vk = key_code(&lower).ok_or_else(|| format!("未知按鍵「{part}」：{s}"))?;
                }
            }
        }
        if chord.vk == 0 {
            return Err(format!("快速鍵缺少主鍵：{s}"));
        }
        Ok(chord)
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
    NAMED.iter().find(|(k, _)| *k == name).map(|&(_, v)| v)
}

impl fmt::Display for KeyChord {
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
        match self.vk {
            0x30..=0x39 | 0x41..=0x5A => write!(f, "{}", self.vk as u8 as char),
            0x70..=0x87 => write!(f, "F{}", self.vk - 0x6F),
            vk => match NAMED.iter().find(|&&(_, v)| v == vk) {
                Some((name, _)) => {
                    let mut c = name.chars();
                    let first = c.next().map(|c| c.to_ascii_uppercase()).unwrap_or_default();
                    write!(f, "{first}{}", c.as_str())
                }
                None => write!(f, "0x{vk:02X}"),
            },
        }
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
        assert_eq!(KeyChord::parse("F2").unwrap().vk, 0x71);
        assert_eq!(KeyChord::parse("alt+up").unwrap().vk, 0x26);
        assert_eq!(KeyChord::parse("Delete").unwrap().to_string(), "Delete");
        assert_eq!(KeyChord::parse("ctrl+l").unwrap().to_string(), "Ctrl+L");
        assert!(KeyChord::parse("Ctrl+").is_err());
        assert!(KeyChord::parse("Ctrl+A+B").is_err());
        assert!(KeyChord::parse("Hyper+A").is_err());
    }
}
