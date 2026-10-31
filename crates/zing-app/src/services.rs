//! 背景服務：工作池、UI 通知、資料夾監看、外部程式啟動。
//!
//! UI 執行緒不做任何 I/O：所有工作在這裡執行，結果經由 `Notifier` 送回，
//! 並以 `PostMessageW` 喚醒 UI（同一批結果只發一次訊息）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use zing_core::enumerate::{EnumError, enumerate};
use zing_core::listing::Listing;
use zing_core::sort::{SortSpec, sorted_order};
use zing_core::text::{natural_cmp, wide_z};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateEventW, GetCurrentThread, GetExitCodeProcess, INFINITE, SetEvent, SetThreadPriority,
    THREAD_MODE_BACKGROUND_BEGIN, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SHGFI_EXETYPE, SHGetFileInfoW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_SHOWNORMAL, WM_APP};
use windows::core::PCWSTR;

pub const WM_APP_NOTIFY: u32 = WM_APP + 1;

pub type TabId = u64;

pub enum Msg {
    /// 列舉進度或完成；`listing` 為目前為止的完整內容（前綴穩定）
    Listing { tab: TabId, generation: u64, listing: Arc<Listing>, sorted: Vec<u32>, done: bool },
    /// `gone`：資料夾不存在時，仍存在的最近的上層（「此電腦」為空字串）
    ListingError { tab: TabId, generation: u64, error: String, gone: Option<String> },
    /// 分頁的資料夾已不存在（被刪除或改名，`check_gone`）；`existing` 是仍存在的最近的上層
    FolderGone { path: String, existing: String },
    Sorted { tab: TabId, generation: u64, spec: SortSpec, sorted: Vec<u32> },
    /// 樹狀列的子資料夾；None 表示這個位置不預先載入（`kind` 為 Probe 時）
    /// `show_hidden`：列舉時是否包含隱藏資料夾（設定在途中改變時丟棄預先載入的結果）
    TreeChildren { path: String, children: Option<crate::model::Children>, kind: TreeLoad, show_hidden: bool },
    /// 樹狀列展開中的資料夾，子資料夾有新增、刪除或改名（`TreeWatch`）
    TreeDirChanged(String),
    /// 全磁碟搜尋的結果（search.rs）
    SearchResults { generation: u64, result: Result<zing_core::indexproto::Response, zing_core::indexproto::QueryError> },
    DirChanged { tab: TabId, generation: u64 },
    /// `renamed`：成功改名的項目（舊的完整路徑, 新的完整路徑）；`vacated`：送出時暫停監看的項目（`FileOp::vacated`）
    FileOpDone { dirs: Vec<String>, error: Option<String>, select: Option<Select>, renamed: Option<(String, String)>, vacated: Vec<String> },
    Status { text: String, error: bool },
    Icon { id: u32, pixels: Option<crate::icons::IconPixels> },
    /// 縮圖（None：沒有縮圖，畫圖示）
    Thumb { id: u32, pixels: Option<crate::icons::IconPixels>, source: crate::thumbs::Source },
    /// 手動指定（`set`）或移除縮圖完成
    ThumbCustom { path: String, set: bool, result: Result<(), String> },
    /// 右鍵選單選了「重新命名」
    MenuRename,
    /// 右鍵選單選了程式自訂的項目
    MenuExtra { id: u32, target: String },
    /// 預先準備好的第三方擴充功能項目（精簡右鍵選單的第一層）
    MenuExtensions { items: Vec<String>, folder: String, menus: Vec<crate::shellext::ExtMenu>, hidden: Vec<String> },
    /// 剪貼簿上的檔案已讀出，交給檔案作業佇列
    Paste { sources: Vec<String>, dest: String, cut: bool },
    /// 背景重新讀取的設定與主題
    ConfigLoaded { config: Box<zing_core::config::Config>, theme: zing_core::theme::Theme, warnings: Vec<String> },
    /// 背景讀取的主題（theme.set）
    ThemeLoaded { name: String, result: Result<zing_core::theme::Theme, String> },
    /// 主題檔寫入完成（另存、覆蓋、匯入）
    ThemeSaved { name: String, result: Result<(), String> },
    /// 監看中的主題檔目前的修改時間
    ThemeFileTime { name: String, mtime: Option<std::time::SystemTime> },
    /// 匯入：(名稱, 內容, 是否已有同名主題)
    ThemeImport(Result<(String, String, bool), String>),
    /// themes 資料夾中的使用者主題名稱（設定頁）
    UserThemes(Vec<String>),
    /// 設定頁：緩慢的右鍵選單擴充功能
    SlowMenuExts(Vec<crate::shellext::SlowExt>),
    /// 系統字型（主題編輯器的字型選單）：（英文名稱, 顯示名稱）
    Fonts(Vec<(String, String)>),
    /// 命令列的圖片圖示讀取完成
    CmdBarImage { path: String, pixels: Option<crate::icons::IconPixels> },
    /// 網址列輸入已在背景確認是否為資料夾
    AddressResolved { pane: usize, target: String, normalized: String, is_dir: bool, cwd: String },
    /// 作業佇列中某個工作的狀態或進度
    Job { id: crate::jobs::JobId, event: crate::jobs::JobEvent },
    /// 資料夾大小：`done` 為 None 是途中回報，Some(true) 完成，Some(false) 取消或略過
    FolderSize { parent: String, name: String, size: zing_core::dirsize::DirSize, done: Option<bool> },
    /// 背景讀出的資料夾大小快取
    FolderSizesLoaded(Vec<zing_core::dirsize::CacheLine>),
    /// 預覽窗格的內容
    Preview { generation: u64, loaded: Box<crate::preview::Loaded> },
    /// Lua 腳本要求執行的命令（結構化，不經命令語言的解析與變數展開）
    ScriptRun(Vec<zing_core::command::Invocation>),
    /// Lua 腳本的 exec(命令腳本)
    ScriptExec(String),
    /// Lua 腳本的 ask()：顯示輸入框，回答送回 `reply`（取消為 None）
    ScriptAsk { prompt: String, default: String, reply: std::sync::mpsc::SyncSender<Option<String>> },
    /// Lua 的 pick_folder()：顯示選擇資料夾的對話方塊，結果送回 reply
    ScriptPickFolder { title: String, default: String, reply: std::sync::mpsc::SyncSender<Option<String>> },
    /// Lua 的 clipboard(text)：經 ShellHost 寫入剪貼簿，結果送回 reply
    ScriptClipboard { text: String, reply: std::sync::mpsc::SyncSender<bool> },
    ScriptsLoaded(crate::script::Loaded),
    /// 預覽處理常式顯示完成（ok 為 false：處理常式失敗）
    PreviewHost { generation: u64, ok: bool },
    /// 背景讀出的各資料夾檢視設定
    FolderViewsLoaded(zing_core::folderview::FolderViews),
    /// 向索引服務查詢資料夾大小的結果（`parent` 底下的子資料夾）
    IndexSizes { parent: String, result: Result<zing_core::indexproto::Response, zing_core::indexproto::QueryError> },
    /// 安裝、移除索引服務或查詢狀態的結果（狀態列訊息，成功時重新查詢）
    IndexAction { text: String, error: bool, changed: bool },
    /// 已安裝的索引服務比這版 zing 舊（版號）
    IndexOutdated { installed: u32 },
    /// 歡迎畫面開啟時查詢索引服務是否已安裝的結果
    IndexInstalled(bool),
    /// scripts 資料夾中的 .lua 檔案改變了（script.rs 的監看）
    ScriptsChanged,
    /// 網址列「›」：那一層的子資料夾名稱
    CrumbFolders { folder: String, names: Vec<String> },
    /// 檢查新版本的結果：最新版的標籤（沒有任何 Release 時為 None）、略過的版本；`manual` 為 update.check 命令
    UpdateChecked { latest: Result<Option<String>, String>, skipped: Option<zing_core::update::Version>, manual: bool },
    /// 下載更新的進度（0–100）
    UpdateProgress { percent: u32 },
    /// 更新安裝完成（Ok(false)：途中取消）
    UpdateInstalled(Result<bool, String>),
    /// 是否為預設的檔案總管（defaultapp.rs）；`toggled`：使用者切換的結果（顯示訊息）
    DefaultApp { result: Result<crate::defaultapp::Status, String>, toggled: bool },
    /// 其他程式要 zing 開啟的資料夾（shellwin.rs 的 Navigate、BrowseObject）
    ShellOpen(String),
    /// 其他程式要選取的項目（`SHOpenFolderAndSelectItems` → `IShellView::SelectItem`）；`rename`：選取後重新命名
    ShellSelect { path: String, rename: bool },
}

/// 檔案作業完成後要選取（並可能進入重新命名）的項目
pub struct Select {
    pub dir: String,
    pub name: String,
    pub rename: bool,
}

#[derive(Clone)]
pub struct Notifier {
    tx: Sender<Msg>,
    hwnd: isize,
    pending: Arc<AtomicBool>,
}

impl Notifier {
    pub fn new(hwnd: HWND) -> (Self, Receiver<Msg>) {
        let (tx, rx) = channel();
        (Self { tx, hwnd: hwnd.0 as isize, pending: Arc::new(AtomicBool::new(false)) }, rx)
    }

    pub fn send(&self, msg: Msg) {
        if self.tx.send(msg).is_ok() && !self.pending.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as _)), WM_APP_NOTIFY, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn status(&self, text: impl Into<String>, error: bool) {
        self.send(Msg::Status { text: text.into(), error });
    }

    /// UI 收到 WM_APP_NOTIFY、開始取出訊息前呼叫
    pub fn acknowledge(&self) {
        self.pending.store(false, Ordering::Release);
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// 固定大小的工作池。工作彼此獨立，取消由各工作自行檢查旗標。
pub struct Pool {
    tx: Sender<Job>,
}

impl Pool {
    pub fn new(threads: usize) -> Self {
        Self::with(threads, "zing-worker", false)
    }

    /// 背景模式的工作池：磁碟與 CPU 優先權最低，用於資料夾大小、樹狀列預先載入等不急的工作
    pub fn background(threads: usize, name: &str) -> Self {
        Self::with(threads, name, true)
    }

    fn with(threads: usize, name: &str, background: bool) -> Self {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..threads {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || {
                    if background {
                        unsafe {
                            let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
                        }
                    }
                    loop {
                        let job = match rx.lock() {
                            Ok(guard) => guard.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
                .expect("無法建立工作執行緒");
        }
        Self { tx }
    }

    pub fn spawn(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.tx.send(Box::new(job));
    }
}

/// 列舉資料夾並排序，分段回報。
pub fn load_dir(
    pool: &Pool,
    n: &Notifier,
    tab: TabId,
    generation: u64,
    path: String,
    spec: SortSpec,
    cancel: Arc<AtomicBool>,
) {
    let n = n.clone();
    pool.spawn(move || {
        let result = enumerate(&path, false, &cancel, |partial| {
            let listing = Arc::new(partial.clone());
            let sorted = sorted_order(&listing, spec);
            n.send(Msg::Listing { tab, generation, listing, sorted, done: false });
        });
        match result {
            Ok(listing) => {
                let listing = Arc::new(listing);
                let sorted = sorted_order(&listing, spec);
                n.send(Msg::Listing { tab, generation, listing, sorted, done: true });
            }
            Err(EnumError::Cancelled) => {}
            Err(e) => {
                let gone = matches!(e, EnumError::NotFound).then(|| nearest_existing(&path));
                n.send(Msg::ListingError { tab, generation, error: e.to_string(), gone });
            }
        }
    });
}

/// 確認分頁的資料夾還在（上層的子資料夾改變、檔案作業之後）；不在就送出 `Msg::FolderGone`
pub fn check_gone(pool: &Pool, n: &Notifier, path: String) {
    let n = n.clone();
    pool.spawn(move || {
        zing_core::guard::io("確認資料夾是否還在");
        if !std::path::Path::new(&path).is_dir() {
            let existing = nearest_existing(&path);
            n.send(Msg::FolderGone { path, existing });
        }
    });
}

/// `path` 的上層中仍存在的最近一個；都不在（磁碟機已移除）就是「此電腦」（空字串）
fn nearest_existing(path: &str) -> String {
    zing_core::guard::io("找出仍存在的上層資料夾");
    let mut p = path.to_string();
    while let Some(up) = zing_core::path::parent(&p) {
        if up.is_empty() || std::path::Path::new(&up).is_dir() {
            return up;
        }
        p = up;
    }
    String::new()
}

pub fn sort_job(pool: &Pool, n: &Notifier, tab: TabId, generation: u64, listing: Arc<Listing>, spec: SortSpec) {
    let n = n.clone();
    pool.spawn(move || {
        let sorted = sorted_order(&listing, spec);
        n.send(Msg::Sorted { tab, generation, spec, sorted });
    });
}

/// 樹狀列載入子資料夾的用途
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeLoad {
    /// 使用者展開，等著顯示
    Expand,
    /// 預先載入可見的節點（只在索引或本機固定磁碟上做）
    Probe,
    /// 以預先載入的內容展開後重新確認
    Refresh,
}

/// 樹狀列：列出子資料夾。有索引時從索引取得（同時知道每個子資料夾是否還有子資料夾），否則列舉。
pub fn tree_children(pool: &Pool, n: &Notifier, path: String, show_hidden: bool, use_index: bool, kind: TreeLoad) {
    let n = n.clone();
    pool.spawn(move || {
        let children = subdirs(&path, show_hidden, use_index, kind == TreeLoad::Probe);
        n.send(Msg::TreeChildren { path, children, kind, show_hidden });
    });
}

/// 網址列「›」：列出 `folder` 的子資料夾，結果以 `Msg::CrumbFolders` 送回
pub fn crumb_folders(pool: &Pool, n: &Notifier, folder: String, show_hidden: bool, use_index: bool) {
    let n = n.clone();
    pool.spawn(move || {
        let names = subdirs(&folder, show_hidden, use_index, false).map(|c| c.names).unwrap_or_default();
        n.send(Msg::CrumbFolders { folder, names });
    });
}

fn subdirs(path: &str, show_hidden: bool, use_index: bool, probe: bool) -> Option<crate::model::Children> {
    if use_index && let Some(c) = crate::index::subdirs(path, show_hidden) {
        return Some(c);
    }
    // 預先載入不碰網路磁碟、隨身碟、光碟機（可能很慢或讓光碟開始轉）
    if probe && !is_fixed_drive(path) {
        return None;
    }
    let never = AtomicBool::new(false);
    let names: Vec<String> = match enumerate(path, true, &never, |_| {}) {
        Ok(l) => {
            let mut idx: Vec<usize> =
                (0..l.len()).filter(|&i| l.entries[i].is_dir() && (show_hidden || !l.entries[i].is_hidden())).collect();
            idx.sort_by(|&a, &b| natural_cmp(l.name(a), l.name(b)));
            idx.into_iter().map(|i| l.name_string(i)).collect()
        }
        Err(_) => Vec::new(),
    };
    let subdirs = vec![None; names.len()];
    Some(crate::model::Children::new(names, subdirs))
}

/// 本機固定磁碟（自動計算與預先載入只在這裡做，不碰網路磁碟與隨身碟）
pub fn is_fixed_drive(p: &str) -> bool {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_FIXED: u32 = 3;
    if p.starts_with("\\\\") || p.len() < 2 || p.as_bytes()[1] != b':' {
        return false;
    }
    let root = wide_z(&format!("{}\\", &p[..2]));
    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_FIXED }
}

/// 資料夾監看。Drop 時停止。
pub struct Watcher {
    stop: isize,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(HANDLE(self.stop as _));
        }
    }
}

pub fn watch(path: &str, n: &Notifier, tab: TabId, generation: u64) -> Option<Watcher> {
    if path.is_empty() {
        return None;
    }
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE;
    let n = n.clone();
    watch_dir(path, filter, move || move || n.send(Msg::DirChanged { tab, generation }))
}

/// 監看資料夾（不含子資料夾）。`start` 在監看執行緒上、開啟監看之前執行（網路路徑可能很慢，不能卡住 UI），
/// 回傳每次 `filter` 的變更發生時呼叫的函式。Drop 時停止
pub fn watch_dir<F: FnMut()>(path: &str, filter: FILE_NOTIFY_CHANGE, start: impl FnOnce() -> F + Send + 'static) -> Option<Watcher> {
    let stop = unsafe { CreateEventW(None, true, false, None).ok()? };
    let stop_raw = stop.0 as isize;
    let path_w = wide_z(path);
    let spawned = std::thread::Builder::new().name("zing-watch".into()).spawn(move || {
        let stop = HANDLE(stop_raw as _);
        let mut on_change = start();
        if let Ok(change) = unsafe { FindFirstChangeNotificationW(PCWSTR(path_w.as_ptr()), false, filter) } {
            loop {
                let r = unsafe { WaitForMultipleObjects(&[stop, change], false, INFINITE) };
                if r.0 == WAIT_OBJECT_0.0 + 1 {
                    on_change();
                    if unsafe { FindNextChangeNotification(change) }.is_err() {
                        break;
                    }
                } else {
                    break;
                }
            }
            unsafe {
                let _ = FindCloseChangeNotification(change);
            }
        }
        unsafe {
            let _ = CloseHandle(stop);
        }
    });
    if spawned.is_err() {
        unsafe {
            let _ = CloseHandle(stop);
        }
        return None;
    }
    Some(Watcher { stop: stop_raw })
}

/// 樹狀列：監看展開中的節點（只看子資料夾名稱的變更），變更時送出 `Msg::TreeDirChanged`。
/// 一條執行緒以 WaitForMultipleObjects 等待所有節點（上限 `TREE_WATCH_MAX` 個）；在背景開啟監看，網路路徑慢也不影響 UI
pub struct TreeWatch {
    tx: Option<Sender<Vec<String>>>,
    wake: Arc<Event>,
    watched: Vec<String>,
}

/// WaitForMultipleObjects 最多 64 個：留一個給喚醒用的事件
pub const TREE_WATCH_MAX: usize = 63;

/// 兩個執行緒共用的事件，最後一個使用者放開時關閉
struct Event(isize);

impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(HANDLE(self.0 as _));
        }
    }
}

impl TreeWatch {
    pub fn new(n: &Notifier) -> Option<Self> {
        let wake = Arc::new(Event(unsafe { CreateEventW(None, false, false, None).ok()? }.0 as isize));
        let (tx, rx) = channel();
        let (n, w) = (n.clone(), wake.clone());
        std::thread::Builder::new().name("zing-tree-watch".into()).spawn(move || tree_watch_loop(&rx, &w, &n)).ok()?;
        Some(Self { tx: Some(tx), wake, watched: Vec::new() })
    }

    /// 要監看的資料夾；與上次相同時不做事（每幀呼叫）
    pub fn set(&mut self, dirs: &[&str]) {
        let dirs = &dirs[..dirs.len().min(TREE_WATCH_MAX)];
        if dirs.len() == self.watched.len() && dirs.iter().zip(&self.watched).all(|(a, b)| a == b) {
            return;
        }
        self.watched = dirs.iter().map(|d| d.to_string()).collect();
        if let Some(tx) = &self.tx {
            let _ = tx.send(self.watched.clone());
        }
        unsafe {
            let _ = SetEvent(HANDLE(self.wake.0 as _));
        }
    }
}

impl Drop for TreeWatch {
    fn drop(&mut self) {
        // 先關閉通道再喚醒：執行緒醒來看到通道已關閉就結束
        self.tx = None;
        unsafe {
            let _ = SetEvent(HANDLE(self.wake.0 as _));
        }
    }
}

fn tree_watch_loop(rx: &Receiver<Vec<String>>, wake: &Event, n: &Notifier) {
    let close = |h: HANDLE| unsafe {
        let _ = FindCloseChangeNotification(h);
    };
    let mut watched: Vec<(String, HANDLE)> = Vec::new();
    loop {
        let mut handles = vec![HANDLE(wake.0 as _)];
        handles.extend(watched.iter().map(|w| w.1));
        let r = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        let i = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        if i == 0 {
            // 只取最新的清單；送出端已關閉就結束
            let mut latest = None;
            loop {
                match rx.try_recv() {
                    Ok(v) => latest = Some(v),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        for (_, h) in watched {
                            close(h);
                        }
                        return;
                    }
                }
            }
            let Some(dirs) = latest else { continue };
            let mut old = std::mem::take(&mut watched);
            for d in dirs {
                if let Some(k) = old.iter().position(|(p, _)| *p == d) {
                    watched.push(old.swap_remove(k));
                    continue;
                }
                let w = wide_z(&d);
                if let Ok(h) = unsafe { FindFirstChangeNotificationW(PCWSTR(w.as_ptr()), false, FILE_NOTIFY_CHANGE_DIR_NAME) } {
                    watched.push((d, h));
                }
            }
            for (_, h) in old {
                close(h);
            }
        } else if i < handles.len() {
            n.send(Msg::TreeDirChanged(watched[i - 1].0.clone()));
            // 資料夾本身被刪除等：不再監看，直到下次 set
            if unsafe { FindNextChangeNotification(watched[i - 1].1) }.is_err() {
                let (_, h) = watched.remove(i - 1);
                close(h);
            }
        } else {
            // WAIT_FAILED：不應發生，避免空轉
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}

/// 開啟檔案或執行程式（見 `start`）。在獨立的 STA 執行緒進行：
/// 某些檔案類型的開啟處理常式會阻塞很久，不能放在 UI 執行緒。
pub fn launch(n: &Notifier, file: String, params: Option<String>, dir: Option<String>, verb: Option<&'static str>, how: Show) {
    let n = n.clone();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let hwnd = HWND(n.hwnd() as _);
        if let Err(e) = start(&file, params.as_deref(), dir.as_deref(), verb, how, hwnd) {
            n.status(tr!("無法開啟 {file}：{e}", file, e), true);
        }
        CoUninitialize();
    });
}

/// 主控台程式的視窗
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Show {
    /// 和檔案總管相同（開啟檔案、網址列輸入、互動式終端機）
    Normal,
    /// 主控台程式（含 .bat、.cmd）不顯示主控台視窗；視窗程式、文件照常開啟
    HideConsole,
}

/// 已啟動的程式
pub enum Started {
    Child(std::process::Child),
    Process(HANDLE),
    /// 開啟文件、網址、使用者取消（例如 UAC）時沒有程序可等
    None,
}

impl Started {
    /// 等程式結束並回傳結束代碼（沒有程序可等時為 None）
    pub fn wait(mut self) -> Option<u32> {
        match &mut self {
            Started::Child(c) => c.wait().ok().and_then(|s| s.code()).map(|c| c as u32),
            Started::Process(h) => unsafe {
                let _ = WaitForSingleObject(*h, INFINITE);
                let mut code = 0u32;
                GetExitCodeProcess(*h, &mut code).ok().map(|_| code)
            },
            Started::None => None,
        }
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        if let Started::Process(h) = self {
            unsafe {
                let _ = CloseHandle(*h);
            }
        }
    }
}

/// 開啟檔案或執行程式（會阻塞：解析路徑、Shell 的開啟處理常式；呼叫端要先以 STA 初始化 COM）。
/// `HideConsole` 且目標是主控台程式時以 CREATE_NO_WINDOW 直接建立程序；其餘交給 ShellExecuteEx
/// （文件、網址、App Paths 登錄的程式名稱、視窗程式、動詞）。
pub fn start(file: &str, params: Option<&str>, dir: Option<&str>, verb: Option<&str>, how: Show, hwnd: HWND) -> Result<Started, String> {
    zing_core::guard::io("啟動程式");
    let params = params.filter(|p| !p.is_empty());
    let dir = dir.filter(|d| !d.is_empty());
    if how == Show::HideConsole
        && verb.is_none()
        && let Some(exe) = resolve_program(file, dir).filter(|e| is_console_program(e))
    {
        return spawn_hidden(&exe, params, dir).map(Started::Child).map_err(|e| e.to_string());
    }
    let file_w = wide_z(file);
    let params_w = params.map(wide_z);
    let dir_w = dir.map(wide_z);
    let verb_w = verb.map(wide_z);
    let ptr = |v: &Option<Vec<u16>>| v.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // run（HideConsole）找不到程式時只回報錯誤，不跳出 Shell 的對話方塊；開啟檔案時保留「選擇開啟方式」等對話方塊
        fMask: SEE_MASK_NOASYNC
            | SEE_MASK_NOCLOSEPROCESS
            | if how == Show::HideConsole { SEE_MASK_FLAG_NO_UI } else { Default::default() },
        hwnd,
        lpVerb: ptr(&verb_w),
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: ptr(&params_w),
        lpDirectory: ptr(&dir_w),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    match unsafe { ShellExecuteExW(&mut info) } {
        Ok(()) if !info.hProcess.is_invalid() => Ok(Started::Process(info.hProcess)),
        Ok(()) => Ok(Started::None),
        // 使用者取消（例如 UAC）不算錯誤
        Err(e) if e.code().0 as u32 == 0x800704C7 => Ok(Started::None),
        Err(e) => Err(e.message()),
    }
}

/// `cmd.exe /d /s /c "命令列"`，不顯示主控台視窗（Lua 的 os.execute、io.popen）
pub fn shell_command(line: &str, dir: Option<&str>) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(system32("cmd.exe"));
    c.raw_arg(format!("/d /s /c \"{line}\""));
    if let Some(d) = dir.filter(|d| !d.is_empty()) {
        c.current_dir(d);
    }
    c.creation_flags(CREATE_NO_WINDOW.0).stdin(std::process::Stdio::null());
    c
}

fn system32(name: &str) -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::path::Path::new(&root).join("System32").join(name)
}

fn spawn_hidden(exe: &std::path::Path, params: Option<&str>, dir: Option<&str>) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    let mut c = if is_batch(exe) {
        // 批次檔要經 cmd.exe 執行
        shell_command(&format!("\"{}\" {}", exe.display(), params.unwrap_or("")), dir)
    } else {
        let mut c = std::process::Command::new(exe);
        if let Some(p) = params {
            c.raw_arg(p);
        }
        if let Some(d) = dir {
            c.current_dir(d);
        }
        c.creation_flags(CREATE_NO_WINDOW.0).stdin(std::process::Stdio::null());
        c
    };
    c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()
}

fn is_batch(p: &std::path::Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd"))
}

/// 找出程式的完整路徑：含路徑時以 dir 為基準，否則依 PATH 搜尋；沒有副檔名時依 PATHEXT 補上。
/// 找不到（App Paths 登錄的名稱、文件、網址、App 執行別名）時回傳 None，交給 ShellExecuteEx
fn resolve_program(file: &str, dir: Option<&str>) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|e| e.starts_with('.'))
        .map(str::to_string)
        .collect();
    let find = |base: PathBuf| -> Option<PathBuf> {
        if base.is_file() {
            return Some(base);
        }
        exts.iter()
            .map(|e| {
                let mut s = base.clone().into_os_string();
                s.push(e);
                PathBuf::from(s)
            })
            .find(|c| c.is_file())
    };
    let p = Path::new(file);
    if file.contains(['\\', '/', ':']) {
        return match dir {
            Some(d) if p.is_relative() => find(Path::new(d).join(p)),
            _ => find(p.to_path_buf()),
        };
    }
    let path = std::env::var_os("PATH")?;
    std::iter::once(system32("")).chain(std::env::split_paths(&path)).find_map(|d| find(d.join(file)))
}

/// 主控台程式（PE 的子系統為主控台、DOS 程式、批次檔）
fn is_console_program(p: &std::path::Path) -> bool {
    if is_batch(p) {
        return true;
    }
    let w = wide_z(&p.to_string_lossy());
    let r = unsafe { SHGetFileInfoW(PCWSTR(w.as_ptr()), Default::default(), None, 0, SHGFI_EXETYPE) };
    // 低位元組是 "PE"（0x4550）或 "MZ"（0x5A4D）、高位元組（Windows 版本）為 0 才是主控台程式
    r >> 16 == 0 && matches!(r & 0xFFFF, 0x4550 | 0x5A4D)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_programs_are_detected() {
        let cmd = resolve_program("cmd", None).unwrap();
        assert!(cmd.to_string_lossy().to_lowercase().ends_with("cmd.exe"), "{}", cmd.display());
        assert!(is_console_program(&cmd));
        assert!(is_console_program(&resolve_program("tar.exe", None).unwrap()));
        assert!(!is_console_program(&resolve_program("notepad", None).unwrap()));
        assert!(resolve_program("不存在的程式-zing", None).is_none());
    }

    #[test]
    fn hidden_program_returns_exit_code() {
        let s = start("cmd", Some("/c exit 7"), None, None, Show::HideConsole, HWND::default()).unwrap();
        assert!(matches!(s, Started::Child(_)));
        assert_eq!(s.wait(), Some(7));
        let out = shell_command("echo 中文 & exit 3", None).output().unwrap();
        assert_eq!(out.status.code(), Some(3));
    }
}
