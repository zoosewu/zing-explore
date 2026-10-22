//! Shell 主機執行緒：右鍵選單、內容對話方塊、剪貼簿都在這裡執行，UI 執行緒只送出請求、不等待。
//!
//! 第三方 Shell 擴充功能可能卡住或當掉；放在獨立的 STA 執行緒（有自己的隱藏視窗與訊息迴圈）後，
//! 即使擴充功能卡住，主視窗仍可繼續繪製與操作。
//!
//! 「逾時」：無法安全地終止卡住的執行緒，所以改成放棄它——下一個請求發現主機忙碌超過
//! `STUCK_AFTER` 時，就標記舊主機為已放棄並建立新的；已放棄的主機之後即使恢復，也不會再跳出選單。
//!
//! 主機視窗是 message-only 視窗（不是頂層視窗）：系統啟用視窗時會同步送 WM_ACTIVATEAPP 給
//! 同一程序的所有頂層視窗，若主機卡住，UI 執行緒會跟著等。選單顯示時才臨時建立頂層的擁有者視窗，
//! 選單關閉後立刻銷毀；也不設擁有者（跨執行緒的擁有關係會合併輸入佇列）。
//!
//! 預先準備：建立右鍵選單最慢的是 `QueryContextMenu`（文字檔約 200 ms，第一次載入擴充功能可達數秒）。
//! 游標在清單停住時 UI 送出 `Prepare`，主機先建好那些項目的選單；按下右鍵時項目相同、剪貼簿沒變、
//! 沒按 Shift、沒放太久就直接顯示。選單關閉後佇列空了再為同樣的項目準備一份。
//! 同一批佇列中被後面的 `Prepare`／`ContextMenu` 取代的 `Prepare` 不執行。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{OleFlushClipboard, OleInitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE, DispatchMessageW, GetMessageW, MSG, PostMessageW, RegisterClassExW,
    SetForegroundWindow, TranslateMessage, WM_APP, WM_NULL, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::core::w;

use crate::services::{Msg, Notifier};
use crate::shell::{self, MenuResult};
use crate::shellext::SlowPolicy;

const WM_APP_JOB: u32 = WM_APP + 10;
/// 主機在「準備中／執行中」超過這個時間，下一個請求就改用新的主機
const STUCK_AFTER: Duration = Duration::from_secs(3);

pub enum ShellJob {
    /// 先建好這些項目（空的話是 `folder` 的背景）的右鍵選單，不顯示；同時載入第三方擴充功能，項目送回 UI。
    /// `slow`：緩慢的擴充功能放不放進第一層；`persist`：讀寫緩慢擴充功能的記錄（隔離模式為 false）
    Prepare { items: Vec<String>, folder: String, slow: SlowPolicy, persist: bool },
    /// 執行精簡選單中第三方擴充功能的項目（`ext` 是 `MenuExtensions` 中的第幾個）
    InvokeExt { items: Vec<String>, folder: String, ext: usize, id: u32, pt: POINT },
    ContextMenu {
        items: Vec<String>,
        folder: String,
        pt: POINT,
        extras: Vec<(u32, String)>,
        target: String,
        /// 按住 Shift：顯示延伸項目；按住 Ctrl：傳給叫用的命令
        extended: bool,
        ctrl: bool,
    },
    SetClipboard { paths: Vec<String>, cut: bool },
    /// 純文字（例如複製路徑）
    /// `status` 為空時不顯示訊息；`reply` 收到是否成功（Lua 的 clipboard()）
    SetText { text: String, status: String, reply: Option<SyncSender<bool>> },
    Paste { dest: String },
    Properties { paths: Vec<String> },
    /// 結束前把剪貼簿資料實際寫入，關閉程式後仍可貼上
    Flush(SyncSender<()>),
}

const IDLE: u8 = 0;
const BUSY: u8 = 1;
/// 選單顯示中：使用者正在操作，不算卡住
const MENU_OPEN: u8 = 2;
/// 預先準備中：第一次載入擴充功能可能要數秒，超過 `PREPARE_STUCK_AFTER` 才算卡住
const PREPARING: u8 = 3;
const PREPARE_STUCK_AFTER: Duration = Duration::from_secs(10);
/// 準備好的選單放太久就重建（擴充功能的項目可能依當下狀態而定）
const PREPARED_MAX_AGE: Duration = Duration::from_secs(60);

/// 預先建好的選單與它對應的項目
struct Prepared {
    items: Vec<String>,
    folder: String,
    slow: SlowPolicy,
    /// 建立時的剪貼簿序號：剪貼簿變了（「貼上」可不可用）就重建
    clip_seq: u32,
    at: Instant,
    menu: shell::PreparedMenu,
}

thread_local! {
    static PREPARED: RefCell<Option<Prepared>> = const { RefCell::new(None) };
    /// 選單關閉後要再準備的項目（佇列空了才做）
    static REPREPARE: RefCell<Option<(Vec<String>, String)>> = const { RefCell::new(None) };
    /// 最近一次 Prepare 的（slow, persist）：再準備時沿用
    static PREPARE_OPTS: RefCell<(SlowPolicy, bool)> = const { RefCell::new((SlowPolicy { hide: true, allow: Vec::new() }, false)) };
    /// 已載入的第三方擴充功能，等使用者在精簡選單中選擇
    static EXTS: RefCell<Option<LoadedExts>> = const { RefCell::new(None) };
}

/// 已載入的第三方擴充功能與它們對應的（項目, 資料夾）
type LoadedExts = (Vec<String>, String, Vec<crate::shellext::LoadedExt>);

/// 預先準備成功的次數（冒煙測試用）
pub static PREPARED_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn prepare(host: HWND, n: &Notifier, items: Vec<String>, folder: String, slow: SlowPolicy, persist: bool) {
    PREPARE_OPTS.with(|o| *o.borrow_mut() = (slow.clone(), persist));
    let fresh = PREPARED.with(|p| {
        p.borrow().as_ref().is_some_and(|p| p.items == items && p.folder == folder && p.slow == slow && p.at.elapsed() < PREPARED_MAX_AGE)
    });
    if fresh {
        return;
    }
    // 先丟掉舊的（釋放擴充功能的物件），再建新的
    PREPARED.with(|p| *p.borrow_mut() = None);
    EXTS.with(|e| *e.borrow_mut() = None);
    let clip_seq = unsafe { GetClipboardSequenceNumber() };
    // 第三方擴充功能（精簡選單的第一層）先做：UI 較早拿到
    let loaded = crate::shellext::load(&items, &folder, &slow, persist);
    n.send(Msg::MenuExtensions { items: items.clone(), folder: folder.clone(), menus: loaded.menus, hidden: loaded.hidden });
    EXTS.with(|e| *e.borrow_mut() = Some((items.clone(), folder.clone(), loaded.exts)));
    if let Ok(menu) = shell::prepare_menu(host, &items, &folder, false) {
        PREPARED_COUNT.fetch_add(1, Ordering::Relaxed);
        PREPARED.with(|p| *p.borrow_mut() = Some(Prepared { items, folder, slow, clip_seq, at: Instant::now(), menu }));
    }
}

/// 取出可以直接使用的預先準備的選單
fn take_prepared(items: &[String], folder: &str, extended: bool) -> Option<shell::PreparedMenu> {
    let p = PREPARED.with(|p| p.borrow_mut().take())?;
    let ok = !extended
        && p.items == items
        && p.folder == folder
        && p.clip_seq == unsafe { GetClipboardSequenceNumber() }
        && p.at.elapsed() < PREPARED_MAX_AGE;
    ok.then_some(p.menu)
}

struct Shared {
    state: AtomicU8,
    busy_since_ms: AtomicU64,
    abandoned: AtomicBool,
}

struct Host {
    tx: Sender<ShellJob>,
    hwnd: isize,
    shared: Arc<Shared>,
}

pub struct ShellHost {
    host: Option<Host>,
    notifier: Notifier,
    main_hwnd: isize,
}

fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

impl ShellHost {
    pub fn new(notifier: Notifier, main_hwnd: HWND) -> Self {
        Self { host: None, notifier, main_hwnd: main_hwnd.0 as isize }
    }

    fn stuck(h: &Host) -> bool {
        let limit = match h.shared.state.load(Ordering::Acquire) {
            BUSY => STUCK_AFTER,
            PREPARING => PREPARE_STUCK_AFTER,
            _ => return false,
        };
        now_ms().saturating_sub(h.shared.busy_since_ms.load(Ordering::Acquire)) > limit.as_millis() as u64
    }

    pub fn submit(&mut self, job: ShellJob) {
        if let Some(h) = &self.host
            && Self::stuck(h)
        {
            h.shared.abandoned.store(true, Ordering::Release);
            self.host = None;
            self.notifier.status(tr!("前一個 Shell 動作沒有回應（可能是第三方擴充功能），已改用新的執行緒"), true);
        }
        if self.host.is_none() {
            self.host = spawn(self.notifier.clone(), self.main_hwnd);
        }
        let Some(h) = &self.host else {
            self.notifier.status(tr!("無法建立 Shell 執行緒"), true);
            return;
        };
        if h.tx.send(job).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(HWND(h.hwnd as _)), WM_APP_JOB, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// 程式結束時呼叫：最多等 500 ms 讓剪貼簿資料寫入
    pub fn shutdown(&mut self) {
        let Some(h) = self.host.take() else { return };
        if Self::stuck(&h) {
            return;
        }
        let (ack_tx, ack_rx) = sync_channel(1);
        if h.tx.send(ShellJob::Flush(ack_tx)).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(HWND(h.hwnd as _)), WM_APP_JOB, WPARAM(0), LPARAM(0));
            }
            let _ = ack_rx.recv_timeout(Duration::from_millis(500));
        }
    }
}

fn spawn(notifier: Notifier, main_hwnd: isize) -> Option<Host> {
    let (tx, rx) = channel::<ShellJob>();
    let shared = Arc::new(Shared { state: AtomicU8::new(IDLE), busy_since_ms: AtomicU64::new(0), abandoned: AtomicBool::new(false) });
    let (hwnd_tx, hwnd_rx) = sync_channel::<isize>(1);
    let thread_shared = shared.clone();
    std::thread::Builder::new()
        .name("zing-shell".into())
        .spawn(move || run(rx, notifier, main_hwnd, thread_shared, hwnd_tx))
        .ok()?;
    let hwnd = hwnd_rx.recv_timeout(Duration::from_secs(2)).ok().filter(|&h| h != 0)?;
    Some(Host { tx, hwnd, shared })
}

struct Ctx {
    rx: Receiver<ShellJob>,
    notifier: Notifier,
    main_hwnd: isize,
    hwnd: isize,
    shared: Arc<Shared>,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

fn run(rx: Receiver<ShellJob>, notifier: Notifier, main_hwnd: isize, shared: Arc<Shared>, hwnd_tx: SyncSender<isize>) {
    unsafe {
        let _ = OleInitialize(None);
        let Ok(instance) = GetModuleHandleW(None) else {
            let _ = hwnd_tx.send(0);
            return;
        };
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("zing.shellhost"),
            ..Default::default()
        };
        // 重建主機時類別已註冊過，失敗可忽略
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("zing.shellhost"),
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        );
        let Ok(hwnd) = hwnd else {
            let _ = hwnd_tx.send(0);
            return;
        };
        CTX.with(|c| *c.borrow_mut() = Some(Ctx { rx, notifier, main_hwnd, hwnd: hwnd.0 as isize, shared }));
        let _ = hwnd_tx.send(hwnd.0 as isize);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 顯示選單用的隱藏頂層視窗（與主機視窗同一個類別，選單訊息會轉給 Shell）
unsafe fn create_menu_owner() -> Option<HWND> {
    unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("zing.shellhost"),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            GetModuleHandleW(None).ok().map(|m| m.into()),
            None,
        )
        .ok()
    }
}

unsafe extern "system" fn host_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if let Some(r) = shell::handle_menu_msg(msg, wparam, lparam) {
        return r;
    }
    if msg == WM_APP_JOB {
        // 逐一取出工作；選單有自己的訊息迴圈，期間不持有 CTX 的借用
        loop {
            let mut jobs: Vec<ShellJob> = Vec::new();
            CTX.with(|c| {
                if let Some(ctx) = c.borrow().as_ref() {
                    jobs.extend(ctx.rx.try_iter());
                }
            });
            if jobs.is_empty() {
                // 佇列空了：為剛關閉的選單再準備一份（同樣的項目再按右鍵也很快）
                if let Some((items, folder)) = REPREPARE.with(|r| r.borrow_mut().take()) {
                    let (slow, persist) = PREPARE_OPTS.with(|o| o.borrow().clone());
                    run_job(ShellJob::Prepare { items, folder, slow, persist });
                    continue;
                }
                break;
            }
            let menu_job = |j: &ShellJob| matches!(j, ShellJob::Prepare { .. } | ShellJob::ContextMenu { .. });
            let superseded: Vec<bool> = (0..jobs.len()).map(|k| jobs[k + 1..].iter().any(menu_job)).collect();
            for (job, later) in jobs.into_iter().zip(superseded) {
                // 後面還有 Prepare 或 ContextMenu：這個 Prepare 已經過時
                if later && matches!(job, ShellJob::Prepare { .. }) {
                    continue;
                }
                run_job(job);
            }
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// 從 CTX 取出這次工作需要的資料（不持有借用）
fn ctx_parts() -> Option<(Notifier, HWND, HWND, Arc<Shared>)> {
    CTX.with(|c| {
        c.borrow().as_ref().map(|ctx| (ctx.notifier.clone(), HWND(ctx.main_hwnd as _), HWND(ctx.hwnd as _), ctx.shared.clone()))
    })
}

fn run_job(job: ShellJob) {
    let Some((n, main, host, shared)) = ctx_parts() else { return };
    shared.busy_since_ms.store(now_ms(), Ordering::Release);
    shared.state.store(BUSY, Ordering::Release);
    match job {
        ShellJob::Prepare { items, folder, slow, persist } => {
            if !shared.abandoned.load(Ordering::Acquire) {
                shared.state.store(PREPARING, Ordering::Release);
                prepare(host, &n, items, folder, slow, persist);
            }
        }
        ShellJob::InvokeExt { items, folder, ext, id, pt } => {
            // 擴充功能可能顯示對話方塊：與選單顯示中相同，不算卡住
            shared.state.store(MENU_OPEN, Ordering::Release);
            let r = EXTS.with(|e| {
                let e = e.borrow();
                match e.as_ref() {
                    Some((i, f, loaded)) if *i == items && *f == folder => loaded.get(ext).map(|x| crate::shellext::invoke(x, id, host, &folder, pt)),
                    _ => None,
                }
            });
            match r {
                Some(Ok(())) => {}
                Some(Err(e)) => n.status(tr!("無法執行：{e}", e = e.message()), true),
                None => n.status(tr!("選單已過期，請再按一次右鍵"), true),
            }
            REPREPARE.with(|r| *r.borrow_mut() = Some((items, folder)));
        }
        ShellJob::ContextMenu { items, folder, pt, extras, target, extended, ctrl } => {
            if shared.abandoned.load(Ordering::Acquire) {
                shared.state.store(IDLE, Ordering::Release);
                return;
            }
            // 測試用（只在 debug build）：第一個選單模擬卡住的 Shell 擴充功能
            #[cfg(debug_assertions)]
            {
                static DELAYED: AtomicBool = AtomicBool::new(false);
                if let Some(ms) = std::env::var("ZING_TEST_SHELL_DELAY_MS").ok().and_then(|v| v.parse().ok())
                    && !DELAYED.swap(true, Ordering::AcqRel)
                {
                    std::thread::sleep(Duration::from_millis(ms));
                }
            }
            let owner = std::cell::Cell::new(None::<HWND>);
            let mut before_show = || {
                // 準備選單期間被放棄（太慢）：不要再跳出過時的選單
                if shared.abandoned.load(Ordering::Acquire) {
                    return None;
                }
                shared.state.store(MENU_OPEN, Ordering::Release);
                // 臨時的頂層擁有者視窗成為前景，選單才能正確接收點擊與在點擊外部時關閉
                let w = unsafe { create_menu_owner() }?;
                unsafe {
                    let _ = SetForegroundWindow(w);
                }
                owner.set(Some(w));
                Some(w)
            };
            let after_close = || unsafe {
                if let Some(w) = owner.take() {
                    let _ = PostMessageW(Some(w), WM_NULL, WPARAM(0), LPARAM(0));
                    let _ = DestroyWindow(w);
                }
                let _ = SetForegroundWindow(main);
            };
            let prepared = match take_prepared(&items, &folder, extended) {
                Some(p) => Ok(p),
                None => shell::prepare_menu(host, &items, &folder, extended),
            };
            let result = prepared.and_then(|p| shell::show_menu(host, p, !items.is_empty(), &folder, pt, &extras, extended, ctrl, &mut before_show, &after_close));
            if !shared.abandoned.load(Ordering::Acquire) {
                REPREPARE.with(|r| *r.borrow_mut() = Some((items, folder)));
            }
            match result {
                Ok(MenuResult::Rename) => n.send(Msg::MenuRename),
                Ok(MenuResult::Extra(id)) => n.send(Msg::MenuExtra { id, target }),
                Ok(_) => {}
                Err(e) => n.status(tr!("無法顯示選單：{e}", e = e.message()), true),
            }
        }
        ShellJob::SetClipboard { paths, cut } => match shell::set_clipboard(&paths, cut) {
            Ok(()) => n.status(if cut { tr!("已剪下 {n} 個項目", n = paths.len()) } else { tr!("已複製 {n} 個項目", n = paths.len()) }, false),
            Err(e) => n.status(tr!("無法存取剪貼簿：{e}", e = e.message()), true),
        },
        ShellJob::SetText { text, status, reply } => {
            let r = crate::window::set_clipboard_text(host, &text);
            match &r {
                Ok(()) if !status.is_empty() => n.status(status, false),
                Ok(()) => {}
                Err(e) if reply.is_none() => n.status(tr!("無法存取剪貼簿：{e}", e = e.message()), true),
                Err(_) => {}
            }
            if let Some(reply) = reply {
                let _ = reply.send(r.is_ok());
            }
        }
        ShellJob::Paste { dest } => match shell::read_clipboard() {
            Some((sources, cut)) => {
                if cut {
                    shell::clear_clipboard();
                }
                n.send(Msg::Paste { sources, dest, cut });
            }
            None => n.status(tr!("剪貼簿上沒有檔案"), true),
        },
        ShellJob::Properties { paths } => shell::properties(host, &paths),
        ShellJob::Flush(ack) => {
            unsafe {
                let _ = OleFlushClipboard();
            }
            let _ = ack.send(());
        }
    }
    shared.state.store(IDLE, Ordering::Release);
}

