//! Direct2D / DirectWrite 繪圖封裝。座標一律為 DIP（96 DPI 下的像素）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};

use zing_core::theme::{Color, Theme};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HMODULE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext,
    ID2D1Factory1, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_EMERGENCY_BREAK, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory,
    IDWriteFontCollection, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_PRESENT_DO_NOT_WAIT, DXGI_SCALING_NONE,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGIDevice1, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, PostMessageW, WM_APP};
use windows::core::{BOOL, HSTRING, Interface, Result};

use crate::icons::Icons;

/// 文字的語系（決定中日韓文字用哪一種字形）：繁體中文 zh-TW、日文 ja-JP；英文介面用 Windows 的地區設定，
/// 檔名中的中日韓文字才會用使用者習慣的字形
fn text_locale() -> HSTRING {
    match zing_core::i18n::lang() {
        zing_core::i18n::Lang::ZhTw => HSTRING::from("zh-TW"),
        zing_core::i18n::Lang::Ja => HSTRING::from("ja-JP"),
        zing_core::i18n::Lang::En => {
            let mut buf = [0u16; 85];
            let n = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut buf) };
            if n > 1 { HSTRING::from(String::from_utf16_lossy(&buf[..n as usize - 1]).as_str()) } else { HSTRING::from("en-US") }
        }
    }
}

/// 系統安裝的字型家族（主題編輯器的字型選單）：（存進主題的英文名稱, 依介面語言的顯示名稱），依顯示名稱排序。
/// 讀取字型資料夾，在背景執行緒呼叫
pub fn system_font_families() -> Vec<(String, String)> {
    use windows::Win32::Graphics::DirectWrite::{IDWriteFontCollection, IDWriteLocalizedStrings};
    zing_core::guard::io("列出系統字型");
    let name_in = |names: &IDWriteLocalizedStrings, locale: &HSTRING| -> Option<String> {
        let (mut idx, mut exists) = (0u32, BOOL(0));
        unsafe {
            names.FindLocaleName(locale, &mut idx, &mut exists).ok()?;
            if !exists.as_bool() {
                return None;
            }
            let len = names.GetStringLength(idx).ok()? as usize;
            let mut buf = vec![0u16; len + 1];
            names.GetString(idx, &mut buf).ok()?;
            Some(String::from_utf16_lossy(&buf[..len]))
        }
    };
    let mut out = Vec::new();
    let Ok(dw) = (unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) }) else { return out };
    let mut coll: Option<IDWriteFontCollection> = None;
    if unsafe { dw.GetSystemFontCollection(&mut coll, false) }.is_err() {
        return out;
    }
    let Some(coll) = coll else { return out };
    let (en, ui) = (HSTRING::from("en-us"), text_locale());
    for i in 0..unsafe { coll.GetFontFamilyCount() } {
        let Ok(names) = (unsafe { coll.GetFontFamily(i).and_then(|f| f.GetFamilyNames()) }) else { continue };
        let value = name_in(&names, &en).or_else(|| {
            // 沒有英文名稱：用第一個
            let len = unsafe { names.GetStringLength(0) }.ok()? as usize;
            let mut buf = vec![0u16; len + 1];
            unsafe { names.GetString(0, &mut buf) }.ok()?;
            Some(String::from_utf16_lossy(&buf[..len]))
        });
        let Some(value) = value.filter(|v| !v.starts_with('@')) else { continue };
        let label = name_in(&names, &ui).unwrap_or_else(|| value.clone());
        out.push((value, label));
    }
    out.sort_by_key(|(_, l)| l.to_lowercase());
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// 背景執行緒建立好硬體繪圖裝置時送出
pub const WM_APP_GPU_READY: u32 = WM_APP + 4;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl Rect {
    pub const fn new(l: f32, t: f32, r: f32, b: f32) -> Self {
        Self { l, t, r, b }
    }
    pub fn w(&self) -> f32 {
        self.r - self.l
    }
    pub fn h(&self) -> f32 {
        self.b - self.t
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        Self::new(self.l + dx, self.t + dy, self.r - dx, self.b - dy)
    }
    pub fn is_empty(&self) -> bool {
        self.r <= self.l || self.b <= self.t
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.l, top: self.t, right: self.r, bottom: self.b }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Font {
    Text,
    TextRight,
    Bold,
    Small,
    SmallRight,
    Glyph,
    GlyphSmall,
    /// 等寬字（網址列的完整路徑）
    Mono,
    /// 縮圖下方的檔名：置中、可換行（長檔名在任意字元處斷行），超出兩行以省略號結尾
    Caption,
}

/// Fluent 圖示字型的字碼（Segoe Fluent Icons / Segoe MDL2 Assets 共用）
pub mod glyph {
    pub const BACK: &str = "\u{E72B}";
    pub const FORWARD: &str = "\u{E72A}";
    pub const UP: &str = "\u{E74A}";
    pub const REFRESH: &str = "\u{E72C}";
    pub const ADD: &str = "\u{E710}";
    pub const CLOSE: &str = "\u{E8BB}";
    pub const CHEVRON_RIGHT: &str = "\u{E76C}";
    pub const CHEVRON_RIGHT_SMALL: &str = "\u{E970}";
    pub const CHEVRON_DOWN_SMALL: &str = "\u{E96E}";
    pub const SORT_UP: &str = "\u{E70E}";
    pub const SORT_DOWN: &str = "\u{E70D}";
    pub const FOLDER: &str = "\u{E8B7}";
    pub const FILE: &str = "\u{E7C3}";
    pub const DRIVE: &str = "\u{EDA2}";
    pub const PC: &str = "\u{E977}";
    pub const STAR: &str = "\u{E734}";
    pub const FILTER: &str = "\u{E71C}";
    pub const SETTINGS: &str = "\u{E713}";
}

struct Formats {
    map: HashMap<Font, IDWriteTextFormat>,
}

/// 綁定到視窗的繪圖裝置：D2D 裝置內容 + flip model 交換鏈
struct Device {
    ctx: ID2D1DeviceContext,
    swap: IDXGISwapChain1,
    target: Option<ID2D1Bitmap1>,
    brush: ID2D1SolidColorBrush,
    warp: bool,
}

pub struct Gfx {
    hwnd: HWND,
    d2d: ID2D1Factory1,
    dw: IDWriteFactory,
    dev: Option<Device>,
    /// 背景建立中的硬體裝置（以原始指標傳遞）
    hw_rx: Option<Receiver<usize>>,
    /// 是否已開始在背景建立硬體裝置
    hw_started: bool,
    /// 目前幀使用的裝置內容與筆刷（與 dev 內的相同）
    rt: Option<ID2D1DeviceContext>,
    brush: Option<ID2D1SolidColorBrush>,
    formats: Formats,
    dpi: f32,
    immediate: bool,
    widths: RefCell<HashMap<(Font, String), f32>>,
    /// 圖示點陣圖（屬於目前的裝置，換裝置時清空，再由 Icons 的像素重建）
    icon_bitmaps: RefCell<HashMap<u32, ID2D1Bitmap1>>,
    thumb_bitmaps: RefCell<HashMap<u32, ID2D1Bitmap1>>,
    /// 預覽窗格目前的圖片（版本, 點陣圖）
    preview_bitmap: RefCell<Option<(u32, ID2D1Bitmap1)>>,
    /// 以字串為鍵的小圖（命令列的圖片圖示）
    keyed_bitmaps: RefCell<HashMap<String, ID2D1Bitmap1>>,
    /// 上一幀 EndDraw（GPU 清空）與 Present 的耗時（毫秒），供效能測試拆解幀時間
    pub last_end_ms: (f64, f64),
}

fn color_f(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a: c.a as f32 / 255.0 }
}

fn create_d3d(driver: D3D_DRIVER_TYPE) -> Result<ID3D11Device> {
    let mut dev = None;
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            None,
        )?;
    }
    dev.ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(0x80004005_u32 as i32)))
}

/// 在背景建立硬體裝置：顯示卡驅動初始化可能要 100 ms 以上，
/// 所以第一幀先用 WARP（軟體）裝置畫出，準備好後再切換。
/// 驅動初始化期間會持有 DXGI 的全域鎖，必須等第一幀呈現後才開始，否則 WARP 也會被卡住。
fn spawn_hardware(hwnd: HWND) -> Receiver<usize> {
    let (tx, rx) = channel();
    let hwnd_raw = hwnd.0 as isize;
    let _ = std::thread::Builder::new().name("zing-gpu-init".into()).spawn(move || {
        if let Ok(dev) = create_d3d(D3D_DRIVER_TYPE_HARDWARE)
            && tx.send(dev.into_raw() as usize).is_ok()
        {
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd_raw as _)), WM_APP_GPU_READY, WPARAM(0), LPARAM(0));
            }
        }
    });
    rx
}

impl Gfx {
    /// `immediate`：效能測試用，Present 不等待垂直同步，才能量到真實的繪製時間。
    pub fn new(hwnd: HWND, dpi: f32, theme: &Theme, immediate: bool) -> Result<Self> {
        let d2d: ID2D1Factory1 = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dw: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let formats = Self::create_formats(&dw, theme)?;
        Ok(Self {
            hwnd,
            d2d,
            dw,
            dev: None,
            hw_rx: None,
            hw_started: false,
            rt: None,
            brush: None,
            formats,
            dpi,
            immediate,
            widths: RefCell::new(HashMap::new()),
            icon_bitmaps: RefCell::new(HashMap::new()),
            thumb_bitmaps: RefCell::new(HashMap::new()),
            preview_bitmap: RefCell::new(None),
            keyed_bitmaps: RefCell::new(HashMap::new()),
            last_end_ms: (0.0, 0.0),
        })
    }

    fn create_formats(dw: &IDWriteFactory, theme: &Theme) -> Result<Formats> {
        let family = HSTRING::from(theme.font.family.as_str());
        let locale = text_locale();
        let size = theme.font.size;
        let glyph_family = if font_exists(dw, "Segoe Fluent Icons") {
            HSTRING::from("Segoe Fluent Icons")
        } else {
            HSTRING::from("Segoe MDL2 Assets")
        };
        let mk = |fam: &HSTRING, weight: DWRITE_FONT_WEIGHT, size: f32, align: DWRITE_TEXT_ALIGNMENT| -> Result<IDWriteTextFormat> {
            unsafe {
                let f = dw.CreateTextFormat(fam, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, &locale)?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                f.SetTextAlignment(align)?;
                let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                let sign = dw.CreateEllipsisTrimmingSign(&f)?;
                f.SetTrimming(&trimming, &sign)?;
                Ok(f)
            }
        };
        let mut map = HashMap::new();
        map.insert(Font::Text, mk(&family, DWRITE_FONT_WEIGHT_NORMAL, size, DWRITE_TEXT_ALIGNMENT_LEADING)?);
        map.insert(Font::TextRight, mk(&family, DWRITE_FONT_WEIGHT_NORMAL, size, DWRITE_TEXT_ALIGNMENT_TRAILING)?);
        map.insert(Font::Bold, mk(&family, DWRITE_FONT_WEIGHT_SEMI_BOLD, size, DWRITE_TEXT_ALIGNMENT_LEADING)?);
        map.insert(Font::Small, mk(&family, DWRITE_FONT_WEIGHT_NORMAL, size - 1.0, DWRITE_TEXT_ALIGNMENT_LEADING)?);
        map.insert(Font::SmallRight, mk(&family, DWRITE_FONT_WEIGHT_NORMAL, size - 1.0, DWRITE_TEXT_ALIGNMENT_TRAILING)?);
        map.insert(Font::Glyph, mk(&glyph_family, DWRITE_FONT_WEIGHT_NORMAL, 16.0, DWRITE_TEXT_ALIGNMENT_CENTER)?);
        map.insert(Font::GlyphSmall, mk(&glyph_family, DWRITE_FONT_WEIGHT_NORMAL, 10.0, DWRITE_TEXT_ALIGNMENT_CENTER)?);
        // 只有路徑樣式的網址列用到；找不到 Cascadia Mono 時 DirectWrite 會改用後備字型
        map.insert(Font::Mono, mk(&HSTRING::from("Cascadia Mono"), DWRITE_FONT_WEIGHT_NORMAL, size, DWRITE_TEXT_ALIGNMENT_LEADING)?);
        let caption = mk(&family, DWRITE_FONT_WEIGHT_NORMAL, size, DWRITE_TEXT_ALIGNMENT_CENTER)?;
        unsafe {
            caption.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK)?;
            caption.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
        }
        map.insert(Font::Caption, caption);
        Ok(Formats { map })
    }

    pub fn apply_theme(&mut self, theme: &Theme) -> Result<()> {
        self.formats = Self::create_formats(&self.dw, theme)?;
        self.widths.borrow_mut().clear();
        Ok(())
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        self.dpi = dpi;
        if let Some(d) = &mut self.dev {
            unsafe {
                d.ctx.SetTarget(None);
                d.ctx.SetDpi(dpi, dpi);
            }
            d.target = None;
        }
    }

    pub fn scale(&self) -> f32 {
        self.dpi / 96.0
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if let Some(d) = &mut self.dev {
            unsafe {
                d.ctx.SetTarget(None);
                d.target = None;
                let _ = d.swap.ResizeBuffers(0, w.max(1), h.max(1), DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0));
            }
        }
    }

    fn take_hardware(&mut self) -> Option<ID3D11Device> {
        let raw = self.hw_rx.as_ref()?.try_recv().ok()?;
        self.hw_rx = None;
        Some(unsafe { ID3D11Device::from_raw(raw as *mut _) })
    }

    /// 背景的硬體裝置準備好了：換掉 WARP 裝置。回傳是否有切換。
    pub fn adopt_hardware(&mut self) -> bool {
        let Some(d3d) = self.take_hardware() else { return false };
        // 同一個視窗同時只能有一個 flip model 交換鏈，先釋放舊的
        self.rt = None;
        self.brush = None;
        self.dev = None;
        self.icon_bitmaps.borrow_mut().clear();
        self.thumb_bitmaps.borrow_mut().clear();
        self.preview_bitmap.borrow_mut().take();
        self.keyed_bitmaps.borrow_mut().clear();
        match self.build_device(d3d, false) {
            Ok(d) => {
                self.dev = Some(d);
                true
            }
            Err(_) => false,
        }
    }

    /// 目前是否以軟體（WARP）繪圖
    pub fn is_warp(&self) -> bool {
        self.dev.as_ref().is_some_and(|d| d.warp)
    }

    fn build_device(&self, d3d: ID3D11Device, warp: bool) -> Result<Device> {
        unsafe {
            let dxgi: IDXGIDevice1 = d3d.cast()?;
            // 只排一幀，降低輸入到畫面的延遲
            let _ = dxgi.SetMaximumFrameLatency(1);
            let ctx = self.d2d.CreateDevice(&dxgi)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_NONE,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };

            let swap = factory.CreateSwapChainForHwnd(&d3d, self.hwnd, &desc, None, None)?;
            let _ = factory.MakeWindowAssociation(self.hwnd, DXGI_MWA_NO_ALT_ENTER);
            ctx.SetDpi(self.dpi, self.dpi);
            let brush = ctx.CreateSolidColorBrush(&color_f(Color::rgb(0)), None)?;
            Ok(Device { ctx, swap, target: None, brush, warp })
        }
    }

    fn ensure_target(&mut self) -> Result<()> {
        if self.dev.is_none() {
            let (d3d, warp) = match self.take_hardware() {
                Some(d) => (d, false),
                None => (create_d3d(D3D_DRIVER_TYPE_WARP)?, true),
            };
            self.dev = Some(self.build_device(d3d, warp)?);
            self.icon_bitmaps.borrow_mut().clear();
        self.thumb_bitmaps.borrow_mut().clear();
        self.preview_bitmap.borrow_mut().take();
        self.keyed_bitmaps.borrow_mut().clear();
            crate::perf::mark("device");
        }
        let d = self.dev.as_mut().unwrap();
        if d.target.is_none() {
            unsafe {
                let surface: IDXGISurface = d.swap.GetBuffer(0)?;
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                    dpiX: self.dpi,
                    dpiY: self.dpi,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    ..Default::default()
                };
                let bmp = d.ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
                d.ctx.SetTarget(&bmp);
                d.target = Some(bmp);
            }
        }
        self.rt = Some(d.ctx.clone());
        self.brush = Some(d.brush.clone());
        Ok(())
    }

    /// 開始一幀。回傳 false 表示無法繪製（例如裝置尚未就緒）。
    pub fn begin(&mut self, clear: Color) -> bool {
        if self.ensure_target().is_err() {
            self.reset_device();
            return false;
        }
        let rt = self.rt.as_ref().unwrap();
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&color_f(clear)));
        }
        true
    }

    pub fn end(&mut self) {
        let Some(d) = &self.dev else { return };
        let t0 = std::time::Instant::now();
        let drawn = unsafe { d.ctx.EndDraw(None, None) };
        let t1 = std::time::Instant::now();
        // 效能模式：不等垂直同步、交換鏈忙碌時直接略過這幀，量到的是純繪製成本
        let (sync, flags) = if self.immediate { (0, DXGI_PRESENT_DO_NOT_WAIT) } else { (1, DXGI_PRESENT(0)) };
        let presented = unsafe { d.swap.Present(sync, flags) };
        self.last_end_ms = ((t1 - t0).as_secs_f64() * 1000.0, t1.elapsed().as_secs_f64() * 1000.0);
        let lost = matches!(&drawn, Err(e) if e.code() == D2DERR_RECREATE_TARGET)
            || presented == DXGI_ERROR_DEVICE_REMOVED
            || presented == DXGI_ERROR_DEVICE_RESET;
        if lost {
            self.reset_device();
        } else if !self.hw_started {
            self.hw_started = true;
            self.hw_rx = Some(spawn_hardware(self.hwnd));
        }
    }

    /// 顯示卡重設或驅動更新：丟棄裝置，下一幀先用 WARP，並重新在背景建立硬體裝置
    fn reset_device(&mut self) {
        self.rt = None;
        self.brush = None;
        self.dev = None;
        self.icon_bitmaps.borrow_mut().clear();
        self.thumb_bitmaps.borrow_mut().clear();
        self.preview_bitmap.borrow_mut().take();
        self.keyed_bitmaps.borrow_mut().clear();
        // 下一次成功呈現後重新在背景建立硬體裝置
        self.hw_rx = None;
        self.hw_started = false;
    }

    pub fn size(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let s = self.scale();
        ((rc.right - rc.left) as f32 / s, (rc.bottom - rc.top) as f32 / s)
    }

    fn brush(&self, c: Color) -> &ID2D1SolidColorBrush {
        let b = self.brush.as_ref().unwrap();
        unsafe { b.SetColor(&color_f(c)) };
        b
    }

    pub fn fill(&self, r: Rect, c: Color) {
        let Some(rt) = &self.rt else { return };
        unsafe { rt.FillRectangle(&r.d2d(), self.brush(c)) };
    }

    pub fn fill_round(&self, r: Rect, radius: f32, c: Color) {
        let Some(rt) = &self.rt else { return };
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { rt.FillRoundedRectangle(&rr, self.brush(c)) };
    }

    pub fn stroke_round(&self, r: Rect, radius: f32, c: Color, width: f32) {
        let Some(rt) = &self.rt else { return };
        let h = width / 2.0;
        let rr = D2D1_ROUNDED_RECT { rect: r.inset(h, h).d2d(), radiusX: radius, radiusY: radius };
        unsafe { rt.DrawRoundedRectangle(&rr, self.brush(c), width, None) };
    }

    pub fn hline(&self, x0: f32, x1: f32, y: f32, c: Color) {
        self.fill(Rect::new(x0, y, x1, y + 1.0), c);
    }

    pub fn vline(&self, x: f32, y0: f32, y1: f32, c: Color) {
        self.fill(Rect::new(x, y0, x + 1.0, y1), c);
    }

    pub fn drop_preview(&self) {
        self.preview_bitmap.borrow_mut().take();
    }

    /// 預覽窗格的圖片：`version` 變了才重建點陣圖；在 `r` 內等比縮放（不放大超過原圖）、置中
    pub fn preview_image(&self, version: u32, p: &crate::icons::IconPixels, r: Rect) -> bool {
        let Some(rt) = &self.rt else { return false };
        let mut slot = self.preview_bitmap.borrow_mut();
        if slot.as_ref().is_none_or(|(v, _)| *v != version) {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let Ok(b) = (unsafe {
                rt.CreateBitmap(D2D_SIZE_U { width: p.w, height: p.h }, Some(p.bgra.as_ptr() as *const _), p.w * 4, &props)
            }) else {
                return false;
            };
            *slot = Some((version, b));
        }
        let (bw, bh) = (p.w as f32, p.h as f32);
        if bw <= 0.0 || bh <= 0.0 || r.w() <= 0.0 || r.h() <= 0.0 {
            return false;
        }
        let s = self.scale();
        let k = (r.w() / (bw / s)).min(r.h() / (bh / s)).min(1.0);
        let (w, h) = (bw / s * k, bh / s * k);
        let snap = |v: f32| (v * s).round() / s;
        let l = snap(r.l + (r.w() - w) / 2.0);
        let t = snap(r.t + (r.h() - h) / 2.0);
        let dest = Rect::new(l, t, l + w, t + h);
        let bmp = &slot.as_ref().unwrap().1;
        unsafe { rt.DrawBitmap(bmp, Some(&dest.d2d()), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None) };
        true
    }

    /// 以 `key` 快取點陣圖，在 `r` 內等比縮放、置中繪製（命令列的圖片圖示）
    pub fn keyed_image(&self, key: &str, p: &crate::icons::IconPixels, r: Rect, opacity: f32) {
        let Some(rt) = &self.rt else { return };
        let mut cache = self.keyed_bitmaps.borrow_mut();
        if !cache.contains_key(key) {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let Ok(b) = (unsafe {
                rt.CreateBitmap(D2D_SIZE_U { width: p.w, height: p.h }, Some(p.bgra.as_ptr() as *const _), p.w * 4, &props)
            }) else {
                return;
            };
            cache.insert(key.to_string(), b);
        }
        let (bw, bh) = (p.w as f32, p.h as f32);
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }
        let k = (r.w() / bw).min(r.h() / bh);
        let (w, h) = (bw * k, bh * k);
        let dest = Rect::new(r.l + (r.w() - w) / 2.0, r.t + (r.h() - h) / 2.0, r.l + (r.w() + w) / 2.0, r.t + (r.h() + h) / 2.0);
        unsafe { rt.DrawBitmap(&cache[key], Some(&dest.d2d()), opacity, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None) };
    }

    /// 縮圖快取丟掉的項目：一併釋放點陣圖
    pub fn drop_thumbs(&self, ids: &[u32]) {
        let mut cache = self.thumb_bitmaps.borrow_mut();
        for id in ids {
            cache.remove(id);
        }
    }

    /// 在 `r` 內等比縮放、置中繪製縮圖；尚未就緒回傳 false
    pub fn thumb(&self, id: u32, thumbs: &crate::thumbs::Thumbs, r: Rect, opacity: f32) -> bool {
        let Some(rt) = &self.rt else { return false };
        let mut cache = self.thumb_bitmaps.borrow_mut();
        if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(id) {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let created = thumbs.with_pixels(id, |p| unsafe {
                rt.CreateBitmap(D2D_SIZE_U { width: p.w, height: p.h }, Some(p.bgra.as_ptr() as *const _), p.w * 4, &props)
                    .map(|b| (b, p.w as f32, p.h as f32))
            });
            match created {
                Some(Ok((b, _, _))) => {
                    e.insert(b);
                }
                _ => return false,
            }
        }
        let bmp = &cache[&id];
        let size = unsafe { bmp.GetPixelSize() };
        let (bw, bh) = (size.width as f32, size.height as f32);
        if bw <= 0.0 || bh <= 0.0 {
            return false;
        }
        // 以實際像素換算成 DIP，不放大超過原圖（小圖示不糊）
        let s = self.scale();
        let k = (r.w() / (bw / s)).min(r.h() / (bh / s)).min(1.0);
        let (w, h) = (bw / s * k, bh / s * k);
        let snap = |v: f32| (v * s).round() / s;
        let l = snap(r.l + (r.w() - w) / 2.0);
        let t = snap(r.t + (r.h() - h) / 2.0);
        let dest = Rect::new(l, t, l + w, t + h);
        unsafe { rt.DrawBitmap(bmp, Some(&dest.d2d()), opacity, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None) };
        true
    }

    pub fn clear_icons(&self) {
        self.icon_bitmaps.borrow_mut().clear();
    }

    /// 在 `r` 中央繪製 16×16 DIP 的圖示；對齊實體像素避免模糊。回傳是否有畫出來。
    pub fn icon(&self, id: u32, icons: &Icons, r: Rect, opacity: f32) -> bool {
        let Some(rt) = &self.rt else { return false };
        let mut cache = self.icon_bitmaps.borrow_mut();
        if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(id) {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let created = icons.with_pixels(id, |p| unsafe {
                rt.CreateBitmap(D2D_SIZE_U { width: p.w, height: p.h }, Some(p.bgra.as_ptr() as *const _), p.w * 4, &props)
            });
            match created {
                Some(Ok(b)) => {
                    e.insert(b);
                }
                _ => return false,
            }
        }
        let s = self.scale();
        let snap = |v: f32| (v * s).round() / s;
        let l = snap((r.l + r.r) / 2.0 - 8.0);
        let t = snap((r.t + r.b) / 2.0 - 8.0);
        let dest = Rect::new(l, t, l + 16.0, t + 16.0);
        unsafe { rt.DrawBitmap(&cache[&id], Some(&dest.d2d()), opacity, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None) };
        true
    }

    /// 繪製 UTF-16 文字（不需轉換，檔名直接從字串池取用）
    pub fn text_w(&self, s: &[u16], font: Font, r: Rect, c: Color) {
        let Some(rt) = &self.rt else { return };
        if r.is_empty() {
            return;
        }
        unsafe {
            rt.DrawText(
                s,
                &self.formats.map[&font],
                &r.d2d(),
                self.brush(c),
                D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }

    pub fn text(&self, s: &str, font: Font, r: Rect, c: Color) {
        let w: Vec<u16> = s.encode_utf16().collect();
        self.text_w(&w, font, r, c);
    }

    pub fn push_clip(&self, r: Rect) {
        if let Some(rt) = &self.rt {
            unsafe { rt.PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) };
        }
    }

    pub fn pop_clip(&self) {
        if let Some(rt) = &self.rt {
            unsafe { rt.PopAxisAlignedClip() };
        }
    }

    /// 文字寬度（DIP），結果快取
    pub fn measure(&self, s: &str, font: Font) -> f32 {
        if let Some(&w) = self.widths.borrow().get(&(font, s.to_string())) {
            return w;
        }
        let w16: Vec<u16> = s.encode_utf16().collect();
        let width = unsafe {
            self.dw
                .CreateTextLayout(&w16, &self.formats.map[&font], 100_000.0, 100.0)
                .and_then(|layout| {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    layout.GetMetrics(&mut m).map(|_| m.widthIncludingTrailingWhitespace)
                })
                .unwrap_or(s.chars().count() as f32 * 8.0)
        };
        let mut cache = self.widths.borrow_mut();
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert((font, s.to_string()), width);
        width
    }
}

fn font_exists(dw: &IDWriteFactory, family: &str) -> bool {
    unsafe {
        let mut coll: Option<IDWriteFontCollection> = None;
        if dw.GetSystemFontCollection(&mut coll, false).is_err() {
            return false;
        }
        let Some(coll) = coll else { return false };
        let mut index = 0u32;
        let mut exists = BOOL(0);
        coll.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists).is_ok() && exists.as_bool()
    }
}
