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
use crate::model::{Children, Expand, Pane, Reveal, Tab, Tree};
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

/// 右鍵選單的自訂項目
pub const MENU_NEW_TAB: u32 = shell::EXTRA_ID_BASE + 1;
pub const MENU_FAV_ADD: u32 = shell::EXTRA_ID_BASE + 2;
pub const MENU_FAV_REMOVE: u32 = shell::EXTRA_ID_BASE + 3;

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
}

/// 有自己訊息迴圈的動作：必須在釋放 App 借用後才執行（見 window.rs）
pub enum Modal {
    DragOut { paths: Vec<String> },
}

pub enum EditKind {
    Rename { tab: TabId, path: String },
    Address,
    Palette,
}

pub struct Edit {
    pub hwnd: HWND,
    pub kind: EditKind,
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
    pub tree_visible: bool,
    pub preview: crate::preview::Preview,
    /// 批次重新命名面板
    pub batch: Option<crate::batch::Batch>,
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
        let (config, warnings, session, theme, theme_warning) = if smoke_dir.is_some() {
            let (config, warnings) = Config::load(None);
            (config, warnings, Session::default(), Theme::dark(), None)
        } else {
            let _allow = zing_core::guard::allow_io("啟動時讀取設定、主題與工作階段");
            let (config, warnings) = settings::load_config();
            let session = if isolated { Session::default() } else { Session::load() };
            let (theme, theme_warning) = settings::resolve_theme(&config.theme);
            (config, warnings, session, theme, theme_warning)
        };
        crate::perf::mark("config");
        let gfx = Gfx::new(hwnd, dpi, &theme, perf.is_some())?;
        crate::perf::mark("gfx");
        let (notifier, rx) = Notifier::new(hwnd);
        let reporter = notifier.clone();
        zing_core::guard::set_reporter(move |m| reporter.status(m, true));
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
        let pool = Pool::new(threads);
        let fileops = FileOps::new(notifier.clone());
        let icons = Icons::new(notifier.clone());
        let thumbs = crate::thumbs::Thumbs::new(notifier.clone(), crate::thumbs::cache_dir(isolated));
        thumbs.set_thumbnailers(config.thumbnailers.clone());
        let shell_host = ShellHost::new(notifier.clone(), hwnd);

        let mut favorites = config.favorites.clone();
        for f in &session.favorites {
            if !favorites.iter().any(|x| x.eq_ignore_ascii_case(f)) {
                favorites.push(f.clone());
            }
        }
        let tree = Tree::build(&favorites, &drives());
        let mut app = App {
            hwnd,
            gfx,
            rules: theme.compiled_rules(),
            tree_side: config.layout.tree,
            tree_visible: session.tree_visible,
            batch: None,
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
            preview_width: Some(self.preview.width),
            columns: Some(self.cols),
            favorites: self.session.favorites.clone(),
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
            services::launch(&self.notifier, f, None, Some(dir.clone()), None);
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
            Msg::FileOpDone { dirs, error, select } => {
                if let Some(e) = error {
                    self.set_status(e, true);
                }
                for d in &dirs {
                    self.sizes.mark_stale(d);
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
            Msg::Job { id, event } => self.on_job_event(id, event),
            Msg::FolderSize { parent, name, size, done } => self.on_folder_size(parent, name, size, done),
            Msg::FolderSizesLoaded(lines) => self.on_size_cache_loaded(lines),
            Msg::FolderViewsLoaded(v) => self.on_folder_views_loaded(v),
            Msg::Preview { generation, loaded } => self.on_preview_loaded(generation, *loaded),
            Msg::Thumb { id, pixels, source } => {
                self.thumbs.store(id, pixels, source);
                self.invalidate();
            }
            Msg::IndexSizes { parent, result } => self.on_index_sizes(parent, result),
            Msg::IndexAction { text, error, changed } => self.on_index_action(text, error, changed),
            Msg::Paste { sources, dest, cut } => self.submit_op(FileOp::Transfer { sources, dest, mov: cut }),
            Msg::ConfigLoaded { config, theme, warnings } => {
                if config.thumbnailers != self.config.thumbnailers {
                    self.thumbs.set_thumbnailers(config.thumbnailers.clone());
                }
                self.config = config;
                self.tree_side = self.config.layout.tree;
                self.apply_theme(theme);
                match warnings.into_iter().next() {
                    Some(w) => self.set_status(w, true),
                    None => self.set_status("已重新載入設定", false),
                }
            }
            Msg::ThemeLoaded(Ok(theme)) => self.apply_theme(theme),
            Msg::ThemeLoaded(Err(e)) => self.set_status(e, true),
            Msg::AddressResolved { pane, target, normalized, is_dir, cwd } => {
                if is_dir && pane < self.panes.len() {
                    self.navigate(pane, normalized, true);
                } else {
                    // 不是資料夾：交給系統開啟（檔案、網址、程式名稱都可以）
                    services::launch(&self.notifier, target, None, Some(cwd), None);
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
            n.send(Msg::ConfigLoaded { config, theme, warnings });
        });
    }

    // ------------------------------------------------------------ 命令

    /// 執行命令腳本（快速鍵、按鈕、選單都經由這裡）
    pub fn exec(&mut self, script: &str) {
        let parsed = {
            let mut vars = |name: &str, param: Option<&str>| self.variable(name, param);
            command::parse(script, &mut vars)
        };
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
                None => Err("沒有目標窗格：先用 Ctrl+\\ 分割窗格".into()),
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
                let target = inv.flag("path").or_else(|| inv.positional().next()).ok_or("nav.goto 需要路徑")?;
                self.navigate(p, target.to_string(), true);
            }
            "address.edit" => window::begin_address_edit(self),
            "palette.open" => self.open_palette(),
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
                    return Err("無法貼到「此電腦」".into());
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
                    return Err("請先選取兩個以上的項目".into());
                }
            }
            "file.newfolder" => {
                let dir = self.tab().path.clone();
                if dir.is_empty() {
                    return Err("無法在「此電腦」建立資料夾".into());
                }
                self.submit_op(FileOp::NewFolder { dir });
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
                let (extras, target) = self.entry_menu_extras(&items, &folder);
                let tab = self.tab();
                let item = self.item_rect(p, tab.focus);
                let y = match item {
                    Some(r) if !items.is_empty() => r.b,
                    _ => pl.list.t,
                };
                let x = if self.grid(p, pl.list).is_some() { item.map_or(pl.list.l, |r| r.l) + 16.0 } else { pl.cols[0].0 + 40.0 };
                let s = self.gfx.scale();
                let mut pt = POINT { x: (x * s) as i32, y: (y.clamp(pl.list.t, pl.list.b) * s) as i32 };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
                }
                self.show_context_menu(items, folder, pt, extras, target);
            }
            "file.copypath" => {
                let paths = self.tab().target_paths();
                let text = if paths.is_empty() { self.tab().path.clone() } else { paths.join("\r\n") };
                self.shell_host.submit(ShellJob::SetText { text, status: "已複製路徑".into() });
            }
            "copy" => {
                let dest = inv.flag("to").filter(|d| !d.is_empty()).ok_or("copy 需要 --to=目的地")?;
                let mut sources: Vec<String> = inv.positional().map(String::from).collect();
                if sources.is_empty() {
                    sources = self.tab().target_paths();
                }
                if sources.is_empty() {
                    return Err("沒有選取任何項目".into());
                }
                self.submit_op(FileOp::Transfer { sources, dest: dest.to_string(), mov: inv.has("move") });
            }
            "run" => {
                let mut args = inv.positional();
                let program = args.next().ok_or("run 需要程式名稱")?.to_string();
                let params: Vec<String> = args.map(quote_arg).collect();
                let dir = self.tab().path.clone();
                services::launch(&self.notifier, program, Some(params.join(" ")), Some(dir), None);
            }
            "open" => {
                for f in inv.positional() {
                    services::launch(&self.notifier, f.to_string(), None, None, None);
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
                let key = inv.flag("by").and_then(SortKey::parse).ok_or("view.sort 需要 --by=name|modified|type|size")?;
                let asc = if inv.has("desc") { Some(false) } else if inv.has("asc") { Some(true) } else { None };
                self.set_sort(key, asc);
            }
            "view.foldersizes" => self.compute_folder_sizes(p, false, false),
            "view.preview" => self.toggle_preview(),
            "view.mode" => {
                let mode = inv.flag("mode").or_else(|| inv.positional().next()).unwrap_or("list").to_string();
                self.set_view_mode(p, &mode)?;
            }
            "view.thumbsize" => {
                let delta = inv.flag("delta").and_then(|d| d.parse::<i32>().ok()).ok_or("view.thumbsize 需要 --delta=")?;
                self.zoom_thumbs(p, delta);
            }
            "index.install" | "index.uninstall" | "index.status" => self.index_command(&inv.name),
            "view.togglehidden" => {
                self.show_hidden = !self.show_hidden;
                let show_hidden = self.show_hidden;
                for pane in &mut self.panes {
                    for t in &mut pane.tabs {
                        t.rebuild_view(show_hidden);
                    }
                }
                self.tree.reset_probes();
                self.set_status(if show_hidden { "顯示隱藏檔案" } else { "不顯示隱藏檔案" }, false);
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
            "theme.set" => {
                let name = inv.flag("name").or_else(|| inv.positional().next()).ok_or("theme.set 需要 --name=")?.to_string();
                let n = self.notifier.clone();
                self.pool.spawn(move || {
                    let (theme, warn) = settings::resolve_theme(&name);
                    n.send(Msg::ThemeLoaded(match warn {
                        Some(w) => Err(w),
                        None => Ok(theme),
                    }));
                });
            }
            "theme.toggle" => {
                // 同一風格的另一種深淺色
                let theme = self.theme.toggled();
                self.apply_theme(theme);
            }
            "config.edit" => {
                let n = self.notifier.clone();
                self.pool.spawn(move || match settings::ensure_user_config() {
                    Ok(p) => services::launch(&n, "notepad.exe".into(), Some(quote_arg(&p.to_string_lossy())), None, None),
                    Err(e) => n.status(format!("無法建立設定檔：{e}"), true),
                });
            }
            "config.reload" => self.reload_config(),
            "app.exit" => window::close(self.hwnd),
            other => return Err(format!("未知命令：{other}")),
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
            return Err("「此電腦」不能加入我的最愛".into());
        }
        if self.is_favorite(&p) {
            return Ok(());
        }
        self.favorites.push(p.clone());
        self.session.favorites.push(p.clone());
        self.save_session_async();
        self.rebuild_tree();
        self.set_status(format!("已加入我的最愛：{p}"), false);
        Ok(())
    }

    pub fn remove_favorite(&mut self, p: &str) -> Result<(), String> {
        if self.config.favorites.iter().any(|f| f.eq_ignore_ascii_case(p)) {
            return Err("這個項目定義在設定檔的 favorites 中，請在設定檔移除（Ctrl+,）".into());
        }
        self.favorites.retain(|f| !f.eq_ignore_ascii_case(p));
        self.session.favorites.retain(|f| !f.eq_ignore_ascii_case(p));
        self.save_session_async();
        self.rebuild_tree();
        self.set_status(format!("已從我的最愛移除：{p}"), false);
        Ok(())
    }

    /// 清單項目右鍵選單的自訂項目：只選了一個資料夾時提供「在新分頁開啟」與我的最愛；
    /// 沒有選取時（背景選單）提供把目前資料夾加入我的最愛
    pub fn entry_menu_extras(&self, items: &[String], folder: &str) -> (Vec<(u32, String)>, String) {
        let tab = self.tab();
        let single_dir = match tab.targets().as_slice() {
            [e] if items.len() == 1 => tab.listing.entries[*e].is_dir(),
            _ => false,
        };
        if single_dir {
            (self.folder_extras(&items[0]), items[0].clone())
        } else if items.is_empty() && !folder.is_empty() && !self.is_favorite(folder) {
            (vec![(MENU_FAV_ADD, "將此資料夾加入我的最愛".into())], folder.to_string())
        } else {
            (Vec::new(), String::new())
        }
    }

    pub fn folder_extras(&self, p: &str) -> Vec<(u32, String)> {
        let fav = if self.is_favorite(p) {
            (MENU_FAV_REMOVE, "從我的最愛移除".to_string())
        } else {
            (MENU_FAV_ADD, "加入我的最愛".to_string())
        };
        vec![(MENU_NEW_TAB, "在新分頁開啟".into()), fav]
    }

    pub fn on_menu_extra(&mut self, id: u32, target: String) {
        let result = match id {
            MENU_NEW_TAB => {
                self.open_in_new_tab(target);
                Ok(())
            }
            MENU_FAV_ADD => self.add_favorite(target),
            MENU_FAV_REMOVE => self.remove_favorite(&target),
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
                    if target.is_empty() || target == "此電腦" {
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
