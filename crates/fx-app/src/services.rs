//! 背景服務：工作池、UI 通知、資料夾監看、外部程式啟動。
//!
//! UI 執行緒不做任何 I/O：所有工作在這裡執行，結果經由 `Notifier` 送回，
//! 並以 `PostMessageW` 喚醒 UI（同一批結果只發一次訊息）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use fx_core::enumerate::{EnumError, enumerate};
use fx_core::listing::Listing;
use fx_core::sort::{SortSpec, sorted_order};
use fx_core::text::{natural_cmp, wide_z};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_SHOWNORMAL, WM_APP};
use windows::core::PCWSTR;

pub const WM_APP_NOTIFY: u32 = WM_APP + 1;

pub type TabId = u64;

pub enum Msg {
    /// 列舉進度或完成；`listing` 為目前為止的完整內容（前綴穩定）
    Listing { tab: TabId, generation: u64, listing: Arc<Listing>, sorted: Vec<u32>, done: bool },
    ListingError { tab: TabId, generation: u64, error: String },
    Sorted { tab: TabId, generation: u64, spec: SortSpec, sorted: Vec<u32> },
    TreeChildren { path: String, names: Vec<String> },
    DirChanged { tab: TabId, generation: u64 },
    FileOpDone { dirs: Vec<String>, error: Option<String>, select: Option<Select> },
    Status { text: String, error: bool },
    Icon { id: u32, pixels: Option<crate::icons::IconPixels> },
    /// 右鍵選單選了「重新命名」
    MenuRename,
    /// 右鍵選單選了程式自訂的項目
    MenuExtra { id: u32, target: String },
    /// 剪貼簿上的檔案已讀出，交給檔案作業佇列
    Paste { sources: Vec<String>, dest: String, cut: bool },
    /// 背景重新讀取的設定與主題
    ConfigLoaded { config: fx_core::config::Config, theme: fx_core::theme::Theme, warnings: Vec<String> },
    /// 背景讀取的主題（theme.set）
    ThemeLoaded(Result<fx_core::theme::Theme, String>),
    /// 網址列輸入已在背景確認是否為資料夾
    AddressResolved { pane: usize, target: String, normalized: String, is_dir: bool, cwd: String },
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
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..threads {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("fx-worker-{i}"))
                .spawn(move || {
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
            Err(e) => n.send(Msg::ListingError { tab, generation, error: e.to_string() }),
        }
    });
}

pub fn sort_job(pool: &Pool, n: &Notifier, tab: TabId, generation: u64, listing: Arc<Listing>, spec: SortSpec) {
    let n = n.clone();
    pool.spawn(move || {
        let sorted = sorted_order(&listing, spec);
        n.send(Msg::Sorted { tab, generation, spec, sorted });
    });
}

/// 樹狀列：列出子資料夾名稱（自然排序）
pub fn tree_children(pool: &Pool, n: &Notifier, path: String, show_hidden: bool) {
    let n = n.clone();
    pool.spawn(move || {
        let never = AtomicBool::new(false);
        let names = match enumerate(&path, true, &never, |_| {}) {
            Ok(l) => {
                let mut idx: Vec<usize> =
                    (0..l.len()).filter(|&i| l.entries[i].is_dir() && (show_hidden || !l.entries[i].is_hidden())).collect();
                idx.sort_by(|&a, &b| natural_cmp(l.name(a), l.name(b)));
                idx.into_iter().map(|i| l.name_string(i)).collect()
            }
            Err(_) => Vec::new(),
        };
        n.send(Msg::TreeChildren { path, names });
    });
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
    let stop = unsafe { CreateEventW(None, true, false, None).ok()? };
    let stop_raw = stop.0 as isize;
    let path_w = wide_z(path);
    let n = n.clone();
    let spawned = std::thread::Builder::new().name("fx-watch".into()).spawn(move || {
        let stop = HANDLE(stop_raw as _);
        // 在背景開啟監看：網路路徑可能很慢，不能卡住 UI
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE;
        if let Ok(change) = unsafe { FindFirstChangeNotificationW(PCWSTR(path_w.as_ptr()), false, filter) } {
            loop {
                let r = unsafe { WaitForMultipleObjects(&[stop, change], false, INFINITE) };
                if r.0 == WAIT_OBJECT_0.0 + 1 {
                    n.send(Msg::DirChanged { tab, generation });
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

/// 以 ShellExecuteEx 開啟檔案或執行程式。在獨立的 STA 執行緒進行：
/// 某些檔案類型的開啟處理常式會阻塞很久，不能放在 UI 執行緒。
pub fn launch(n: &Notifier, file: String, params: Option<String>, dir: Option<String>, verb: Option<&'static str>) {
    let n = n.clone();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let file_w = wide_z(&file);
        let params_w = params.as_deref().map(wide_z);
        let dir_w = dir.as_deref().filter(|d| !d.is_empty()).map(wide_z);
        let verb_w = verb.map(wide_z);
        let ptr = |v: &Option<Vec<u16>>| v.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC,
            hwnd: HWND(n.hwnd() as _),
            lpVerb: ptr(&verb_w),
            lpFile: PCWSTR(file_w.as_ptr()),
            lpParameters: ptr(&params_w),
            lpDirectory: ptr(&dir_w),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        if let Err(e) = ShellExecuteExW(&mut info) {
            // 使用者取消（例如 UAC）不算錯誤
            if e.code().0 as u32 != 0x800704C7 {
                n.status(format!("無法開啟 {file}：{}", e.message()), true);
            }
        }
        CoUninitialize();
    });
}
