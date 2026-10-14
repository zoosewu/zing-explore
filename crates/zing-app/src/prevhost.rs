//! Windows 預覽處理常式（IPreviewHandler）的主機：PDF、Office、影片等由檔案總管用的同一套處理常式顯示。
//!
//! 第三方處理常式可能卡住，所以：
//! - 在專用的 STA 執行緒上建立一個**沒有擁有者**的彈出視窗承載處理常式，疊在預覽窗格的內容區。
//!   跨執行緒的父子或擁有者關係都會讓系統連結兩個執行緒的輸入佇列，處理常式卡住時會連帶卡住 UI；
//!   獨立的頂層視窗不會。視窗不搶焦點（WS_EX_NOACTIVATE），主視窗啟用時再把它放回上層。
//! - UI 只以 PostThreadMessage 送出請求、以 SWP_ASYNCWINDOWPOS／ShowWindowAsync 調整視窗，從不等待。
//! - 看門狗：送出後 3 秒沒有回報完成就放棄這條執行緒（下次換新的），改顯示縮圖。
//! - 優先以 CLSCTX_LOCAL_SERVER 建立（在 prevhost.exe 中執行，處理常式當掉不影響本程式）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, STGM_READ,
    STGM_SHARE_DENY_NONE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    ASSOCF_INIT_DEFAULTTOSTAR, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW, IInitializeWithItem, IPreviewHandler,
    IPreviewHandlerVisuals, IShellItem, SHCreateItemFromParsingName, SHCreateStreamOnFileEx,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_TOP, MSG, PostThreadMessageW, RegisterClassW,
    SW_HIDE, SW_SHOWNOACTIVATE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SetWindowPos, ShowWindow, ShowWindowAsync,
    TranslateMessage, WM_APP, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{GUID, Interface, PCWSTR, w};
use zing_core::text::wide_z;

use crate::services::{Msg, Notifier};

/// 看門狗：送出後多久沒完成就放棄
pub const TIMEOUT_MS: u32 = 3000;
const WM_HOST: u32 = WM_APP + 30;

/// 冒煙測試：讓文字檔也走預覽處理常式、模擬處理常式卡住
#[cfg(feature = "smoke")]
pub static TEST_FORCE_TEXT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "smoke")]
pub static TEST_HANG_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 檔案類型註冊的預覽處理常式（{8895b1c6-…} 是 IPreviewHandler 的介面 ID）
pub fn handler_for(ext: &str) -> Option<GUID> {
    if ext.is_empty() {
        return None;
    }
    let assoc = wide_z(&format!(".{ext}"));
    let mut buf = [0u16; 64];
    let mut len = buf.len() as u32;
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_DEFAULTTOSTAR,
            ASSOCSTR_SHELLEXTENSION,
            PCWSTR(assoc.as_ptr()),
            w!("{8895b1c6-b41f-4c1c-a562-0d564250836f}"),
            Some(windows::core::PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
    };
    if hr.is_err() {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..n]);
    GUID::try_from(s.trim_matches(|c| c == '{' || c == '}')).ok()
}

/// 顯示一個檔案的預覽：位置為螢幕座標（px），顏色給支援 IPreviewHandlerVisuals 的處理常式
pub struct ShowReq {
    pub generation: u64,
    pub path: String,
    pub clsid: GUID,
    pub rect: RECT,
    pub bg: COLORREF,
    pub fg: COLORREF,
}

enum Req {
    Show(ShowReq),
    Move(RECT),
    Hide,
}

/// UI 端：目前的主機執行緒（卡住時換新的）
#[derive(Default)]
pub struct Host {
    thread: u32,
    /// 主機視窗（給 UI 以非同步方式調整位置、放回上層）
    hwnd: Arc<AtomicIsize>,
    /// 被看門狗放棄後設為 false：執行緒恢復後自行關閉視窗並結束，不再顯示
    alive: Arc<AtomicBool>,
}

impl Host {
    fn ensure(&mut self, n: &Notifier) {
        if self.thread != 0 {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let hwnd = Arc::new(AtomicIsize::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        let (h, a) = (hwnd.clone(), alive.clone());
        let n = n.clone();
        let _ = std::thread::Builder::new().name("zing-previewhost".into()).spawn(move || run(tx, h, a, n));
        self.thread = rx.recv().unwrap_or(0);
        self.hwnd = hwnd;
        self.alive = alive;
    }

    fn post(&self, r: Req) {
        if self.thread == 0 {
            return;
        }
        let p = Box::into_raw(Box::new(r));
        if unsafe { PostThreadMessageW(self.thread, WM_HOST, WPARAM(0), LPARAM(p as isize)) }.is_err() {
            drop(unsafe { Box::from_raw(p) });
        }
    }

    pub fn show(&mut self, n: &Notifier, req: ShowReq) {
        self.ensure(n);
        self.post(Req::Show(req));
    }

    pub fn move_to(&self, rect: RECT) {
        self.post(Req::Move(rect));
    }

    pub fn hide(&self) {
        self.post(Req::Hide);
        let h = self.hwnd.load(Ordering::Relaxed);
        if h != 0 {
            unsafe {
                let _ = ShowWindowAsync(HWND(h as _), SW_HIDE);
            }
        }
    }

    /// 主視窗啟用時：把主機視窗放回主視窗上方（非同步）
    pub fn raise(&self) {
        let h = self.hwnd.load(Ordering::Relaxed);
        if h != 0 {
            unsafe {
                let _ = SetWindowPos(HWND(h as _), Some(HWND_TOP), 0, 0, 0, 0, SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE);
            }
        }
    }

    /// 看門狗逾時：放棄這條執行緒（它可能永遠卡住），隱藏它的視窗，下次建立新的
    pub fn abandon(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        let h = self.hwnd.load(Ordering::Relaxed);
        if h != 0 {
            unsafe {
                let _ = ShowWindowAsync(HWND(h as _), SW_HIDE);
            }
        }
        self.thread = 0;
        self.hwnd = Arc::new(AtomicIsize::new(0));
        self.alive = Arc::new(AtomicBool::new(true));
    }

    #[cfg(feature = "smoke")]
    pub fn window(&self) -> Option<HWND> {
        let h = self.hwnd.load(Ordering::Relaxed);
        (h != 0).then_some(HWND(h as _))
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn run(ready: std::sync::mpsc::Sender<u32>, hwnd_out: Arc<AtomicIsize>, alive: Arc<AtomicBool>, n: Notifier) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(host_proc),
            hInstance: instance.into(),
            lpszClassName: w!("zing.previewhost"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("zing.previewhost"),
            w!(""),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .unwrap_or_default();
        hwnd_out.store(hwnd.0 as isize, Ordering::Relaxed);
        // 建立訊息佇列後才回報執行緒 ID（PostThreadMessage 需要佇列）
        let mut msg = MSG::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::PeekMessageW(&mut msg, None, 0, 0, windows::Win32::UI::WindowsAndMessaging::PM_NOREMOVE);
        let _ = ready.send(GetCurrentThreadId());

        let mut current: Option<IPreviewHandler> = None;
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_HOST && msg.hwnd.is_invalid() {
                let req = *Box::from_raw(msg.lParam.0 as *mut Req);
                match req {
                    Req::Show(ShowReq { generation, path, clsid, rect, bg, fg }) => {
                        if let Some(h) = current.take() {
                            let _ = h.Unload();
                        }
                        // 處理常式卡住時不能留下舊畫面：載入期間視窗隱藏
                        let _ = ShowWindow(hwnd, SW_HIDE);
                        let w = rect.right - rect.left;
                        let hgt = rect.bottom - rect.top;
                        let _ = SetWindowPos(hwnd, Some(HWND_TOP), rect.left, rect.top, w, hgt, SWP_NOACTIVATE);
                        let result = show(hwnd, &path, &clsid, w, hgt, bg, fg);
                        // 卡太久已被看門狗放棄：收拾後結束這條執行緒
                        if !alive.load(Ordering::Relaxed) {
                            if let Some(h) = result {
                                let _ = h.Unload();
                            }
                            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
                            return;
                        }
                        let ok = result.is_some();
                        if ok {
                            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        } else {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                        }
                        current = result;
                        n.send(Msg::PreviewHost { generation, ok });
                    }
                    Req::Move(rect) => {
                        let (w, hgt) = (rect.right - rect.left, rect.bottom - rect.top);
                        let _ = SetWindowPos(hwnd, Some(HWND_TOP), rect.left, rect.top, w, hgt, SWP_NOACTIVATE);
                        if let Some(h) = &current {
                            let _ = h.SetRect(&RECT { left: 0, top: 0, right: w, bottom: hgt });
                        }
                    }
                    Req::Hide => {
                        if let Some(h) = current.take() {
                            let _ = h.Unload();
                        }
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    }
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 建立處理常式、以檔案（或串流、Shell 項目）初始化並顯示
unsafe fn show(hwnd: HWND, path: &str, clsid: &GUID, w: i32, h: i32, bg: COLORREF, fg: COLORREF) -> Option<IPreviewHandler> {
    unsafe {
        #[cfg(feature = "smoke")]
        {
            let ms = TEST_HANG_MS.swap(0, Ordering::Relaxed);
            if ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(ms as u64));
            }
        }
        let handler: IPreviewHandler = CoCreateInstance(clsid, None, CLSCTX_LOCAL_SERVER)
            .or_else(|_| CoCreateInstance(clsid, None, CLSCTX_INPROC_SERVER))
            .ok()?;
        let wpath = wide_z(path);
        let mode = STGM_READ.0 | STGM_SHARE_DENY_NONE.0;
        let inited = if let Ok(i) = handler.cast::<IInitializeWithFile>() {
            i.Initialize(PCWSTR(wpath.as_ptr()), mode).is_ok()
        } else if let Ok(i) = handler.cast::<IInitializeWithStream>() {
            SHCreateStreamOnFileEx(PCWSTR(wpath.as_ptr()), mode, 0, false, None).is_ok_and(|s| i.Initialize(&s, mode).is_ok())
        } else if let Ok(i) = handler.cast::<IInitializeWithItem>() {
            SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wpath.as_ptr()), None).is_ok_and(|item| i.Initialize(&item, mode).is_ok())
        } else {
            false
        };
        if !inited {
            return None;
        }
        if let Ok(v) = handler.cast::<IPreviewHandlerVisuals>() {
            let _ = v.SetBackgroundColor(bg);
            let _ = v.SetTextColor(fg);
        }
        handler.SetWindow(hwnd, &RECT { left: 0, top: 0, right: w, bottom: h }).ok()?;
        handler.DoPreview().ok()?;
        Some(handler)
    }
}
