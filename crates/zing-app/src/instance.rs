//! 已經有 zing 開著時，從外部開啟的資料夾（命令列參數）轉給它開新分頁，不另外開視窗。
//!
//! - 每個 zing（隔離模式除外）開著具名的 mutex：它存在表示已有 zing 在執行（或正在啟動）。
//! - 以資料夾啟動時，以 WM_COPYDATA 轉給看得見的 zing 視窗。mutex 存在但還沒有看得見的視窗（同時開啟多個資料夾，
//!   第一個還在啟動）就等它出現，最多 3 秒。視窗拒絕（隔離模式、正在結束、權限較高的程序收不到）就自己開視窗。
//! - 轉送前 `AllowSetForegroundWindow`：使用者剛開啟的是這個程序，收到的視窗才能把自己帶到前景。

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowExW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    SW_RESTORE, SendMessageTimeoutW, SetForegroundWindow, ShowWindow, WM_COPYDATA,
};
use windows::core::{PCWSTR, w};

use crate::app::App;

/// WM_COPYDATA 的 dwData：開啟資料夾（內容是 UTF-16 路徑，空的是「此電腦」）
pub const OPEN_MAGIC: usize = 0x7a69_6e67;
/// 路徑的上限（位元組）
const MAX_BYTES: u32 = 64 * 1024;

enum Sent {
    Accepted,
    /// 有看得見的 zing 視窗，但都不接受
    Refused,
    NoWindow,
}

/// 啟動時呼叫（建立視窗之前）。`open`：命令列要開啟的資料夾。回傳 false 表示已轉給執行中的 zing，這個程序直接結束
pub fn start(open: Option<&str>) -> bool {
    // handle 不關閉：程序結束時才釋放，mutex 在最後一個 zing 結束後消失
    let existed = match unsafe { CreateMutexW(None, false, w!("Local\\zing-explore.instance")) } {
        Ok(_) => (unsafe { GetLastError() }) == ERROR_ALREADY_EXISTS,
        Err(_) => false,
    };
    let Some(path) = open.filter(|_| existed) else { return true };
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match send(path) {
            Sent::Accepted => return false,
            Sent::Refused => return true,
            Sent::NoWindow if Instant::now() >= deadline => return true,
            Sent::NoWindow => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn send(path: &str) -> Sent {
    let data: Vec<u16> = path.encode_utf16().collect();
    let cds = COPYDATASTRUCT { dwData: OPEN_MAGIC, cbData: (data.len() * 2) as u32, lpData: data.as_ptr() as *mut _ };
    let mut found = false;
    let mut after: Option<HWND> = None;
    while let Ok(h) = unsafe { FindWindowExW(None, after, w!("zing.main"), PCWSTR::null()) } {
        after = Some(h);
        if !unsafe { IsWindowVisible(h) }.as_bool() {
            continue;
        }
        found = true;
        let mut pid = 0;
        let mut result = 0usize;
        let sent = unsafe {
            GetWindowThreadProcessId(h, Some(&mut pid));
            let _ = AllowSetForegroundWindow(pid);
            SendMessageTimeoutW(h, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG | SMTO_BLOCK, 3000, Some(&mut result))
        };
        if sent.0 != 0 && result == 1 {
            return Sent::Accepted;
        }
    }
    if found { Sent::Refused } else { Sent::NoWindow }
}

/// 收到的 WM_COPYDATA（不是開啟資料夾的回傳 None，交給 DefWindowProc）；接受時回傳 true
pub fn on_copy_data(hwnd: HWND, lparam: LPARAM) -> Option<bool> {
    let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
    if cds.dwData != OPEN_MAGIC {
        return None;
    }
    if cds.cbData > MAX_BYTES || cds.cbData % 2 != 0 {
        return Some(false);
    }
    let path = if cds.cbData == 0 || cds.lpData.is_null() {
        String::new()
    } else {
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2) })
    };
    let accepted = crate::window::with_app(|a| a.open_from_outside(path)).unwrap_or(false);
    if accepted {
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
    }
    Some(accepted)
}

impl App {
    /// 另一個程序轉來的資料夾；隔離模式（效能、冒煙測試）與正在結束時不接受
    pub fn open_from_outside(&mut self, path: String) -> bool {
        if self.isolated || self.pending_exit {
            return false;
        }
        self.open_folder(path);
        true
    }

    /// 開啟資料夾：已經在某個分頁中就切換過去，否則在作用中的窗格加開分頁
    pub fn open_folder(&mut self, path: String) {
        let path = zing_core::path::normalize(&path);
        let found = zing_core::startup::find(self.panes.iter().map(|p| p.tabs.iter().map(|t| t.path.as_str())), &path);
        match found {
            Some((p, t)) => {
                self.active_pane = p;
                self.panes[p].active = t;
                self.sync_tree();
            }
            None => self.open_in_new_tab(path),
        }
        self.focus = crate::app::Focus::List;
        self.invalidate();
    }
}
