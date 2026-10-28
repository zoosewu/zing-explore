//! 設定頁（`settings.open`）的項目，以及把單一設定寫回使用者的 config.toml。
//!
//! 寫回時只改那一行的值（`key = 值`），保留檔案其餘的內容、排列與註解（包括同一行值後面的註解）；
//! 沒有這一行就插入在同一節的最後一個值後面，沒有這一節就加在檔案最後。修改後重新解析整份檔案，
//! 結果不是預期的值（例如使用者寫成 `layout = { tree = "left" }`）就放棄、不寫入。

use crate::config::{Config, MenuStyle, PalettePosition, RawConfig, Startup, TreeSide};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// 固定的選項：（設定檔中的值, 顯示名稱）；顯示名稱以 `i18n::tr` 翻譯
    Choice(&'static [(&'static str, &'static str)]),
    Bool,
    Text,
    /// 主題：內建主題＋themes 資料夾中的使用者主題
    Theme,
    /// 不是設定值：執行命令（開啟快速鍵編輯器等）
    Action(&'static str),
    /// 緩慢的右鍵選單擴充功能的表格（每個可單獨切換；值是照樣顯示的 CLSID，以逗號分隔）
    SlowList,
    /// 分頁的清單（值以 `|` 分隔，路徑不會含 `|`）：點一下改成目前開著的分頁
    Tabs,
    /// 不是設定值：是否為預設的檔案總管（登錄中的狀態，見 zing-app 的 defaultapp.rs）
    DefaultApp,
}

#[derive(Clone, Copy, Debug)]
pub struct Pref {
    /// 設定檔中的位置：`language`、`layout.tree`（節.名稱）；Action 為空字串
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    /// 顯示在名稱下方的說明（可為空；以 \n 分成多行）
    pub hint: &'static str,
    pub kind: Kind,
}

const LANGUAGES: &[(&str, &str)] =
    &[("auto", tr_key!("跟隨 Windows")), ("zh-TW", "繁體中文"), ("en", "English"), ("ja", "日本語")];
const TREE_SIDES: &[(&str, &str)] = &[("left", tr_key!("左側")), ("right", tr_key!("右側")), ("hidden", tr_key!("隱藏"))];
const MENU_STYLES: &[(&str, &str)] =
    &[("compact", tr_key!("精簡（先顯示常用項目）")), ("full", tr_key!("完整的 Windows 選單"))];
const STARTUPS: &[(&str, &str)] =
    &[("session", tr_key!("還原上次的分頁")), ("new", tr_key!("一個新分頁（此電腦）")), ("tabs", tr_key!("指定的分頁"))];
const PALETTE_POSITIONS: &[(&str, &str)] =
    &[("top", tr_key!("視窗上方")), ("center", tr_key!("正中央（輸入框固定）")), ("middle", tr_key!("正中央（整個面板置中）"))];

/// 內建主題：（名稱, 顯示名稱）
pub const BUILTIN_THEMES: &[(&str, &str)] = &[
    ("system", tr_key!("緊湊（跟隨系統深淺色）")),
    ("dark", tr_key!("緊湊深色")),
    ("light", tr_key!("緊湊淺色")),
    ("fluent", tr_key!("Fluent（跟隨系統深淺色）")),
    ("fluent-dark", tr_key!("Fluent 深色")),
    ("fluent-light", tr_key!("Fluent 淺色")),
    ("balanced", tr_key!("折衷（跟隨系統深淺色）")),
    ("balanced-dark", tr_key!("折衷深色")),
    ("balanced-light", tr_key!("折衷淺色")),
];

pub const PREFS: &[Pref] = &[
    Pref { key: "language", group: tr_key!("一般"), label: tr_key!("介面語言"), hint: "", kind: Kind::Choice(LANGUAGES) },
    Pref {
        key: "",
        group: tr_key!("一般"),
        label: tr_key!("設為預設的檔案總管"),
        hint: tr_key!("資料夾、磁碟機與 Win+E 改用 zing 開啟；關閉即還原"),
        kind: Kind::DefaultApp,
    },
    Pref {
        key: "startup",
        group: tr_key!("一般"),
        label: tr_key!("啟動時開啟"),
        hint: tr_key!("以資料夾啟動（例如從桌面開啟）時，另外開啟那個資料夾"),
        kind: Kind::Choice(STARTUPS),
    },
    Pref {
        key: "start_tabs",
        group: tr_key!("一般"),
        label: tr_key!("指定的分頁"),
        hint: tr_key!("點一下改成目前開著的分頁；「啟動時開啟」選「指定的分頁」時使用"),
        kind: Kind::Tabs,
    },
    Pref { key: "show_hidden", group: tr_key!("一般"), label: tr_key!("顯示隱藏檔案"), hint: "", kind: Kind::Bool },
    Pref {
        key: "update.check",
        group: tr_key!("一般"),
        label: tr_key!("啟動時檢查新版本"),
        hint: tr_key!("每天最多連線到 GitHub 一次；只在有功能更新（次版號以上）時提示"),
        kind: Kind::Bool,
    },
    Pref { key: "theme", group: tr_key!("外觀"), label: tr_key!("主題"), hint: "", kind: Kind::Theme },
    Pref {
        key: "",
        group: tr_key!("外觀"),
        label: tr_key!("自訂主題…"),
        hint: tr_key!("調整顏色、字型、大小，改了立即看到效果"),
        kind: Kind::Action("theme.edit"),
    },
    Pref { key: "layout.tree", group: tr_key!("外觀"), label: tr_key!("樹狀列位置"), hint: "", kind: Kind::Choice(TREE_SIDES) },
    Pref {
        key: "palette.position",
        group: tr_key!("外觀"),
        label: tr_key!("命令面板位置"),
        hint: "",
        kind: Kind::Choice(PALETTE_POSITIONS),
    },
    Pref { key: "command_bar.visible", group: tr_key!("命令列"), label: tr_key!("顯示命令列"), hint: "", kind: Kind::Bool },
    Pref {
        key: "command_bar.labels",
        group: tr_key!("命令列"),
        label: tr_key!("命令列按鈕顯示文字"),
        hint: tr_key!("關閉後只顯示圖示（游標停在按鈕上會顯示名稱）"),
        kind: Kind::Bool,
    },
    Pref {
        key: "",
        group: tr_key!("命令列"),
        label: tr_key!("自訂命令列…"),
        hint: tr_key!("選擇要顯示的按鈕、順序、名稱與圖示"),
        kind: Kind::Action("cmdbar.edit"),
    },
    Pref {
        key: "context_menu.style",
        group: tr_key!("右鍵選單"),
        label: tr_key!("右鍵選單樣式"),
        hint: tr_key!("精簡：立即出現，「顯示更多選項」或 Shift+右鍵開啟完整的 Windows 選單"),
        kind: Kind::Choice(MENU_STYLES),
    },
    Pref {
        key: "context_menu.hide_slow",
        group: tr_key!("右鍵選單"),
        label: tr_key!("隱藏緩慢的擴充功能項目"),
        hint: tr_key!("讓選單變慢的第三方項目（自動偵測）只在「顯示更多選項」中"),
        kind: Kind::Bool,
    },
    Pref {
        key: "context_menu.allow_slow",
        group: tr_key!("右鍵選單"),
        label: tr_key!("緩慢的擴充功能"),
        hint: tr_key!("開啟的項目照樣放在第一層；7 天後重新量"),
        kind: Kind::SlowList,
    },
    Pref {
        key: "folder_sizes.auto",
        group: tr_key!("資料夾大小"),
        label: tr_key!("自動計算資料夾大小"),
        hint: tr_key!("沒有 NTFS 索引時，進入本機磁碟上的資料夾就在背景計算"),
        kind: Kind::Bool,
    },
    Pref {
        key: "",
        group: tr_key!("資料夾大小"),
        label: tr_key!("啟用 NTFS 索引服務（建議）"),
        hint: tr_key!("資料夾大小立即顯示，不必逐一掃描每個子資料夾\n全磁碟搜尋：數百萬個檔案也能即時找到\n需要系統管理員權限（會跳出 UAC）"),
        kind: Kind::Action("index.install"),
    },
    Pref { key: "", group: tr_key!("資料夾大小"), label: tr_key!("NTFS 索引服務狀態"), hint: "", kind: Kind::Action("index.status") },
    Pref { key: "", group: tr_key!("資料夾大小"), label: tr_key!("移除 NTFS 索引服務"), hint: "", kind: Kind::Action("index.uninstall") },
    Pref { key: "", group: tr_key!("其他"), label: tr_key!("編輯快速鍵"), hint: "", kind: Kind::Action("keys.edit") },
    Pref {
        key: "",
        group: tr_key!("其他"),
        label: tr_key!("編輯設定檔"),
        hint: tr_key!("工具列按鈕、縮圖產生器等其他設定"),
        kind: Kind::Action("config.edit"),
    },
];

/// 設定的分類（`Pref::group`，依 `PREFS` 中第一次出現的順序）
pub fn groups() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for p in PREFS {
        if !out.contains(&p.group) {
            out.push(p.group);
        }
    }
    out
}

fn tree_side_code(t: TreeSide) -> &'static str {
    match t {
        TreeSide::Left => "left",
        TreeSide::Right => "right",
        TreeSide::Hidden => "hidden",
    }
}

fn menu_style_code(s: MenuStyle) -> &'static str {
    match s {
        MenuStyle::Compact => "compact",
        MenuStyle::Full => "full",
    }
}

fn startup_code(s: Startup) -> &'static str {
    match s {
        Startup::Session => "session",
        Startup::New => "new",
        Startup::Tabs => "tabs",
    }
}

fn palette_code(p: PalettePosition) -> &'static str {
    match p {
        PalettePosition::Top => "top",
        PalettePosition::Center => "center",
        PalettePosition::Middle => "middle",
    }
}

/// 選項的顯示名稱（找不到就顯示值本身，例如使用者主題）
pub fn choice_label(pref: &Pref, value: &str) -> String {
    let list = match pref.kind {
        Kind::Choice(list) => list,
        Kind::Theme => BUILTIN_THEMES,
        _ => return value.to_string(),
    };
    list.iter().find(|(v, _)| v.eq_ignore_ascii_case(value)).map_or_else(|| value.to_string(), |(_, l)| crate::i18n::tr(l).to_string())
}

impl Config {
    /// 設定頁顯示的值（設定檔中的寫法；布林為 "true"／"false"）
    pub fn pref_value(&self, key: &str) -> String {
        match key {
            "language" => self.language_setting.clone(),
            "startup" => startup_code(self.startup).into(),
            "start_tabs" => self.start_tabs.join("|"),
            "show_hidden" => self.show_hidden.to_string(),
            "theme" => self.theme.clone(),
            "layout.tree" => tree_side_code(self.layout.tree).into(),
            "palette.position" => palette_code(self.palette.position).into(),
            "folder_sizes.auto" => self.folder_sizes.auto.to_string(),
            "command_bar.visible" => self.command_bar.visible.to_string(),
            "command_bar.labels" => self.command_bar.labels.to_string(),
            "context_menu.style" => menu_style_code(self.context_menu.style).into(),
            "context_menu.hide_slow" => self.context_menu.hide_slow.to_string(),
            "context_menu.allow_slow" => self.context_menu.allow_slow.join(","),
            "update.check" => self.update.check.to_string(),
            _ => String::new(),
        }
    }

    /// 在記憶體中修改一個設定（設定頁立即套用）
    pub fn set_pref(&mut self, key: &str, value: &str) -> Result<(), String> {
        let bad = || tr!("{key} 的值不合法：{value}", key, value);
        match key {
            "language" => {
                self.language = crate::i18n::Lang::parse(value).ok_or_else(bad)?;
                self.language_setting = value.to_string();
            }
            "startup" => self.startup = toml_enum(value).ok_or_else(bad)?,
            "start_tabs" => self.start_tabs = value.split('|').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect(),
            "show_hidden" => self.show_hidden = value.parse().map_err(|_| bad())?,
            "theme" => self.theme = value.to_string(),
            "layout.tree" => self.layout.tree = toml_enum(value).ok_or_else(bad)?,
            "palette.position" => self.palette.position = toml_enum(value).ok_or_else(bad)?,
            "folder_sizes.auto" => self.folder_sizes.auto = value.parse().map_err(|_| bad())?,
            "command_bar.visible" => self.command_bar.visible = value.parse().map_err(|_| bad())?,
            "command_bar.labels" => self.command_bar.labels = value.parse().map_err(|_| bad())?,
            "context_menu.style" => self.context_menu.style = toml_enum(value).ok_or_else(bad)?,
            "context_menu.hide_slow" => self.context_menu.hide_slow = value.parse().map_err(|_| bad())?,
            "context_menu.allow_slow" => {
                self.context_menu.allow_slow =
                    value.split(',').map(|c| c.trim().to_ascii_lowercase()).filter(|c| !c.is_empty()).collect()
            }
            "update.check" => self.update.check = value.parse().map_err(|_| bad())?,
            _ => return Err(bad()),
        }
        Ok(())
    }
}

fn toml_enum<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    #[derive(serde::Deserialize)]
    struct W<T> {
        v: T,
    }
    toml::from_str::<W<T>>(&format!("v = {}", toml::Value::String(value.into()))).ok().map(|w| w.v)
}

/// 設定值在 TOML 中的寫法（布林不加引號）
pub fn toml_literal(pref: &Pref, value: &str) -> String {
    match pref.kind {
        Kind::Bool => value.to_string(),
        Kind::SlowList => toml::Value::Array(
            value.split(',').filter(|c| !c.is_empty()).map(|c| toml::Value::String(c.to_string())).collect(),
        )
        .to_string(),
        Kind::Tabs => toml::Value::Array(
            value.split('|').filter(|c| !c.is_empty()).map(|c| toml::Value::String(c.to_string())).collect(),
        )
        .to_string(),
        _ => toml::Value::String(value.to_string()).to_string(),
    }
}

/// config.toml 的一行：表頭 `[名稱]`、陣列表 `[[名稱]]`（不會是要改的節）、`名稱 = 值`
enum Line<'a> {
    Table(&'a str),
    ArrayTable,
    /// `名稱 = …`：名稱、等號後的位置
    Value(&'a str, usize),
    Other,
}

fn classify(line: &str) -> Line<'_> {
    let t = line.trim_start();
    if t.starts_with("[[") {
        return Line::ArrayTable;
    }
    if let Some(rest) = t.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((name, _)) => Line::Table(name.trim()),
            None => Line::Other,
        };
    }
    if t.starts_with('#') {
        return Line::Other;
    }
    let Some(eq) = line.find('=') else { return Line::Other };
    let name = line[..eq].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Line::Other;
    }
    Line::Value(name, eq + 1)
}

/// 等號後的內容分成值與行尾註解（`#` 可能出現在字串中，所以逐一嘗試到值能解析為止）
fn split_comment(rest: &str) -> Option<(&str, &str)> {
    let parses = |v: &str| toml::from_str::<toml::Table>(&format!("v = {v}")).is_ok();
    for (i, _) in rest.match_indices('#') {
        if parses(&rest[..i]) {
            return Some((&rest[..i], &rest[i..]));
        }
    }
    parses(rest).then_some((rest, ""))
}

/// 把 config.toml 中的 `key`（`名稱` 或 `節.名稱`）改成 `literal`（TOML 寫法），回傳新的檔案內容
pub fn write_pref(src: &str, key: &str, literal: &str) -> Result<String, String> {
    let (section, name) = match key.split_once('.') {
        Some((s, n)) => (Some(s), n),
        None => (None, key),
    };
    let newline = if src.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    // 目標節的範圍 [start, end)：最上層是第一個表頭之前
    let mut start = if section.is_none() { Some(0) } else { None };
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate() {
        match classify(l) {
            Line::Table(t) if start.is_none() && Some(t) == section => start = Some(i + 1),
            Line::Table(_) | Line::ArrayTable if start.is_some() => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let unchanged = || tr!("無法自動修改 config.toml 中的 {key}，請以文字編輯器修改", key);
    match start {
        Some(start) => {
            let mut last_value = None;
            let mut found = None;
            for (i, l) in lines.iter().enumerate().take(end).skip(start) {
                if let Line::Value(n, at) = classify(l) {
                    last_value = Some(i);
                    if n == name {
                        found = Some((i, at));
                    }
                }
            }
            match found {
                Some((i, at)) => {
                    let line = &lines[i];
                    let (value, comment) = split_comment(&line[at..]).ok_or_else(unchanged)?;
                    // 保留等號後與註解前的空白
                    let lead = &value[..value.len() - value.trim_start().len()];
                    let trail = if comment.is_empty() { "" } else { &value[value.trim_end().len()..] };
                    lines[i] = format!("{}{lead}{literal}{trail}{comment}", &line[..at]);
                }
                None => lines.insert(last_value.map_or(start, |i| i + 1), format!("{name} = {literal}")),
            }
        }
        None => {
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            lines.push(String::new());
            lines.push(format!("[{}]", section.unwrap_or_default()));
            lines.push(format!("{name} = {literal}"));
        }
    }
    let mut out = lines.join(newline);
    out.push_str(newline);
    // 確認整份檔案仍然合法，而且讀回來就是新的值
    let table: toml::Table = toml::from_str(&out).map_err(|_| unchanged())?;
    let expect: toml::Value = toml::from_str::<toml::Table>(&format!("v = {literal}")).map_err(|_| unchanged())?["v"].clone();
    let got = match section {
        Some(s) => table.get(s).and_then(|t| t.get(name)),
        None => table.get(name),
    };
    if got != Some(&expect) || toml::from_str::<RawConfig>(&out).is_err() {
        return Err(unchanged());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_value_and_keeps_comments() {
        let src = "# 說明\ntheme = \"dark\"   # 我的主題\nshow_hidden = false\n\n[layout]\n# 位置\ntree = \"left\"\n\n[[key]]\nkey = \"F5\"\ncmd = \"nav.refresh\"\n";
        let out = write_pref(src, "theme", "\"light\"").unwrap();
        assert_eq!(out, src.replace("theme = \"dark\"   # 我的主題", "theme = \"light\"   # 我的主題"));
        let out = write_pref(src, "layout.tree", "\"right\"").unwrap();
        assert!(out.contains("# 位置\ntree = \"right\"\n\n[[key]]"), "{out}");
        let out = write_pref(src, "show_hidden", "true").unwrap();
        assert!(out.contains("show_hidden = true\n"));
    }

    #[test]
    fn hash_inside_string() {
        let src = "start_path = \"C:\\\\#tmp\" # 註解\n";
        let out = write_pref(src, "start_path", "\"D:\\\\\"").unwrap();
        assert_eq!(out, "start_path = \"D:\\\\\" # 註解\n");
    }

    #[test]
    fn inserts_missing_key_and_section() {
        let src = "# 只有註解\ntheme = \"dark\"\n\n[layout]\ntree = \"left\"\n\n[[key]]\nkey = \"F5\"\ncmd = \"x\"\n";
        let out = write_pref(src, "language", "\"en\"").unwrap();
        assert!(out.starts_with("# 只有註解\ntheme = \"dark\"\nlanguage = \"en\"\n"), "{out}");
        // 新的節加在最後（[[key]] 之後仍然合法）
        let out = write_pref(src, "palette.position", "\"middle\"").unwrap();
        assert!(out.ends_with("cmd = \"x\"\n\n[palette]\nposition = \"middle\"\n"), "{out}");
        let (cfg, w) = Config::load(Some(&out));
        assert!(w.is_empty());
        assert_eq!(cfg.palette.position, PalettePosition::Middle);
        // 空檔案
        assert_eq!(write_pref("", "show_hidden", "true").unwrap(), "show_hidden = true\n");
        // 同名的鍵在其他節不受影響
        let src = "[palette]\nposition = \"top\"\n";
        let out = write_pref(src, "position", "\"x\"").unwrap();
        assert_eq!(out, "position = \"x\"\n[palette]\nposition = \"top\"\n");
    }

    #[test]
    fn keeps_crlf() {
        let src = "theme = \"dark\"\r\n[layout]\r\ntree = \"left\"\r\n";
        assert_eq!(write_pref(src, "layout.tree", "\"hidden\"").unwrap(), "theme = \"dark\"\r\n[layout]\r\ntree = \"hidden\"\r\n");
    }

    #[test]
    fn refuses_what_it_cannot_edit() {
        // 行內表：插入 [layout] 會重複定義
        assert!(write_pref("layout = { tree = \"left\" }\n", "layout.tree", "\"right\"").is_err());
        // 多行字串
        assert!(write_pref("start_path = '''\nC:\\\n'''\n", "start_path", "\"D:\"").is_err());
    }

    #[test]
    fn default_templates_are_editable() {
        for lang in crate::i18n::Lang::ALL {
            let mut src = crate::config::default_config_for(lang).to_string();
            for p in PREFS.iter().filter(|p| !p.key.is_empty()) {
                let value = match p.kind {
                    Kind::Choice(list) => list[1].0,
                    Kind::Bool => "true",
                    Kind::Theme => "fluent-dark",
                    Kind::SlowList => "{a},{b}",
                    Kind::Tabs => "C:\\Users|D:\\a,b",
                    _ => "C:\\Users",
                };
                src = write_pref(&src, p.key, &toml_literal(p, value)).unwrap_or_else(|e| panic!("{lang:?} {}: {e}", p.key));
                let (mut cfg, w) = Config::load(Some(&src));
                assert!(w.is_empty(), "{w:?}");
                assert_eq!(cfg.pref_value(p.key), value, "{}", p.key);
                cfg.set_pref(p.key, value).unwrap();
            }
            // 只改值：行數不變
            assert_eq!(src.lines().count(), crate::config::default_config_for(lang).lines().count());
        }
    }

    #[test]
    fn set_pref_in_memory() {
        let (mut cfg, _) = Config::load(None);
        cfg.set_pref("layout.tree", "right").unwrap();
        assert_eq!(cfg.layout.tree, TreeSide::Right);
        cfg.set_pref("language", "ja").unwrap();
        assert_eq!((cfg.language, cfg.pref_value("language").as_str()), (crate::i18n::Lang::Ja, "ja"));
        assert!(cfg.set_pref("palette.position", "left").is_err());
        assert!(cfg.set_pref("show_hidden", "maybe").is_err());
        let pref = |key: &str| PREFS.iter().find(|p| p.key == key).unwrap();
        assert_eq!(choice_label(pref("layout.tree"), "right"), "右側");
        assert_eq!(choice_label(pref("theme"), "我的主題"), "我的主題");
        cfg.set_pref("start_tabs", "C:\\| |D:\\x").unwrap();
        assert_eq!(cfg.start_tabs, ["C:\\", "D:\\x"]);
        assert_eq!(toml_literal(pref("start_tabs"), &cfg.pref_value("start_tabs")), r"['C:\', 'D:\x']");
        cfg.set_pref("startup", "new").unwrap();
        assert_eq!((cfg.startup, cfg.pref_value("startup").as_str()), (Startup::New, "new"));
        assert!(cfg.set_pref("startup", "later").is_err());
        cfg.set_pref("context_menu.allow_slow", "{AB},,{cd}").unwrap();
        assert_eq!(cfg.context_menu.allow_slow, ["{ab}", "{cd}"]);
        assert!(Config::load(None).0.context_menu.hide_slow);
    }

    #[test]
    fn groups_keep_order() {
        let g = groups();
        assert_eq!(g.first(), Some(&PREFS[0].group));
        assert_eq!(g.len(), PREFS.iter().map(|p| p.group).collect::<std::collections::HashSet<_>>().len());
    }
}
