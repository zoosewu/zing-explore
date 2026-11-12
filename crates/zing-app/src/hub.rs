//! 多視窗：每個視窗一條 UI 執行緒（各自的 `App`，與檔案總管相同；一個視窗卡住不影響其他視窗），
//! 主執行緒是 Hub：
//! - 開視窗的執行緒，最後一個視窗結束時結束程序；
//! - 寫入 session.toml（只有 Hub 寫）：各視窗送來快照，Hub 合併成一個檔案（第一個視窗在最上層，其餘在 `windows`）；
//! - 轉送共用的變更（設定、主題、我的最愛…）給其他視窗（`Shared`）。
//!
//! 視窗送給 Hub 的訊息經 channel（送出不等待），Hub 不是 UI 執行緒、沒有視窗與訊息佇列，
//! 送給視窗的經各視窗的 `Notifier`。

use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};

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
    /// 冒煙測試：回答目前的（主題, 我的最愛）
    #[cfg(feature = "smoke")]
    Probe(Sender<(String, Vec<String>)>),
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
}

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
}

/// 主執行緒：開第一個視窗，處理 Hub 的訊息直到最後一個視窗結束
pub fn run(first: WindowInit) {
    let (tx, rx) = channel();
    let _ = HUB.set(tx);
    let mut hub = Hub { rx, running: 0, next_id: 0, notifiers: Vec::new(), windows: Vec::new(), latest: None, dirty: false };
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
        }
    }

    /// 合併各視窗的工作階段寫入 session.toml（Hub 的執行緒，不是 UI 執行緒）
    fn save(&mut self) {
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
