//! 使用者的設定檔、主題檔與工作階段（視窗、分頁、我的最愛）放在 zing.exe 所在的資料夾（可攜式）：
//! 重灌系統或換電腦時連同程式一起複製即可。可重建的快取不放這裡，見 CLAUDE.md「資料存放位置」。

use std::path::PathBuf;

use zing_core::config::Config;
use zing_core::theme::{Style, Theme};
use serde::{Deserialize, Serialize};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::w;

/// 使用者資料的資料夾：zing.exe 所在的資料夾
pub fn app_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("."))
    })
    .clone()
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.toml")
}

pub fn themes_dir() -> PathBuf {
    app_dir().join("themes")
}

/// 快速鍵編輯器寫的檔案（優先於 config.toml 的 [[key]]）
pub fn keys_path() -> PathBuf {
    app_dir().join("keys.toml")
}

/// 命令列的按鈕（「自訂命令列」寫的檔案）
pub fn toolbar_path() -> PathBuf {
    app_dir().join("toolbar.toml")
}

fn session_path() -> PathBuf {
    app_dir().join("session.toml")
}

/// 讀取設定。回傳設定與警告訊息（顯示在狀態列）。
pub fn load_config() -> (Config, Vec<String>) {
    zing_core::guard::io("讀取設定檔");
    let user = std::fs::read_to_string(config_path()).ok();
    let keys = std::fs::read_to_string(keys_path()).ok();
    let toolbar = std::fs::read_to_string(toolbar_path()).ok();
    Config::load_files(user.as_deref(), keys.as_deref(), toolbar.as_deref())
}

/// 系統是否使用淺色應用程式主題
pub fn system_prefers_light() -> bool {
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok() && value != 0
}

/// 依名稱取得主題：`system`、內建的 `dark` / `light`，或 themes 資料夾中的 `<名稱>.toml`。
pub fn resolve_theme(name: &str) -> (Theme, Option<String>) {
    let name = name.trim();
    // 內建主題（system、dark、light、fluent、balanced-dark…）；只有用到「跟隨系統」時才讀登錄
    let lower = name.to_ascii_lowercase();
    let follows_system = lower.is_empty() || lower == "system" || Style::parse(&lower).is_some();
    if let Some(t) = Theme::resolve_builtin(name, follows_system && system_prefers_light()) {
        return (t, None);
    }
    zing_core::guard::io("讀取主題檔");
    let file = themes_dir().join(format!("{name}.toml"));
    match std::fs::read_to_string(&file) {
        Ok(src) => match Theme::from_toml(&src) {
            Ok(t) => (t, None),
            Err(e) => (Theme::dark(), Some(tr!("主題「{name}」載入失敗：{e}", name, e))),
        },
        Err(_) => (Theme::dark(), Some(tr!("找不到主題「{name}」（{file}）", name, file = file.display()))),
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub maximized: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneState {
    pub tabs: Vec<String>,
    pub active: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub window: Option<WindowState>,
    pub panes: Vec<PaneState>,
    pub active_pane: usize,
    pub tree_visible: bool,
    pub tree_width: Option<f32>,
    pub preview_visible: bool,
    /// 最後選用的主題（與設定檔相同時不記）
    pub theme: Option<String>,
    pub preview_width: Option<f32>,
    pub columns: Option<[f32; 3]>,
    /// 用 Ctrl+D 加入的我的最愛（設定檔中的我的最愛另外合併）
    pub favorites: Vec<String>,
    /// 我的最愛的顯示順序（含設定檔中的）；調整過順序才有，不在其中的排在後面
    pub favorite_order: Vec<String>,
    /// 命令面板最近執行的命令（最新在前）
    pub palette_recent: Vec<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            window: None,
            panes: Vec::new(),
            active_pane: 0,
            tree_visible: true,
            tree_width: None,
            preview_visible: false,
            theme: None,
            preview_width: None,
            columns: None,
            favorites: Vec::new(),
            favorite_order: Vec::new(),
            palette_recent: Vec::new(),
        }
    }
}

impl Session {
    pub fn load() -> Self {
        zing_core::guard::io("讀取工作階段");
        std::fs::read_to_string(session_path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        zing_core::guard::io("儲存工作階段");
        std::fs::create_dir_all(app_dir())?;
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        // 先寫暫存檔再改名，避免寫到一半當機留下損壞的檔案
        let tmp = session_path().with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, session_path())
    }
}

/// themes 資料夾中的使用者主題（檔名，不含 .toml），依名稱排序
pub fn user_themes() -> Vec<String> {
    zing_core::guard::io("列出使用者主題");
    let mut v: Vec<String> = std::fs::read_dir(themes_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".toml").filter(|s| !s.is_empty()).map(String::from)
        })
        .collect();
    v.sort_by(|a, b| zing_core::namequery::natural_cmp(a.to_lowercase().as_bytes(), b.to_lowercase().as_bytes()));
    v
}

/// 設定頁：把 config.toml 中的 `key` 改成 `literal`（只改那一行，見 `zing_core::prefs::write_pref`）。
/// 設定頁連續修改時依序寫入
pub fn write_pref(key: &str, literal: &str) -> Result<(), String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    zing_core::guard::io("寫入設定檔");
    let path = ensure_user_config().map_err(|e| tr!("無法建立設定檔：{e}", e))?;
    let src = std::fs::read_to_string(&path).map_err(|e| tr!("無法讀取 {path}：{e}", path = path.display(), e))?;
    let out = zing_core::prefs::write_pref(&src, key, literal)?;
    if out != src {
        std::fs::write(&path, out).map_err(|e| tr!("無法寫入 {path}：{e}", path = path.display(), e))?;
    }
    Ok(())
}

/// 使用者設定檔不存在時，以預設設定為範本建立
pub fn ensure_user_config() -> std::io::Result<PathBuf> {
    zing_core::guard::io("建立設定檔");
    let p = config_path();
    if !p.exists() {
        std::fs::create_dir_all(app_dir())?;
        std::fs::write(&p, zing_core::config::default_config_for(zing_core::i18n::lang()))?;
    }
    Ok(p)
}
