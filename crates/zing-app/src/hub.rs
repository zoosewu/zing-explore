//! 多視窗：每個視窗一條 UI 執行緒（各自的 `App`，與檔案總管相同；一個視窗卡住不影響其他視窗），
//! 主執行緒是 Hub：開啟視窗的執行緒、在最後一個視窗結束時結束程序。
//!
//! 視窗送給 Hub 的訊息經 channel（送出不等待），Hub 不是 UI 執行緒、沒有視窗與訊息佇列。

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

/// 開一個視窗需要的參數
pub struct WindowInit {
    pub perf: Option<crate::perf::Perf>,
    pub smoke: Option<crate::smoke::Smoke>,
    /// 要開啟的資料夾（命令列、Ctrl+N）
    pub open: Option<String>,
}

enum HubMsg {
    /// 視窗的執行緒結束
    Closed,
    Open(Box<WindowInit>),
}

static HUB: OnceLock<Sender<HubMsg>> = OnceLock::new();

/// 主執行緒：開第一個視窗，處理 Hub 的訊息直到最後一個視窗結束
pub fn run(first: WindowInit) {
    let (tx, rx) = channel();
    let _ = HUB.set(tx);
    let mut windows = 0usize;
    spawn(first, &mut windows);
    while windows > 0 {
        match rx.recv() {
            Ok(HubMsg::Closed) => windows -= 1,
            Ok(HubMsg::Open(init)) => spawn(*init, &mut windows),
            Err(_) => break,
        }
    }
}

/// 從任何執行緒要求開新視窗
#[allow(dead_code)]
pub fn open_window(init: WindowInit) {
    send(HubMsg::Open(Box::new(init)));
}

fn send(msg: HubMsg) {
    if let Some(tx) = HUB.get() {
        let _ = tx.send(msg);
    }
}

fn spawn(init: WindowInit, windows: &mut usize) {
    let started = std::thread::Builder::new()
        .name("zing-ui".into())
        // Lua、Shell 的回呼都在 UI 執行緒：堆疊留大一點（只保留位址空間，用到才配置）
        .stack_size(8 << 20)
        .spawn(move || {
            crate::window_thread(init);
            send(HubMsg::Closed);
        });
    if started.is_ok() {
        *windows += 1;
    }
}
