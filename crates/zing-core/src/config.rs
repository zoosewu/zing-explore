//! 使用者設定：快速鍵、工具列按鈕、主題、佈局、我的最愛。

use serde::{Deserialize, Serialize};

use crate::keymap::KeyChord;

pub const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

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
    /// 視窗正中央
    Center,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteConfig {
    pub position: PalettePosition,
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
    pub theme: Option<String>,
    pub show_hidden: Option<bool>,
    pub start_path: Option<String>,
    pub favorites: Option<Vec<String>>,
    pub layout: Option<Layout>,
    pub palette: Option<PaletteConfig>,
    pub folder_sizes: Option<FolderSizesConfig>,
    pub key: Vec<KeyBinding>,
    pub button: Option<Vec<Button>>,
    pub thumbnailer: Vec<Thumbnailer>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub theme: String,
    pub show_hidden: bool,
    pub start_path: String,
    pub favorites: Vec<String>,
    pub layout: Layout,
    pub palette: PaletteConfig,
    pub folder_sizes: FolderSizesConfig,
    /// 解析後的快速鍵；後定義者優先
    pub keys: Vec<(KeyChord, String)>,
    pub buttons: Vec<Button>,
    /// 外部命令縮圖產生器（預設 + 使用者，使用者的排前面、先比對）
    pub thumbnailers: Vec<Thumbnailer>,
}

impl Config {
    /// 由預設設定與使用者設定（可無）組合。回傳設定與非致命的警告。
    pub fn load(user_src: Option<&str>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let base: RawConfig = toml::from_str(DEFAULT_CONFIG).expect("內建預設設定必須合法");
        let user = match user_src.map(toml::from_str::<RawConfig>) {
            Some(Ok(u)) => u,
            Some(Err(e)) => {
                warnings.push(format!("設定檔格式錯誤，已改用預設值：{e}"));
                RawConfig::default()
            }
            None => RawConfig::default(),
        };

        let mut keys = Vec::new();
        for b in base.key.iter().chain(user.key.iter()) {
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

        let cfg = Config {
            theme: user.theme.or(base.theme).unwrap_or_else(|| "system".into()),
            show_hidden: user.show_hidden.or(base.show_hidden).unwrap_or(false),
            start_path: user.start_path.or(base.start_path).unwrap_or_default(),
            favorites: user.favorites.or(base.favorites).unwrap_or_default(),
            layout: user.layout.or(base.layout).unwrap_or_default(),
            palette: user.palette.or(base.palette).unwrap_or_default(),
            folder_sizes: user.folder_sizes.or(base.folder_sizes).unwrap_or_default(),
            keys,
            buttons: user.button.or(base.button).unwrap_or_default(),
            thumbnailers: user.thumbnailer.into_iter().chain(base.thumbnailer).collect(),
        };
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
        assert!(!cfg.buttons.is_empty());
        assert_eq!(cfg.palette.position, PalettePosition::Top);
    }

    #[test]
    fn palette_position() {
        let (cfg, warn) = Config::load(Some("[palette]\nposition = \"center\"\n"));
        assert!(warn.is_empty(), "{warn:?}");
        assert_eq!(cfg.palette.position, PalettePosition::Center);
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
