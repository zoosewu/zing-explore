//! 預覽窗格（Alt+P）：顯示焦點所在項目的內容與 Windows 屬性（與檔案總管詳細資料窗格相同的欄位）。
//!
//! - 焦點停留 150 ms 才讀取（快速按方向鍵經過的項目不讀），在專用的 STA 執行緒進行，只處理最新的請求；
//!   結果以 generation 丟棄過時的。
//! - 文字檔：前 256 KB，自動判斷編碼（`zing_core::textpreview`），以等寬字顯示、可捲動。
//! - 資料夾：縮圖、項目數（大小沿用資料夾大小的結果）。
//! - 其他（圖片、影片、PDF…）：有註冊預覽處理常式的類型交給 `prevhost`（與檔案總管相同的顯示），
//!   沒有或失敗、逾時時顯示 Windows 提供的大縮圖。

use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Globalization::{CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyDescription, IPropertyDescriptionList, PDFF_DEFAULT};
use windows::Win32::UI::Shell::{IShellItem2, SHCreateItemFromParsingName};
use windows::core::PCWSTR;
use zing_core::path;
use zing_core::text::wide_z;
use zing_core::textpreview;

use crate::app::App;
use crate::icons::IconPixels;
use crate::services::Msg;

/// 焦點停留多久才讀取
pub const DELAY_MS: u32 = 150;
/// 文字預覽讀取的上限
const TEXT_BYTES: usize = 256 << 10;
/// 檔案總管詳細資料窗格的欄位清單（System.PropList.PreviewDetails）
const PREVIEW_DETAILS: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0xc9944a21_a406_48fe_8225_aec7e24c211b),
    pid: 8,
};

pub enum Body {
    /// 圖片、影片、PDF… 的縮圖
    Image(Option<IconPixels>),
    Text { lines: Vec<String>, truncated: bool },
    Folder { image: Option<IconPixels>, items: Option<usize> },
}

pub struct Loaded {
    pub path: String,
    pub body: Body,
    /// 這個類型註冊的預覽處理常式
    pub handler: Option<windows::core::GUID>,
    /// (欄位名稱, 顯示文字)
    pub props: Vec<(String, String)>,
}

struct Job {
    generation: u64,
    path: String,
    is_dir: bool,
    px: u32,
    show_hidden: bool,
}

pub struct Preview {
    pub visible: bool,
    pub width: f32,
    /// 想要預覽的項目（焦點所在）
    pub target: String,
    pub generation: u64,
    pub loaded: Option<Loaded>,
    /// 文字的捲動位置（DIP）
    pub scroll: f32,
    /// 讀取中（target 已送出、結果還沒回來）
    pub loading: bool,
    /// 圖片在 D2D 快取中的版本（換內容時遞增）
    pub image_id: u32,
    tx: Option<Sender<Job>>,
    pub host: crate::prevhost::Host,
    pub host_state: HostState,
    /// 上次送給主機的位置（螢幕座標，px）
    host_rect: Option<windows::Win32::Foundation::RECT>,
}

/// 預覽處理常式的狀態
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostState {
    /// 沒有使用（顯示自己畫的內容）
    Off,
    /// 已送出，等待完成（看門狗計時中）
    Waiting,
    Shown,
    /// 失敗或逾時：改顯示縮圖
    Failed,
    /// 有面板疊在上面或視窗最小化，暫時隱藏
    Suspended,
}

impl Preview {
    pub fn new(visible: bool, width: f32) -> Self {
        Preview {
            visible,
            width,
            target: String::new(),
            generation: 0,
            loaded: None,
            scroll: 0.0,
            loading: false,
            image_id: 0,
            tx: None,
            host: Default::default(),
            host_state: HostState::Off,
            host_rect: None,
        }
    }
}

fn worker(rx: Receiver<Job>, n: crate::services::Notifier) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    while let Ok(mut job) = rx.recv() {
        // 只處理最新的請求
        while let Ok(newer) = rx.try_recv() {
            job = newer;
        }
        let loaded = load(&job);
        n.send(Msg::Preview { generation: job.generation, loaded: Box::new(loaded) });
    }
}

fn load(job: &Job) -> Loaded {
    zing_core::guard::io("讀取預覽");
    let props = properties(&job.path);
    let mut handler = None;
    let body = if job.is_dir {
        let never = std::sync::atomic::AtomicBool::new(false);
        let items = zing_core::enumerate::enumerate(&job.path, false, &never, |_| {}).ok().map(|l| {
            (0..l.len()).filter(|&i| job.show_hidden || !l.entries[i].is_hidden()).count()
        });
        Body::Folder { image: crate::thumbs::shell_image(&job.path, job.px), items }
    } else {
        #[cfg(feature = "smoke")]
        let force = crate::prevhost::TEST_FORCE_TEXT.load(std::sync::atomic::Ordering::Relaxed);
        #[cfg(not(feature = "smoke"))]
        let force = false;
        match read_head(&job.path) {
            Some(head) if !force && textpreview::looks_like_text(ext(&job.path), &head[..head.len().min(4096)]) => {
                let text = textpreview::decode(&head, ansi);
                let (lines, cut) = textpreview::lines(&text);
                Body::Text { lines, truncated: cut || head.len() == TEXT_BYTES }
            }
            _ => {
                handler = crate::prevhost::handler_for(ext(&job.path));
                Body::Image(crate::thumbs::shell_image(&job.path, job.px))
            }
        }
    };
    Loaded { path: job.path.clone(), body, props, handler }
}

fn ext(p: &str) -> &str {
    let name = path::file_name(p);
    name.rfind('.').map_or("", |i| &name[i + 1..])
}

fn read_head(p: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path::long_form(p)).ok()?;
    let mut buf = Vec::with_capacity(TEXT_BYTES);
    f.take(TEXT_BYTES as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// 系統字碼頁（繁體中文 Windows 為 Big5）
fn ansi(b: &[u8]) -> String {
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    let n = unsafe { MultiByteToWideChar(CP_ACP, flags, b, None) };
    if n <= 0 {
        return String::from_utf8_lossy(b).into_owned();
    }
    let mut w = vec![0u16; n as usize];
    unsafe { MultiByteToWideChar(CP_ACP, flags, b, Some(&mut w)) };
    String::from_utf16_lossy(&w)
}

/// 檔案總管詳細資料窗格的欄位（依檔案類型不同）與格式化後的值
fn properties(p: &str) -> Vec<(String, String)> {
    let w = wide_z(&path::long_form(p));
    let mut out = Vec::new();
    unsafe {
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem2>(PCWSTR(w.as_ptr()), None) else { return out };
        let Ok(list) = item.GetPropertyDescriptionList::<IPropertyDescriptionList>(&PREVIEW_DETAILS) else { return out };
        for i in 0..list.GetCount().unwrap_or(0) {
            let Ok(desc) = list.GetAt::<IPropertyDescription>(i) else { continue };
            let mut key = PROPERTYKEY::default();
            if desc.GetPropertyKey(&mut key).is_err() {
                continue;
            }
            let Ok(value) = item.GetProperty(&key) else { continue };
            let (Ok(label), Ok(text)) = (desc.GetDisplayName(), desc.FormatForDisplay(&value, PDFF_DEFAULT)) else {
                continue;
            };
            let (l, t) = (clean(label.to_string().unwrap_or_default()), clean(text.to_string().unwrap_or_default()));
            CoTaskMemFree(Some(label.0 as _));
            CoTaskMemFree(Some(text.0 as _));
            if !l.is_empty() && !t.is_empty() {
                out.push((l, t));
            }
        }
    }
    out
}

/// 去掉 Shell 在日期等文字中插入的方向控制字元
fn clean(s: String) -> String {
    s.chars().filter(|c| !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}')).collect::<String>().trim().to_string()
}

impl App {
    /// 繪製前：焦點換了就記下新的目標並（重新）開始計時
    pub fn update_preview_target(&mut self) {
        if !self.preview.visible {
            return;
        }
        let tab = self.tab();
        let target = tab.focused_entry().filter(|_| !tab.view.is_empty()).map(|e| {
            if tab.path.is_empty() { tab.listing.name_string(e) } else { tab.full_path(e) }
        });
        let target = target.unwrap_or_default();
        if target == self.preview.target {
            return;
        }
        self.preview.target = target;
        self.preview.scroll = 0.0;
        // 換了項目：先收起處理常式的視窗
        self.stop_preview_host(HostState::Off);
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), crate::app::TIMER_PREVIEW, DELAY_MS, None);
        }
    }

    /// 計時到了：送出讀取
    pub fn on_preview_timer(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), crate::app::TIMER_PREVIEW);
        }
        let path = self.preview.target.clone();
        self.preview.generation += 1;
        if path.is_empty() {
            self.preview.loaded = None;
            self.preview.loading = false;
            self.invalidate();
            return;
        }
        let is_dir = {
            let tab = self.tab();
            tab.focused_entry().is_some_and(|e| tab.listing.entries[e].is_dir())
        };
        self.preview.loading = true;
        let px = (self.preview.width * self.gfx.scale()).clamp(64.0, 1600.0) as u32;
        let job = Job { generation: self.preview.generation, path, is_dir, px, show_hidden: self.show_hidden };
        if self.preview.tx.is_none() {
            let (tx, rx) = channel();
            let n = self.notifier.clone();
            let _ = std::thread::Builder::new().name("zing-preview".into()).spawn(move || worker(rx, n));
            self.preview.tx = Some(tx);
        }
        let _ = self.preview.tx.as_ref().unwrap().send(job);
        self.invalidate();
    }

    pub fn on_preview_loaded(&mut self, generation: u64, loaded: Loaded) {
        if generation != self.preview.generation {
            return;
        }
        self.preview.loading = false;
        self.preview.image_id += 1;
        let handler = loaded.handler;
        self.preview.loaded = Some(loaded);
        self.preview.host_rect = None;
        if handler.is_some() {
            self.start_preview_host();
        } else {
            self.stop_preview_host(HostState::Off);
        }
        self.invalidate();
    }

    /// 預覽窗格內容區的螢幕座標（px）
    fn preview_screen_rect(&self) -> Option<windows::Win32::Foundation::RECT> {
        let pr = self.layout.preview?;
        let (_, content, _) = self.preview_rects(pr);
        if content.w() < 8.0 || content.h() < 8.0 {
            return None;
        }
        let s = self.gfx.scale();
        let mut tl = windows::Win32::Foundation::POINT { x: (content.l * s).round() as i32, y: (content.t * s).round() as i32 };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut tl);
        }
        Some(windows::Win32::Foundation::RECT {
            left: tl.x,
            top: tl.y,
            right: tl.x + (content.w() * s).round() as i32,
            bottom: tl.y + (content.h() * s).round() as i32,
        })
    }

    fn start_preview_host(&mut self) {
        let (Some(ld), Some(rect)) = (self.preview.loaded.as_ref(), self.preview_screen_rect()) else { return };
        let Some(clsid) = ld.handler else { return };
        let c = &self.theme.color;
        let bgr = |c: zing_core::theme::Color| {
            windows::Win32::Foundation::COLORREF(c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16)
        };
        let (bg, fg, path) = (bgr(c.surface), bgr(c.text), ld.path.clone());
        let generation = self.preview.generation;
        let req = crate::prevhost::ShowReq { generation, path, clsid, rect, bg, fg };
        self.preview.host.show(&self.notifier, req);
        self.preview.host_rect = Some(rect);
        self.preview.host_state = HostState::Waiting;
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(
                Some(self.hwnd),
                crate::app::TIMER_PREVIEW_WATCHDOG,
                crate::prevhost::TIMEOUT_MS,
                None,
            );
        }
    }

    fn stop_preview_host(&mut self, state: HostState) {
        if matches!(self.preview.host_state, HostState::Waiting | HostState::Shown) {
            self.preview.host.hide();
        }
        self.preview.host_state = state;
        self.preview.host_rect = None;
    }

    /// 主機回報完成
    pub fn on_preview_host(&mut self, generation: u64, ok: bool) {
        if generation != self.preview.generation || self.preview.host_state != HostState::Waiting {
            return;
        }
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), crate::app::TIMER_PREVIEW_WATCHDOG);
        }
        self.preview.host_state = if ok { HostState::Shown } else { HostState::Failed };
        self.invalidate();
    }

    /// 看門狗：時間到還沒完成就放棄這條主機執行緒，改顯示縮圖
    pub fn on_preview_watchdog(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), crate::app::TIMER_PREVIEW_WATCHDOG);
        }
        if self.preview.host_state == HostState::Waiting {
            self.preview.host.abandon();
            self.preview.host_state = HostState::Failed;
            self.set_status("預覽處理常式沒有回應，改顯示縮圖", false);
            self.invalidate();
        }
    }

    /// 繪製後：主機視窗跟著預覽窗格移動；有面板疊在上面、視窗最小化時暫時隱藏
    pub fn sync_preview_host(&mut self) {
        let has_handler = self.preview.loaded.as_ref().is_some_and(|l| l.handler.is_some() && l.path == self.preview.target);
        let minimized = unsafe { windows::Win32::UI::WindowsAndMessaging::IsIconic(self.hwnd) }.as_bool();
        let covered = self.palette.is_some() || self.batch.is_some();
        let want = self.preview.visible && has_handler && !covered && !minimized;
        match self.preview.host_state {
            HostState::Waiting | HostState::Shown if !want => self.stop_preview_host(HostState::Suspended),
            HostState::Suspended if want => self.start_preview_host(),
            HostState::Waiting | HostState::Shown => {
                if let Some(r) = self.preview_screen_rect()
                    && self.preview.host_rect != Some(r)
                {
                    self.preview.host.move_to(r);
                    self.preview.host_rect = Some(r);
                }
            }
            _ => {}
        }
    }

    pub fn toggle_preview(&mut self) {
        self.preview.visible = !self.preview.visible;
        if !self.preview.visible {
            self.stop_preview_host(HostState::Off);
            self.preview.target.clear();
            self.preview.loaded = None;
            self.preview.loading = false;
            self.gfx.drop_preview();
        }
        self.invalidate();
    }

    /// 文字預覽的捲動範圍
    pub fn preview_max_scroll(&self, text_h: f32) -> f32 {
        match self.preview.loaded.as_ref().map(|l| &l.body) {
            Some(Body::Text { lines, .. }) => (lines.len() as f32 * self.caption_line_h() + 8.0 - text_h).max(0.0),
            _ => 0.0,
        }
    }
}
