//! 效能量測模式（tools/perf.ps1 使用）。
//!
//! `fx --perf-out=結果.json [--perf-dir=資料夾] [--perf-scroll-frames=N] [--perf-theme] [--perf-idle-ms=N]`
//!
//! 依序量測：冷啟動到第一幀、開啟資料夾的首屏與全部載入時間、捲動每幀時間、
//! 主題切換時間、閒置記憶體，寫成 JSON 後自動關閉。

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use fx_core::theme::{Mode, Theme};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::WindowsAndMessaging::SetTimer;

use crate::app::{App, TIMER_PERF, TIMER_PERF_GPU};

#[derive(Default)]
pub struct Perf {
    out: PathBuf,
    dir: Option<String>,
    scroll_frames: u32,
    theme: bool,
    idle_ms: u32,
    results: Vec<(String, f64)>,
    started: bool,
    nav_start: Option<Instant>,
    first_screen_pending: bool,
    loaded_pending: bool,
    /// 已等待過硬體繪圖裝置（捲動測試要量穩定狀態的 GPU 繪製）
    gpu_waited: bool,
    finished: bool,
}

impl Perf {
    pub fn from_args(args: &[String]) -> Option<Self> {
        let get = |k: &str| args.iter().find_map(|a| a.strip_prefix(&format!("--{k}=")).map(String::from));
        let out = get("perf-out")?;
        ENABLED.store(true, Ordering::Relaxed);
        Some(Self {
            out: PathBuf::from(out),
            dir: get("perf-dir"),
            scroll_frames: get("perf-scroll-frames").and_then(|v| v.parse().ok()).unwrap_or(0),
            theme: args.iter().any(|a| a == "--perf-theme"),
            idle_ms: get("perf-idle-ms").and_then(|v| v.parse().ok()).unwrap_or(0),
            ..Default::default()
        })
    }

    fn record(&mut self, key: &str, v: f64) {
        self.results.push((key.to_string(), v));
    }

    /// 由 App 在使用中分頁收到列舉結果時呼叫
    pub fn on_listing(&mut self, visible: usize, done: bool) {
        if self.nav_start.is_none() {
            return;
        }
        if visible > 0 && !self.results.iter().any(|(k, _)| k == "first_screen_ms") {
            self.first_screen_pending = true;
        }
        if done {
            self.loaded_pending = true;
        }
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static MARKS: Mutex<Vec<(&'static str, f64)>> = Mutex::new(Vec::new());

/// 啟動階段的時間點（距離程序建立的毫秒數）；只在效能模式記錄
pub fn mark(name: &'static str) {
    if ENABLED.load(Ordering::Relaxed)
        && let Ok(mut m) = MARKS.lock() {
            m.push((name, ms_since_process_start()));
        }
}

fn ms_since_process_start() -> f64 {
    unsafe {
        let (mut created, mut exit, mut kernel, mut user) = Default::default();
        let _ = GetProcessTimes(GetCurrentProcess(), &mut created, &mut exit, &mut kernel, &mut user);
        let now = GetSystemTimePreciseAsFileTime();
        let v = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64;
        (v(now) - v(created)) / 10_000.0
    }
}

/// 每次繪製後呼叫，推進量測流程
pub fn after_paint(app: &mut App) {
    let Some(perf) = app.perf.as_mut() else { return };
    if perf.finished {
        return;
    }
    if !perf.started {
        perf.started = true;
        perf.record("startup_ms", ms_since_process_start());
        let warp = app.gfx.is_warp();
        let perf = app.perf.as_mut().unwrap();
        perf.record("first_frame_warp", warp as u8 as f64);
        for (name, ms) in MARKS.lock().map(|m| m.clone()).unwrap_or_default() {
            perf.record(&format!("phase_{name}_ms"), ms);
        }
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(app.hwnd),
                crate::window::WM_APP_DEFERRED_INIT,
                Default::default(),
                Default::default(),
            );
        }
        if let Some(dir) = perf.dir.clone() {
            perf.nav_start = Some(Instant::now());
            app.navigate(0, dir, false);
        } else {
            schedule_finish(app);
        }
        return;
    }
    if perf.first_screen_pending {
        perf.first_screen_pending = false;
        let ms = perf.nav_start.unwrap().elapsed().as_secs_f64() * 1000.0;
        perf.record("first_screen_ms", ms);
    }
    if perf.loaded_pending && app.gfx.is_warp() && !perf.gpu_waited {
        // 等硬體裝置就緒（WM_APP_GPU_READY）；沒有 GPU 時 3 秒後改用 WARP 量測
        perf.gpu_waited = true;
        unsafe { SetTimer(Some(app.hwnd), TIMER_PERF_GPU, 3000, None) };
        return;
    }
    if perf.loaded_pending {
        perf.loaded_pending = false;
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(app.hwnd), TIMER_PERF_GPU);
        }
        let ms = perf.nav_start.unwrap().elapsed().as_secs_f64() * 1000.0;
        perf.record("load_all_ms", ms);
        let (items, bytes) = (app.tab().listing.len(), app.tab().listing.heap_bytes());
        let perf = app.perf.as_mut().unwrap();
        perf.record("items", items as f64);
        perf.record("listing_mb", bytes as f64 / 1_048_576.0);
        let frames = perf.scroll_frames;
        let theme = perf.theme;
        if frames > 0 {
            scroll_test(app, frames);
        }
        if theme {
            theme_test(app);
        }
        schedule_finish(app);
    }
}

fn scroll_test(app: &mut App, frames: u32) {
    let step = app.row_h() * 3.0;
    let p = app.active_pane;
    let list_h = app.layout.panes.first().map_or(600.0, |pl| pl.list.h());
    let max = app.max_scroll(p, list_h);
    let mut times = Vec::with_capacity(frames as usize);
    for _ in 0..frames {
        let tab = app.tab_mut();
        tab.scroll = if tab.scroll + step > max { 0.0 } else { tab.scroll + step };
        let t = Instant::now();
        app.paint();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let pct = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
    let warp = app.gfx.is_warp();
    let perf = app.perf.as_mut().unwrap();
    perf.record("scroll_warp", warp as u8 as f64);
    perf.record("frame_p50_ms", pct(0.5));
    perf.record("frame_p99_ms", pct(0.99));
    perf.record("frame_max_ms", *times.last().unwrap());
}

fn theme_test(app: &mut App) {
    let other = if app.theme.mode == Mode::Dark { Theme::light() } else { Theme::dark() };
    let original = app.theme.clone();
    let t = Instant::now();
    app.apply_theme(other);
    app.paint();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    app.apply_theme(original);
    app.perf.as_mut().unwrap().record("theme_switch_ms", ms);
}

fn schedule_finish(app: &mut App) {
    let idle = app.perf.as_ref().map_or(0, |p| p.idle_ms);
    if idle > 0 {
        unsafe { SetTimer(Some(app.hwnd), TIMER_PERF, idle, None) };
    } else {
        finish(app);
    }
}

pub fn finish(app: &mut App) {
    let Some(perf) = app.perf.as_mut() else { return };
    if perf.finished {
        return;
    }
    perf.finished = true;
    let mut pmc = PROCESS_MEMORY_COUNTERS_EX2 { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32, ..Default::default() };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS, pmc.cb);
    }
    perf.record("private_working_set_mb", pmc.PrivateWorkingSetSize as f64 / 1_048_576.0);
    perf.record("private_bytes_mb", pmc.PrivateUsage as f64 / 1_048_576.0);
    let body: Vec<String> = perf.results.iter().map(|(k, v)| format!("  \"{k}\": {v:.3}")).collect();
    let json = format!("{{\n{}\n}}\n", body.join(",\n"));
    let _ = std::fs::write(&perf.out, json);
    crate::window::close(app.hwnd);
}
