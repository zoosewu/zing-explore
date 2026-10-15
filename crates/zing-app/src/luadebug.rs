//! Lua 除錯器的傳輸層：具名管道上的 DAP（Debug Adapter Protocol，`Content-Length` 分框的 JSON）。
//!
//! - 管道 `\\.\pipe\zing-lua-debug-<使用者名稱>`，只允許目前使用者存取、拒絕遠端連線；VS Code 的擴充功能
//!   （`tools/vscode-zing-lua`）以 `DebugAdapterNamedPipeServer` 連進來。隔離模式（冒煙測試）在名稱後加上程序 ID，
//!   不會和正在使用的 zing 衝突。
//! - 讀取在這裡的執行緒，收到的訊息交給回呼（送進腳本執行緒的工作佇列）；寫入在腳本執行緒（`send`）。
//!   管道以 overlapped 模式開啟，讀寫可以同時進行。
//! - 除錯邏輯（中斷點、單步、堆疊、變數）在 `debugger.lua`。

use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};
use zing_core::text::wide_z;

/// 一個已連線的管道（overlapped）
struct Conn(HANDLE);
// HANDLE 只是代號；讀寫各用自己的 OVERLAPPED，可以跨執行緒同時使用
unsafe impl Send for Conn {}
unsafe impl Sync for Conn {}

static CONN: Mutex<Option<Arc<Conn>>> = Mutex::new(None);

/// 管道名稱；`isolated`（冒煙測試）時加上程序 ID
pub fn pipe_name(isolated: bool) -> String {
    let user: String = std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let base = format!(r"\\.\pipe\zing-lua-debug-{user}");
    if isolated { format!("{base}-{}", std::process::id()) } else { base }
}

/// 送出一個 DAP 訊息（腳本執行緒）；沒有連線時回傳 false
pub fn send(json: &str) -> bool {
    let Some(conn) = CONN.lock().ok().and_then(|c| c.clone()) else { return false };
    let mut data = format!("Content-Length: {}\r\n\r\n", json.len()).into_bytes();
    data.extend_from_slice(json.as_bytes());
    let mut done = 0;
    while done < data.len() {
        match overlapped(conn.0, |ov| unsafe { WriteFile(conn.0, Some(&data[done..]), None, Some(ov)) }) {
            Ok(n) if n > 0 => done += n as usize,
            _ => return false,
        }
    }
    true
}

/// 開始接受連線（背景執行緒）。`on_message(Some(訊息))` 為收到的訊息，`on_message(None)` 為連線中斷。
/// 管道名稱已被其他程式佔用時不做任何事
pub fn start(name: String, on_message: impl Fn(Option<String>) + Send + 'static) {
    let _ = std::thread::Builder::new().name("zing-lua-debug".into()).spawn(move || {
        let Some(sa) = security() else { return };
        let wname = wide_z(&name);
        let mut first = true;
        loop {
            let mut open = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
            if first {
                // 確保管道是自己建立的，不是其他程式先佔用的
                open |= FILE_FLAG_FIRST_PIPE_INSTANCE;
            }
            let pipe = unsafe {
                CreateNamedPipeW(
                    PCWSTR(wname.as_ptr()),
                    open,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    64 << 10,
                    64 << 10,
                    0,
                    Some(&sa),
                )
            };
            if pipe.is_invalid() {
                return;
            }
            first = false;
            // 用戶端在 CreateNamedPipe 與 ConnectNamedPipe 之間就連上時回傳 ERROR_PIPE_CONNECTED
            let connected = match overlapped(pipe, |ov| unsafe { ConnectNamedPipe(pipe, Some(ov)) }) {
                Ok(_) => true,
                Err(code) => code == ERROR_PIPE_CONNECTED.to_hresult(),
            };
            if connected {
                let conn = Arc::new(Conn(pipe));
                *CONN.lock().unwrap() = Some(conn.clone());
                read_loop(&conn, &on_message);
                *CONN.lock().unwrap() = None;
                on_message(None);
            }
            unsafe {
                let _ = DisconnectNamedPipe(pipe);
                let _ = CloseHandle(pipe);
            }
        }
    });
}

/// 讀取 `Content-Length` 分框的訊息，直到連線中斷
fn read_loop(conn: &Conn, on_message: &impl Fn(Option<String>)) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 16 << 10];
    loop {
        while let Some((body_start, len)) = frame(&buf) {
            if buf.len() < body_start + len {
                break;
            }
            let body = String::from_utf8_lossy(&buf[body_start..body_start + len]).into_owned();
            buf.drain(..body_start + len);
            on_message(Some(body));
        }
        match overlapped(conn.0, |ov| unsafe { ReadFile(conn.0, Some(&mut chunk), None, Some(ov)) }) {
            Ok(n) if n > 0 => buf.extend_from_slice(&chunk[..n as usize]),
            _ => return,
        }
    }
}

/// 標頭的結尾位置與 Content-Length
fn frame(buf: &[u8]) -> Option<(usize, usize)> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..end]).ok()?;
    let len = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse().ok())?
    })?;
    Some((end + 4, len))
}

/// 執行一個 overlapped 操作並等它完成，回傳傳輸的位元組數或錯誤碼
fn overlapped(
    h: HANDLE,
    op: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
) -> Result<u32, windows::core::HRESULT> {
    unsafe {
        let event = CreateEventW(None, true, false, None).map_err(|e| e.code())?;
        let mut ov = OVERLAPPED { hEvent: event, ..Default::default() };
        let result = match op(&mut ov) {
            Err(e) if e.code() != ERROR_IO_PENDING.to_hresult() => Err(e.code()),
            _ => {
                let mut n = 0u32;
                GetOverlappedResult(h, &ov, &mut n, true).map(|_| n).map_err(|e| e.code())
            }
        };
        let _ = CloseHandle(event);
        result
    }
}

/// 只允許目前使用者存取的安全性描述元（存活到程序結束）
fn security() -> Option<SECURITY_ATTRIBUTES> {
    let sid = current_user_sid()?;
    let sddl = wide_z(&format!("D:P(A;;GA;;;{sid})"));
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }
        .ok()?;
    Some(SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    })
}

fn current_user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let ok = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as _), len, &mut len);
        let _ = CloseHandle(token);
        ok.ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let text = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0 as _)));
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn frames_messages_over_pipe() {
        let name = format!(r"\\.\pipe\zing-lua-debug-test-{}", std::process::id());
        let (tx, rx) = std::sync::mpsc::channel();
        start(name.clone(), move |m| {
            let _ = tx.send(m);
        });
        // 等管道建立
        let mut client = None;
        for _ in 0..100 {
            if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(&name) {
                client = Some(f);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut client = client.expect("連不上管道");
        // 兩則訊息分成不完整的片段送出
        let a = r#"{"seq":1,"command":"initialize"}"#;
        let b = r#"{"seq":2,"command":"中文"}"#;
        let raw = format!("Content-Length: {}\r\n\r\n{a}Content-Length: {}\r\n\r\n{b}", a.len(), b.len());
        let (p1, p2) = raw.as_bytes().split_at(30);
        client.write_all(p1).unwrap();
        client.flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        client.write_all(p2).unwrap();
        let t = std::time::Duration::from_secs(2);
        assert_eq!(rx.recv_timeout(t).unwrap().as_deref(), Some(a));
        assert_eq!(rx.recv_timeout(t).unwrap().as_deref(), Some(b));
        // 回應
        assert!(send(r#"{"ok":true}"#));
        let mut got = vec![0u8; 64];
        let n = client.read(&mut got).unwrap();
        assert_eq!(std::str::from_utf8(&got[..n]).unwrap(), "Content-Length: 11\r\n\r\n{\"ok\":true}");
        drop(client);
        assert_eq!(rx.recv_timeout(t).unwrap(), None);
    }
}
