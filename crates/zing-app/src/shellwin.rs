//! 讓其他程式找得到 zing 的視窗：登記到 Windows 的 ShellWindows（Explorer 視窗的清單）。
//!
//! VS Code 的「在檔案總管中顯示」、瀏覽器的「在資料夾中顯示」呼叫 `SHOpenFolderAndSelectItems`：
//! 找「正在顯示那個資料夾」的視窗，找不到就以預設動作開啟資料夾（設為預設的檔案總管後是 zing），再等新視窗登記，
//! 最多 20 秒；之後經 `IWebBrowserApp::Document` →（`IServiceProvider`）`SID_SFolderView` → `IShellView::SelectItem` 選取檔案。
//! 沒有登記時呼叫端每次都卡滿 20 秒（VS Code、瀏覽器依序處理，下一次要排隊），也不會選取檔案。
//!
//! - 位置改變時一律先取消登記，再以 `RegisterPending`（帶著位置）＋`Register` 重新登記：等待中的呼叫端只在登記的
//!   那一刻比對位置，只呼叫 `OnNavigate` 它看不到（實測）。
//! - 登記在專用的執行緒，UI 執行緒不等跨程序的 COM 呼叫。COM 物件是 agile 的（windows-rs 的 `implement`），
//!   其他程序的呼叫在 COM 的 RPC 執行緒上執行：狀態放在 `Mutex`，要開啟的資料夾、要選取的檔案以 `Msg` 送回 UI。
//! - `IDispatch` 以系統的型別庫（ShDocVw）實作：`Shell.Application.Windows()` 之類的腳本讀得到 `LocationURL` 等屬性。
//! - 不支援的（回傳 E_NOTIMPL）：讀取 zing 的選取（`Document` 的自動化介面、`GetItemObject`、`IFolderView`）、
//!   視窗位置與大小、`Quit`、上一頁／下一頁、Explorer 框架的選單、工具列、狀態列。

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use windows::Win32::Foundation::{E_NOINTERFACE, E_NOTIMPL, HWND, LPARAM, LRESULT, RECT, SHANDLE_PTR, VARIANT_BOOL, VARIANT_FALSE, VARIANT_TRUE, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch,
    IDispatch_Impl, IServiceProvider, IServiceProvider_Impl, IStream, ITypeInfo,
};
use windows::Win32::System::Ole::{DispGetIDsOfNames, DispInvoke, IOleWindow_Impl, LoadRegTypeLib, OLEMENUGROUPWIDTHS};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::{InitVariantFromBuffer, VARIANT};
use windows::Win32::UI::Controls::{LPFNSVADDPROPSHEETPAGE, TBBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERID_ComputerFolder, FOLDERSETTINGS, ILClone, ILCombine, ILCreateFromPathW, ILFree, ILGetSize, IShellBrowser, IShellBrowser_Impl,
    IShellView, IShellView_Impl, IShellWindows, IWebBrowser_Impl, IWebBrowserApp, IWebBrowserApp_Impl, PathCreateFromUrlW,
    SHGetKnownFolderIDList, SHGetNameFromIDList, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, SWC_BROWSER, ShellWindows,
    UrlCreateFromPathW, _SVGIO,
};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, HMENU, MSG, PostThreadMessageW, TranslateMessage, WM_APP, WM_QUIT};
use windows::core::{BOOL, BSTR, GUID, HRESULT, IUnknownImpl, Interface, PCWSTR, PWSTR, Ref, Result, implement};
use zing_core::text::wide_z;

use crate::services::{Msg, Notifier};

/// Internet Explorer／Explorer 自動化的型別庫（IWebBrowserApp 的 IDispatch 資訊）
const LIBID_SHDOCVW: GUID = GUID::from_u128(0xeab22ac0_30c1_11cf_a7eb_0000c05bae0b);
/// IShellView::SelectItem 的旗標
const SVSI_SELECT: u32 = 0x1;
const SVSI_EDIT: u32 = 0x3;
/// BrowseObject：相對於目前的資料夾、上一層
const SBSP_RELATIVE: u32 = 0x1000;
const SBSP_PARENT: u32 = 0x2000;
const WM_WAKE: u32 = WM_APP + 1;

enum Req {
    Show(String),
    Stop(Sender<()>),
}

/// UI 執行緒持有：送出目前的位置
pub struct Host {
    tx: Sender<Req>,
    thread: u32,
    /// 最後送出的位置（重繪時比對，改變才送）
    last: Option<String>,
}

impl Host {
    pub fn start(hwnd: HWND, notifier: Notifier) -> Option<Host> {
        let (tx, rx) = channel();
        let (ready_tx, ready_rx) = channel();
        let h = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("shellwin".into())
            .spawn(move || run(HWND(h as _), notifier, rx, ready_tx))
            .ok()?;
        let thread = ready_rx.recv_timeout(Duration::from_secs(2)).ok()?;
        Some(Host { tx, thread, last: None })
    }

    /// 目前的資料夾（空字串是「此電腦」）；和上次相同就不送
    pub fn show(&mut self, path: &str) {
        if self.last.as_deref() == Some(path) {
            return;
        }
        self.last = Some(path.to_string());
        self.send(Req::Show(path.to_string()));
    }

    /// 結束時取消登記（最多等 300 ms：Explorer 沒有回應也不拖住關閉）
    pub fn stop(&self) {
        let (done_tx, done_rx) = channel();
        self.send(Req::Stop(done_tx));
        let _ = done_rx.recv_timeout(Duration::from_millis(300));
    }

    fn send(&self, req: Req) {
        if self.tx.send(req).is_ok() {
            unsafe {
                let _ = PostThreadMessageW(self.thread, WM_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl crate::app::App {
    /// 設為預設的檔案總管時才登記（否則瀏覽器的「在資料夾中顯示」會被 zing 接走）；隔離模式不登記
    pub fn sync_shell_windows(&mut self) {
        let want = !self.isolated && self.default_app == Some(crate::defaultapp::Status::On);
        match (&self.shell_windows, want) {
            (None, true) => self.shell_windows = Host::start(self.hwnd, self.notifier.clone()),
            (Some(h), false) => {
                h.stop();
                self.shell_windows = None;
            }
            _ => {}
        }
    }

    /// 重繪時呼叫：目前的資料夾改變就重新登記
    pub fn report_shell_location(&mut self) {
        let path = self.tab().path.clone();
        if let Some(h) = &mut self.shell_windows {
            h.show(&path);
        }
    }

    /// 其他程式要求選取的項目（`IShellView::SelectItem`）：開啟（或切換到）它所在的資料夾，列舉完成後選取
    pub fn select_from_outside(&mut self, target: String, rename: bool) {
        let target = zing_core::path::normalize(&target);
        let Some(folder) = zing_core::path::parent(&target) else { return };
        let name = if zing_core::path::is_root(&target) { target.clone() } else { zing_core::path::file_name(&target).to_string() };
        self.open_folder(folder);
        let p = self.active_pane;
        let tab = self.tab_mut();
        if tab.loading {
            tab.pending_select = Some((name, rename));
        } else if let Some(i) = tab.find_view(&name) {
            tab.select_only(i);
            self.ensure_visible(p);
            if rename {
                self.start_rename();
            }
        }
        self.invalidate();
    }
}

// ---------------------------------------------------------------- 背景執行緒

/// COM 物件從這裡讀目前的位置（呼叫可能來自任何執行緒）
struct State {
    /// 主視窗（HWND 不是 Send）
    hwnd: isize,
    notifier: Notifier,
    path: String,
    pidl: Option<Pidl>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

thread_local! {
    static TYPE_INFO: RefCell<Option<Option<ITypeInfo>>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&State) -> R) -> Option<R> {
    STATE.lock().ok()?.as_ref().map(f)
}

fn run(hwnd: HWND, notifier: Notifier, rx: Receiver<Req>, ready: Sender<u32>) {
    if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
        return;
    }
    *STATE.lock().unwrap() = Some(State { hwnd: hwnd.0 as isize, notifier, path: String::new(), pidl: None });
    // 先建立訊息佇列，再告訴 UI 執行緒可以 PostThreadMessage
    let mut msg = MSG::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PeekMessageW(&mut msg, None, 0, 0, windows::Win32::UI::WindowsAndMessaging::PM_NOREMOVE);
    }
    let _ = ready.send(unsafe { GetCurrentThreadId() });
    let view: IShellView = View.into();
    let browser: IShellBrowser = Browser { view }.into();
    let app: IWebBrowserApp = Window { browser }.into();
    let windows: Option<IShellWindows> = unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok() };
    let mut cookie: Option<i32> = None;
    'outer: while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        if msg.hwnd.is_invalid() && msg.message == WM_WAKE {
            // 只處理最後一個位置
            let mut show = None;
            while let Ok(req) = rx.try_recv() {
                match req {
                    Req::Show(p) => show = Some(p),
                    Req::Stop(done) => {
                        if let (Some(sw), Some(c)) = (&windows, cookie.take()) {
                            unsafe {
                                let _ = sw.Revoke(c);
                            }
                        }
                        let _ = done.send(());
                        break 'outer;
                    }
                }
            }
            if let (Some(sw), Some(path)) = (&windows, show) {
                cookie = register(sw, &app, cookie, path);
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if msg.message == WM_QUIT {
            break;
        }
    }
    STATE.lock().unwrap().take();
    drop((app, windows));
    unsafe { CoUninitialize() };
}

/// 取消舊的登記，以新的位置重新登記；回傳新的 cookie
fn register(sw: &IShellWindows, app: &IWebBrowserApp, old: Option<i32>, path: String) -> Option<i32> {
    zing_core::guard::io("登記到 ShellWindows（跨程序呼叫 Explorer）");
    unsafe {
        if let Some(c) = old {
            let _ = sw.Revoke(c);
        }
    }
    let pidl = Pidl::folder(&path);
    let hwnd = HWND(with_state(|s| s.hwnd)? as _);
    if let Some(s) = STATE.lock().unwrap().as_mut() {
        s.path = path;
        s.pidl = pidl.as_ref().map(Pidl::clone_of);
    }
    let pidl = pidl?;
    unsafe {
        let var = InitVariantFromBuffer(pidl.0 as *const _, ILGetSize(Some(pidl.0))).ok()?;
        let tid = windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, None) as i32;
        // 執行緒要是視窗所屬的（UI 執行緒）：Register 以此找到這筆等待中的登記，cookie 才是同一個
        sw.RegisterPending(tid, &var, &VARIANT::default(), SWC_BROWSER).ok()?;
        let cookie = sw.Register(app, hwnd.0 as i32, SWC_BROWSER).ok()?;
        let _ = sw.OnNavigate(cookie, &var);
        Some(cookie)
    }
}

/// 擁有的 ITEMIDLIST
struct Pidl(*mut ITEMIDLIST);

// 只在 STATE 的鎖之內使用
unsafe impl Send for Pidl {}

impl Pidl {
    /// 資料夾的 ITEMIDLIST；空字串是「此電腦」
    fn folder(path: &str) -> Option<Pidl> {
        let p = if path.is_empty() {
            unsafe { SHGetKnownFolderIDList(&FOLDERID_ComputerFolder, 0, None).ok()? }
        } else {
            unsafe { ILCreateFromPathW(PCWSTR(wide_z(path).as_ptr())) }
        };
        (!p.is_null()).then_some(Pidl(p))
    }

    fn clone_of(&self) -> Pidl {
        Pidl(unsafe { ILClone(self.0) })
    }

    fn path(p: *const ITEMIDLIST) -> Option<String> {
        unsafe { take_name(SHGetNameFromIDList(p, SIGDN_FILESYSPATH).ok()?) }
    }
}

/// SHGetNameFromIDList 配置的字串：轉成 String 並釋放
unsafe fn take_name(s: PWSTR) -> Option<String> {
    let r = unsafe { s.to_string().ok() };
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(s.0 as _)) };
    r
}

impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { ILFree(Some(self.0)) };
    }
}

/// 目前資料夾之下的項目（`SelectItem`、相對的 `BrowseObject` 傳來的是相對 ITEMIDLIST）的完整路徑
fn child_path(child: *const ITEMIDLIST) -> Option<String> {
    let full = {
        let s = STATE.lock().ok()?;
        let base = s.as_ref()?.pidl.as_ref()?;
        Pidl(unsafe { ILCombine(Some(base.0), Some(child)) })
    };
    Pidl::path(full.0)
}

fn send(msg: Msg) {
    with_state(|s| s.notifier.send(msg));
}

fn current_path() -> String {
    with_state(|s| s.path.clone()).unwrap_or_default()
}

fn nope<T>() -> Result<T> {
    Err(E_NOTIMPL.into())
}

// ---------------------------------------------------------------- COM 物件

/// 視窗本身：IWebBrowserApp（SHOpenFolderAndSelectItems 要的）＋IServiceProvider；Document 也是它
#[implement(IWebBrowserApp, IServiceProvider)]
struct Window {
    browser: IShellBrowser,
}

fn type_info() -> Option<ITypeInfo> {
    TYPE_INFO.with(|t| {
        t.borrow_mut()
            .get_or_insert_with(|| unsafe { LoadRegTypeLib(&LIBID_SHDOCVW, 1, 1, 0).and_then(|lib| lib.GetTypeInfoOfGuid(&IWebBrowserApp::IID)).ok() })
            .clone()
    })
}

impl IDispatch_Impl for Window_Impl {
    fn GetTypeInfoCount(&self) -> Result<u32> {
        Ok(type_info().is_some() as u32)
    }
    fn GetTypeInfo(&self, index: u32, _: u32) -> Result<ITypeInfo> {
        match type_info() {
            Some(t) if index == 0 => Ok(t),
            _ => nope(),
        }
    }
    fn GetIDsOfNames(&self, _: *const GUID, names: *const PCWSTR, count: u32, _: u32, ids: *mut i32) -> Result<()> {
        let ti = type_info().ok_or(windows::core::Error::from(E_NOTIMPL))?;
        unsafe { DispGetIDsOfNames(&ti, names, count, ids) }
    }
    fn Invoke(&self, id: i32, _: *const GUID, _: u32, flags: DISPATCH_FLAGS, params: *const DISPPARAMS, result: *mut VARIANT, excep: *mut EXCEPINFO, arg: *mut u32) -> Result<()> {
        let ti = type_info().ok_or(windows::core::Error::from(E_NOTIMPL))?;
        let me: IWebBrowserApp = self.to_interface();
        unsafe { DispInvoke(me.as_raw(), &ti, id, flags.0, params as *mut _, result, excep, arg) }
    }
}

impl IServiceProvider_Impl for Window_Impl {
    fn QueryService(&self, _sid: *const GUID, riid: *const GUID, ppv: *mut *mut core::ffi::c_void) -> Result<()> {
        // SHOpenFolderAndSelectItems 以 SID_SFolderView 要 IShellView；其他（SID_STopLevelBrowser…）依要的介面回答
        let view: Option<IShellView> = unsafe { self.browser.QueryActiveShellView().ok() };
        let r: HRESULT = match view {
            Some(v) if unsafe { v.query(riid, ppv) }.is_ok() => HRESULT(0),
            _ => unsafe { self.browser.query(riid, ppv) },
        };
        r.ok()
    }
}

impl IWebBrowser_Impl for Window_Impl {
    fn GoBack(&self) -> Result<()> {
        nope()
    }
    fn GoForward(&self) -> Result<()> {
        nope()
    }
    fn GoHome(&self) -> Result<()> {
        nope()
    }
    fn GoSearch(&self) -> Result<()> {
        nope()
    }
    /// 換資料夾（檔案路徑或 file:// 網址）
    fn Navigate(&self, url: &BSTR, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT) -> Result<()> {
        let url = url.to_string();
        let path = if url.to_ascii_lowercase().starts_with("file:") {
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            unsafe { PathCreateFromUrlW(PCWSTR(wide_z(&url).as_ptr()), PWSTR(buf.as_mut_ptr()), &mut len, 0)? };
            String::from_utf16_lossy(&buf[..len as usize])
        } else {
            url
        };
        send(Msg::ShellOpen(path));
        Ok(())
    }
    fn Refresh(&self) -> Result<()> {
        nope()
    }
    fn Refresh2(&self, _: *const VARIANT) -> Result<()> {
        nope()
    }
    fn Stop(&self) -> Result<()> {
        nope()
    }
    fn Application(&self) -> Result<IDispatch> {
        Ok(self.to_interface::<IWebBrowserApp>().into())
    }
    fn Parent(&self) -> Result<IDispatch> {
        Ok(self.to_interface::<IWebBrowserApp>().into())
    }
    fn Container(&self) -> Result<IDispatch> {
        nope()
    }
    fn Document(&self) -> Result<IDispatch> {
        Ok(self.to_interface::<IWebBrowserApp>().into())
    }
    fn TopLevelContainer(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn Type(&self) -> Result<BSTR> {
        nope()
    }
    fn Left(&self) -> Result<i32> {
        nope()
    }
    fn SetLeft(&self, _: i32) -> Result<()> {
        nope()
    }
    fn Top(&self) -> Result<i32> {
        nope()
    }
    fn SetTop(&self, _: i32) -> Result<()> {
        nope()
    }
    fn Width(&self) -> Result<i32> {
        nope()
    }
    fn SetWidth(&self, _: i32) -> Result<()> {
        nope()
    }
    fn Height(&self) -> Result<i32> {
        nope()
    }
    fn SetHeight(&self, _: i32) -> Result<()> {
        nope()
    }
    fn LocationName(&self) -> Result<BSTR> {
        let name = (|| {
            let s = STATE.lock().ok()?;
            let p = s.as_ref()?.pidl.as_ref()?;
            unsafe { take_name(SHGetNameFromIDList(p.0, SIGDN_NORMALDISPLAY).ok()?) }
        })();
        Ok(BSTR::from(name.unwrap_or_default()))
    }
    /// 目前資料夾的 file:// 網址（「此電腦」是空字串，與 Explorer 相同）
    fn LocationURL(&self) -> Result<BSTR> {
        let path = current_path();
        if path.is_empty() {
            return Ok(BSTR::new());
        }
        let mut buf = [0u16; 2048];
        let mut len = buf.len() as u32;
        unsafe { UrlCreateFromPathW(PCWSTR(wide_z(&path).as_ptr()), PWSTR(buf.as_mut_ptr()), &mut len, 0)? };
        Ok(BSTR::from_wide(&buf[..len as usize]))
    }
    fn Busy(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
}

impl IWebBrowserApp_Impl for Window_Impl {
    fn Quit(&self) -> Result<()> {
        nope()
    }
    fn ClientToWindow(&self, _: *mut i32, _: *mut i32) -> Result<()> {
        nope()
    }
    fn PutProperty(&self, _: &BSTR, _: &VARIANT) -> Result<()> {
        nope()
    }
    fn GetProperty(&self, _: &BSTR) -> Result<VARIANT> {
        nope()
    }
    fn Name(&self) -> Result<BSTR> {
        Ok(BSTR::from("zing-explore"))
    }
    fn HWND(&self) -> Result<SHANDLE_PTR> {
        Ok(SHANDLE_PTR(with_state(|s| s.hwnd).unwrap_or(0)))
    }
    fn FullName(&self) -> Result<BSTR> {
        Ok(BSTR::from(crate::defaultapp::current_exe()))
    }
    fn Path(&self) -> Result<BSTR> {
        let exe = crate::defaultapp::current_exe();
        Ok(BSTR::from(zing_core::path::parent(&exe).unwrap_or_default()))
    }
    fn Visible(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn SetVisible(&self, _: VARIANT_BOOL) -> Result<()> {
        Ok(())
    }
    fn StatusBar(&self) -> Result<VARIANT_BOOL> {
        nope()
    }
    fn SetStatusBar(&self, _: VARIANT_BOOL) -> Result<()> {
        nope()
    }
    fn StatusText(&self) -> Result<BSTR> {
        nope()
    }
    fn SetStatusText(&self, _: &BSTR) -> Result<()> {
        nope()
    }
    fn ToolBar(&self) -> Result<i32> {
        nope()
    }
    fn SetToolBar(&self, _: i32) -> Result<()> {
        nope()
    }
    fn MenuBar(&self) -> Result<VARIANT_BOOL> {
        nope()
    }
    fn SetMenuBar(&self, _: VARIANT_BOOL) -> Result<()> {
        nope()
    }
    fn FullScreen(&self) -> Result<VARIANT_BOOL> {
        nope()
    }
    fn SetFullScreen(&self, _: VARIANT_BOOL) -> Result<()> {
        nope()
    }
}

#[implement(IShellBrowser)]
struct Browser {
    view: IShellView,
}

impl IOleWindow_Impl for Browser_Impl {
    fn GetWindow(&self) -> Result<HWND> {
        with_state(|s| HWND(s.hwnd as _)).ok_or(E_NOTIMPL.into())
    }
    fn ContextSensitiveHelp(&self, _: BOOL) -> Result<()> {
        nope()
    }
}

impl IShellBrowser_Impl for Browser_Impl {
    fn InsertMenusSB(&self, _: HMENU, _: *mut OLEMENUGROUPWIDTHS) -> Result<()> {
        nope()
    }
    fn SetMenuSB(&self, _: HMENU, _: isize, _: HWND) -> Result<()> {
        nope()
    }
    fn RemoveMenusSB(&self, _: HMENU) -> Result<()> {
        nope()
    }
    fn SetStatusTextSB(&self, _: &PCWSTR) -> Result<()> {
        nope()
    }
    fn EnableModelessSB(&self, _: BOOL) -> Result<()> {
        nope()
    }
    fn TranslateAcceleratorSB(&self, _: *const MSG, _: u16) -> Result<()> {
        nope()
    }
    /// 換資料夾：絕對、相對於目前的資料夾，或上一層
    fn BrowseObject(&self, pidl: *const ITEMIDLIST, flags: u32) -> Result<()> {
        let path = if flags & SBSP_PARENT != 0 {
            zing_core::path::parent(&current_path())
        } else if flags & SBSP_RELATIVE != 0 {
            child_path(pidl)
        } else {
            Pidl::path(pidl)
        };
        let path = path.ok_or(windows::core::Error::from(E_NOTIMPL))?;
        send(Msg::ShellOpen(path));
        Ok(())
    }
    fn GetViewStateStream(&self, _: u32) -> Result<IStream> {
        nope()
    }
    fn GetControlWindow(&self, _: u32) -> Result<HWND> {
        nope()
    }
    fn SendControlMsg(&self, _: u32, _: u32, _: WPARAM, _: LPARAM, _: *mut LRESULT) -> Result<()> {
        nope()
    }
    fn QueryActiveShellView(&self) -> Result<IShellView> {
        Ok(self.view.clone())
    }
    fn OnViewWindowActive(&self, _: Ref<IShellView>) -> Result<()> {
        Ok(())
    }
    fn SetToolbarItems(&self, _: *const TBBUTTON, _: u32, _: u32) -> Result<()> {
        nope()
    }
}

#[implement(IShellView)]
struct View;

impl IOleWindow_Impl for View_Impl {
    fn GetWindow(&self) -> Result<HWND> {
        with_state(|s| HWND(s.hwnd as _)).ok_or(E_NOTIMPL.into())
    }
    fn ContextSensitiveHelp(&self, _: BOOL) -> Result<()> {
        nope()
    }
}

impl IShellView_Impl for View_Impl {
    fn TranslateAccelerator(&self, _: *const MSG) -> HRESULT {
        E_NOTIMPL
    }
    fn EnableModeless(&self, _: BOOL) -> Result<()> {
        nope()
    }
    fn UIActivate(&self, _: u32) -> Result<()> {
        Ok(())
    }
    fn Refresh(&self) -> Result<()> {
        nope()
    }
    fn CreateViewWindow(&self, _: Ref<IShellView>, _: *const FOLDERSETTINGS, _: Ref<IShellBrowser>, _: *const RECT) -> Result<HWND> {
        nope()
    }
    fn DestroyViewWindow(&self) -> Result<()> {
        nope()
    }
    fn GetCurrentInfo(&self) -> Result<FOLDERSETTINGS> {
        nope()
    }
    fn AddPropertySheetPages(&self, _: u32, _: LPFNSVADDPROPSHEETPAGE, _: LPARAM) -> Result<()> {
        nope()
    }
    fn SaveViewState(&self) -> Result<()> {
        nope()
    }
    /// 選取目前資料夾中的項目（SVSI_EDIT：選取後重新命名）；沒有選取旗標的（取消選取等）不處理
    fn SelectItem(&self, pidl: *const ITEMIDLIST, flags: u32) -> Result<()> {
        if flags & SVSI_SELECT == 0 {
            return Ok(());
        }
        let path = child_path(pidl).ok_or(windows::core::Error::from(E_NOTIMPL))?;
        send(Msg::ShellSelect { path, rename: flags & SVSI_EDIT == SVSI_EDIT });
        Ok(())
    }
    fn GetItemObject(&self, _: &_SVGIO, _: *const GUID, _: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(E_NOINTERFACE.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// 其他程序呼叫 SHOpenFolderAndSelectItems：找到登記的視窗、收到選取，而且很快返回（沒有登記時卡 20 秒）。
    /// 視窗事先登記在那個資料夾，不會以預設動作開啟資料夾、不動登錄：
    /// cargo test -p zing-app shell_select_from_other_process -- --ignored --nocapture
    #[test]
    #[ignore]
    fn shell_select_from_other_process() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WINDOW_EX_STYLE, WS_OVERLAPPED};
        let dir = zing_core::testdir::dir().join(format!("zing-shellwin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.txt"), "x").unwrap();
        let folder = dir.to_string_lossy().into_owned();
        let hwnd = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), windows::core::w!("STATIC"), windows::core::w!("zing shellwin test"), WS_OVERLAPPED, 0, 0, 10, 10, None, None, None, None)
                .unwrap()
        };
        let (notifier, rx) = Notifier::new(hwnd);
        let mut host = Host::start(hwnd, notifier).expect("啟動 shellwin 執行緒");
        host.show(&folder);
        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["shellwin::tests::shell_select_child", "--ignored", "--exact", "--nocapture"])
            .env("ZING_SHELLWIN_FILE", dir.join("b.txt"))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // 呼叫端會送訊息給視窗：等待時照樣處理訊息（zing 的 UI 執行緒本來就在訊息迴圈中）
        while child.try_wait().unwrap().is_none() && started.elapsed() < Duration::from_secs(30) {
            let mut msg = MSG::default();
            unsafe {
                while windows::Win32::UI::WindowsAndMessaging::PeekMessageW(&mut msg, None, 0, 0, windows::Win32::UI::WindowsAndMessaging::PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let elapsed = started.elapsed();
        let _ = child.kill();
        let out = child.wait_with_output().unwrap();
        println!("{}", String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains("[child]")).collect::<Vec<_>>().join("\n"));
        let got = rx.recv_timeout(Duration::from_secs(2));
        host.stop();
        let _ = std::fs::remove_dir_all(&dir);
        match got {
            Ok(Msg::ShellSelect { path, rename }) => {
                assert!(path.eq_ignore_ascii_case(&format!("{folder}\\b.txt")), "{path}");
                assert!(!rename);
            }
            Ok(_) => panic!("收到其他訊息"),
            Err(e) => panic!("沒有收到選取：{e:?}"),
        }
        assert!(elapsed < Duration::from_secs(5), "呼叫端花了 {elapsed:?}");
    }

    #[test]
    #[ignore]
    fn shell_select_child() {
        let Ok(file) = std::env::var("ZING_SHELLWIN_FILE") else { return };
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let started = Instant::now();
            let full = ILCreateFromPathW(PCWSTR(wide_z(&file).as_ptr()));
            let parent = std::path::Path::new(&file).parent().unwrap().to_string_lossy().into_owned();
            let folder = ILCreateFromPathW(PCWSTR(wide_z(&parent).as_ptr()));
            let child = windows::Win32::UI::Shell::ILFindLastID(full) as *const ITEMIDLIST;
            let r = windows::Win32::UI::Shell::SHOpenFolderAndSelectItems(folder, Some(&[child]), 0);
            println!("  [child] SHOpenFolderAndSelectItems：{r:?}，{:?}", started.elapsed());
        }
    }
}
