//! 主題：顏色、字型、尺寸、效果。主題檔只含資料、不含腳本。
//!
//! 缺少的欄位一律以內建預設補上，所以舊版或不完整的主題檔仍可載入。

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::text::{Pattern, wide};

pub const FORMAT_VERSION: u32 = 1;

/// sRGB 顏色（含 alpha），以 `#rrggbb` 或 `#rrggbbaa` 表示
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(hex: u32) -> Self {
        Self { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8, a: 255 }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let h = s.strip_prefix('#').ok_or_else(|| format!("顏色需以 # 開頭：{s}"))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| format!("顏色格式錯誤：{s}"))?;
        match h.len() {
            6 => Ok(Self::rgb(v)),
            8 => Ok(Self { a: v as u8, ..Self::rgb(v >> 8) }),
            _ => Err(format!("顏色需為 #rrggbb 或 #rrggbbaa：{s}")),
        }
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    pub fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backdrop {
    /// 標題列使用 Mica
    #[default]
    Mica,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Colors {
    pub background: Color,
    /// 工具列、分頁列、樹狀列等次要區域
    pub surface: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub border: Color,
    pub hover: Color,
    pub selection: Color,
    pub selection_inactive: Color,
    pub accent: Color,
    pub folder: Color,
    pub error: Color,
    pub file_rule: Vec<FileRule>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileRule {
    /// 萬用字元樣式，例如 `*.rs`
    #[serde(rename = "match")]
    pub pattern: String,
    pub text: Color,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Font {
    pub family: String,
    pub size: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub row_height: f32,
    pub radius: f32,
    pub toolbar_height: f32,
    pub tab_height: f32,
    pub status_height: f32,
    pub tree_width: f32,
    pub padding: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    pub backdrop: Backdrop,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub format_version: u32,
    pub name: String,
    pub mode: Mode,
    pub color: Colors,
    pub font: Font,
    pub metrics: Metrics,
    pub effects: Effects,
}

impl Default for Colors {
    fn default() -> Self {
        Theme::dark().color
    }
}

impl Default for Font {
    fn default() -> Self {
        Self { family: "Segoe UI Variable Text".into(), size: 13.0 }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            row_height: 26.0,
            radius: 6.0,
            toolbar_height: 40.0,
            tab_height: 34.0,
            status_height: 26.0,
            tree_width: 240.0,
            padding: 8.0,
        }
    }
}

impl Default for Effects {
    fn default() -> Self {
        Self { backdrop: Backdrop::Mica }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            name: "dark".into(),
            mode: Mode::Dark,
            color: Colors {
                background: Color::rgb(0x1c1c1c),
                surface: Color::rgb(0x202020),
                text: Color::rgb(0xe8e8e8),
                text_secondary: Color::rgb(0x9a9a9a),
                border: Color::rgb(0x333333),
                hover: Color::rgb(0x2a2a2a),
                selection: Color::rgb(0x0f4a85),
                selection_inactive: Color::rgb(0x37373d),
                accent: Color::rgb(0x4cc2ff),
                folder: Color::rgb(0xe8b84a),
                error: Color::rgb(0xff6b6b),
                file_rule: Vec::new(),
            },
            font: Font::default(),
            metrics: Metrics::default(),
            effects: Effects::default(),
        }
    }

    pub fn light() -> Self {
        Self {
            name: "light".into(),
            mode: Mode::Light,
            color: Colors {
                background: Color::rgb(0xffffff),
                surface: Color::rgb(0xf3f3f3),
                text: Color::rgb(0x1b1b1b),
                text_secondary: Color::rgb(0x6b6b6b),
                border: Color::rgb(0xe0e0e0),
                hover: Color::rgb(0xf0f0f0),
                selection: Color::rgb(0xcce4f7),
                selection_inactive: Color::rgb(0xe5e5e5),
                accent: Color::rgb(0x005fb8),
                folder: Color::rgb(0xd9a12b),
                error: Color::rgb(0xc42b1c),
                file_rule: Vec::new(),
            },
            ..Self::dark()
        }
    }

    pub fn builtin(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Self::dark()),
            "light" => Some(Self::light()),
            _ => None,
        }
    }

    /// 解析主題 TOML。未指定 `mode` 的欄位會以對應模式的內建主題補上。
    pub fn from_toml(src: &str) -> Result<Self, String> {
        let raw: toml::Table = toml::from_str(src).map_err(|e| format!("主題檔格式錯誤：{e}"))?;
        let version = raw.get("format_version").and_then(|v| v.as_integer()).unwrap_or(1);
        if version > FORMAT_VERSION as i64 {
            return Err(format!("主題檔版本 {version} 比程式支援的版本新（{FORMAT_VERSION}）"));
        }
        let mode = match raw.get("mode").and_then(|v| v.as_str()) {
            Some("light") => Mode::Light,
            _ => Mode::Dark,
        };
        let base = if mode == Mode::Light { Self::light() } else { Self::dark() };
        let mut merged = toml::Table::try_from(&base).map_err(|e| e.to_string())?;
        merge(&mut merged, raw);
        let theme: Theme = merged.try_into().map_err(|e: toml::de::Error| format!("主題檔內容錯誤：{e}"))?;
        theme.validate()?;
        Ok(theme)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    fn validate(&self) -> Result<(), String> {
        let m = &self.metrics;
        let sane = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        if !sane(self.font.size, 6.0, 48.0) {
            return Err("font.size 需介於 6 到 48".into());
        }
        if !sane(m.row_height, 12.0, 96.0) || !sane(m.tree_width, 80.0, 1200.0) {
            return Err("metrics 數值超出合理範圍".into());
        }
        Ok(())
    }

    /// 預先編譯的檔案上色規則，供繪製時使用
    pub fn compiled_rules(&self) -> Vec<(Pattern, Color)> {
        self.color.file_rule.iter().map(|r| (Pattern::new(&r.pattern), r.text)).collect()
    }

    pub fn font_family_wide(&self) -> Vec<u16> {
        wide(&self.font.family)
    }
}

/// 遞迴合併：`over` 的值覆蓋 `base`，表格逐鍵合併。
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_theme_fills_defaults() {
        let t = Theme::from_toml(
            r##"
            name = "Midnight"
            mode = "light"
            [color]
            accent = "#ff000080"
            [[color.file_rule]]
            match = "*.rs"
            text = "#f0883e"
            [font]
            size = 14
            "##,
        )
        .unwrap();
        assert_eq!(t.name, "Midnight");
        assert_eq!(t.color.accent, Color { r: 255, g: 0, b: 0, a: 128 });
        assert_eq!(t.color.background, Theme::light().color.background);
        assert_eq!(t.font.size, 14.0);
        assert_eq!(t.font.family, Font::default().family);
        assert_eq!(t.color.file_rule.len(), 1);
    }

    #[test]
    fn roundtrip_and_errors() {
        let t = Theme::light();
        assert_eq!(Theme::from_toml(&t.to_toml()).unwrap(), t);
        assert!(Theme::from_toml("[color]\naccent = \"red\"").is_err());
        assert!(Theme::from_toml("format_version = 99").is_err());
        assert!(Theme::from_toml("[font]\nsize = 1000").is_err());
    }
}
