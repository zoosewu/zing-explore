#![windows_subsystem = "windows"]
//! fx：Windows 11 檔案總管。

mod app;
mod gfx;
mod icons;
mod input;
mod model;
mod perf;
mod services;
mod settings;
mod shell;
mod shellhost;
#[cfg(feature = "smoke")]
mod smoke;
/// 一般建置不含冒煙測試（tools/smoke.ps1 以 `--features smoke` 建置），不增加執行檔大小
#[cfg(not(feature = "smoke"))]
mod smoke {
    use windows::Win32::Foundation::HWND;

    pub struct Smoke;

    impl Smoke {
        /// 收到冒煙測試參數卻沒有這個功能：直接結束，不要用使用者的工作階段開出一般視窗
        pub fn from_args(args: &[String]) -> Option<Self> {
            if args.iter().any(|a| a.starts_with("--smoke-")) {
                std::process::exit(2);
            }
            None
        }

        pub fn dir(&self) -> &str {
            ""
        }
    }

    pub fn start(_hwnd: HWND, _smoke: Smoke) {}

    pub fn tick(_hwnd: HWND) {}
}
mod view;
mod window;

use windows::Win32::Graphics::Gdi::UpdateWindow;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostMessageW, SW_SHOWMAXIMIZED, SW_SHOWNOACTIVATE, SW_SHOWNORMAL, ShowWindow, TranslateMessage,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let perf = perf::Perf::from_args(&args);
    let smoke = smoke::Smoke::from_args(&args);
    perf::mark("main");
    fx_core::guard::mark_ui_thread();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if let Err(e) = run(perf, smoke) {
        fatal(&format!("fx 無法啟動：{}", e.message()));
    }
}

fn run(perf: Option<perf::Perf>, smoke: Option<smoke::Smoke>) -> windows::core::Result<()> {
    let is_perf = perf.is_some();
    let isolated = is_perf || smoke.is_some();
    window::register_class()?;
    let saved = if isolated {
        None
    } else {
        let _allow = fx_core::guard::allow_io("啟動時讀取工作階段（第一幀之前必須知道視窗位置）");
        settings::Session::load().window
    };
    // 效能與冒煙測試固定視窗位置與大小，結果才可重現
    let rect = if isolated {
        Some((100, 100, 1280, 800))
    } else {
        saved.as_ref().filter(|w| w.w > 200 && w.h > 150).map(|w| (w.x, w.y, w.w, w.h))
    };
    let hwnd = window::create_window(rect)?;
    perf::mark("window");
    let dpi = unsafe { GetDpiForWindow(hwnd) } as f32;

    // 先建立 App 並開始背景列舉，再顯示視窗：資料可能在第一幀前就到了
    let app = app::App::new(hwnd, dpi, perf, smoke.as_ref().map(|s| s.dir().to_string()))?;
    app::apply_window_theme(hwnd, &app.theme);
    window::refresh_edit_style(&app);
    window::APP.with(|a| *a.borrow_mut() = Some(app));
    perf::mark("app");

    let maximized = saved.is_some_and(|w| w.maximized);
    unsafe {
        // 冒煙測試不搶前景：使用者可以繼續操作其他視窗
        let show = if smoke.is_some() {
            SW_SHOWNOACTIVATE
        } else if maximized {
            SW_SHOWMAXIMIZED
        } else {
            SW_SHOWNORMAL
        };
        let _ = ShowWindow(hwnd, show);
        let _ = UpdateWindow(hwnd);
        perf::mark("update_window");
        if !is_perf {
            let _ = PostMessageW(Some(hwnd), window::WM_APP_DEFERRED_INIT, Default::default(), Default::default());
        }
        if let Some(s) = smoke {
            smoke::start(hwnd, s);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            #[cfg(debug_assertions)]
            let started = std::time::Instant::now();
            DispatchMessageW(&msg);
            #[cfg(debug_assertions)]
            window::check_stall(&msg, started.elapsed());
        }
    }
    window::APP.with(|a| a.borrow_mut().take());
    Ok(())
}

fn fatal(text: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MessageBoxW};
    let t = fx_core::text::wide_z(text);
    unsafe {
        MessageBoxW(None, windows::core::PCWSTR(t.as_ptr()), windows::core::w!("fx"), MB_ICONERROR);
    }
}
