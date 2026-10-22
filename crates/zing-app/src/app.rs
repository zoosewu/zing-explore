//! App：整個視窗的狀態、背景結果處理、導覽與命令執行。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use zing_core::command::{self, Invocation, quote_arg};
use zing_core::config::{Config, TreeSide};
use zing_core::enumerate::drives;
use zing_core::path;
use zing_core::sort::{SortKey, SortSpec, sorted_order};
use zing_core::text::Pattern;
use zing_core::theme::{Backdrop, Color, Mode, Theme};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Dwm::{DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::gfx::Gfx;
use crate::icons::Icons;
use crate::jobs::Jobs;
use crate::model::{Children, Expand, NodeKind, Pane, Reveal, Tab, Tree};
use crate::palette::Palette;
use crate::perf::Perf;
use crate::services::{TreeLoad, self, Msg, Notifier, Pool, TabId};
use crate::settings::{self, Session};
use crate::shell::{self, FileOp, FileOps};
use crate::shellhost::{ShellHost, ShellJob};
use crate::sizes::FolderSizes;
use crate::view::{Hit, Layout};
use crate::window;

pub const TIMER_REFRESH: usize = 1;
pub const TIMER_STATUS: usize = 2;
pub const TIMER_PERF: usize = 3;
pub const TIMER_PERF_GPU: usize = 4;
pub const TIMER_SMOKE: usize = 5;
pub const TIMER_JOBS: usize = 6;
/// 結束時等待檔案作業取消完成的上限
pub const TIMER_EXIT: usize = 7;
/// 輪詢索引服務，確認目前顯示的資料夾大小有沒有變動
pub const TIMER_INDEX: usize = 8;
pub const INDEX_POLL_MS: u32 = 2000;
/// 預覽窗格：焦點停留一段時間才讀取
pub const TIMER_PREVIEW: usize = 9;
/// 以文字編輯器修改主題檔時，每秒檢查是否已存檔
pub const TIMER_THEME_WATCH: usize = 10;
/// 預覽處理常式的看門狗
pub const TIMER_PREVIEW_WATCHDOG: usize = 11;
/// 樹狀列展開中的資料夾有變更：合併短時間內的變更後重新載入
pub const TIMER_TREE_REFRESH: usize = 12;
/// 全磁碟搜尋：輸入停頓後才查詢
pub const TIMER_SEARCH: usize = 13;
/// 游標停留提示（tooltip.rs）
pub const TIMER_TIP: usize = 14;
/// 游標在清單停住後預先準備右鍵選單（shellhost.rs）
pub const TIMER_MENU_PREPARE: usize = 15;

/// 右鍵選單的自訂項目
pub const MENU_NEW_TAB: u32 = shell::EXTRA_ID_BASE + 1;
pub const MENU_FAV_ADD: u32 = shell::EXTRA_ID_BASE + 2;
pub const MENU_FAV_REMOVE: u32 = shell::EXTRA_ID_BASE + 3;
pub const MENU_FAV_UP: u32 = shell::EXTRA_ID_BASE + 4;
pub const MENU_FAV_DOWN: u32 = shell::EXTRA_ID_BASE + 5;
pub const MENU_THUMB_SET: u32 = shell::EXTRA_ID_BASE + 6;
pub const MENU_THUMB_CLEAR: u32 = shell::EXTRA_ID_BASE + 7;
pub const MENU_BATCH_RENAME: u32 = shell::EXTRA_ID_BASE + 8;

/// 超過這個數量的排序交給背景執行緒，避免卡住 UI
const INLINE_SORT_LIMIT: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    List,
    Tree,
}

/// 滑鼠按下後的拖曳狀態
pub enum Press {
    Row { pane: usize, row: usize, x: f32, y: f32, deselect_on_up: bool },
    Scroll { pane: usize, grab: f32 },
    ColResize { col: usize, start_x: f32, start_w: f32 },
    TreeSplit { start_x: f32, start_w: f32 },
    PreviewSplit { start_x: f32, start_w: f32 },
    /// 主題編輯面板：拖曳數字欄位的滑桿（`row` 是面板中的列）
    ThemeSlider { field: usize, row: usize },
    /// 自訂命令列面板：拖曳按鈕調整順序（`item` 是命令列的第幾個按鈕）
    BarEditDrag { item: usize },
}

/// 有自己訊息迴圈的動作：必須在釋放 App 借用後才執行（見 window.rs）
pub enum Modal {
    DragOut { paths: Vec<String> },
    /// 主題編輯器：選色對話框
    PickColor { target: crate::themepanel::Target, initial: String },
    ImportTheme,
    ExportTheme { name: String, text: String },
    /// 匯入的主題與現有的同名：詢問是否覆蓋
    ConfirmThemeOverwrite { name: String, text: String },
    /// Lua 的 pick_folder()
    PickFolder { title: String, default: String, reply: std::sync::mpsc::SyncSender<Option<String>> },
    /// 設定縮圖：選擇圖片
    PickThumb { path: String },
    /// 自訂命令列：選擇按鈕的圖片
    PickBarIcon { item: usize },
}

pub enum EditKind {
    Rename { tab: TabId, path: String },
    Address,
    Palette,
    /// 主題編輯器：欄位的值、另存新主題的名稱
    ThemeField { field: usize },
    ThemeSaveAs,
    /// 主題編輯器：檔案顏色的萬用字元樣式
    ThemeRule { rule: usize },
    /// 設定頁的文字欄位
    Pref { field: usize },
    /// 自訂命令列：按鈕的名稱、自訂命令的命令腳本
    BarLabel { item: usize },
    BarCustom,
    /// 快速鍵編輯面板的搜尋框（內容改變時即時篩選）
    KeysSearch,
    /// 自訂命令列的搜尋框（輸入時即時篩選）
    BarSearch,
    /// Lua 腳本的 ask()
    Ask,
}

/// 輸入框（`ask`）的答案交給誰
pub enum AskReply {
    /// Lua 的 `ask()`：腳本執行緒等待回答
    Script(std::sync::mpsc::SyncSender<Option<String>>),
    /// 命令腳本中的 `{ask:提示}`：收集到所有答案後再執行
    Command { script: String, answers: Vec<(String, String)> },
}

pub struct Edit {
    pub hwnd: HWND,
    pub kind: EditKind,
    /// 重新命名：以 D2D 畫在輸入框外的框線（DIP）；`fit` 為 Some 時框的寬度跟著文字，最寬到這個右緣
    pub frame: Option<crate::gfx::Rect>,
    pub fit: Option<f32>,
}

pub struct App {
    pub hwnd: HWND,
    pub gfx: Gfx,
    pub theme: Theme,
    pub rules: Vec<(Pattern, Color)>,
    pub config: Config,
    pub panes: Vec<Pane>,
    pub active_pane: usize,
    pub tree: Tree,
    /// 樹狀列預先載入的執行緒（背景模式）；第一幀之後才建立
    pub tree_pool: Option<Pool>,
    /// 第一幀之後才開始預先載入樹狀列
    pub probes_enabled: bool,
    /// 監看樹狀列展開中的節點；第一次有展開的節點時才建立
    pub tree_watch: Option<services::TreeWatch>,
    /// 有變更、等著重新載入的樹狀列節點
    pub tree_dirty: Vec<String>,
    pub tree_visible: bool,
    pub preview: crate::preview::Preview,
    /// 批次重新命名面板
    pub batch: Option<crate::batch::Batch>,
    /// 目前主題的名稱（內建名稱或 themes 資料夾中的檔名）
    pub theme_name: String,
    pub theme_editor: Option<crate::themepanel::ThemeEditor>,
    /// 設定頁（settings.open）
    pub settings_page: Option<crate::prefspage::SettingsPage>,
    /// 自訂命令列的面板（cmdbar.edit）
    pub cmdbar_editor: Option<crate::cmdbaredit::CmdBarEditor>,
    /// 快速鍵編輯面板（keys.edit）
    pub keys_panel: Option<crate::keyspanel::KeysPanel>,
    /// 開啟中的下拉選單（menu.rs）
    pub menu: Option<crate::menu::Menu>,
    /// 游標停留提示（tooltip.rs）
    pub tip: Option<crate::tooltip::Tip>,
    /// 以鍵盤（F10）操作命令列時，焦點所在的按鈕（命令列索引）
    pub bar_focus: Option<usize>,
    /// 最後一次請 ShellHost 預先準備右鍵選單的（項目, 資料夾）
    pub menu_prepared: Option<(Vec<String>, String)>,
    /// 精簡右鍵選單的「顯示更多選項」要開的完整選單（ctxmenu.rs）
    pub pending_shell_menu: Option<crate::ctxmenu::PendingShellMenu>,
    /// ShellHost 預先載入的第三方擴充功能項目（精簡右鍵選單的第一層）
    pub menu_exts: Option<crate::ctxmenu::MenuExts>,
    /// 命令列的圖片圖示（路徑 → 讀取結果；None 表示讀取中或讀不到）
    pub cmdbar_images: std::cell::RefCell<std::collections::HashMap<String, Option<std::rc::Rc<crate::icons::IconPixels>>>>,
    pub scripts: crate::script::Scripts,
    /// Lua 腳本的 ask() 等待中的提示與回答通道
    pub ask: Option<(String, AskReply)>,
    /// 以文字編輯器修改中的主題檔：(主題名稱, 上次看到的修改時間)
    pub theme_watch: Option<(String, Option<std::time::SystemTime>)>,
    pub tree_side: TreeSide,
    pub tree_width: f32,
    pub focus: Focus,
    pub show_hidden: bool,
    /// 修改日期、類型、大小欄寬（名稱欄填滿其餘空間）
    pub cols: [f32; 3],
    pub favorites: Vec<String>,
    pub pool: Pool,
    pub notifier: Notifier,
    rx: Receiver<Msg>,
    pub fileops: FileOps,
    pub jobs: Jobs,
    pub sizes: FolderSizes,
    /// 每個資料夾的檢視設定（排序…），見 views.rs
    pub folder_views: zing_core::folderview::FolderViews,
    /// 已確認結束，等待檔案作業取消完成後關閉視窗
    pub pending_exit: bool,
    pub icons: Icons,
    pub thumbs: crate::thumbs::Thumbs,
    pub shell_host: ShellHost,
    next_tab_id: TabId,
    pub status: Option<(String, bool, Instant)>,
    title: String,
    pub hover: Hit,
    pub press: Option<Press>,
    /// 拖放進來時游標所在的窗格與列（列為 None 表示放到目前資料夾）
    pub drop_hover: Option<(usize, Option<usize>)>,
    pub edit: Option<Edit>,
    pub modal: Option<Modal>,
    pub palette: Option<Palette>,
    pub palette_recent: Vec<String>,
    pub layout: Layout,
    /// 按鍵已觸發命令時，吃掉緊接著的 WM_CHAR
    pub suppress_char: bool,
    pub perf: Option<Perf>,
    /// 效能或冒煙測試：使用預設設定、不讀寫使用者的工作階段
    pub isolated: bool,
    session: Session,
}

impl App {
    /// `smoke_dir`：冒煙測試的起始資料夾（使用預設設定與深色主題，結果才可重現）
    pub fn new(hwnd: HWND, dpi: f32, perf: Option<Perf>, smoke_dir: Option<String>) -> windows::core::Result<Self> {
        let isolated = perf.is_some() || smoke_dir.is_some();
        // 第一幀之前必須知道設定與主題；檔案很小且在本機，屬於明確的例外
        let (mut config, warnings, session, theme, theme_warning, theme_name) = if smoke_dir.is_some() {
            let (config, warnings) = Config::load(None);
            (config, warnings, Session::default(), Theme::dark(), None, "dark".to_string())
        } else {
            let _allow = zing_core::guard::allow_io("啟動時讀取設定、主題與工作階段");
            let (config, warnings) = settings::load_config();
            let session = if isolated { Session::default() } else { Session::load() };
            // 最後選用的主題優先於設定檔
            let theme_name = session.theme.clone().unwrap_or_else(|| config.theme.clone());
            let (theme, theme_warning) = settings::resolve_theme(&theme_name);
            (config, warnings, session, theme, theme_warning, theme_name)
        };
        // 隔離模式（效能、冒煙測試）固定用繁體中文，結果才可重現
        if isolated {
            config.language = zing_core::i18n::Lang::ZhTw;
        }
        zing_core::i18n::set(config.language);
        crate::perf::mark("config");
        let gfx = Gfx::new(hwnd, dpi, &theme, perf.is_some())?;
        crate::perf::mark("gfx");
        let (notifier, rx) = Notifier::new(hwnd);
        let reporter = notifier.clone();
        zing_core::guard::set_reporter(move |m| reporter.status(m, true));
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
        let pool = Pool::new(threads);
        let fileops = FileOps::new(notifier.clone(), !isolated);
        let icons = Icons::new(notifier.clone());
        let thumbs = crate::thumbs::Thumbs::new(notifier.clone(), crate::thumbs::cache_dir(isolated), crate::thumbs::custom_dir(isolated));
        thumbs.set_thumbnailers(config.thumbnailers.clone());
        let shell_host = ShellHost::new(notifier.clone(), hwnd);

        let mut favorites = config.favorites.clone();
        for f in &session.favorites {
            if !favorites.iter().any(|x| x.eq_ignore_ascii_case(f)) {
                favorites.push(f.clone());
            }
        }
        crate::model::order_favorites(&mut favorites, &session.favorite_order);
        let tree = Tree::build(&favorites, &drives());
        let mut app = App {
            hwnd,
            gfx,
            rules: theme.compiled_rules(),
            tree_side: config.layout.tree,
            tree_visible: session.tree_visible,
            batch: None,
            theme_name,
            theme_editor: None,
            settings_page: None,
            cmdbar_editor: None,
            keys_panel: None,
            menu: None,
            tip: None,
            bar_focus: None,
            menu_prepared: None,
            pending_shell_menu: None,
            menu_exts: None,
            cmdbar_images: Default::default(),
            scripts: Default::default(),
            ask: None,
            theme_watch: None,
            preview: crate::preview::Preview::new(session.preview_visible, session.preview_width.unwrap_or(360.0)),
            tree_width: session.tree_width.unwrap_or(theme.metrics.tree_width),
            cols: session.columns.unwrap_or([150.0, 120.0, 100.0]),
            theme,
            show_hidden: config.show_hidden,
            config,
            panes: Vec::new(),
            active_pane: 0,
            tree,
            tree_pool: None,
            probes_enabled: false,
            tree_watch: None,
            tree_dirty: Vec::new(),
            focus: Focus::List,
            favorites,
            pool,
            notifier,
            rx,
            fileops,
            jobs: Jobs::default(),
            sizes: FolderSizes::default(),
            folder_views: Default::default(),
            pending_exit: false,
            icons,
            thumbs,
            shell_host,
            next_tab_id: 1,
            status: None,
            title: String::new(),
            hover: Hit::None,
            press: None,
            drop_hover: None,
            edit: None,
            modal: None,
            palette: None,
            palette_recent: session.palette_recent.clone(),
            layout: Layout::default(),
            suppress_char: false,
            perf,
            isolated,
            session,
        };

        // 起始分頁：效能測試固定從「此電腦」開始；否則依設定或上次的工作階段
        let start_tabs: Vec<Vec<String>> = if let Some(d) = smoke_dir {
            vec![vec![d]]
        } else if app.perf.is_some() {
            vec![vec![String::new()]]
        } else if !app.config.start_path.is_empty() {
            vec![vec![path::normalize(&app.config.start_path)]]
        } else if !app.session.panes.is_empty() {
            app.session.panes.iter().map(|p| p.tabs.clone()).filter(|t| !t.is_empty()).collect()
        } else {
            vec![vec![std::env::var("USERPROFILE").unwrap_or_default()]]
        };
        for (pi, tabs) in start_tabs.into_iter().enumerate() {
            let active = app.session.panes.get(pi).map_or(0, |p| p.active);
            let tabs: Vec<Tab> = tabs.into_iter().map(|p| app.new_tab(p)).collect();
            let active = active.min(tabs.len() - 1);
            app.panes.push(Pane { tabs, active });
            for t in 0..app.panes[pi].tabs.len() {
                app.load(pi, t);
            }
        }
        if app.panes.is_empty() {
            let t = app.new_tab(String::new());
            app.panes.push(Pane { tabs: vec![t], active: 0 });
            app.load(0, 0);
        }
        app.active_pane = app.session.active_pane.min(app.panes.len() - 1);
        app.sync_tree();
        if let Some(w) = warnings.into_iter().chain(theme_warning).next() {
            app.set_status(w, true);
        }
        Ok(app)
    }

    pub fn session_snapshot(&self) -> Session {
        Session {
            window: window::placement(self.hwnd),
            panes: self
                .panes
                .iter()
                .map(|p| settings::PaneState { tabs: p.tabs.iter().map(|t| t.path.clone()).collect(), active: p.active })
                .collect(),
            active_pane: self.active_pane,
            tree_visible: self.tree_visible,
            tree_width: Some(self.tree_width),
            preview_visible: self.preview.visible,
            theme: (self.theme_name != self.config.theme).then(|| self.theme_name.clone()),
            preview_width: Some(self.preview.width),
            columns: Some(self.cols),
            favorites: self.session.favorites.clone(),
            favorite_order: self.session.favorite_order.clone(),
            palette_recent: self.palette_recent.clone(),
        }
    }

    /// 結束時同步儲存（之後程序即結束，不能交給背景）
    pub fn save_session(&self) {
        if !self.isolated {
            let _ = self.session_snapshot().save();
        }
    }

    /// 執行中的儲存交給背景執行緒
    fn save_session_async(&self) {
        if !self.isolated {
            let snapshot = self.session_snapshot();
            self.pool.spawn(move || {
                let _ = snapshot.save();
            });
        }
    }

    // ------------------------------------------------------------ 存取工具

    pub fn tab(&self) -> &Tab {
        self.panes[self.active_pane].tab()
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        self.panes[self.active_pane].tab_mut()
    }

    fn new_tab(&mut self, p: String) -> Tab {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let view = self.folder_views.get(&p);
        Tab::new(id, p, view)
    }

    pub fn find_tab(&self, id: TabId) -> Option<(usize, usize)> {
        self.panes.iter().enumerate().find_map(|(p, pane)| pane.tabs.iter().position(|t| t.id == id).map(|t| (p, t)))
    }

    /// 視窗標題顯示目前資料夾（工作列與 Alt+Tab 看得到）
    pub fn update_title(&mut self) {
        let title = format!("{} - zing-explore", self.tab().title());
        if title != self.title {
            let w = zing_core::text::wide_z(&title);
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(self.hwnd, windows::core::PCWSTR(w.as_ptr()));
            }
            self.title = title;
        }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some((text.into(), error, Instant::now()));
        unsafe { SetTimer(Some(self.hwnd), TIMER_STATUS, if error { 6000 } else { 3000 }, None) };
        self.invalidate();
    }

    pub fn clear_status(&mut self) {
        self.status = None;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_STATUS);
        }
        self.invalidate();
    }

    // ------------------------------------------------------------ 載入與導覽

    /// 開始載入分頁目前的路徑（取消先前的載入）
    fn load(&mut self, p: usize, t: usize) {
        let notifier = self.notifier.clone();
        let tab = &mut self.panes[p].tabs[t];
        tab.cancel.store(true, Ordering::Relaxed);
        tab.cancel = Arc::new(AtomicBool::new(false));
        tab.generation += 1;
        tab.loading = true;
        tab.error = None;
        services::load_dir(&self.pool, &notifier, tab.id, tab.generation, tab.path.clone(), tab.spec, tab.cancel.clone());
        if !tab.refreshing {
            tab.nav_generation += 1;
            tab.watcher = services::watch(&tab.path, &notifier, tab.id, tab.nav_generation);
        }
    }

    pub fn navigate(&mut self, p: usize, target: String, record: bool) {
        let target = path::normalize(&target);
        let view = self.folder_views.get(&target);
        let tab = self.panes[p].tab_mut();
        if record && !target.eq_ignore_ascii_case(&tab.path) {
            let old = std::mem::take(&mut tab.path);
            tab.back.push(old);
            tab.forward.clear();
        }
        // 套用這個資料夾記住的排序方式
        tab.spec = view.sort;
        tab.mode = view.mode;
        tab.thumb = view.thumb;
        tab.path = target;
        tab.reset_contents();
        tab.watcher = None;
        tab.size_cancel = crate::sizes::new_cancel(&tab.size_cancel);
        let t = self.panes[p].active;
        self.load(p, t);
        if p == self.active_pane {
            self.sync_tree();
        }
        if self.scripts.on_navigate {
            let (ctx, n) = (self.script_ctx(), self.notifier.clone());
            let path = self.panes[p].tabs[t].path.clone();
            self.scripts.navigate(&n, &path, ctx);
        }
        self.invalidate();
    }

    fn refresh(&mut self, p: usize, t: usize) {
        let tab = &mut self.panes[p].tabs[t];
        tab.refresh_due = false;
        if tab.loading && !tab.refreshing {
            return;
        }
        tab.refreshing = !tab.listing.is_empty();
        self.load(p, t);
    }

    pub fn go_back(&mut self) {
        let p = self.active_pane;
        let tab = self.tab_mut();
        if let Some(prev) = tab.back.pop() {
            let cur = std::mem::take(&mut tab.path);
            tab.forward.push(cur.clone());
            self.navigate(p, prev, false);
            self.tab_mut().pending_select = Some((path::file_name(&cur).to_string(), false));
        }
    }

    pub fn go_forward(&mut self) {
        let p = self.active_pane;
        let tab = self.tab_mut();
        if let Some(next) = tab.forward.pop() {
            let cur = std::mem::take(&mut tab.path);
            tab.back.push(cur);
            self.navigate(p, next, false);
        }
    }

    pub fn go_up(&mut self) {
        let cur = self.tab().path.clone();
        if let Some(parent) = path::parent(&cur) {
            let p = self.active_pane;
            self.navigate(p, parent, true);
            // 回到上一層時選取剛剛所在的資料夾
            let name = if path::is_root(&cur) { cur.clone() } else { path::file_name(&cur).to_string() };
            self.tab_mut().pending_select = Some((name, false));
        }
    }

    /// 開啟焦點項目：資料夾就進入，檔案交給系統開啟
    pub fn open_focused(&mut self) {
        if self.focus == Focus::Tree {
            self.open_tree_node(self.tree.focus);
            return;
        }
        let tab = self.tab();
        let Some(e) = tab.focused_entry() else { return };
        if tab.listing.entries[e].is_dir() {
            let p = self.active_pane;
            let target = tab.full_path(e);
            self.navigate(p, target, true);
            return;
        }
        let files: Vec<String> = tab
            .targets()
            .into_iter()
            .filter(|&i| !tab.listing.entries[i].is_dir())
            .take(16)
            .map(|i| tab.full_path(i))
            .collect();
        let dir = tab.path.clone();
        for f in files {
            services::launch(&self.notifier, f, None, Some(dir.clone()), None, services::Show::Normal);
        }
    }

    pub fn open_tree_node(&mut self, i: usize) {
        let Some(n) = self.tree.nodes.get(i) else { return };
        if matches!(n.kind, crate::model::NodeKind::FavRoot) {
            return;
        }
        let target = n.path.clone();
        let p = self.active_pane;
        self.navigate(p, target, true);
    }

    pub fn toggle_tree_node(&mut self, i: usize) {
        if i >= self.tree.nodes.len() {
            return;
        }
        if self.tree.nodes[i].expanded {
            self.tree.collapse(i);
        } else {
            match self.tree.begin_expand(i) {
                Expand::Load(p) => self.load_tree(p, TreeLoad::Expand),
                // 以預先載入的內容立即展開；內容放了一陣子時在背景重新確認
                Expand::Done { path, stale: true } => self.load_tree(path, TreeLoad::Refresh),
                Expand::Done { .. } | Expand::No => {}
            }
        }
        self.invalidate();
    }

    fn load_tree(&mut self, path: String, kind: TreeLoad) {
        let use_index = !self.isolated;
        let show_hidden = self.show_hidden;
        // 使用者展開走一般的工作池（不排在預先載入後面）
        let pool = match kind {
            TreeLoad::Probe => self.tree_pool.get_or_insert_with(|| Pool::background(2, "zing-tree")),
            _ => &self.pool,
        };
        services::tree_children(pool, &self.notifier, path, show_hidden, use_index, kind);
    }

    /// 繪製前：樹狀列可見範圍內還沒預先載入的節點送出預先載入（第一幀之後才開始）
    pub fn schedule_tree_probes(&mut self) {
        if !self.probes_enabled {
            return;
        }
        let Some(tr) = self.layout.tree else { return };
        let rh = self.row_h();
        let first = (self.tree.scroll / rh).floor().max(0.0) as usize;
        // 多取幾列，捲動時下一批已經準備好
        let count = (tr.h() / rh).ceil() as usize + 8;
        for path in self.tree.take_probes(first.saturating_sub(4), count + 4) {
            self.load_tree(path, TreeLoad::Probe);
        }
        self.sync_tree_watch();
    }

    /// 監看展開中的節點（資料夾監看只看清單中的資料夾，樹狀列的子資料夾要另外看）
    fn sync_tree_watch(&mut self) {
        let dirs: Vec<&str> = self.tree.nodes.iter().filter(|n| n.expanded && !n.path.is_empty()).map(|n| n.path.as_str()).collect();
        if dirs.is_empty() && self.tree_watch.is_none() {
            return;
        }
        if self.tree_watch.is_none() {
            self.tree_watch = services::TreeWatch::new(&self.notifier);
        }
        if let Some(w) = &mut self.tree_watch {
            w.set(&dirs);
        }
    }

    pub fn on_tree_dir_changed(&mut self, path: String) {
        if !self.tree_dirty.iter().any(|p| p.eq_ignore_ascii_case(&path)) {
            self.tree_dirty.push(path);
        }
        unsafe { SetTimer(Some(self.hwnd), TIMER_TREE_REFRESH, 250, None) };
    }

    pub fn on_tree_refresh_timer(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TREE_REFRESH);
        }
        for path in std::mem::take(&mut self.tree_dirty) {
            if self.tree.nodes.iter().any(|n| n.expanded && n.path.eq_ignore_ascii_case(&path)) {
                self.load_tree(path, TreeLoad::Refresh);
            }
        }
    }

    /// 樹狀列展開到目前資料夾
    pub fn sync_tree(&mut self) {
        self.tree.reveal = Some(self.tab().path.clone());
        self.step_tree_reveal();
    }

    fn step_tree_reveal(&mut self) {
        match self.tree.step_reveal() {
            Reveal::Load(p) => self.load_tree(p, TreeLoad::Expand),
            Reveal::Done(Some(i)) => {
                self.tree.focus = i;
                self.ensure_tree_visible(i);
            }
            Reveal::Done(None) => {}
        }
    }

    pub fn ensure_tree_visible(&mut self, i: usize) {
        let Some(tr) = self.layout.tree else { return };
        let rh = self.row_h();
        let y = i as f32 * rh;
        if y < self.tree.scroll {
            self.tree.scroll = y;
        } else if y + rh + 8.0 > self.tree.scroll + tr.h() {
            self.tree.scroll = y + rh + 8.0 - tr.h();
        }
        self.tree.scroll = self.tree.scroll.clamp(0.0, self.tree_max_scroll(tr.h()));
    }

    pub fn ensure_visible(&mut self, p: usize) {
        let list = self.layout.panes.get(p).map(|pl| pl.list);
        let list_h = list.map_or(400.0, |l| l.h());
        let max = self.max_scroll(p, list_h);
        let focus = self.panes[p].tab().focus;
        // 清單：一列；縮圖：一格
        let (y, h) = match list.and_then(|l| self.grid(p, l)) {
            Some(g) => {
                let c = g.cell(focus);
                (c.y, c.h)
            }
            None => (focus as f32 * self.row_h(), self.row_h()),
        };
        let tab = self.panes[p].tab_mut();
        if y < tab.scroll {
            tab.scroll = y;
        } else if y + h > tab.scroll + list_h {
            tab.scroll = y + h - list_h;
        }
        tab.scroll = tab.scroll.clamp(0.0, max);
    }

    pub fn set_sort(&mut self, key: SortKey, ascending: Option<bool>) {
        let tab = self.tab_mut();
        let asc = ascending.unwrap_or(if tab.spec.key == key { !tab.spec.ascending } else { key == SortKey::Name });
        tab.spec = SortSpec { key, ascending: asc };
        let (p, t) = (self.active_pane, self.panes[self.active_pane].active);
        self.remember_view(p, t);
        self.resort(p, t);
        self.ensure_visible(p);
        self.invalidate();
    }

    /// 依分頁目前的排序方式重新排序（大量項目交給背景）
    pub fn resort(&mut self, p: usize, t: usize) {
        let show_hidden = self.show_hidden;
        let tab = &mut self.panes[p].tabs[t];
        if tab.listing.len() <= INLINE_SORT_LIMIT {
            tab.sorted = sorted_order(&tab.listing, tab.spec);
            tab.rebuild_view(show_hidden);
        } else {
            let (id, generation, listing, spec) = (tab.id, tab.generation, tab.listing.clone(), tab.spec);
            services::sort_job(&self.pool, &self.notifier, id, generation, listing, spec);
        }
        self.invalidate();
    }

    fn sizes_dirty(&mut self, path_key: String) {
        self.sizes.mark_dirty(path_key);
    }

    pub fn set_filter(&mut self, text: String) {
        let show_hidden = self.show_hidden;
        self.tab_mut().set_filter(text, show_hidden);
        self.invalidate();
    }

    // ------------------------------------------------------------ 背景結果

    pub fn process_notifications(&mut self) {
        self.notifier.acknowledge();
        while let Ok(msg) = self.rx.try_recv() {
            self.on_service_msg(msg);
        }
        self.apply_size_updates();
        self.invalidate();
    }

    fn on_service_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Listing { tab, generation, listing, sorted, done } => {
                let Some((p, t)) = self.find_tab(tab) else { return };
                let show_hidden = self.show_hidden;
                let is_active = p == self.active_pane && t == self.panes[p].active;
                let tab = &mut self.panes[p].tabs[t];
                if generation != tab.generation || (tab.refreshing && !done) {
                    return;
                }
                tab.apply_listing(listing, sorted, show_hidden);
                tab.refreshing = false;
                if done {
                    tab.loading = false;
                    // 已知的資料夾大小寫進新清單；設定為自動時計算
                    let path_key = crate::sizes::key(&tab.path);
                    if self.sizes.has_parent(&path_key) {
                        self.sizes_dirty(path_key);
                    }
                    self.folder_sizes_on_enter(p, t);
                    let tab = &mut self.panes[p].tabs[t];
                    if let Some((name, rename)) = tab.pending_select.take() {
                        match tab.find_view(&name) {
                            Some(i) => {
                                tab.select_only(i);
                                self.ensure_visible(p);
                                if rename && is_active {
                                    self.start_rename();
                                }
                            }
                            None if !rename => {}
                            // 新資料夾可能比通知早一步列舉完成；保留給下一次重新整理
                            None => self.panes[p].tabs[t].pending_select = Some((name, rename)),
                        }
                    }
                }
                if is_active
                    && let Some(perf) = &mut self.perf {
                        perf.on_listing(self.panes[p].tabs[t].view.len(), done);
                    }
            }
            Msg::ListingError { tab, generation, error } => {
                if let Some((p, t)) = self.find_tab(tab) {
                    let tab = &mut self.panes[p].tabs[t];
                    if generation == tab.generation {
                        tab.loading = false;
                        tab.refreshing = false;
                        tab.error = Some(error);
                    }
                }
            }
            Msg::Sorted { tab, generation, spec, sorted } => {
                if let Some((p, t)) = self.find_tab(tab) {
                    let show_hidden = self.show_hidden;
                    let tab = &mut self.panes[p].tabs[t];
                    if generation == tab.generation && spec == tab.spec {
                        tab.sorted = sorted;
                        tab.rebuild_view(show_hidden);
                    }
                }
            }
            Msg::TreeDirChanged(path) => self.on_tree_dir_changed(path),
            Msg::SearchResults { generation, result } => self.on_search_results(generation, result),
            Msg::TreeChildren { path, children, kind } => match kind {
                TreeLoad::Probe => self.tree.set_probe(&path, children),
                TreeLoad::Refresh => {
                    if let Some(c) = children {
                        self.tree.reconcile(&path, c);
                    }
                }
                TreeLoad::Expand => {
                    let empty = || Children::new(Vec::new(), Vec::new());
                    self.tree.insert_children(&path, children.unwrap_or_else(empty));
                    if self.tree.reveal.is_some() {
                        self.step_tree_reveal();
                    }
                }
            },
            Msg::DirChanged { tab, generation } => {
                if let Some((p, t)) = self.find_tab(tab) {
                    let tab = &mut self.panes[p].tabs[t];
                    if generation == tab.nav_generation {
                        self.sizes.mark_stale(&tab.path);
                        tab.refresh_due = true;
                        // 合併短時間內的大量變更
                        unsafe { SetTimer(Some(self.hwnd), TIMER_REFRESH, 250, None) };
                    }
                }
            }
            Msg::FileOpDone { dirs, error, select, renamed } => {
                if let Some(e) = error {
                    self.set_status(e, true);
                }
                if let Some((old, new)) = renamed {
                    self.on_renamed(&old, &new);
                }
                for d in &dirs {
                    self.sizes.mark_stale(d);
                    // 樹狀列：資料夾監看可能沒有涵蓋這個節點（收合中、超過監看數量），直接重新載入
                    self.tree.invalidate(d);
                    if self.tree.nodes.iter().any(|n| n.expanded && n.path.eq_ignore_ascii_case(d)) {
                        self.load_tree(d.clone(), TreeLoad::Refresh);
                    }
                }
                for p in 0..self.panes.len() {
                    for t in 0..self.panes[p].tabs.len() {
                        let tp = self.panes[p].tabs[t].path.clone();
                        if !dirs.iter().any(|d| d.eq_ignore_ascii_case(&tp)) {
                            continue;
                        }
                        if let Some(s) = &select
                            && s.dir.eq_ignore_ascii_case(&tp) && p == self.active_pane && t == self.panes[p].active {
                                self.panes[p].tabs[t].pending_select = Some((s.name.clone(), s.rename));
                            }
                        self.refresh(p, t);
                    }
                }
            }
            Msg::Status { text, error } => self.set_status(text, error),
            Msg::Icon { id, pixels } => self.icons.store(id, pixels),
            Msg::MenuRename => self.start_rename(),
            Msg::MenuExtra { id, target } => self.on_menu_extra(id, target),
            Msg::MenuExtensions { items, folder, menus, hidden } => self.menu_exts = Some(crate::ctxmenu::MenuExts { items, folder, menus, hidden }),
            Msg::Job { id, event } => self.on_job_event(id, event),
            Msg::FolderSize { parent, name, size, done } => self.on_folder_size(parent, name, size, done),
            Msg::FolderSizesLoaded(lines) => self.on_size_cache_loaded(lines),
            Msg::FolderViewsLoaded(v) => self.on_folder_views_loaded(v),
            Msg::Preview { generation, loaded } => self.on_preview_loaded(generation, *loaded),
            Msg::PreviewHost { generation, ok } => self.on_preview_host(generation, ok),
            Msg::ScriptRun(invs) => {
                for inv in invs {
                    if let Err(e) = self.run(&inv) {
                        self.set_status(e, true);
                    }
                }
                self.invalidate();
            }
            Msg::ScriptExec(script) => self.exec(&script),
            Msg::ScriptAsk { prompt, default, reply } => self.open_ask(prompt, default, AskReply::Script(reply)),
            Msg::ScriptPickFolder { title, default, reply } => self.open_pick_folder(title, default, reply),
            Msg::ScriptClipboard { text, reply } => {
                self.shell_host.submit(ShellJob::SetText { text, status: String::new(), reply: Some(reply) })
            }
            Msg::ScriptsLoaded(l) => self.on_scripts_loaded(l),
            Msg::Thumb { id, pixels, source } => {
                self.thumbs.store(id, pixels, source);
                self.invalidate();
            }
            Msg::ThumbCustom { path: p, set, result } => {
                match result {
                    Ok(()) => {
                        self.thumbs.forget(&p);
                        let name = path::file_name(&p);
                        self.set_status(if set { tr!("已設定縮圖：{name}", name) } else { tr!("已移除自訂縮圖：{name}", name) }, false);
                    }
                    Err(e) => self.set_status(tr!("無法設定縮圖：{e}", e), true),
                }
                self.invalidate();
            }
            Msg::IndexSizes { parent, result } => self.on_index_sizes(parent, result),
            Msg::IndexAction { text, error, changed } => self.on_index_action(text, error, changed),
            Msg::IndexOutdated { installed } => self.on_index_outdated(installed),
            Msg::Paste { sources, dest, cut } => self.submit_op(FileOp::Transfer { sources, dest, mov: cut }),
            Msg::ConfigLoaded { mut config, theme, warnings } => {
                if config.thumbnailers != self.config.thumbnailers {
                    self.thumbs.set_thumbnailers(config.thumbnailers.clone());
                }
                if self.isolated {
                    config.language = self.config.language;
                }
                let lang = config.language;
                self.config = *config;
                self.set_language(lang);
                // 重新載入設定時以設定檔的主題為準
                self.theme_name = self.config.theme.clone();
                self.tree_side = self.config.layout.tree;
                self.apply_theme(theme);
                match warnings.into_iter().next() {
                    Some(w) => self.set_status(w, true),
                    None => self.set_status(tr!("已重新載入設定"), false),
                }
            }
            Msg::ThemeLoaded { name, result } => self.on_theme_loaded(name, result),
            Msg::ThemeSaved { name, result } => self.on_theme_saved(name, result),
            Msg::ThemeFileTime { name, mtime } => self.on_theme_file_time(name, mtime),
            Msg::ThemeImport(result) => self.on_theme_import(result),
            Msg::UserThemes(themes) => self.on_user_themes(themes),
            Msg::SlowMenuExts(list) => self.on_slow_menu_exts(list),
            Msg::Fonts(fonts) => self.on_fonts(fonts),
            Msg::CmdBarImage { path, pixels } => self.on_cmdbar_image(path, pixels),
            Msg::AddressResolved { pane, target, normalized, is_dir, cwd } => {
                if is_dir && pane < self.panes.len() {
                    self.navigate(pane, normalized, true);
                } else {
                    // 不是資料夾：交給系統開啟（檔案、網址、程式名稱都可以）
                    services::launch(&self.notifier, target, None, Some(cwd), None, services::Show::Normal);
                }
            }
        }
    }

    pub fn on_refresh_timer(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_REFRESH);
        }
        for p in 0..self.panes.len() {
            for t in 0..self.panes[p].tabs.len() {
                if self.panes[p].tabs[t].refresh_due {
                    self.refresh(p, t);
                }
            }
        }
    }

    // ------------------------------------------------------------ 主題

    pub fn apply_theme(&mut self, theme: Theme) {
        self.rules = theme.compiled_rules();
        let _ = self.gfx.apply_theme(&theme);
        self.theme = theme;
        apply_window_theme(self.hwnd, &self.theme);
        window::refresh_edit_style(self);
        self.invalidate();
    }

    /// 在背景重新讀取設定與主題，完成後以 Msg::ConfigLoaded 套用
    pub fn reload_config(&mut self) {
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            let (config, mut warnings) = settings::load_config();
            let (theme, warn) = settings::resolve_theme(&config.theme);
            warnings.extend(warn);
            n.send(Msg::ConfigLoaded { config: Box::new(config), theme, warnings });
        });
    }

    // ------------------------------------------------------------ 命令

    /// 執行命令腳本（快速鍵、按鈕、選單都經由這裡）
    pub fn exec(&mut self, script: &str) {
        self.exec_with(script, Vec::new());
    }

    /// `answers`：已回答的 `{ask:提示}`（提示, 答案）。還有沒回答的就先顯示輸入框，回答後再執行（Esc 取消整個腳本）；
    /// 同樣的提示只問一次
    pub fn exec_with(&mut self, script: &str, answers: Vec<(String, String)>) {
        let mut missing: Option<String> = None;
        let parsed = {
            let mut vars = |name: &str, param: Option<&str>| {
                if name != "ask" {
                    return self.variable(name, param);
                }
                let prompt = param.filter(|p| !p.trim().is_empty()).unwrap_or(tr!("輸入")).to_string();
                match answers.iter().find(|(p, _)| *p == prompt) {
                    Some((_, a)) => Ok(Some(vec![a.clone()])),
                    None => {
                        missing.get_or_insert(prompt);
                        Ok(Some(vec![String::new()]))
                    }
                }
            };
            command::parse(script, &mut vars)
        };
        if parsed.is_ok()
            && let Some(prompt) = missing
        {
            self.open_ask(prompt, String::new(), AskReply::Command { script: script.to_string(), answers });
            return;
        }
        match parsed {
            Ok(invocations) => {
                for inv in invocations {
                    if let Err(e) = self.run(&inv) {
                        self.set_status(e, true);
                        break;
                    }
                }
            }
            Err(e) => self.set_status(e, true),
        }
        self.invalidate();
    }

    fn variable(&self, name: &str, param: Option<&str>) -> Result<Option<Vec<String>>, String> {
        let tab = self.tab();
        let focused = tab.focused_entry();
        let one = |s: String| Ok(Some(vec![s]));
        match name {
            "sel" => Ok(Some(tab.target_paths())),
            "selnames" => Ok(Some(tab.targets().into_iter().map(|i| tab.listing.name_string(i)).collect())),
            "path" => one(tab.path.clone()),
            "dest" => match self.dest_path() {
                Some(d) => one(d),
                None => Err(tr!("沒有目標窗格：先用 Ctrl+\\ 分割窗格").into()),
            },
            "file" => one(focused.map(|i| tab.full_path(i)).unwrap_or_default()),
            "name" => one(focused.map(|i| tab.listing.name_string(i)).unwrap_or_default()),
            "stem" | "ext" => {
                let n = focused.map(|i| tab.listing.name_string(i)).unwrap_or_default();
                let (stem, ext) = match n.rfind('.') {
                    Some(k) if k > 0 => (n[..k].to_string(), n[k + 1..].to_string()),
                    _ => (n, String::new()),
                };
                one(if name == "stem" { stem } else { ext })
            }
            "date" => one(format_now(param.unwrap_or("%Y-%m-%d"))),
            _ => Ok(None),
        }
    }

    /// 另一個窗格（目前窗格的下一個）的路徑
    fn dest_path(&self) -> Option<String> {
        if self.panes.len() < 2 {
            return None;
        }
        Some(self.panes[(self.active_pane + 1) % self.panes.len()].tab().path.clone())
    }

    fn run(&mut self, inv: &Invocation) -> Result<(), String> {
        let p = self.active_pane;
        match inv.name.as_str() {
            "nav.back" => self.go_back(),
            "nav.forward" => self.go_forward(),
            "nav.up" => self.go_up(),
            "nav.open" => self.open_focused(),
            "nav.refresh" => {
                let t = self.panes[p].active;
                self.refresh(p, t);
            }
            "nav.home" => self.navigate(p, std::env::var("USERPROFILE").unwrap_or_default(), true),
            "nav.goto" => {
                let target = inv.flag("path").or_else(|| inv.positional().next()).ok_or(tr!("nav.goto 需要路徑"))?;
                self.navigate(p, target.to_string(), true);
            }
            "address.edit" => window::begin_address_edit(self),
            "palette.open" => self.open_palette(),
            "search.open" => self.open_search(),
            "keys.edit" => self.open_keys_panel(),
            "settings.open" => self.open_settings(),
            "cmdbar.edit" => self.open_cmdbar_editor(),
            "cmdbar.hide" => {
                if let Some(i) = zing_core::prefs::PREFS.iter().position(|p| p.key == "command_bar.visible") {
                    self.set_pref(i, "false".into());
                    self.set_status(tr!("已隱藏命令列；可在設定頁重新顯示"), false);
                }
            }
            "jobs.toggle" => self.jobs_toggle(),
            "jobs.clear" => self.jobs_clear(),
            "file.copy" | "file.cut" => {
                let paths = self.tab().target_paths();
                if paths.is_empty() {
                    return Ok(());
                }
                let cut = inv.name == "file.cut";
                self.shell_host.submit(ShellJob::SetClipboard { paths, cut });
            }
            "file.paste" => {
                let dest = self.tab().path.clone();
                if dest.is_empty() {
                    return Err(tr!("無法貼到「此電腦」").into());
                }
                self.shell_host.submit(ShellJob::Paste { dest });
            }
            "file.delete" => {
                let paths = self.tab().target_paths();
                if !paths.is_empty() && !self.tab().path.is_empty() {
                    self.submit_op(FileOp::Delete { paths, permanent: inv.has("permanent") });
                }
            }
            // 選取多個項目時開啟批次重新命名
            "file.rename" => {
                if !self.open_batch_rename() {
                    self.start_rename();
                }
            }
            "file.batchrename" => {
                if !self.open_batch_rename() {
                    return Err(tr!("請先選取兩個以上的項目").into());
                }
            }
            "file.newfolder" => {
                let dir = self.tab().path.clone();
                if dir.is_empty() {
                    return Err(tr!("無法在「此電腦」建立資料夾").into());
                }
                self.submit_op(FileOp::NewFolder { dir });
            }
            "file.newfile" => {
                let dir = self.tab().path.clone();
                if dir.is_empty() {
                    return Err(tr!("無法在「此電腦」建立檔案").into());
                }
                self.submit_op(FileOp::NewFile { dir });
            }
            "file.properties" => {
                let paths = self.tab().target_paths();
                let paths = if paths.is_empty() { vec![self.tab().path.clone()] } else { paths };
                self.shell_host.submit(ShellJob::Properties { paths });
            }
            "menu.context" => {
                // 鍵盤開啟右鍵選單：顯示在焦點項目下方
                self.layout = self.compute_layout();
                let tab = self.tab();
                let pl = &self.layout.panes[p];
                let (items, folder) = (tab.target_paths(), tab.path.clone());
                let item = self.item_rect(p, tab.focus);
                let y = match item {
                    Some(r) if !items.is_empty() => r.b,
                    _ => pl.list.t,
                };
                let x = if self.grid(p, pl.list).is_some() { item.map_or(pl.list.l, |r| r.l) + 16.0 } else { pl.cols[0].0 + 40.0 };
                let y = y.clamp(pl.list.t, pl.list.b);
                let s = self.gfx.scale();
                let mut pt = POINT { x: (x * s) as i32, y: (y * s) as i32 };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
                }
                self.open_context_menu(items, folder, x, y, pt);
            }
            "file.copypath" => {
                let paths = self.tab().target_paths();
                let text = if paths.is_empty() { self.tab().path.clone() } else { paths.join("\r\n") };
                self.shell_host.submit(ShellJob::SetText { text, status: tr!("已複製路徑").into(), reply: None });
            }
            "copy" => {
                let dest = inv.flag("to").filter(|d| !d.is_empty()).ok_or(tr!("copy 需要 --to=目的地"))?;
                let mut sources: Vec<String> = inv.positional().map(String::from).collect();
                if sources.is_empty() {
                    sources = self.tab().target_paths();
                }
                if sources.is_empty() {
                    return Err(tr!("沒有選取任何項目").into());
                }
                self.submit_op(FileOp::Transfer { sources, dest: dest.to_string(), mov: inv.has("move") });
            }
            "run" => {
                let mut args = inv.positional();
                let program = args.next().ok_or(tr!("run 需要程式名稱"))?.to_string();
                let params: Vec<String> = args.map(quote_arg).collect();
                let dir = self.tab().path.clone();
                // 主控台程式預設不顯示視窗；--console 用於互動式終端機（cmd、pwsh…）
                let how = if inv.has("console") { services::Show::Normal } else { services::Show::HideConsole };
                services::launch(&self.notifier, program, Some(params.join(" ")), Some(dir), None, how);
            }
            "open" => {
                for f in inv.positional() {
                    services::launch(&self.notifier, f.to_string(), None, None, None, services::Show::Normal);
                }
            }
            "sel.all" => self.tab_mut().select_all(),
            "sel.none" => self.tab_mut().clear_selection(),
            "sel.invert" => self.tab_mut().invert_selection(),
            "tab.new" => {
                let target = inv.flag("path").map(String::from).unwrap_or_else(|| self.tab().path.clone());
                self.open_in_new_tab(target);
            }
            "tab.close" => self.close_tab(p, self.panes[p].active),
            "tab.next" | "tab.prev" => {
                let pane = &mut self.panes[p];
                let n = pane.tabs.len();
                pane.active = if inv.name == "tab.next" { (pane.active + 1) % n } else { (pane.active + n - 1) % n };
                self.sync_tree();
            }
            "pane.split" => {
                let target = self.tab().path.clone();
                let tab = self.new_tab(target);
                self.panes.insert(p + 1, Pane { tabs: vec![tab], active: 0 });
                self.load(p + 1, 0);
                self.active_pane = p + 1;
            }
            "pane.close" => {
                if self.panes.len() > 1 {
                    self.panes.remove(p);
                    self.active_pane = p.min(self.panes.len() - 1);
                    self.sync_tree();
                }
            }
            "pane.focusnext" => {
                let tree_visible = self.layout.tree.is_some();
                match self.focus {
                    Focus::Tree => {
                        self.focus = Focus::List;
                        self.active_pane = 0;
                    }
                    Focus::List if p + 1 < self.panes.len() => self.active_pane = p + 1,
                    Focus::List if tree_visible => self.focus = Focus::Tree,
                    Focus::List => self.active_pane = 0,
                }
                self.sync_tree();
            }
            "tree.toggle" => {
                if self.tree_side == TreeSide::Hidden {
                    self.tree_side = TreeSide::Left;
                    self.tree_visible = true;
                } else {
                    self.tree_visible = !self.tree_visible;
                }
                if !self.tree_visible {
                    self.focus = Focus::List;
                }
            }
            "view.sort" => {
                let asc = if inv.has("desc") { Some(false) } else if inv.has("asc") { Some(true) } else { None };
                // 只有 --asc／--desc：維持目前的排序欄位
                let key = match inv.flag("by") {
                    Some(b) => SortKey::parse(b).ok_or(tr!("view.sort 需要 --by=name|modified|type|size"))?,
                    None if asc.is_some() => self.tab().spec.key,
                    None => return Err(tr!("view.sort 需要 --by=name|modified|type|size").into()),
                };
                self.set_sort(key, asc);
            }
            "view.foldersizes" => self.compute_folder_sizes(p, false, false),
            "view.preview" => self.toggle_preview(),
            "theme.edit" => self.open_theme_editor(),
            "theme.editfile" => self.open_theme_file(),
            "theme.import" => self.modal = Some(Modal::ImportTheme),
            "thumb.set" | "thumb.clear" => {
                let target = match inv.flag("path") {
                    Some(p) => p.to_string(),
                    None => {
                        let tab = self.tab();
                        tab.focused_entry().filter(|_| !tab.view.is_empty()).map(|e| tab.full_path(e)).ok_or(tr!("沒有選取項目"))?
                    }
                };
                match (inv.name.as_str(), inv.flag("image")) {
                    ("thumb.clear", _) => self.thumbs.set_custom(target, None),
                    (_, Some(img)) => self.thumbs.set_custom(target, Some(img.to_string())),
                    _ => self.modal = Some(Modal::PickThumb { path: target }),
                }
            }
            "theme.export" => self.export_theme(),
            "view.mode" => {
                let mode = inv.flag("mode").or_else(|| inv.positional().next()).unwrap_or("list").to_string();
                self.set_view_mode(p, &mode)?;
            }
            "view.thumbsize" => {
                let delta = inv.flag("delta").and_then(|d| d.parse::<i32>().ok()).ok_or(tr!("view.thumbsize 需要 --delta="))?;
                self.zoom_thumbs(p, delta);
            }
            "index.install" | "index.uninstall" | "index.status" => self.index_command(&inv.name),
            "view.togglehidden" => {
                self.set_show_hidden(!self.show_hidden);
                let show_hidden = self.show_hidden;
                self.set_status(if show_hidden { tr!("顯示隱藏檔案") } else { tr!("不顯示隱藏檔案") }, false);
            }
            "filter.clear" => self.set_filter(String::new()),
            "filter.set" => self.set_filter(inv.flag("text").unwrap_or_default().to_string()),
            "favorites.add" => {
                let target = inv.flag("path").map(String::from).unwrap_or_else(|| self.tab().path.clone());
                self.add_favorite(target)?;
            }
            "favorites.remove" => {
                let target = inv.flag("path").map(String::from).unwrap_or_else(|| self.tab().path.clone());
                self.remove_favorite(&target)?;
            }
            "favorites.move" => {
                let by = inv.flag("by").and_then(|d| d.parse::<isize>().ok()).ok_or(tr!("favorites.move 需要 --by=（負數往上）"))?;
                // 預設：樹狀列焦點所在的我的最愛，否則目前資料夾
                let focused = self.tree.nodes.get(self.tree.focus).filter(|n| self.focus == Focus::Tree && n.kind == NodeKind::Favorite);
                let target = match inv.flag("path") {
                    Some(p) => p.to_string(),
                    None => focused.map_or_else(|| self.tab().path.clone(), |n| n.path.clone()),
                };
                self.move_favorite(&target, by)?;
            }
            "theme.set" => {
                let name = inv.flag("name").or_else(|| inv.positional().next()).ok_or(tr!("theme.set 需要 --name="))?.to_string();
                self.load_theme_by_name(name);
            }
            "theme.toggle" => {
                // 同一風格的另一種深淺色
                let theme = self.theme.toggled();
                self.theme_name = theme.name.clone();
                self.apply_theme(theme);
            }
            "config.edit" => {
                let n = self.notifier.clone();
                self.pool.spawn(move || match settings::ensure_user_config() {
                    Ok(p) => services::launch(
                        &n,
                        "notepad.exe".into(),
                        Some(quote_arg(&p.to_string_lossy())),
                        None,
                        None,
                        services::Show::Normal,
                    ),
                    Err(e) => n.status(tr!("無法建立設定檔：{e}", e), true),
                });
            }
            "config.reload" => self.reload_config(),
            "script.reload" => {
                let n = self.notifier.clone();
                self.scripts.load(&n, self.isolated);
            }
            "app.exit" => window::close(self.hwnd),
            // Lua 腳本定義的命令（全域函式）
            other if self.scripts.has(other) => {
                let ctx = self.script_ctx();
                let n = self.notifier.clone();
                self.scripts.call(&n, other, ctx);
            }
            other => return Err(tr!("未知命令：{other}", other)),
        }
        Ok(())
    }

    /// 交給 Shell 主機顯示右鍵選單（不等待結果，選擇的項目以訊息送回）
    pub fn show_context_menu(&mut self, items: Vec<String>, folder: String, pt: POINT, extras: Vec<(u32, String)>, target: String) {
        let mods = crate::input::Mods::current();
        self.shell_host.submit(ShellJob::ContextMenu { items, folder, pt, extras, target, extended: mods.shift, ctrl: mods.ctrl });
    }

    pub fn is_favorite(&self, p: &str) -> bool {
        self.favorites.iter().any(|f| f.eq_ignore_ascii_case(p))
    }

    pub fn add_favorite(&mut self, p: String) -> Result<(), String> {
        let p = path::normalize(&p);
        if p.is_empty() {
            return Err(tr!("「此電腦」不能加入我的最愛").into());
        }
        if self.is_favorite(&p) {
            return Ok(());
        }
        self.favorites.push(p.clone());
        self.session.favorites.push(p.clone());
        self.save_session_async();
        self.rebuild_tree();
        self.set_status(tr!("已加入我的最愛：{p}", p), false);
        Ok(())
    }

    /// 切換介面語言（設定檔的 language）：大部分文字在繪製時翻譯，樹狀列的根節點要重建
    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        self.show_hidden = show_hidden;
        for pane in &mut self.panes {
            for t in &mut pane.tabs {
                t.rebuild_view(show_hidden);
            }
        }
        self.tree.reset_probes();
        self.invalidate();
    }

    pub fn set_language(&mut self, lang: zing_core::i18n::Lang) {
        self.config.language = lang;
        if zing_core::i18n::lang() == lang {
            return;
        }
        zing_core::i18n::set(lang);
        // 文字格式的語系跟著換（中日韓文字的字形）
        let _ = self.gfx.apply_theme(&self.theme);
        self.rebuild_tree();
        self.invalidate();
    }

    /// 資料夾改名後：我的最愛、分頁中的舊路徑換成新路徑
    fn on_renamed(&mut self, old: &str, new: &str) {
        let replace = |v: &mut Vec<String>| {
            let mut changed = false;
            for f in v.iter_mut() {
                if let Some(p) = crate::model::replace_prefix(f, old, new) {
                    *f = p;
                    changed = true;
                }
            }
            changed
        };
        let fav = replace(&mut self.favorites);
        replace(&mut self.session.favorites);
        replace(&mut self.session.favorite_order);
        if fav {
            self.save_session_async();
            self.rebuild_tree();
        }
        for p in 0..self.panes.len() {
            for t in 0..self.panes[p].tabs.len() {
                if let Some(np) = crate::model::replace_prefix(&self.panes[p].tabs[t].path, old, new) {
                    self.panes[p].tabs[t].path = np;
                    if p == self.active_pane && t == self.panes[p].active {
                        self.sync_tree();
                    }
                    self.load(p, t);
                }
            }
        }
    }

    /// 我的最愛往下移 `by` 個位置（負數往上）；順序記在工作階段（設定檔中的也能調整）
    pub fn move_favorite(&mut self, p: &str, by: isize) -> Result<(), String> {
        let i = self.favorites.iter().position(|f| f.eq_ignore_ascii_case(p)).ok_or_else(|| tr!("不在我的最愛中：{p}", p))?;
        let j = (i as isize + by).clamp(0, self.favorites.len() as isize - 1) as usize;
        if i == j {
            return Ok(());
        }
        let f = self.favorites.remove(i);
        self.favorites.insert(j, f.clone());
        self.session.favorite_order = self.favorites.clone();
        self.save_session_async();
        // 原地重排（不重建樹狀列，展開的子樹與焦點都保留）
        self.tree.reorder_favorites(&self.favorites);
        // 焦點跟著移動的項目
        if let Some(k) = self.tree.nodes.iter().position(|n| n.kind == NodeKind::Favorite && n.path.eq_ignore_ascii_case(&f)) {
            self.tree.focus = k;
            self.ensure_tree_visible(k);
        }
        self.invalidate();
        Ok(())
    }

    pub fn remove_favorite(&mut self, p: &str) -> Result<(), String> {
        if self.config.favorites.iter().any(|f| f.eq_ignore_ascii_case(p)) {
            return Err(tr!("這個項目定義在設定檔的 favorites 中，請在設定檔移除（Ctrl+,）").into());
        }
        self.favorites.retain(|f| !f.eq_ignore_ascii_case(p));
        self.session.favorites.retain(|f| !f.eq_ignore_ascii_case(p));
        self.session.favorite_order.retain(|f| !f.eq_ignore_ascii_case(p));
        self.save_session_async();
        self.rebuild_tree();
        self.set_status(tr!("已從我的最愛移除：{p}", p), false);
        Ok(())
    }

    /// 清單項目右鍵選單的自訂項目：只選了一個資料夾時提供「在新分頁開啟」與我的最愛；
    /// 選了多個項目時提供批次重新命名（系統選單沒有）；沒有選取時（背景選單）提供把目前資料夾加入我的最愛。
    /// 複製路徑、在終端機開啟 Windows 11 的選單已經有，不重複加
    pub fn entry_menu_extras(&self, items: &[String], folder: &str) -> (Vec<(u32, String)>, String) {
        let tab = self.tab();
        let single_dir = match tab.targets().as_slice() {
            [e] if items.len() == 1 => tab.listing.entries[*e].is_dir(),
            _ => false,
        };
        if let [one] = items {
            let mut extras = if single_dir { self.folder_extras(one) } else { Vec::new() };
            extras.push((MENU_THUMB_SET, tr!("設定縮圖…").into()));
            if self.thumbs.is_custom(one) {
                extras.push((MENU_THUMB_CLEAR, tr!("移除自訂縮圖").into()));
            }
            (extras, one.clone())
        } else if items.len() > 1 {
            (vec![(MENU_BATCH_RENAME, tr!("批次重新命名").into())], String::new())
        } else if items.is_empty() && !folder.is_empty() && !self.is_favorite(folder) {
            (vec![(MENU_FAV_ADD, tr!("將此資料夾加入我的最愛").into())], folder.to_string())
        } else {
            (Vec::new(), String::new())
        }
    }

    pub fn folder_extras(&self, p: &str) -> Vec<(u32, String)> {
        let fav = if self.is_favorite(p) {
            (MENU_FAV_REMOVE, tr!("從我的最愛移除").to_string())
        } else {
            (MENU_FAV_ADD, tr!("加入我的最愛").to_string())
        };
        vec![(MENU_NEW_TAB, tr!("在新分頁開啟").into()), fav]
    }

    pub fn on_menu_extra(&mut self, id: u32, target: String) {
        let result = match id {
            MENU_NEW_TAB => {
                self.open_in_new_tab(target);
                Ok(())
            }
            MENU_FAV_ADD => self.add_favorite(target),
            MENU_FAV_REMOVE => self.remove_favorite(&target),
            MENU_FAV_UP => self.move_favorite(&target, -1),
            MENU_THUMB_SET => {
                self.modal = Some(Modal::PickThumb { path: target });
                Ok(())
            }
            MENU_THUMB_CLEAR => {
                self.thumbs.set_custom(target, None);
                Ok(())
            }
            MENU_FAV_DOWN => self.move_favorite(&target, 1),
            MENU_BATCH_RENAME => {
                self.exec("file.batchrename");
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.set_status(e, true);
        }
        self.invalidate();
    }

    /// 在目前窗格開新分頁並切換過去
    pub fn open_in_new_tab(&mut self, target: String) {
        let p = self.active_pane;
        let tab = self.new_tab(path::normalize(&target));
        let pane = &mut self.panes[p];
        pane.tabs.insert(pane.active + 1, tab);
        pane.active += 1;
        let t = pane.active;
        self.load(p, t);
        self.sync_tree();
    }

    fn rebuild_tree(&mut self) {
        let scroll = self.tree.scroll;
        self.tree = Tree::build(&self.favorites, &drives());
        self.tree.scroll = scroll;
        self.sync_tree();
    }

    pub fn close_tab(&mut self, p: usize, t: usize) {
        if self.panes[p].tabs.len() <= 1 {
            if self.panes.len() > 1 {
                self.panes.remove(p);
                self.active_pane = self.active_pane.min(self.panes.len() - 1);
                self.sync_tree();
            }
            return;
        }
        let pane = &mut self.panes[p];
        let tab = pane.tabs.remove(t);
        tab.cancel.store(true, Ordering::Relaxed);
        if pane.active >= pane.tabs.len() || pane.active > t {
            pane.active = pane.active.saturating_sub(1);
        }
        self.sync_tree();
        self.invalidate();
    }

    pub fn start_rename(&mut self) {
        let tab = self.tab();
        if tab.path.is_empty() {
            return;
        }
        if let Some(e) = tab.focused_entry() {
            let (id, full) = (tab.id, tab.full_path(e));
            let p = self.active_pane;
            self.ensure_visible(p);
            self.paint();
            window::begin_rename(self, id, full);
        }
    }

    /// 編輯框結束（Enter / 失去焦點為確認，Esc 為取消）。`hwnd` 為送出通知的編輯框：
    /// 通知是 post 的，抵達前編輯框可能已關閉、甚至換成另一個，這時忽略
    pub fn finish_edit(&mut self, commit: bool, hwnd: HWND) {
        if self.edit.as_ref().is_none_or(|e| e.hwnd != hwnd) {
            return;
        }
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::Ask)) {
            let text = self.edit.as_ref().map(|e| window::edit_text(e.hwnd));
            self.finish_ask(commit, text);
            return;
        }
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::Palette)) {
            if commit {
                self.run_palette(None);
            } else {
                self.close_palette();
            }
            return;
        }
        let Some(edit) = self.edit.take() else { return };
        let text = window::edit_text(edit.hwnd);
        window::destroy_edit(edit.hwnd);
        if commit {
            match edit.kind {
                EditKind::Rename { tab, path: full } => {
                    let new_name = text.trim().to_string();
                    let old_name = path::file_name(&full).to_string();
                    if !new_name.is_empty() && new_name != old_name && self.find_tab(tab).is_some() {
                        self.submit_op(FileOp::Rename { path: full, new_name });
                    }
                }
                EditKind::Address => {
                    let target = text.trim().to_string();
                    let pane = self.active_pane;
                    if target.is_empty() || target == "此電腦" || target == tr!("此電腦") {
                        self.navigate(pane, String::new(), true);
                    } else {
                        // 檢查是否為資料夾會碰磁碟（網路路徑可能很慢），交給背景
                        let normalized = path::normalize(&target);
                        let cwd = self.tab().path.clone();
                        let n = self.notifier.clone();
                        self.pool.spawn(move || {
                            zing_core::guard::io("檢查網址列路徑");
                            let is_dir = std::path::Path::new(&normalized).is_dir();
                            n.send(Msg::AddressResolved { pane, target, normalized, is_dir, cwd });
                        });
                    }
                }
                // 命令面板在函式開頭已處理
                EditKind::Palette => {}
                EditKind::ThemeField { field } => self.commit_theme_field(field, &text),
                EditKind::ThemeSaveAs => self.save_theme_as(&text),
                EditKind::ThemeRule { rule } => self.commit_rule_pattern(rule, &text),
                EditKind::Pref { field } => self.set_pref(field, text.trim().to_string()),
                EditKind::BarLabel { item } => self.commit_bar_label(item, &text),
                EditKind::BarCustom => self.commit_bar_custom(&text),
                EditKind::KeysSearch | EditKind::BarSearch => {}
                EditKind::Ask => {}
            }
        }
        window::focus_main(self.hwnd);
        self.invalidate();
    }
}

pub fn apply_window_theme(hwnd: HWND, theme: &Theme) {
    unsafe {
        let dark: i32 = (theme.mode == Mode::Dark) as i32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
        // DWMSBT_MAINWINDOW = 2（Mica），DWMSBT_NONE = 1
        let backdrop: i32 = if theme.effects.backdrop == Backdrop::Mica { 2 } else { 1 };
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as *const _, 4);
    }
}

/// `{date:%Y%m%d}` 用的簡易格式化（本地時間）
fn format_now(fmt: &str) -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    fmt.replace("%Y", &format!("{:04}", t.wYear))
        .replace("%m", &format!("{:02}", t.wMonth))
        .replace("%d", &format!("{:02}", t.wDay))
        .replace("%H", &format!("{:02}", t.wHour))
        .replace("%M", &format!("{:02}", t.wMinute))
        .replace("%S", &format!("{:02}", t.wSecond))
}
