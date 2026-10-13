//! Win32 視窗：訊息處理、就地編輯框、模態動作。
//!
//! App 存在 thread_local 的 RefCell 中。右鍵選單、拖放等有自己的訊息迴圈，
//! 期間會重入 wndproc；所以這些動作一律在釋放 App 借用後才執行（`Modal`），
//! 萬一仍遇到重入借用，訊息交給 DefWindowProc 而不是當掉。

use std::cell::RefCell;

use fx_core::path;
use fx_core::text::wide_z;
use fx_core::theme::Color;
use windows::Win32::Foundation::{COLORREF, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DeleteObject,
    EndPaint, FW_NORMAL, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT, ScreenToClient, SetBkColor,
    SetTextColor,
};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{IDropTarget, OleInitialize, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::UI::Controls::{EM_SETSEL, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::app::{App, Edit, EditKind, Modal, TIMER_PERF, TIMER_PERF_GPU, TIMER_REFRESH, TIMER_SMOKE, TIMER_STATUS};
use crate::input::Mods;
use crate::services::{TabId, WM_APP_NOTIFY};
use crate::shell::{self, DropTarget};

pub const WM_APP_EDIT_DONE: u32 = WM_APP + 2;
pub const WM_APP_DEFERRED_INIT: u32 = WM_APP + 3;

thread_local! {
    pub static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static EDIT_STYLE: RefCell<EditStyle> = RefCell::new(EditStyle::default());
    static DROP_TARGET: RefCell<Option<IDropTarget>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct EditStyle {
    font: Option<HFONT>,
    brush: Option<HBRUSH>,
    text: COLORREF,
    bg: COLORREF,
}

/// 取得 App 的可變借用；正在借用中（重入）時回傳 None
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn colorref(c: Color) -> COLORREF {
    COLORREF(c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16)
}

fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    let v = lparam.0 as u32;
    ((v & 0xFFFF) as i16 as i32, (v >> 16) as i16 as i32)
}

pub fn register_class() -> windows::core::Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("fx.main"),
            hIcon: LoadIconW(None, IDI_APPLICATION)?,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }
    }
    Ok(())
}

pub fn create_window(rect: Option<(i32, i32, i32, i32)>) -> windows::core::Result<HWND> {
    let (x, y, w, h) = rect.unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT, 1280, 800));
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("fx.main"),
            w!("fx"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(GetModuleHandleW(None)?.into()),
            None,
        )
    }
}

pub fn placement(hwnd: HWND) -> Option<crate::settings::WindowState> {
    let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    unsafe { GetWindowPlacement(hwnd, &mut wp).ok()? };
    let r = wp.rcNormalPosition;
    Some(crate::settings::WindowState {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
        maximized: wp.showCmd == SW_SHOWMAXIMIZED.0 as u32,
    })
}

pub fn close(hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn focus_main(hwnd: HWND) {
    unsafe {
        let _ = SetFocus(Some(hwnd));
    }
}

pub fn set_clipboard_text(hwnd: HWND, text: &str) -> windows::core::Result<()> {
    fx_core::guard::io("寫入剪貼簿");
    const CF_UNICODETEXT: u32 = 13;
    let w = wide_z(text);
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let result = (|| {
            EmptyClipboard()?;
            let mem = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2)?;
            let p = GlobalLock(mem) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                let _ = GlobalUnlock(mem);
            }
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0)))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

// ---------------------------------------------------------------- 就地編輯框

pub fn refresh_edit_style(app: &App) {
    let scale = app.gfx.scale();
    let c = &app.theme.color;
    EDIT_STYLE.with(|s| {
        let mut s = s.borrow_mut();
        unsafe {
            if let Some(f) = s.font.take() {
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
            if let Some(b) = s.brush.take() {
                let _ = DeleteObject(HGDIOBJ(b.0));
            }
            let family = wide_z("Segoe UI");
            s.font = Some(CreateFontW(
                -((app.theme.font.size * scale).round() as i32),
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                PCWSTR(family.as_ptr()),
            ));
            s.brush = Some(CreateSolidBrush(colorref(c.background)));
            s.text = colorref(c.text);
            s.bg = colorref(c.background);
        }
    });
    if let Some(e) = &app.edit {
        unsafe {
            let font = EDIT_STYLE.with(|s| s.borrow().font);
            if let Some(f) = font {
                SendMessageW(e.hwnd, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(1)));
            }
        }
    }
}

/// 建立編輯框。`rect` 為 DIP；`sel_end` 為預設選取的結尾（UTF-16 單位）
fn create_edit(app: &mut App, rect: crate::gfx::Rect, text: &str, sel_end: usize, kind: EditKind) {
    if let Some(old) = app.edit.take() {
        destroy_edit(old.hwnd);
    }
    let s = app.gfx.scale();
    let font_px = app.theme.font.size * s;
    let h = (font_px * 1.45).round();
    let cy = (rect.t + rect.b) / 2.0 * s;
    let text_w = wide_z(text);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            PCWSTR(text_w.as_ptr()),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x80), // ES_AUTOHSCROLL
            (rect.l * s).round() as i32,
            (cy - h / 2.0).round() as i32,
            (rect.w() * s).round() as i32,
            h as i32,
            Some(app.hwnd),
            None,
            GetModuleHandleW(None).ok().map(|m| m.into()),
            None,
        )
    };
    let Ok(hwnd) = hwnd else { return };
    unsafe {
        if let Some(f) = EDIT_STYLE.with(|st| st.borrow().font) {
            SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(0)));
        }
        SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(sel_end as isize)));
        let _ = SetWindowSubclass(hwnd, Some(edit_proc), 1, 0);
        let _ = SetFocus(Some(hwnd));
    }
    app.edit = Some(Edit { hwnd, kind });
}

pub fn begin_rename(app: &mut App, tab: TabId, full: String) {
    let p = app.active_pane;
    app.layout = app.compute_layout();
    let Some(pl) = app.layout.panes.get(p) else { return };
    let t = app.panes[p].tab();
    let rh = app.row_h();
    let y = pl.list.t + t.focus as f32 * rh - t.scroll;
    let (nl, nr) = pl.cols[0];
    let rect = crate::gfx::Rect::new(nl + 32.0, y, nr - 4.0, y + rh);
    let name = path::file_name(&full).to_string();
    let is_dir = t.focused_entry().is_some_and(|e| t.listing.entries[e].is_dir());
    // 預設只選取主檔名，不含副檔名
    let sel_end = match name.rfind('.') {
        Some(k) if k > 0 && !is_dir => name[..k].encode_utf16().count(),
        _ => name.encode_utf16().count(),
    };
    create_edit(app, rect, &name, sel_end, EditKind::Rename { tab, path: full });
}

pub fn begin_address_edit(app: &mut App) {
    app.layout = app.compute_layout();
    let r = app.layout.address.inset(8.0, 3.0);
    let text = if app.tab().path.is_empty() { "此電腦".to_string() } else { app.tab().path.clone() };
    let len = text.encode_utf16().count();
    create_edit(app, r, &text, len, EditKind::Address);
    app.invalidate();
}

pub fn edit_text(hwnd: HWND) -> String {
    let mut buf = vec![0u16; 32 * 1024];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

pub fn destroy_edit(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        let parent = GetParent(hwnd).unwrap_or_default();
        match msg {
            WM_GETDLGCODE => return LRESULT(DLGC_WANTALLKEYS as isize),
            WM_KEYDOWN if wparam.0 == 0x0D || wparam.0 == 0x1B => {
                let commit = (wparam.0 == 0x0D) as usize;
                let _ = PostMessageW(Some(parent), WM_APP_EDIT_DONE, WPARAM(commit), LPARAM(0));
                return LRESULT(0);
            }
            // Ctrl+A 全選（舊版 EDIT 控制項不支援）
            WM_KEYDOWN if wparam.0 == b'A' as usize && Mods::current().ctrl => {
                SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                return LRESULT(0);
            }
            WM_CHAR if wparam.0 == 0x0D || wparam.0 == 0x1B || wparam.0 == 0x01 => return LRESULT(0),
            WM_KILLFOCUS => {
                let _ = PostMessageW(Some(parent), WM_APP_EDIT_DONE, WPARAM(1), LPARAM(0));
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}

// ---------------------------------------------------------------- 模態動作

thread_local! {
    /// 這次訊息處理中執行過模態動作（拖放）：時間長是正常的，卡頓監看略過
    static IN_MODAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 這次訊息處理中有已知會阻塞、但受 COM 限制必須在 UI 執行緒的 Shell 呼叫（見 CLAUDE.md 已知缺口）
pub fn exempt_stall() {
    IN_MODAL.with(|m| m.set(true));
}

/// debug build：UI 訊息處理超過 100 ms 就回報（debug 程式碼比 release 慢數倍，所以門檻放寬）。
/// 視窗拖曳／縮放、系統選單有自己的訊息迴圈，略過。
#[cfg(debug_assertions)]
pub fn check_stall(msg: &MSG, elapsed: std::time::Duration) {
    let modal = IN_MODAL.with(|m| m.replace(false));
    if modal || matches!(msg.message, WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN | WM_SYSCOMMAND | WM_NCLBUTTONDBLCLK) {
        return;
    }
    if elapsed.as_millis() > 100 {
        fx_core::guard::report(format!("UI 訊息 0x{:04X} 處理了 {} ms", msg.message, elapsed.as_millis()));
    }
}

fn run_modal(hwnd: HWND, modal: Modal) {
    IN_MODAL.with(|m| m.set(true));
    match modal {
        Modal::DragOut { paths } => {
            let _ = shell::drag_out(hwnd, &paths);
            with_app(|a| {
                a.drop_hover = None;
                a.invalidate();
            });
        }
    }
}

fn screen_to_dip(hwnd: HWND, pt: POINT) -> (f32, f32) {
    let mut p = pt;
    unsafe {
        let _ = ScreenToClient(hwnd, &mut p);
    }
    let s = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    (p.x as f32 / s, p.y as f32 / s)
}

/// 視窗註冊的放置目標（冒煙測試直接呼叫它模擬拖放）
#[cfg(feature = "smoke")]
pub fn drop_target() -> Option<IDropTarget> {
    DROP_TARGET.with(|d| d.borrow().clone())
}

fn drop_resolve(pt: POINT) -> Option<String> {
    with_app(|a| {
        let (x, y) = screen_to_dip(a.hwnd, pt);
        a.drop_target_at(x, y)
    })
    .flatten()
}

fn drop_hover(pt: Option<POINT>) {
    if pt.is_none() {
        with_app(|a| {
            a.drop_hover = None;
            a.invalidate();
        });
    }
}

/// 第一幀之後才做的初始化：OLE（剪貼簿、拖放）不影響啟動時間
fn deferred_init(hwnd: HWND) {
    unsafe {
        let _ = OleInitialize(None);
        let target: IDropTarget = DropTarget::new(hwnd, drop_resolve, drop_hover).into();
        if RegisterDragDrop(hwnd, &target).is_ok() {
            DROP_TARGET.with(|d| *d.borrow_mut() = Some(target));
        }
    }
}

// ---------------------------------------------------------------- 視窗程序

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if let Some(r) = shell::handle_menu_msg(msg, wparam, lparam) {
        return r;
    }
    let result = handle(hwnd, msg, wparam, lparam);
    // 模態動作在釋放借用後執行
    if let Some(Some(modal)) = with_app(|a| a.modal.take()) {
        run_modal(hwnd, modal);
    }
    match result {
        Some(r) => r,
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn handle(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let scale = || unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    let dip = |lp: LPARAM| {
        let (x, y) = lparam_xy(lp);
        let s = scale();
        (x as f32 / s, y as f32 / s)
    };
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut ps);
            }
            with_app(|a| {
                a.paint();
                crate::perf::after_paint(a);
            });
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_SIZE => {
            let (w, h) = lparam_xy(lparam);
            with_app(|a| {
                a.gfx.resize(w as u32, h as u32);
                if let Some(e) = a.edit.take() {
                    destroy_edit(e.hwnd);
                }
                a.paint();
            });
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xFFFF) as f32;
            let r = unsafe { &*(lparam.0 as *const RECT) };
            with_app(|a| {
                a.gfx.set_dpi(dpi);
                refresh_edit_style(a);
            });
            unsafe {
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            Some(LRESULT(0))
        }
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            let s = scale();
            info.ptMinTrackSize = POINT { x: (520.0 * s) as i32, y: (320.0 * s) as i32 };
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            let (x, y) = dip(lparam);
            with_app(|a| a.on_mouse_move(x, y));
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            with_app(|a| a.on_mouse_leave());
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            let (x, y) = dip(lparam);
            focus_main(hwnd);
            with_app(|a| a.on_lbutton_down(x, y));
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            with_app(|a| a.on_lbutton_up());
            Some(LRESULT(0))
        }
        WM_LBUTTONDBLCLK => {
            let (x, y) = dip(lparam);
            with_app(|a| a.on_double_click(x, y));
            Some(LRESULT(0))
        }
        WM_RBUTTONUP => {
            let (x, y) = dip(lparam);
            let (px, py) = lparam_xy(lparam);
            let mut pt = POINT { x: px, y: py };
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
            }
            focus_main(hwnd);
            with_app(|a| a.on_rbutton_up(x, y, pt));
            Some(LRESULT(0))
        }
        WM_MBUTTONUP => {
            let (x, y) = dip(lparam);
            with_app(|a| a.on_mbutton_up(x, y));
            Some(LRESULT(0))
        }
        WM_XBUTTONUP => {
            // 滑鼠側鍵當成虛擬按鍵（VK_XBUTTON1 = 5、VK_XBUTTON2 = 6），可在設定檔綁定
            let vk = if (wparam.0 >> 16) & 0xFFFF == 1 { 0x05 } else { 0x06 };
            with_app(|a| a.on_key(vk, Mods::current()));
            Some(LRESULT(1))
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
            let (px, py) = lparam_xy(lparam);
            let (x, y) = screen_to_dip(hwnd, POINT { x: px, y: py });
            with_app(|a| a.on_wheel(x, y, delta));
            Some(LRESULT(0))
        }
        WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
            let resize = with_app(|a| {
                matches!(a.hover, crate::view::Hit::HeaderEdge(..) | crate::view::Hit::TreeSplitter)
                    || matches!(a.press, Some(crate::app::Press::ColResize { .. } | crate::app::Press::TreeSplit { .. }))
            })
            .unwrap_or(false);
            if resize {
                unsafe {
                    SetCursor(LoadCursorW(None, IDC_SIZEWE).ok());
                }
                return Some(LRESULT(1));
            }
            None
        }
        WM_CAPTURECHANGED => {
            with_app(|a| a.press = None);
            Some(LRESULT(0))
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wparam.0 as u16;
            // 單獨按下修飾鍵不處理
            if matches!(vk, 0x10..=0x12) {
                return None;
            }
            match with_app(|a| a.on_key(vk, Mods::current())) {
                Some(true) => Some(LRESULT(0)),
                _ => None,
            }
        }
        // Shift+F10 與選單鍵：由 DefWindowProc 轉成 WM_CONTEXTMENU（lParam = -1），
        // 在按鍵放開後才開選單，避免 F10 放開時把選單關掉
        WM_CONTEXTMENU if lparam.0 == -1 || lparam.0 as u32 == u32::MAX => {
            with_app(|a| a.exec("menu.context"));
            Some(LRESULT(0))
        }
        WM_CHAR => {
            with_app(|a| a.on_char(wparam.0 as u16));
            Some(LRESULT(0))
        }
        WM_SYSCHAR => {
            if with_app(|a| std::mem::take(&mut a.suppress_char)).unwrap_or(false) {
                return Some(LRESULT(0));
            }
            None
        }
        WM_APP_NOTIFY => {
            with_app(|a| a.process_notifications());
            Some(LRESULT(0))
        }
        WM_APP_EDIT_DONE => {
            with_app(|a| a.finish_edit(wparam.0 == 1));
            Some(LRESULT(0))
        }
        WM_APP_DEFERRED_INIT => {
            deferred_init(hwnd);
            Some(LRESULT(0))
        }
        crate::gfx::WM_APP_GPU_READY => {
            with_app(|a| {
                if a.gfx.adopt_hardware() {
                    a.paint();
                    crate::perf::after_paint(a);
                }
            });
            Some(LRESULT(0))
        }
        WM_TIMER => {
            match wparam.0 {
                TIMER_REFRESH => {
                    with_app(|a| a.on_refresh_timer());
                }
                TIMER_STATUS => {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_STATUS);
                    }
                    with_app(|a| a.clear_status());
                }
                TIMER_PERF_GPU => {
                    with_app(crate::perf::after_paint);
                }
                TIMER_SMOKE => crate::smoke::tick(hwnd),
                TIMER_PERF => {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_PERF);
                    }
                    with_app(crate::perf::finish);
                }
                _ => {}
            }
            Some(LRESULT(0))
        }
        WM_SETTINGCHANGE => {
            let changed = if lparam.0 != 0 {
                let s = unsafe { PCWSTR(lparam.0 as *const u16).to_string().unwrap_or_default() };
                s == "ImmersiveColorSet"
            } else {
                false
            };
            if changed {
                with_app(|a| {
                    if a.config.theme.eq_ignore_ascii_case("system") {
                        let (t, _) = crate::settings::resolve_theme("system");
                        a.apply_theme(t);
                    }
                });
            }
            None
        }
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as _);
            let (brush, text, bg) = EDIT_STYLE.with(|s| {
                let s = s.borrow();
                (s.brush, s.text, s.bg)
            });
            unsafe {
                SetTextColor(hdc, text);
                SetBkColor(hdc, bg);
            }
            brush.map(|b| LRESULT(b.0 as isize))
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            with_app(|a| a.invalidate());
            None
        }
        WM_CLOSE => {
            let _allow = fx_core::guard::allow_io("結束時儲存工作階段（之後程序即結束）");
            with_app(|a| a.save_session());
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            Some(LRESULT(0))
        }
        WM_DESTROY => {
            with_app(|a| a.shell_host.shutdown());
            unsafe {
                let _ = RevokeDragDrop(hwnd);
            }
            DROP_TARGET.with(|d| d.borrow_mut().take());
            unsafe { PostQuitMessage(0) };
            Some(LRESULT(0))
        }
        _ => None,
    }
}
