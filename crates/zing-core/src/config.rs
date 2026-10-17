//! 使用者設定：快速鍵、工具列按鈕、主題、佈局、我的最愛。

use serde::{Deserialize, Serialize};

use crate::keymap::KeyChord;

pub const DEFAULT_CONFIG: &str = include_str!("default_config.toml");
/// 英文、日文版的預設設定（只有註解與按鈕名稱不同）：建立使用者設定檔時當作範本
const DEFAULT_CONFIG_EN: &str = include_str!("default_config.en.toml");
const DEFAULT_CONFIG_JA: &str = include_str!("default_config.ja.toml");

/// 建立使用者設定檔時的範本（註解用 `lang` 的語言）
pub fn default_config_for(lang: crate::i18n::Lang) -> &'static str {
    match lang {
        crate::i18n::Lang::ZhTw => DEFAULT_CONFIG,
        crate::i18n::Lang::En => DEFAULT_CONFIG_EN,
        crate::i18n::Lang::Ja => DEFAULT_CONFIG_JA,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TreeSide {
    #[default]
    Left,
    Right,
    Hidden,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub tree: TreeSide,
}

/// 命令面板的位置
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PalettePosition {
    /// 視窗上方
    #[default]
    Top,
    /// 視窗正中央（以清單填滿時的高度置中，結果變少時輸入框不動）
    Center,
    /// 整個面板永遠置中（結果變少時輸入框跟著移動）
    Middle,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteConfig {
    pub position: PalettePosition,
}

/// 命令列（工具列下方的按鈕列）
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandBarConfig {
    pub visible: bool,
    /// false：按鈕只顯示圖示
    pub labels: bool,
}

impl Default for CommandBarConfig {
    fn default() -> Self {
        CommandBarConfig { visible: true, labels: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderSizesConfig {
    /// 沒有索引時，進入本機固定磁碟上的資料夾就逐資料夾計算子資料夾大小
    pub auto: bool,
}

impl Default for FolderSizesConfig {
    fn default() -> Self {
        FolderSizesConfig { auto: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: String,
    pub cmd: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Button {
    pub label: String,
    pub cmd: String,
}

/// 外部命令縮圖產生器：檔名符合 `match`（以 `;` 分隔的萬用字元）時執行 `cmd` 產生 PNG。
/// `cmd` 中的 `{in}` 換成來源檔案、`{out}` 換成要寫入的 PNG、`{size}` 換成邊長（px）；
/// 直接啟動程式、不經過命令殼層（需要時自行寫 `cmd /c …`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thumbnailer {
    #[serde(rename = "match")]
    pub pattern: String,
    pub cmd: String,
}

impl Thumbnailer {
    pub fn matches(&self, name: &[u16]) -> bool {
        self.pattern.split(';').map(str::trim).filter(|p| !p.is_empty()).any(|p| crate::text::Pattern::new(p).matches(name))
    }

    pub fn command_line(&self, input: &str, output: &str, size: u32) -> String {
        self.cmd.replace("{in}", input).replace("{out}", output).replace("{size}", &size.to_string())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RawConfig {
    pub language: Option<String>,
    pub theme: Option<String>,
    pub show_hidden: Option<bool>,
    pub start_path: Option<String>,
    pub favorites: Option<Vec<String>>,
    pub layout: Option<Layout>,
    pub palette: Option<PaletteConfig>,
    pub folder_sizes: Option<FolderSizesConfig>,
    pub command_bar: Option<CommandBarConfig>,
    pub key: Vec<KeyBinding>,
    pub button: Option<Vec<Button>>,
    pub thumbnailer: Vec<Thumbnailer>,
}

/// `keys.toml`（快速鍵編輯器寫的檔案）
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct RawKeys {
    key: Vec<KeyBinding>,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// 介面語言：auto（跟隨 Windows 的顯示語言）| zh-TW | en | ja
    pub language: crate::i18n::Lang,
    /// 設定檔中的寫法（`auto`、`zh-TW`、`en`、`ja`）：設定頁顯示用
    pub language_setting: String,
    pub theme: String,
    pub show_hidden: bool,
    pub start_path: String,
    pub favorites: Vec<String>,
    pub layout: Layout,
    pub palette: PaletteConfig,
    pub folder_sizes: FolderSizesConfig,
    pub command_bar: CommandBarConfig,
    /// 命令列的按鈕（toolbar.toml，沒有時為預設＋舊版的 [[button]]）
    pub cmdbar: Vec<crate::cmdbar::Item>,
    /// 解析後的快速鍵；後定義者優先
    pub keys: Vec<(KeyChord, String)>,
    /// 預設設定＋config.toml 的快速鍵（不含 keys.toml）：快速鍵編輯器以此判斷哪些是改過的
    pub config_keys: Vec<(KeyChord, String)>,
    pub buttons: Vec<Button>,
    /// 外部命令縮圖產生器（預設 + 使用者，使用者的排前面、先比對）
    pub thumbnailers: Vec<Thumbnailer>,
}

impl Config {
    /// 由預設設定與使用者設定（可無）組合。回傳設定與非致命的警告。
    pub fn load(user_src: Option<&str>) -> (Self, Vec<String>) {
        Self::load_with_keys(user_src, None)
    }

    /// 另外套用快速鍵編輯器寫的 `keys.toml`（優先於 config.toml 的 `[[key]]`）
    pub fn load_with_keys(user_src: Option<&str>, keys_src: Option<&str>) -> (Self, Vec<String>) {
        Self::load_files(user_src, keys_src, None)
    }

    /// 另外套用命令列的 `toolbar.toml`
    pub fn load_files(user_src: Option<&str>, keys_src: Option<&str>, toolbar_src: Option<&str>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let base: RawConfig = toml::from_str(DEFAULT_CONFIG).expect("內建預設設定必須合法");
        let user = match user_src.map(toml::from_str::<RawConfig>) {
            Some(Ok(u)) => u,
            Some(Err(e)) => {
                warnings.push(tr!("設定檔格式錯誤，已改用預設值：{e}", e));
                RawConfig::default()
            }
            None => RawConfig::default(),
        };

        let edited = match keys_src.map(toml::from_str::<RawKeys>) {
            Some(Ok(k)) => k.key,
            Some(Err(e)) => {
                warnings.push(tr!("keys.toml 格式錯誤，已忽略：{e}", e));
                Vec::new()
            }
            None => Vec::new(),
        };
        let mut keys = Vec::new();
        let mut apply = |list: &[KeyBinding], keys: &mut Vec<(KeyChord, String)>| {
            for b in list {
                match KeyChord::parse(&b.key) {
                    Ok(chord) => {
                        keys.retain(|(c, _)| *c != chord);
                        if !b.cmd.trim().is_empty() {
                            keys.push((chord, b.cmd.clone()));
                        }
                    }
                    Err(e) => warnings.push(e),
                }
            }
        };
        apply(&base.key, &mut keys);
        apply(&user.key, &mut keys);
        let config_keys = keys.clone();
        apply(&edited, &mut keys);

        let language = user.language.or(base.language).unwrap_or_default();
        let (language, language_setting) = match crate::i18n::Lang::parse(&language) {
            Some(l) => (l, language),
            None => {
                warnings.push(tr!("language 需為 auto、zh-TW、en 或 ja：{language}", language));
                (crate::i18n::system(), "auto".into())
            }
        };
        let cfg = Config {
            language,
            language_setting,
            theme: user.theme.or(base.theme).unwrap_or_else(|| "system".into()),
            show_hidden: user.show_hidden.or(base.show_hidden).unwrap_or(false),
            start_path: user.start_path.or(base.start_path).unwrap_or_default(),
            favorites: user.favorites.or(base.favorites).unwrap_or_default(),
            layout: user.layout.or(base.layout).unwrap_or_default(),
            palette: user.palette.or(base.palette).unwrap_or_default(),
            folder_sizes: user.folder_sizes.or(base.folder_sizes).unwrap_or_default(),
            keys,
            config_keys,
            command_bar: user.command_bar.or(base.command_bar).unwrap_or_default(),
            cmdbar: Vec::new(),
            buttons: user.button.or(base.button).unwrap_or_default(),
            thumbnailers: user.thumbnailer.into_iter().chain(base.thumbnailer).collect(),
        };
        let mut cfg = cfg;
        let (items, warn) = crate::cmdbar::load(toolbar_src, &cfg.buttons);
        cfg.cmdbar = items;
        warnings.extend(warn);
        (cfg, warnings)
    }

    pub fn command_for(&self, chord: KeyChord) -> Option<&str> {
        self.keys.iter().find(|(c, _)| *c == chord).map(|(_, cmd)| cmd.as_str())
    }

    pub fn key_for(&self, cmd: &str) -> Option<KeyChord> {
        self.keys.iter().find(|(_, c)| c == cmd).map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnailer_match_and_command() {
        let (cfg, w) = Config::load(Some("[[thumbnailer]]\nmatch = \"*.stl; *.3MF\"\ncmd = 'stl-thumb -s {size} \"{in}\" \"{out}\"'"));
        assert!(w.is_empty(), "{w:?}");
        let t = &cfg.thumbnailers[0];
        let w16 = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert!(t.matches(&w16("模型.STL")) && t.matches(&w16("a.3mf")));
        assert!(!t.matches(&w16("a.stl.txt")));
        assert_eq!(t.command_line(r"C:\a b\x.stl", r"C:\t\1.png", 128), r#"stl-thumb -s 128 "C:\a b\x.stl" "C:\t\1.png""#);
    }

    #[test]
    fn defaults_parse() {
        let (cfg, warn) = Config::load(None);
        assert!(warn.is_empty(), "{warn:?}");
        let k = KeyChord::parse("Ctrl+C").unwrap();
        assert_eq!(cfg.command_for(k), Some("file.copy"));
        assert!(cfg.buttons.is_empty());
        assert_eq!(cfg.cmdbar, crate::cmdbar::defaults());
        assert!(cfg.command_bar.visible && cfg.command_bar.labels);
        assert_eq!(cfg.palette.position, PalettePosition::Top);
    }

    #[test]
    fn localized_templates_match() {
        let summary = |c: &Config| {
            let mut keys: Vec<(String, String)> = c.keys.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            keys.sort();
            let buttons: Vec<String> = c.buttons.iter().map(|b| b.cmd.clone()).collect();
            (keys, buttons, c.theme.clone(), c.show_hidden, c.layout.tree, c.palette.position, c.folder_sizes.auto, c.command_bar.clone())
        };
        let (zh, _) = Config::load(None);
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Ja] {
            let (c, warn) = Config::load(Some(default_config_for(lang)));
            assert!(warn.is_empty(), "{lang:?}: {warn:?}");
            assert_eq!(summary(&c), summary(&zh), "{lang:?} 的範本與預設設定不一致");
        }
    }

    #[test]
    fn language() {
        let (cfg, warn) = Config::load(Some("language = \"ja\""));
        assert!(warn.is_empty(), "{warn:?}");
        assert_eq!(cfg.language, crate::i18n::Lang::Ja);
        let (_, warn) = Config::load(Some("language = \"klingon\""));
        assert_eq!(warn.len(), 1);
    }

    #[test]
    fn palette_position() {
        let (cfg, warn) = Config::load(Some("[palette]\nposition = \"center\"\n"));
        assert!(warn.is_empty(), "{warn:?}");
        assert_eq!(cfg.palette.position, PalettePosition::Center);
        let (cfg, _) = Config::load(Some("[palette]\nposition = \"middle\"\n"));
        assert_eq!(cfg.palette.position, PalettePosition::Middle);
        let (_, warn) = Config::load(Some("[palette]\nposition = \"left\"\n"));
        assert_eq!(warn.len(), 1, "不合法的值要警告並改用預設");
    }

    #[test]
    fn user_overrides() {
        let (cfg, warn) = Config::load(Some(
            r#"
            theme = "light"
            [layout]
            tree = "right"
            [[key]]
            key = "Ctrl+C"
            cmd = "run notepad {sel}"
            [[key]]
            key = "Delete"
            cmd = ""
            [[key]]
            key = "Bogus+Q"
            cmd = "x"
            "#,
        ));
        assert_eq!(warn.len(), 1);
        assert_eq!(cfg.theme, "light");
        assert_eq!(cfg.layout.tree, TreeSide::Right);
        assert_eq!(cfg.command_for(KeyChord::parse("ctrl+c").unwrap()), Some("run notepad {sel}"));
        assert_eq!(cfg.command_for(KeyChord::parse("Delete").unwrap()), None);
        let (_, warn) = Config::load(Some("theme = ["));
        assert_eq!(warn.len(), 1);
    }
}
