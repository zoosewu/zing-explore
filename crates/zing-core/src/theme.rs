//! 主題：顏色、字型、尺寸、效果。主題檔只含資料、不含腳本。
//!
//! 內建三種風格，各有深色與淺色：
//! - `compact`（預設）：高密度、直角，選取列為實心強調色
//! - `fluent`：寬鬆、圓角，接近 Windows 11 檔案總管
//! - `balanced`：Fluent 外觀、中等密度
//!
//! 主題檔以 `style` 與 `mode` 選定內建主題當基礎，缺少的欄位一律以它補上，
//! 所以舊版或不完整的主題檔仍可載入。

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

/// 內建風格：決定尺寸、圓角、網址列樣式與預設配色
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    #[default]
    Compact,
    Fluent,
    Balanced,
}

impl Style {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "compact" => Some(Style::Compact),
            "fluent" => Some(Style::Fluent),
            "balanced" => Some(Style::Balanced),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Style::Compact => "compact",
            Style::Fluent => "fluent",
            Style::Balanced => "balanced",
        }
    }
}

/// 網址列：完整路徑文字（可直接看出、複製路徑）或可點選的麵包屑
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AddressStyle {
    #[default]
    Path,
    Breadcrumb,
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
    /// 焦點所在的選取列上的文字（選取色為實心強調色時用深色字）
    pub selection_text: Color,
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

/// 介面外框的樣式（主題能改變的版面細節）
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chrome {
    pub address: AddressStyle,
    /// 作用中的分頁上緣畫一條強調色線（不論有幾個窗格）
    pub tab_accent: bool,
    /// 選取底色從圖示右側開始：選取色與資料夾圖示同色系時（緊湊風格的實心黃），圖示才看得見
    pub selection_after_icon: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub format_version: u32,
    pub name: String,
    pub style: Style,
    pub mode: Mode,
    pub color: Colors,
    pub font: Font,
    pub metrics: Metrics,
    pub effects: Effects,
    pub chrome: Chrome,
}

impl Default for Colors {
    fn default() -> Self {
        Theme::dark().color
    }
}

impl Default for Font {
    fn default() -> Self {
        Theme::dark().font
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Theme::dark().metrics
    }
}

impl Default for Chrome {
    fn default() -> Self {
        Theme::dark().chrome
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

/// 內建主題的配色：(背景, 次要區域, 文字, 次要文字, 框線, 滑過, 選取, 選取文字, 非作用中選取, 強調, 資料夾)
type Palette = [u32; 11];

const COMPACT_DARK: Palette =
    [0x1b1b1b, 0x141414, 0xe8e8e8, 0xa0a0a0, 0x333333, 0x262626, 0xf2c230, 0x141414, 0x3d3520, 0xf2c230, 0xf2c230];
const COMPACT_LIGHT: Palette =
    [0xffffff, 0xf0f0f0, 0x1b1b1b, 0x5c5c5c, 0xd4d4d4, 0xebebeb, 0xf2c230, 0x1b1b1b, 0xf3e5b5, 0x8a6500, 0xd9a12b];
const FLUENT_DARK: Palette =
    [0x272727, 0x1f1f1f, 0xf3f3f3, 0xa8a8a8, 0x3a3a3a, 0x333333, 0x4a3f1c, 0xf3f3f3, 0x383838, 0xf2c230, 0xf2c230];
const BALANCED_DARK: Palette =
    [0x222222, 0x1c1c1c, 0xf0f0f0, 0xafafaf, 0x353535, 0x2e2e2e, 0x4a3f1c, 0xf0f0f0, 0x363636, 0xf2c230, 0xf2c230];
/// Fluent 與折衷的淺色共用
const SOFT_LIGHT: Palette =
    [0xffffff, 0xf3f3f3, 0x1b1b1b, 0x616161, 0xe0e0e0, 0xf0f0f0, 0xfbeec4, 0x1b1b1b, 0xececec, 0x8a6500, 0xd9a12b];

impl Theme {
    /// 預設主題：緊湊、深色
    pub fn dark() -> Self {
        Self::builtin_style(Style::Compact, Mode::Dark)
    }

    pub fn light() -> Self {
        Self::builtin_style(Style::Compact, Mode::Light)
    }

    /// 內建風格的深色或淺色主題
    pub fn builtin_style(style: Style, mode: Mode) -> Self {
        let palette = match (style, mode) {
            (Style::Compact, Mode::Dark) => COMPACT_DARK,
            (Style::Compact, Mode::Light) => COMPACT_LIGHT,
            (Style::Fluent, Mode::Dark) => FLUENT_DARK,
            (Style::Balanced, Mode::Dark) => BALANCED_DARK,
            (_, Mode::Light) => SOFT_LIGHT,
        };
        let [background, surface, text, text_secondary, border, hover, selection, selection_text, selection_inactive, accent, folder] =
            palette.map(Color::rgb);
        let error = Color::rgb(if mode == Mode::Dark { 0xff6b6b } else { 0xc42b1c });
        // (字級, 列高, 圓角, 工具列, 分頁列, 狀態列, 樹狀列寬, 間距)
        let (size, row_height, radius, toolbar_height, tab_height, status_height, tree_width, padding) = match style {
            Style::Compact => (12.0, 22.0, 0.0, 32.0, 28.0, 22.0, 220.0, 4.0),
            Style::Fluent => (14.0, 36.0, 6.0, 52.0, 40.0, 30.0, 248.0, 8.0),
            Style::Balanced => (13.0, 28.0, 4.0, 44.0, 34.0, 26.0, 232.0, 6.0),
        };
        let mode_name = if mode == Mode::Dark { "dark" } else { "light" };
        Self {
            format_version: FORMAT_VERSION,
            name: if style == Style::Compact { mode_name.into() } else { format!("{}-{mode_name}", style.name()) },
            style,
            mode,
            color: Colors {
                background,
                surface,
                text,
                text_secondary,
                border,
                hover,
                selection,
                selection_text,
                selection_inactive,
                accent,
                folder,
                error,
                file_rule: Vec::new(),
            },
            font: Font { family: "Segoe UI Variable Text".into(), size },
            metrics: Metrics { row_height, radius, toolbar_height, tab_height, status_height, tree_width, padding },
            effects: Effects { backdrop: Backdrop::Mica },
            chrome: Chrome {
                address: if style == Style::Compact { AddressStyle::Path } else { AddressStyle::Breadcrumb },
                tab_accent: style == Style::Compact,
                selection_after_icon: style == Style::Compact,
            },
        }
    }

    /// 內建主題名稱：`dark`、`light`（緊湊）、`<風格>-dark`、`<風格>-light`；
    /// `system` 與單獨的風格名稱（`compact`、`fluent`、`balanced`）依系統的深淺色設定
    pub fn resolve_builtin(name: &str, prefers_light: bool) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        let system = if prefers_light { Mode::Light } else { Mode::Dark };
        let (style, mode) = match name.as_str() {
            "" | "system" => (Style::Compact, system),
            "dark" => (Style::Compact, Mode::Dark),
            "light" => (Style::Compact, Mode::Light),
            other => match other.rsplit_once('-') {
                Some((st, "dark")) => (Style::parse(st)?, Mode::Dark),
                Some((st, "light")) => (Style::parse(st)?, Mode::Light),
                _ => (Style::parse(other)?, system),
            },
        };
        Some(Self::builtin_style(style, mode))
    }

    /// 同一風格的另一種深淺色（theme.toggle）
    pub fn toggled(&self) -> Self {
        let mode = if self.mode == Mode::Dark { Mode::Light } else { Mode::Dark };
        Self::builtin_style(self.style, mode)
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
        let style = match raw.get("style").and_then(|v| v.as_str()) {
            None => Style::Compact,
            Some(s) => Style::parse(s).ok_or_else(|| format!("style 需為 compact、fluent 或 balanced：{s}"))?,
        };
        let base = Self::builtin_style(style, mode);
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
    fn builtin_styles_and_names() {
        let d = Theme::dark();
        assert_eq!((d.style, d.metrics.row_height, d.metrics.radius, d.chrome.address), (Style::Compact, 22.0, 0.0, AddressStyle::Path));
        let f = Theme::resolve_builtin("fluent-light", false).unwrap();
        assert_eq!((f.style, f.mode, f.chrome.address), (Style::Fluent, Mode::Light, AddressStyle::Breadcrumb));
        assert_eq!(f.name, "fluent-light");
        // 單獨的風格名稱與 system 依系統深淺色
        assert_eq!(Theme::resolve_builtin("Balanced", true).unwrap().mode, Mode::Light);
        assert_eq!(Theme::resolve_builtin("system", false).unwrap(), Theme::dark());
        assert_eq!(Theme::resolve_builtin("light", false).unwrap(), Theme::light());
        assert_eq!(Theme::resolve_builtin("compact-light", false).unwrap(), Theme::light());
        assert!(Theme::resolve_builtin("midnight", false).is_none());
        assert!(Theme::resolve_builtin("neon-dark", false).is_none());
        // 切換深淺色保留風格
        assert_eq!(f.toggled(), Theme::builtin_style(Style::Fluent, Mode::Dark));
        // 每個內建主題都能存成主題檔再讀回
        for st in [Style::Compact, Style::Fluent, Style::Balanced] {
            for m in [Mode::Dark, Mode::Light] {
                let t = Theme::builtin_style(st, m);
                assert_eq!(Theme::from_toml(&t.to_toml()).unwrap(), t);
            }
        }
    }

    #[test]
    fn user_theme_on_style_base() {
        let t = Theme::from_toml("style = \"fluent\"\n[color]\naccent = \"#4cc2ff\"").unwrap();
        assert_eq!(t.metrics.row_height, 36.0);
        assert_eq!(t.color.accent, Color::rgb(0x4cc2ff));
        assert_eq!(t.color.background, Theme::builtin_style(Style::Fluent, Mode::Dark).color.background);
        assert!(Theme::from_toml("style = \"retro\"").is_err());
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
