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
        let h = s.strip_prefix('#').ok_or_else(|| tr!("顏色需以 # 開頭：{s}", s))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| tr!("顏色格式錯誤：{s}", s))?;
        match h.len() {
            6 => Ok(Self::rgb(v)),
            8 => Ok(Self { a: v as u8, ..Self::rgb(v >> 8) }),
            _ => Err(tr!("顏色需為 #rrggbb 或 #rrggbbaa：{s}", s)),
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
        let raw: toml::Table = toml::from_str(src).map_err(|e| tr!("主題檔格式錯誤：{e}", e))?;
        let version = raw.get("format_version").and_then(|v| v.as_integer()).unwrap_or(1);
        if version > FORMAT_VERSION as i64 {
            return Err(tr!("主題檔版本 {version} 比程式支援的版本新（{FORMAT_VERSION}）", version, FORMAT_VERSION));
        }
        let mode = match raw.get("mode").and_then(|v| v.as_str()) {
            Some("light") => Mode::Light,
            _ => Mode::Dark,
        };
        let style = match raw.get("style").and_then(|v| v.as_str()) {
            None => Style::Compact,
            Some(s) => Style::parse(s).ok_or_else(|| tr!("style 需為 compact、fluent 或 balanced：{s}", s))?,
        };
        let base = Self::builtin_style(style, mode);
        let mut merged = toml::Table::try_from(&base).map_err(|e| e.to_string())?;
        merge(&mut merged, raw);
        let theme: Theme = merged.try_into().map_err(|e: toml::de::Error| tr!("主題檔內容錯誤：{e}", e))?;
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
            return Err(tr!("font.size 需介於 6 到 48").into());
        }
        if !sane(m.row_height, 12.0, 96.0) || !sane(m.tree_width, 80.0, 1200.0) {
            return Err(tr!("metrics 數值超出合理範圍").into());
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

// ---------------------------------------------------------------- 主題編輯器

/// 欄位的種類（決定編輯方式）
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldKind {
    Color,
    Number { min: f32, max: f32, step: f32 },
    Text,
    /// 依序循環的選項（存檔的值）
    Choice(&'static [&'static str]),
    Bool,
}

/// 主題編輯器可編輯的欄位：分組、名稱、在主題 TOML 中的路徑
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    pub group: &'static str,
    pub label: &'static str,
    pub path: &'static [&'static str],
    pub kind: FieldKind,
}

const fn color(label: &'static str, key: &'static [&'static str]) -> Field {
    Field { group: tr_key!("配色"), label, path: key, kind: FieldKind::Color }
}

const fn num(group: &'static str, label: &'static str, path: &'static [&'static str], min: f32, max: f32, step: f32) -> Field {
    Field { group, label, path, kind: FieldKind::Number { min, max, step } }
}

pub const FIELDS: &[Field] = &[
    color(tr_key!("背景"), &["color", "background"]),
    color(tr_key!("次要區域"), &["color", "surface"]),
    color(tr_key!("文字"), &["color", "text"]),
    color(tr_key!("次要文字"), &["color", "text_secondary"]),
    color(tr_key!("框線"), &["color", "border"]),
    color(tr_key!("滑過"), &["color", "hover"]),
    color(tr_key!("選取"), &["color", "selection"]),
    color(tr_key!("選取的文字"), &["color", "selection_text"]),
    color(tr_key!("非作用中的選取"), &["color", "selection_inactive"]),
    color(tr_key!("強調色"), &["color", "accent"]),
    color(tr_key!("資料夾"), &["color", "folder"]),
    color(tr_key!("錯誤"), &["color", "error"]),
    Field { group: tr_key!("字型"), label: tr_key!("字型名稱"), path: &["font", "family"], kind: FieldKind::Text },
    num(tr_key!("字型"), tr_key!("字級"), &["font", "size"], 8.0, 24.0, 0.5),
    num(tr_key!("尺寸"), tr_key!("列高"), &["metrics", "row_height"], 16.0, 64.0, 1.0),
    num(tr_key!("尺寸"), tr_key!("圓角"), &["metrics", "radius"], 0.0, 16.0, 1.0),
    num(tr_key!("尺寸"), tr_key!("工具列高度"), &["metrics", "toolbar_height"], 24.0, 80.0, 1.0),
    num(tr_key!("尺寸"), tr_key!("分頁列高度"), &["metrics", "tab_height"], 20.0, 64.0, 1.0),
    num(tr_key!("尺寸"), tr_key!("狀態列高度"), &["metrics", "status_height"], 16.0, 48.0, 1.0),
    num(tr_key!("尺寸"), tr_key!("樹狀列寬度"), &["metrics", "tree_width"], 120.0, 600.0, 10.0),
    num(tr_key!("尺寸"), tr_key!("間距"), &["metrics", "padding"], 0.0, 24.0, 1.0),
    Field { group: tr_key!("外觀"), label: tr_key!("網址列"), path: &["chrome", "address"], kind: FieldKind::Choice(&["path", "breadcrumb"]) },
    Field { group: tr_key!("外觀"), label: tr_key!("分頁強調線"), path: &["chrome", "tab_accent"], kind: FieldKind::Bool },
    Field { group: tr_key!("外觀"), label: tr_key!("選取底色避開圖示"), path: &["chrome", "selection_after_icon"], kind: FieldKind::Bool },
    Field { group: tr_key!("外觀"), label: tr_key!("標題列 Mica 效果"), path: &["effects", "backdrop"], kind: FieldKind::Choice(&["mica", "none"]) },
];

/// 欄位的說明（主題編輯面板的提示：這個顏色或尺寸用在畫面的哪裡）
pub fn field_hint(f: &Field) -> Option<&'static str> {
    Some(match *f.path.last()? {
        "background" => tr!("檔案清單與預覽窗格的底色"),
        "surface" => tr!("工具列、命令列、分頁列、樹狀列、狀態列與側邊面板的底色"),
        "text" => tr!("一般的文字"),
        "text_secondary" => tr!("日期、大小等欄位、說明文字與不能用的按鈕"),
        "border" => tr!("區域之間的分隔線，以及輸入框與按鈕的外框"),
        "hover" => tr!("游標停在項目或按鈕上時的底色"),
        "selection" => tr!("選取項目的底色（使用中的窗格）"),
        "selection_text" => tr!("選取項目上的文字"),
        "selection_inactive" => tr!("另一個窗格或視窗不在前景時，選取項目的底色"),
        "accent" => tr!("主要按鈕、目前的分頁、焦點外框與連結"),
        "folder" => tr!("資料夾圖示"),
        "error" => tr!("錯誤訊息與刪除按鈕"),
        "row_height" => tr!("檔案清單每一列的高度"),
        "radius" => tr!("按鈕、選取底色等的圓角半徑（0 為直角）"),
        "padding" => tr!("元件內側的留白"),
        "tab_accent" => tr!("在目前的分頁上方畫一條強調色的線"),
        "selection_after_icon" => tr!("選取底色從圖示右側開始，圖示不被蓋住"),
        "address" => tr!("顯示完整路徑，或可以點每一層的麵包屑"),
        "backdrop" => tr!("Windows 11 的半透明標題列材質"),
        _ => return None,
    })
}

/// 選項的顯示名稱
pub fn choice_label(v: &str) -> &str {
    match v {
        "path" => tr!("完整路徑"),
        "breadcrumb" => tr!("麵包屑"),
        "mica" => tr!("開"),
        "none" => tr!("關"),
        "true" => tr!("是"),
        "false" => tr!("否"),
        other => other,
    }
}

impl Theme {
    fn table(&self) -> toml::Table {
        toml::Table::try_from(self).unwrap_or_default()
    }

    /// 欄位目前的值（顏色為 #rrggbb、數字去掉多餘的 0、布林為 true／false）
    pub fn field_value(&self, f: &Field) -> String {
        let t = self.table();
        let mut v: Option<&toml::Value> = None;
        let mut cur = &t;
        for (k, key) in f.path.iter().enumerate() {
            let next = cur.get(*key);
            if k + 1 == f.path.len() {
                v = next;
            } else if let Some(toml::Value::Table(sub)) = next {
                cur = sub;
            }
        }
        match v {
            Some(toml::Value::String(s)) => s.clone(),
            Some(toml::Value::Float(x)) => format!("{x}").trim_end_matches(".0").to_string(),
            Some(toml::Value::Integer(i)) => i.to_string(),
            Some(toml::Value::Boolean(b)) => b.to_string(),
            _ => String::new(),
        }
    }

    /// 設定一個欄位；值不合法時回傳錯誤（不改變主題）
    pub fn with_field(&self, f: &Field, value: &str) -> Result<Theme, String> {
        let value = value.trim();
        let new = match f.kind {
            FieldKind::Color => toml::Value::String(Color::parse(value)?.to_hex()),
            FieldKind::Number { min, max, .. } => {
                let label = crate::i18n::tr(f.label);
                let x: f32 = value.parse().map_err(|_| tr!("{label} 需為數字", label))?;
                if !x.is_finite() || x < min || x > max {
                    return Err(tr!("{label} 需介於 {min} 到 {max}", label, min, max));
                }
                toml::Value::Float(x as f64)
            }
            FieldKind::Text if value.is_empty() => return Err(tr!("{label} 不能是空的", label = crate::i18n::tr(f.label))),
            FieldKind::Text => toml::Value::String(value.to_string()),
            FieldKind::Choice(opts) if !opts.contains(&value) => {
                return Err(tr!("{label} 的值不正確：{value}", label = crate::i18n::tr(f.label), value));
            }
            FieldKind::Choice(_) => toml::Value::String(value.to_string()),
            FieldKind::Bool => toml::Value::Boolean(value == "true"),
        };
        let mut t = self.table();
        let mut cur = &mut t;
        for key in &f.path[..f.path.len() - 1] {
            cur = cur
                .entry(key.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .ok_or(tr!("主題格式錯誤"))?;
        }
        cur.insert(f.path[f.path.len() - 1].to_string(), new);
        let theme: Theme = t.try_into().map_err(|e: toml::de::Error| e.to_string())?;
        theme.validate()?;
        Ok(theme)
    }

    /// 數字欄位加減一級（滾輪），超出範圍時停在邊界
    pub fn nudge_field(&self, f: &Field, steps: i32) -> Option<Theme> {
        let FieldKind::Number { min, max, step } = f.kind else { return None };
        let cur: f32 = self.field_value(f).parse().ok()?;
        let next = (cur + step * steps as f32).clamp(min, max);
        self.with_field(f, &format!("{next}")).ok()
    }

    /// 選項與布林欄位換到下一個值
    pub fn cycle_field(&self, f: &Field) -> Option<Theme> {
        let cur = self.field_value(f);
        let next = match f.kind {
            FieldKind::Choice(opts) => {
                let i = opts.iter().position(|o| *o == cur).unwrap_or(0);
                opts[(i + 1) % opts.len()].to_string()
            }
            FieldKind::Bool => (cur != "true").to_string(),
            _ => return None,
        };
        self.with_field(f, &next).ok()
    }

    /// 存成主題檔：只寫與 `base`（同風格、同深淺色的內建主題）不同的欄位，以及名稱、風格、深淺色
    pub fn diff_toml(&self, base: &Theme) -> String {
        let (a, b) = (self.table(), base.table());
        let mut out = toml::Table::new();
        for key in ["format_version", "name", "style", "mode"] {
            if let Some(v) = a.get(key) {
                out.insert(key.to_string(), v.clone());
            }
        }
        for (k, v) in &a {
            if out.contains_key(k) {
                continue;
            }
            match (v, b.get(k)) {
                (toml::Value::Table(ta), Some(toml::Value::Table(tb))) => {
                    let sub: toml::Table = ta.iter().filter(|(kk, vv)| tb.get(*kk) != Some(*vv)).map(|(kk, vv)| (kk.clone(), vv.clone())).collect();
                    if !sub.is_empty() {
                        out.insert(k.clone(), toml::Value::Table(sub));
                    }
                }
                (v, Some(bv)) if v == bv => {}
                (v, _) => {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
        toml::to_string_pretty(&out).unwrap_or_default()
    }

    /// 這個主題對應的內建基礎（同風格、同深淺色）
    pub fn base(&self) -> Theme {
        Self::builtin_style(self.style, self.mode)
    }

    /// 欄位是否與內建基礎相同（編輯器只在改過的欄位顯示「還原」）
    pub fn is_default(&self, f: &Field) -> bool {
        self.field_value(f) == self.base().field_value(f)
    }

    /// 欄位還原成內建基礎的值
    pub fn reset_field(&self, f: &Field) -> Option<Theme> {
        self.with_field(f, &self.base().field_value(f)).ok()
    }

    /// 換一個基礎風格或深淺色，保留使用者改過的欄位與檔案顏色。換深淺色時，背景、文字等配色不沿用
    /// （深色主題的背景放到淺色主題上會看不清楚），只保留強調色與資料夾的顏色。
    pub fn rebased(&self, style: Style, mode: Mode) -> Theme {
        let old = self.base();
        let mut t = Theme::builtin_style(style, mode);
        t.name = self.name.clone();
        for f in FIELDS {
            let keep_color = mode == self.mode || matches!(f.path, ["color", "accent"] | ["color", "folder"]);
            if f.kind == FieldKind::Color && !keep_color {
                continue;
            }
            let v = self.field_value(f);
            if v != old.field_value(f)
                && let Ok(n) = t.with_field(f, &v)
            {
                t = n;
            }
        }
        t.color.file_rule = self.color.file_rule.clone();
        t
    }
}

/// 主題編輯器的色票：點一下套用；其他顏色用「更多顏色…」或輸入色碼
pub const SWATCHES: [u32; 16] = [
    0xf2c230, 0xff8c00, 0xe81123, 0xe3008c, 0x8764b8, 0x0078d4, 0x00b7c3, 0x00cc6a,
    0x10893e, 0x7a7574, 0xffffff, 0xe8e8e8, 0xa0a0a0, 0x333333, 0x1b1b1b, 0x000000,
];

/// 新增檔案顏色時可選的類別：（名稱, 萬用字元樣式, 預設顏色）
pub const FILE_PRESETS: &[(&str, &str, u32)] = &[
    (tr_key!("圖片"), "*.jpg;*.jpeg;*.png;*.gif;*.bmp;*.webp;*.heic;*.svg", 0x4fc3f7),
    (tr_key!("影片"), "*.mp4;*.mkv;*.mov;*.avi;*.wmv;*.webm", 0xba68c8),
    (tr_key!("音樂"), "*.mp3;*.flac;*.wav;*.m4a;*.aac;*.ogg", 0xf06292),
    (tr_key!("文件"), "*.pdf;*.doc;*.docx;*.xls;*.xlsx;*.ppt;*.pptx;*.txt;*.md", 0x81c784),
    (tr_key!("壓縮檔"), "*.zip;*.7z;*.rar;*.tar;*.gz", 0xffb74d),
    (tr_key!("執行檔"), "*.exe;*.msi;*.bat;*.cmd;*.ps1", 0xe57373),
    (tr_key!("程式碼"), "*.rs;*.py;*.js;*.ts;*.c;*.cpp;*.h;*.cs;*.java;*.go;*.lua", 0xaed581),
];

/// 檔案顏色規則的顯示名稱：與某個類別的樣式相同就顯示類別名稱
pub fn rule_label(pattern: &str) -> Option<&'static str> {
    FILE_PRESETS.iter().find(|(_, p, _)| *p == pattern).map(|(l, _, _)| crate::i18n::tr(l))
}

/// 基礎風格的選項：（值, 顯示名稱）
pub const STYLE_CHOICES: [(Style, &str); 3] =
    [(Style::Compact, tr_key!("緊湊")), (Style::Fluent, tr_key!("Fluent")), (Style::Balanced, tr_key!("折衷"))];
pub const MODE_CHOICES: [(Mode, &str); 2] = [(Mode::Dark, tr_key!("深色")), (Mode::Light, tr_key!("淺色"))];

/// 主題名稱能不能當檔名（themes\<名稱>.toml），且不與內建主題同名
pub fn valid_theme_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err(tr!("名稱不能是空的").into());
    }
    if let Some(p) = crate::batchrename::validate(n) {
        return Err(tr!("名稱不能當檔名：{reason}", reason = p.describe()));
    }
    if Theme::resolve_builtin(n, false).is_some() {
        return Err(tr!("「{n}」是內建主題的名稱", n));
    }
    Ok(())
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
mod editor_tests {
    use super::*;

    fn field(label: &str) -> &'static Field {
        FIELDS.iter().find(|f| f.label == label).unwrap()
    }

    #[test]
    fn reset_and_default() {
        let t = Theme::dark();
        let f = field("強調色");
        assert!(t.is_default(f));
        let t2 = t.with_field(f, "#ff0000").unwrap();
        assert!(!t2.is_default(f));
        assert_eq!(t2.reset_field(f).unwrap().color.accent, t.color.accent);
    }

    #[test]
    fn rebase_keeps_changes() {
        let t = Theme::dark()
            .with_field(field("強調色"), "#ff0000")
            .unwrap()
            .with_field(field("背景"), "#000000")
            .unwrap()
            .with_field(field("列高"), "30")
            .unwrap();
        // 換風格：配色、尺寸都保留
        let f = t.rebased(Style::Fluent, Mode::Dark);
        assert_eq!((f.style, f.color.accent, f.color.background, f.metrics.row_height), (Style::Fluent, Color::rgb(0xff0000), Color::rgb(0), 30.0));
        // 沒改過的欄位用新風格的值
        assert_eq!(f.metrics.radius, Theme::builtin_style(Style::Fluent, Mode::Dark).metrics.radius);
        // 換深淺色：背景不沿用，強調色保留
        let l = t.rebased(Style::Compact, Mode::Light);
        assert_eq!(l.color.background, Theme::light().color.background);
        assert_eq!(l.color.accent, Color::rgb(0xff0000));
        assert_eq!(l.metrics.row_height, 30.0);
    }

    #[test]
    fn rule_labels() {
        assert_eq!(rule_label(FILE_PRESETS[0].1), Some("圖片"));
        assert_eq!(rule_label("*.foo"), None);
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
    fn editor_fields_get_set() {
        let t = Theme::dark();
        let accent = FIELDS.iter().find(|f| f.path == ["color", "accent"]).unwrap();
        assert_eq!(t.field_value(accent), "#f2c230");
        let t2 = t.with_field(accent, "#FF0000").unwrap();
        assert_eq!(t2.color.accent, Color::rgb(0xff0000));
        assert!(t.with_field(accent, "red").is_err());
        let row = FIELDS.iter().find(|f| f.label == "列高").unwrap();
        assert_eq!(t.field_value(row), "22");
        assert_eq!(t.nudge_field(row, 2).unwrap().metrics.row_height, 24.0);
        assert_eq!(t.nudge_field(row, -100).unwrap().metrics.row_height, 16.0);
        assert!(t.with_field(row, "1000").is_err());
        let addr = FIELDS.iter().find(|f| f.label == "網址列").unwrap();
        assert_eq!(t.cycle_field(addr).unwrap().chrome.address, AddressStyle::Breadcrumb);
        let tab = FIELDS.iter().find(|f| f.label == "分頁強調線").unwrap();
        assert!(!t.cycle_field(tab).unwrap().chrome.tab_accent);
        // 每個欄位都讀得到值
        for f in FIELDS {
            assert!(!t.field_value(f).is_empty(), "{}", f.label);
        }
    }

    #[test]
    fn diff_only_changed_fields() {
        let base = Theme::builtin_style(Style::Fluent, Mode::Light);
        let mut t = base.clone();
        t.name = "我的主題".into();
        t.color.accent = Color::rgb(0x00ff00);
        t.metrics.row_height = 30.0;
        let text = t.diff_toml(&base);
        assert!(text.contains("accent") && text.contains("row_height") && text.contains("style = \"fluent\""));
        assert!(!text.contains("background") && !text.contains("tree_width"), "{text}");
        assert_eq!(Theme::from_toml(&text).unwrap(), t);
        assert!(valid_theme_name("我的主題").is_ok());
        assert!(valid_theme_name("dark").is_err() && valid_theme_name("a/b").is_err() && valid_theme_name(" ").is_err());
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
