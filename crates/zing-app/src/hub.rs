//! 多視窗：每個視窗一條 UI 執行緒（各自的 `App`，與檔案總管相同；一個視窗卡住不影響其他視窗），
//! 主執行緒是 Hub：
//! - 開視窗的執行緒，最後一個視窗結束時結束程序；
//! - 寫入 session.toml（只有 Hub 寫）：各視窗送來快照，Hub 合併成一個檔案（第一個視窗在最上層，其餘在 `windows`）；
//! - 轉送共用的變更（設定、主題、我的最愛…）給其他視窗（`Shared`）。
//!
//! 視窗送給 Hub 的訊息經 channel（送出不等待），Hub 不是 UI 執行緒、沒有視窗與訊息佇列，
//! 送給視窗的經各視窗的 `Notifier`。

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crate::services::{Msg, Notifier};
use crate::settings::{Session, WindowSession};

/// 開一個視窗需要的參數
#[derive(Default)]
pub struct WindowInit {
    pub perf: Option<crate::perf::Perf>,
    pub smoke: Option<crate::smoke::Smoke>,
    /// 要開啟的資料夾（命令列、Ctrl+N）
    pub open: Option<String>,
    /// 程式啟動時的第一個視窗：負責只做一次的事（檢查更新、索引服務的版本、歡迎畫面、還原其他視窗）
    pub primary: bool,
    /// 由隔離模式（冒煙測試）的視窗開出來的：同樣不讀寫使用者的檔案
    pub isolated: bool,
    /// 還原上次的視窗（工作階段的 `windows`）
    pub restore: Option<WindowSession>,
    /// 視窗的位置與大小（實體像素）；None 時用工作階段或預設
    pub rect: Option<(i32, i32, i32, i32)>,
    /// Hub 指定的編號（開啟的順序）
    pub id: u32,
    /// 從開它的視窗接手的共用狀態
    pub inherit: Inherit,
}

/// 新視窗從開它的視窗接手的狀態：復原與重做的堆疊、進行中的檔案作業（之後的變化經 `Shared` 同步）
#[derive(Default)]
pub struct Inherit {
    pub undo: Vec<crate::undo::Undo>,
    pub redo: Vec<(String, crate::shell::FileOp)>,
    pub jobs: Vec<crate::jobs::JobInfo>,
}

/// App 知道的、關於自己這個視窗的事
#[derive(Clone, Copy, Debug)]
pub struct WinInfo {
    pub id: u32,
    pub primary: bool,
    /// 效能、冒煙測試（或由它們開出的視窗）：不讀寫使用者的檔案
    pub isolated: bool,
}

/// 所有視窗共用、一個視窗改了其他視窗跟著改的狀態
#[derive(Clone, Debug)]
pub enum Shared {
    /// 設定頁改了一個設定（已寫入 config.toml）
    Pref { key: String, value: String },
    /// 切換了主題（名稱：內建或 themes 資料夾中的檔名）
    Theme(String),
    /// 我的最愛：（全部依顯示順序, 工作階段中的, 工作階段中的順序）
    Favorites { all: Vec<String>, session: Vec<String>, order: Vec<String> },
    /// 設定檔、快速鍵、命令列的檔案改了：重新讀取（主題不變）
    ConfigFiles,
    /// 冒煙測試：回答目前的狀態
    #[cfg(feature = "smoke")]
    Probe(Sender<Probe>),
    /// 其他視窗送出的檔案作業（作業佇列介面在每個視窗都顯示全部的作業）
    JobAdded(crate::jobs::JobInfo),
    /// 檔案作業改變了這些資料夾（`renamed`：改名的資料夾，分頁與樹狀列跟著換）
    FilesChanged { dirs: Vec<String>, renamed: Option<(String, String)> },
    /// 復原與重做的堆疊（每個視窗一份相同的）
    Undo(crate::undo::UndoEvent),
    /// 資料夾的檢視設定（views.rs）
    FolderView { path: String, view: zing_core::folderview::FolderView, when: u64 },
}

/// 冒煙測試問其他視窗的狀態
#[cfg(feature = "smoke")]
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub theme: String,
    pub favorites: Vec<String>,
    /// 作用中的分頁
    pub tab: String,
    /// 分頁總數
    pub tabs: usize,
    /// 另一個視窗拖曳中的分頁要插入的位置
    pub tab_insert: Option<(usize, usize)>,
    /// 第一個窗格分頁列的最右邊（螢幕座標，實體像素）
    pub strip_end: Option<(i32, i32)>,
    pub jobs: Vec<crate::jobs::JobId>,
    pub undo: Option<String>,
}

/// 暫停其他視窗的監看：Hub 先告訴作業執行緒要等幾個視窗，各視窗暫停後回報
pub enum PauseAck {
    Expect(usize),
    Paused,
}

enum HubMsg {
    /// 視窗的執行緒結束
    Closed(u32),
    Open(Box<WindowInit>),
    /// 視窗建立好了：之後送給它的訊息經這個 Notifier
    Register(u32, Notifier),
    /// 視窗目前的工作階段（關閉時、共用的部分改變時）
    Snapshot(u32, Box<Session>),
    Broadcast(u32, Shared),
    CloseAll,
    /// 視窗正在關閉：之後送給它的訊息改送給其他視窗
    Unregister(u32),
    /// 檔案作業的進度：送給每個視窗
    Job(crate::jobs::JobId, crate::jobs::JobEvent),
    /// 檔案作業完成：送給送出它的視窗，它已關閉時送給其他視窗
    ToWindow(u32, Box<Msg>),
    /// 送給這個 HWND 的視窗（拖曳分頁）；它已關閉時丟掉，放過去的分頁改開新視窗
    ToHwnd(isize, Box<Msg>),
    PauseOthers { from: u32, paths: Vec<String>, ack: Sender<PauseAck> },
    ResumeOthers { from: u32, paths: Vec<String> },
    /// 資料夾大小快取：視窗目前的全部記錄（Hub 與檔案合併後寫入）
    SaveSizes(Vec<zing_core::dirsize::CacheLine>),
    /// 資料夾的檢視設定改變；`persist` 為 false（隔離模式）時只轉送、不寫檔
    FolderView { from: u32, path: String, view: zing_core::folderview::FolderView, when: u64, persist: bool },
}

/// 還沒開始關閉的視窗數：關閉時原子地減一，所以同時關閉多個視窗（app.exit、close_all）也只有一個是「最後一個」
static OPEN: AtomicUsize = AtomicUsize::new(0);

static HUB: OnceLock<Sender<HubMsg>> = OnceLock::new();

struct Hub {
    rx: Receiver<HubMsg>,
    /// 執行中的視窗數（包含還沒 Register 的）
    running: usize,
    next_id: u32,
    notifiers: Vec<(u32, Notifier)>,
    /// 各視窗自己的工作階段（依編號，也就是開啟的順序）
    windows: Vec<(u32, WindowSession)>,
    /// 最後收到的快照（共用的部分以它為準）；隔離模式的視窗不送，所以沒有時不寫檔
    latest: Option<Session>,
    dirty: bool,
    /// 等著寫入的資料夾大小（各視窗送來的）
    sizes: Vec<zing_core::dirsize::CacheLine>,
    /// 資料夾的檢視設定（第一次有變更時從檔案讀出）；有變更時為 true
    views: Option<zing_core::folderview::FolderViews>,
    views_dirty: bool,
}

/// 主執行緒：開第一個視窗，處理 Hub 的訊息直到最後一個視窗結束
pub fn run(first: WindowInit) {
    let (tx, rx) = channel();
    let _ = HUB.set(tx);
    let mut hub = Hub {
        rx,
        running: 0,
        next_id: 0,
        notifiers: Vec::new(),
        windows: Vec::new(),
        latest: None,
        dirty: false,
        sizes: Vec::new(),
        views: None,
        views_dirty: false,
    };
    hub.spawn(first);
    while hub.running > 0 {
        let Ok(msg) = hub.rx.recv() else { break };
        hub.handle(msg);
        // 一次處理完排隊的訊息再寫檔
        while let Ok(msg) = hub.rx.try_recv() {
            hub.handle(msg);
        }
        hub.save();
    }
}

impl Hub {
    fn handle(&mut self, msg: HubMsg) {
        match msg {
            HubMsg::Closed(id) => {
                self.running -= 1;
                self.notifiers.retain(|(i, _)| *i != id);
                // 關掉的不是最後一個視窗：下次不還原它
                if self.running > 0 {
                    self.windows.retain(|(i, _)| *i != id);
                    self.dirty = true;
                }
            }
            HubMsg::Open(init) => self.spawn(*init),
            HubMsg::Register(id, n) => self.notifiers.push((id, n)),
            HubMsg::Snapshot(id, session) => {
                let part = session.window_part();
                match self.windows.iter_mut().find(|(i, _)| *i == id) {
                    Some((_, w)) => *w = part,
                    None => {
                        self.windows.push((id, part));
                        self.windows.sort_by_key(|(i, _)| *i);
                    }
                }
                self.latest = Some(*session);
                self.dirty = true;
            }
            HubMsg::Broadcast(from, shared) => {
                for (_, n) in self.notifiers.iter().filter(|(i, _)| *i != from) {
                    n.send(Msg::Shared(shared.clone()));
                }
            }
            HubMsg::CloseAll => {
                for (_, n) in &self.notifiers {
                    n.send(Msg::CloseWindow);
                }
            }
            HubMsg::Unregister(id) => self.notifiers.retain(|(i, _)| *i != id),
            HubMsg::Job(id, event) => {
                for (_, n) in &self.notifiers {
                    n.send(Msg::Job { id, event: event.clone() });
                }
            }
            HubMsg::ToWindow(id, msg) => {
                let target = self.notifiers.iter().find(|(i, _)| *i == id).or(self.notifiers.first());
                if let Some((_, n)) = target {
                    n.send(*msg);
                }
            }
            HubMsg::ToHwnd(h, msg) => match (self.notifiers.iter().find(|(_, n)| n.hwnd() == h), *msg) {
                (Some((_, n)), msg) => n.send(msg),
                (None, Msg::TabDrop(drop)) => self.spawn(WindowInit {
                    restore: Some(WindowSession {
                        panes: vec![crate::settings::PaneState { tabs: vec![drop.path], active: 0 }],
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                (None, _) => {}
            },
            HubMsg::PauseOthers { from, paths, ack } => {
                let others: Vec<&Notifier> = self.notifiers.iter().filter(|(i, _)| *i != from).map(|(_, n)| n).collect();
                let _ = ack.send(PauseAck::Expect(others.len()));
                for n in others {
                    n.send(Msg::PauseWatches { paths: paths.clone(), ack: ack.clone() });
                }
            }
            HubMsg::ResumeOthers { from, paths } => {
                for (_, n) in self.notifiers.iter().filter(|(i, _)| *i != from) {
                    n.send(Msg::ResumeWatches(paths.clone()));
                }
            }
            HubMsg::SaveSizes(lines) => self.sizes.extend(lines),
            HubMsg::FolderView { from, path, view, when, persist } => {
                for (_, n) in self.notifiers.iter().filter(|(i, _)| *i != from) {
                    n.send(Msg::Shared(Shared::FolderView { path: path.clone(), view, when }));
                }
                if persist {
                    let views = self.views.get_or_insert_with(|| {
                        crate::views::file()
                            .and_then(|f| std::fs::read_to_string(f).ok())
                            .map(|t| zing_core::folderview::FolderViews::from_text(&t))
                            .unwrap_or_default()
                    });
                    views.set(&path, view, when);
                    self.views_dirty = true;
                }
            }
        }
    }

    fn spawn(&mut self, mut init: WindowInit) {
        init.id = self.next_id;
        self.next_id += 1;
        let id = init.id;
        if let Some(r) = &init.restore
            && !init.isolated
        {
            self.windows.push((id, r.clone()));
        }
        let started = std::thread::Builder::new()
            .name("zing-ui".into())
            // Lua、Shell 的回呼都在 UI 執行緒：堆疊留大一點（只保留位址空間，用到才配置）
            .stack_size(8 << 20)
            .spawn(move || {
                crate::window_thread(init);
                send(HubMsg::Closed(id));
            });
        if started.is_ok() {
            self.running += 1;
            OPEN.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// 寫入 session.toml（合併各視窗的工作階段）與快取檔（Hub 的執行緒，不是 UI 執行緒）
    fn save(&mut self) {
        if !self.sizes.is_empty() {
            crate::sizes::write_cache(std::mem::take(&mut self.sizes));
        }
        if std::mem::take(&mut self.views_dirty)
            && let (Some(views), Some(file)) = (&self.views, crate::views::file())
        {
            write_atomic(&file, &views.to_text());
        }
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        if let Some(out) = merge(self.latest.as_ref(), &self.windows) {
            let _ = out.save();
        }
    }
}

/// 合併成一個工作階段：第一個視窗在最上層（舊版讀得懂），其餘在 `windows`；共用的部分取最後收到的快照
fn merge(latest: Option<&Session>, windows: &[(u32, WindowSession)]) -> Option<Session> {
    let (latest, (_, first)) = (latest?, windows.first()?);
    let mut out = latest.clone();
    out.set_window_part(first.clone());
    out.windows = windows.iter().skip(1).map(|(_, w)| w.clone()).collect();
    Some(out)
}

fn send(msg: HubMsg) {
    if let Some(tx) = HUB.get() {
        let _ = tx.send(msg);
    }
}

/// 開新視窗（任何執行緒）
pub fn open_window(init: WindowInit) {
    send(HubMsg::Open(Box::new(init)));
}

/// 視窗建立好了（App::new）
pub fn register(id: u32, notifier: Notifier) {
    send(HubMsg::Register(id, notifier));
}

/// 視窗目前的工作階段（Hub 合併後寫檔；隔離模式不送）
pub fn snapshot(id: u32, session: Session) {
    send(HubMsg::Snapshot(id, Box::new(session)));
}

/// 共用的變更轉送給其他視窗
pub fn broadcast(from: u32, shared: Shared) {
    send(HubMsg::Broadcast(from, shared));
}

/// 關閉所有視窗（安裝更新後重新啟動）
pub fn close_all() {
    send(HubMsg::CloseAll);
}

/// 送給這個程序的另一個視窗（tabmove.rs）
pub fn to_window(hwnd: isize, msg: Msg) {
    send(HubMsg::ToHwnd(hwnd, Box::new(msg)));
}

/// 資料夾大小快取（sizes.rs）
pub fn save_sizes(lines: Vec<zing_core::dirsize::CacheLine>) {
    send(HubMsg::SaveSizes(lines));
}

/// 資料夾的檢視設定改變（views.rs）：轉送給其他視窗，`persist` 時寫入檔案
pub fn folder_view(from: u32, path: String, view: zing_core::folderview::FolderView, when: u64, persist: bool) {
    send(HubMsg::FolderView { from, path, view, when, persist });
}

/// 先寫暫存檔再改名（寫到一半當機不會留下損壞的檔案）；只在 Hub 的執行緒呼叫，暫存檔不會被同時寫
pub fn write_atomic(file: &std::path::Path, text: &str) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = file.with_extension("tsv.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(tmp, file);
    }
}

/// 視窗正在關閉（WM_DESTROY）
pub fn unregister(id: u32) {
    send(HubMsg::Unregister(id));
}

/// 視窗開始關閉（WM_CLOSE，每個視窗只呼叫一次）：回傳是不是最後一個
pub fn begin_close() -> bool {
    OPEN.fetch_sub(1, Ordering::AcqRel) == 1
}

/// 使用者取消了關閉
pub fn cancel_close() {
    OPEN.fetch_add(1, Ordering::AcqRel);
}

/// 檔案作業執行緒送訊息的路線：進度給每個視窗，完成給送出作業的視窗（已關閉時給其他視窗）。
/// 沒有 Hub（單元測試）時直接送給 `n`
#[derive(Clone)]
pub struct Route {
    pub win: u32,
    pub n: Notifier,
}

impl Route {
    pub fn job(&self, id: crate::jobs::JobId, event: crate::jobs::JobEvent) {
        match HUB.get() {
            Some(_) => send(HubMsg::Job(id, event)),
            None => self.n.send(Msg::Job { id, event }),
        }
    }

    pub fn done(&self, msg: Msg) {
        match HUB.get() {
            Some(_) => send(HubMsg::ToWindow(self.win, Box::new(msg))),
            None => self.n.send(msg),
        }
    }

    /// 送出作業的視窗（Shell 對話框的擁有者）
    pub fn hwnd(&self) -> isize {
        self.n.hwnd()
    }

    /// 作業開始前暫停其他視窗在 `paths` 之下的監看（Windows 拒絕移走有監看的資料夾）；
    /// 在作業執行緒等各視窗回報，最多 500 ms（某個視窗卡住也不會一直等）
    pub fn pause_others(&self, paths: &[String]) {
        if paths.is_empty() || HUB.get().is_none() {
            return;
        }
        let (tx, rx) = channel();
        send(HubMsg::PauseOthers { from: self.win, paths: paths.to_vec(), ack: tx });
        // Hub 先送 Expect（之後各視窗才收到要求），所以第一個一定是它
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut waiting = None;
        while waiting != Some(0) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { return };
            match (rx.recv_timeout(left), waiting) {
                (Ok(PauseAck::Expect(n)), _) => waiting = Some(n),
                (Ok(PauseAck::Paused), Some(w)) => waiting = Some(w - 1),
                (Ok(PauseAck::Paused), None) | (Err(_), _) => return,
            }
        }
    }

    /// 作業結束：恢復其他視窗的監看
    pub fn resume_others(&self, paths: &[String]) {
        if !paths.is_empty() && HUB.get().is_some() {
            send(HubMsg::ResumeOthers { from: self.win, paths: paths.to_vec() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PaneState;

    fn win(tab: &str) -> WindowSession {
        WindowSession { panes: vec![PaneState { tabs: vec![tab.into()], active: 0 }], ..Default::default() }
    }

    fn win_session(tab: &str) -> Session {
        let mut s = Session::default();
        s.set_window_part(win(tab));
        s
    }

    #[test]
    fn merge_puts_first_window_on_top_and_rest_in_windows() {
        let latest = Session { favorites: vec![r"C:\fav".into()], ..win_session(r"Z:\") };
        let out = merge(Some(&latest), &[(0, win(r"C:\a")), (3, win(r"D:\b"))]).unwrap();
        assert_eq!(out.panes[0].tabs, vec![r"C:\a".to_string()]);
        assert_eq!(out.windows.len(), 1);
        assert_eq!(out.windows[0].panes[0].tabs, vec![r"D:\b".to_string()]);
        assert_eq!(out.favorites, vec![r"C:\fav".to_string()]);
        // 隔離模式沒有快照、沒有視窗：不寫
        assert!(merge(None, &[(0, win(r"C:\a"))]).is_none());
        assert!(merge(Some(&latest), &[]).is_none());
    }

    #[test]
    fn session_file_with_and_without_windows() {
        // 舊版的檔案（沒有 windows）照樣讀得到
        let old: Session = toml::from_str("active_pane = 0\n[[panes]]\ntabs = ['C:\\']\nactive = 0\n").unwrap();
        assert!(old.windows.is_empty());
        assert_eq!(old.panes[0].tabs, vec![r"C:\".to_string()]);
        // 只有一個視窗時不寫 windows
        let one = toml::to_string_pretty(&win_session(r"C:\")).unwrap();
        assert!(!one.contains("windows"), "{one}");
        // 第二個視窗來回
        let mut two = win_session(r"C:\");
        two.windows.push(win(r"D:\"));
        let back: Session = toml::from_str(&toml::to_string_pretty(&two).unwrap()).unwrap();
        assert_eq!(back.windows[0].panes[0].tabs, vec![r"D:\".to_string()]);
        assert!(back.windows[0].tree_visible, "預設與 Session 相同");
    }
}
