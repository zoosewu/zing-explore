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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderSizesConfig {
    /// 進入本機固定磁碟上的資料夾時自動計算子資料夾大小
    pub auto: bool,
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
