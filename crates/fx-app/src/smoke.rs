//! 自動化 UI 冒煙測試（tools/smoke.ps1 使用）。
//!
//! `fx --smoke-out=結果.json --smoke-dir=測試資料夾 [--smoke-shots=截圖資料夾]`
//!
//! 在程序內直接對視窗送出鍵盤、滑鼠訊息，走與真實輸入相同的視窗程序；修飾鍵用 SetKeyboardState
//! 設定本執行緒的按鍵狀態。不移動真實游標、視窗不搶前景，使用者操作電腦時也能執行。
//! 每一步送出輸入後輪詢 App 狀態與磁碟，逾時即失敗並停止；結束時寫出報告並關閉。
//!
//! 測試資料夾由 tools/smoke.ps1 建立。不測右鍵選單、剪貼簿、刪除：會搶前景視窗、
//! 覆蓋使用者的剪貼簿、留東西在資源回收筒。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fx_core::path;
use fx_core::sort::SortKey;
use fx_core::theme::Mode;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, POINTL, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardState, SetKeyboardState, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F2, VK_HOME,
    VK_LEFT, VK_MENU, VK_OEM_5, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, KillTimer, SendMessageW, SetTimer, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONUP,
};

use crate::app::{App, EditKind, Focus, TIMER_SMOKE};
use crate::window::with_app;

pub struct Smoke {
    out: PathBuf,
    dir: String,
    shots: Option<PathBuf>,
}

impl Smoke {
    pub fn from_args(args: &[String]) -> Option<Self> {
        let get = |k: &str| args.iter().find_map(|a| a.strip_prefix(&format!("--{k}=")).map(String::from));
        Some(Self {
            out: PathBuf::from(get("smoke-out")?),
            dir: path::normalize(&get("smoke-dir")?),
            shots: get("smoke-shots").map(PathBuf::from),
        })
    }

    pub fn dir(&self) -> &str {
        &self.dir
    }
}

// ---------------------------------------------------------------- 驅動

type Act = Box<dyn FnOnce(&mut Ctx)>;
type Check = Box<dyn FnMut(&mut Ctx, &mut App) -> Result<(), String>>;

struct Step {
    name: String,
    act: Option<Act>,
    check: Check,
    timeout: Duration,
}

impl Step {
    /// 檔案作業、Shell 拖放：給比較長的時間
    fn slow(mut self) -> Self {
        self.timeout = Duration::from_secs(10);
        self
    }
}

fn step(
    name: &str,
    act: impl FnOnce(&mut Ctx) + 'static,
    check: impl FnMut(&mut Ctx, &mut App) -> Result<(), String> + 'static,
) -> Step {
    Step { name: name.into(), act: Some(Box::new(act)), check: Box::new(check), timeout: Duration::from_secs(4) }
}

/// 步驟之間共用的資料
struct Ctx {
    hwnd: HWND,
    dir: String,
    shots: Option<PathBuf>,
    /// 截圖的平均亮度（名稱, 0–255）
    luma: Vec<(String, f64)>,
    started: Instant,
    /// 數值型的觀察結果（拖放時的解析次數等）
    notes: Vec<(&'static str, String)>,
}

impl Ctx {
    fn sub(&self, name: &str) -> String {
        path::join(&self.dir, name)
    }

    fn note(&self, key: &str) -> Option<&str> {
        self.notes.iter().rev().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }
}

struct Driver {
    ctx: Ctx,
    out: PathBuf,
    steps: VecDeque<Step>,
    results: Vec<(String, bool, f64, String)>,
    /// 執行期間狀態列出現過的錯誤
    errors: Vec<String>,
    last_status: String,
}

thread_local! {
    static DRIVER: RefCell<Option<Driver>> = const { RefCell::new(None) };
}

pub fn start(hwnd: HWND, smoke: Smoke) {
    let ctx = Ctx {
        hwnd,
        dir: smoke.dir,
        shots: smoke.shots,
        luma: Vec::new(),
        started: Instant::now(),
        notes: Vec::new(),
    };
    let driver = Driver {
        steps: steps(&ctx).into(),
        ctx,
        out: smoke.out,
        results: Vec::new(),
        errors: Vec::new(),
        last_status: String::new(),
    };
    DRIVER.with(|d| *d.borrow_mut() = Some(driver));
    unsafe { SetTimer(Some(hwnd), TIMER_SMOKE, 10, None) };
}

/// 計時器呼叫（不在 App 借用中）。動作會同步送出視窗訊息，期間若有巢狀的計時器訊息就略過。
pub fn tick(hwnd: HWND) {
    let Some(mut d) = DRIVER.with(|d| d.borrow_mut().take()) else { return };
    if d.advance() {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_SMOKE);
        }
        d.finish();
        crate::window::close(hwnd);
    } else {
        DRIVER.with(|x| *x.borrow_mut() = Some(d));
    }
}

impl Driver {
    /// 推進一步；全部完成或失敗時回傳 true
    fn advance(&mut self) -> bool {
        let Some(step) = self.steps.front_mut() else { return true };
        if let Some(act) = step.act.take() {
            // 動作與檢查分在不同的計時器週期，讓動作引發的訊息（繪製、背景結果）先處理
            self.ctx.started = Instant::now();
            act(&mut self.ctx);
            return false;
        }
        let ctx = &mut self.ctx;
        let (errors, last) = (&mut self.errors, &mut self.last_status);
        let r = {
            // 檢查會讀磁碟、寫截圖；被測的程式碼在動作中執行，不受影響
            let _allow = fx_core::guard::allow_io("冒煙測試的驗證");
            with_app(|a| {
                if let Some((text, true, _)) = &a.status
                    && text != last
                {
                    *last = text.clone();
                    errors.push(text.clone());
                }
                (step.check)(ctx, a)
            })
        };
        let elapsed = ctx.started.elapsed();
        let ms = elapsed.as_secs_f64() * 1000.0;
        match r {
            Some(Ok(())) => {
                self.results.push((step.name.clone(), true, ms, String::new()));
                self.steps.pop_front();
                self.steps.is_empty()
            }
            Some(Err(why)) if elapsed > step.timeout => {
                self.results.push((step.name.clone(), false, ms, why));
                true
            }
            _ => false,
        }
    }

    fn finish(self) {
        let _allow = fx_core::guard::allow_io("冒煙測試寫出報告");
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        let list = |v: &[String]| v.iter().map(|s| format!("\"{}\"", esc(s))).collect::<Vec<_>>().join(", ");
        let violations = fx_core::guard::take_violations();
        let skipped: Vec<String> = self.steps.iter().skip(1).map(|s| s.name.clone()).collect();
        let ok = self.steps.is_empty() && self.errors.is_empty() && violations.is_empty();
        let steps: Vec<String> = self
            .results
            .iter()
            .map(|(name, ok, ms, why)| {
                format!("    {{ \"name\": \"{}\", \"ok\": {ok}, \"ms\": {ms:.1}, \"detail\": \"{}\" }}", esc(name), esc(why))
            })
            .collect();
        let notes: Vec<String> = self.ctx.notes.iter().map(|(k, v)| format!("\"{k}\": \"{}\"", esc(v))).collect();
        let json = format!(
            "{{\n  \"ok\": {ok},\n  \"steps\": [\n{}\n  ],\n  \"notes\": {{ {} }},\n  \"not_run\": [{}],\n  \"status_errors\": [{}],\n  \"guard_violations\": [{}]\n}}\n",
            steps.join(",\n"),
            notes.join(", "),
            list(&skipped),
            list(&self.errors),
            list(&violations),
        );
        let _ = std::fs::write(&self.out, json);
    }
}

// ---------------------------------------------------------------- 送出輸入

fn send(h: HWND, msg: u32, w: usize, l: isize) {
    unsafe {
        SendMessageW(h, msg, Some(WPARAM(w)), Some(LPARAM(l)));
    }
}

/// 在指定的修飾鍵狀態下執行（"c" Ctrl、"s" Shift、"a" Alt），結束後還原
fn with_mods(mods: &str, f: impl FnOnce()) {
    unsafe {
        let mut saved = [0u8; 256];
        let _ = GetKeyboardState(&mut saved);
        let mut state = saved;
        for (c, vk) in [('c', VK_CONTROL), ('s', VK_SHIFT), ('a', VK_MENU)] {
            state[vk.0 as usize] = if mods.contains(c) { 0x80 } else { 0 };
        }
        let _ = SetKeyboardState(&state);
        f();
        let _ = SetKeyboardState(&saved);
    }
}

/// TranslateMessage 會為這個按鍵產生的字元
fn key_char(vk: u16, mods: &str) -> Option<u16> {
    let ctrl = mods.contains('c');
    match vk {
        0x41..=0x5A if ctrl => Some(vk - 0x40),
        0x41..=0x5A if mods.contains('s') => Some(vk),
        0x41..=0x5A => Some(vk + 0x20),
        0x08 | 0x0D | 0x1B if !ctrl => Some(vk),
        0x09 | 0x20 if !ctrl => Some(vk),
        _ => None,
    }
}

/// 按下並放開一個鍵，連同它產生的字元訊息
fn key_to(h: HWND, mods: &str, vk: VIRTUAL_KEY) {
    let alt = mods.contains('a');
    let l = 1 | if alt { 1 << 29 } else { 0 };
    with_mods(mods, || {
        send(h, if alt { WM_SYSKEYDOWN } else { WM_KEYDOWN }, vk.0 as usize, l);
        if let Some(ch) = key_char(vk.0, mods) {
            send(h, if alt { WM_SYSCHAR } else { WM_CHAR }, ch as usize, l);
        }
        send(h, if alt { WM_SYSKEYUP } else { WM_KEYUP }, vk.0 as usize, l | (3 << 30));
    });
}

fn key(ctx: &Ctx, mods: &str, vk: VIRTUAL_KEY) {
    key_to(ctx.hwnd, mods, vk);
}

fn letter(c: char) -> VIRTUAL_KEY {
    VIRTUAL_KEY(c.to_ascii_uppercase() as u16)
}

fn type_to(h: HWND, text: &str) {
    for u in text.encode_utf16() {
        send(h, WM_CHAR, u as usize, 1);
    }
}

/// 目前的編輯框（重新命名、網址列）
fn edit_hwnd() -> Option<HWND> {
    with_app(|a| a.edit.as_ref().map(|e| e.hwnd)).flatten()
}

/// 在編輯框輸入文字後按 Enter
fn edit_commit(text: &str) {
    if let Some(e) = edit_hwnd() {
        type_to(e, text);
        key_to(e, "", VK_RETURN);
    }
}

fn scale(h: HWND) -> f32 {
    unsafe { GetDpiForWindow(h) as f32 / 96.0 }
}

/// 用戶區 DIP 座標 → 滑鼠訊息的 lParam
fn mouse_lparam(h: HWND, (x, y): (f32, f32)) -> isize {
    let s = scale(h);
    let (px, py) = ((x * s).round() as i32, (y * s).round() as i32);
    ((py as u16 as isize) << 16) | px as u16 as isize
}

fn screen_point(h: HWND, (x, y): (f32, f32)) -> POINT {
    let s = scale(h);
    let mut pt = POINT { x: (x * s).round() as i32, y: (y * s).round() as i32 };
    unsafe {
        let _ = ClientToScreen(h, &mut pt);
    }
    pt
}

fn mk(mods: &str) -> usize {
    if mods.contains('c') { MK_CONTROL.0 as usize } else { 0 }
}

fn click(ctx: &Ctx, mods: &str, at: Option<(f32, f32)>) {
    let Some(at) = at else { return };
    let (h, l) = (ctx.hwnd, mouse_lparam(ctx.hwnd, at));
    with_mods(mods, || {
        send(h, WM_MOUSEMOVE, mk(mods), l);
        send(h, WM_LBUTTONDOWN, mk(mods) | MK_LBUTTON.0 as usize, l);
        send(h, WM_LBUTTONUP, mk(mods), l);
    });
}

fn double_click(ctx: &Ctx, at: Option<(f32, f32)>) {
    let Some(at) = at else { return };
    let (h, l) = (ctx.hwnd, mouse_lparam(ctx.hwnd, at));
    send(h, WM_MOUSEMOVE, 0, l);
    send(h, WM_LBUTTONDOWN, MK_LBUTTON.0 as usize, l);
    send(h, WM_LBUTTONUP, 0, l);
    send(h, WM_LBUTTONDBLCLK, MK_LBUTTON.0 as usize, l);
    send(h, WM_LBUTTONUP, 0, l);
}

fn wheel(ctx: &Ctx, at: Option<(f32, f32)>, delta: i16) {
    let Some(at) = at else { return };
    let pt = screen_point(ctx.hwnd, at);
    let l = ((pt.y as u16 as isize) << 16) | pt.x as u16 as isize;
    send(ctx.hwnd, WM_MOUSEWHEEL, (delta as u16 as usize) << 16, l);
}

// ---------------------------------------------------------------- 讀取畫面位置與狀態

/// 使用中窗格某個項目所在列的中心（DIP）
fn row_pos(name: &str) -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let p = a.active_pane;
        let pl = &a.layout.panes[p];
        let t = a.panes[p].tab();
        let i = t.find_view(name)?;
        Some((pl.cols[0].0 + 40.0, pl.list.t + (i as f32 + 0.5) * a.row_h() - t.scroll))
    })
    .flatten()
}

fn header_pos(col: usize) -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let pl = &a.layout.panes[a.active_pane];
        (pl.cols[col].0 + 30.0, (pl.header.t + pl.header.b) / 2.0)
    })
}

fn list_center() -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let pl = &a.layout.panes[a.active_pane];
        ((pl.list.l + pl.list.r) / 2.0, (pl.list.t + pl.list.b) / 2.0)
    })
}

fn names(a: &App) -> Vec<String> {
    let t = a.tab();
    t.view.iter().map(|&e| t.listing.name_string(e as usize)).collect()
}

fn selected(a: &App) -> Vec<String> {
    let t = a.tab();
    t.selected_entries().into_iter().map(|e| t.listing.name_string(e)).collect()
}

/// 使用中分頁已載入完成並停在 `dir`
fn at(a: &App, dir: &str) -> Result<(), String> {
    let t = a.tab();
    if !t.path.eq_ignore_ascii_case(dir) {
        return Err(format!("目前在「{}」", t.path));
    }
    if t.loading || t.refreshing {
        return Err("仍在載入".into());
    }
    Ok(())
}

fn expect<T: PartialEq + std::fmt::Debug>(what: &str, got: T, want: T) -> Result<(), String> {
    if got == want { Ok(()) } else { Err(format!("{what}：得到 {got:?}，預期 {want:?}")) }
}

fn exists(p: &str) -> bool {
    std::path::Path::new(p).exists()
}

// ---------------------------------------------------------------- 截圖

/// 擷取視窗用戶區（PrintWindow 可擷取翻轉模型交換鏈的內容，視窗被遮住也可以）
fn capture(h: HWND) -> Option<(i32, i32, Vec<u8>)> {
    unsafe {
        let mut rc = RECT::default();
        GetClientRect(h, &mut rc).ok()?;
        let (w, ht) = (rc.right, rc.bottom);
        if w <= 0 || ht <= 0 {
            return None;
        }
        let wdc = GetDC(Some(h));
        let mdc = CreateCompatibleDC(Some(wdc));
        let bmp = CreateCompatibleBitmap(wdc, w, ht);
        let old = SelectObject(mdc, bmp.into());
        // PW_CLIENTONLY | PW_RENDERFULLCONTENT
        let ok = PrintWindow(h, mdc, PRINT_WINDOW_FLAGS(1 | 2)).as_bool();
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -ht,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * ht * 4) as usize];
        SelectObject(mdc, old);
        let lines = GetDIBits(mdc, bmp, 0, ht as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mdc);
        ReleaseDC(Some(h), wdc);
        (ok && lines == ht).then_some((w, ht, px))
    }
}

/// 32 位元由上而下的 BMP
fn write_bmp(p: &std::path::Path, w: i32, h: i32, px: &[u8]) -> std::io::Result<()> {
    let mut f = Vec::with_capacity(54 + px.len());
    let size = 54 + px.len() as u32;
    f.extend_from_slice(b"BM");
    f.extend_from_slice(&size.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&54u32.to_le_bytes());
    f.extend_from_slice(&40u32.to_le_bytes());
    f.extend_from_slice(&w.to_le_bytes());
    f.extend_from_slice(&(-h).to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&32u16.to_le_bytes());
    f.extend_from_slice(&[0u8; 24]);
    f.extend_from_slice(px);
    std::fs::write(p, f)
}

/// 截圖並檢查不是空白畫面；記錄平均亮度供之後比較
fn screenshot(ctx: &mut Ctx, name: &str) -> Result<(), String> {
    // 等呈現完成（翻轉模型的內容在 Present 之後才會被合成）
    if ctx.started.elapsed() < Duration::from_millis(200) {
        return Err("等待呈現".into());
    }
    let (w, h, px) = capture(ctx.hwnd).ok_or("擷取失敗")?;
    let mut colors = std::collections::HashSet::new();
    let mut sum = 0.0;
    let mut n = 0.0;
    for &[b, g, r, _] in px.as_chunks::<4>().0.iter().step_by(7) {
        colors.insert(u32::from_le_bytes([b, g, r, 0]));
        sum += 0.114 * b as f64 + 0.587 * g as f64 + 0.299 * r as f64;
        n += 1.0;
    }
    if let Some(dir) = &ctx.shots {
        let _ = std::fs::create_dir_all(dir);
        write_bmp(&dir.join(format!("{name}.bmp")), w, h, &px).map_err(|e| e.to_string())?;
    }
    // 有文字（反鋸齒）與圖示的畫面會有大量顏色；空白或全黑只有一兩種
    if colors.len() < 40 {
        return Err(format!("畫面像是空白（只有 {} 種顏色）", colors.len()));
    }
    ctx.luma.push((name.to_string(), sum / n));
    Ok(())
}

fn luma(ctx: &Ctx, name: &str) -> f64 {
    ctx.luma.iter().find(|(n, _)| n == name).map_or(-1.0, |(_, v)| *v)
}

// ---------------------------------------------------------------- 拖放

/// 對放置目標模擬一次左鍵拖曳：經過 `over` 中的各點後放在 `drop_at`（`copy`：按住 Ctrl）
fn simulate_drop(ctx: &mut Ctx, source: &str, over: &[(f32, f32)], drop_at: (f32, f32), copy: bool) {
    let Some(target) = crate::window::drop_target() else {
        ctx.notes.push(("drop_error", "沒有註冊放置目標".into()));
        return;
    };
    // 資料物件在真實拖放中由拖曳來源（其他程式）建立，不算 fx 的 UI 執行緒 I/O
    let data = {
        let _allow = fx_core::guard::allow_io("冒煙測試扮演拖曳來源");
        crate::shell::data_object(&[source.to_string()])
    };
    let data = match data {
        Ok(d) => d,
        Err(e) => {
            ctx.notes.push(("drop_error", e.message()));
            return;
        }
    };
    let keys = MODIFIERKEYS_FLAGS(MK_LBUTTON.0 | if copy { MK_CONTROL.0 } else { 0 });
    let all = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
    let pt = |p: (f32, f32)| {
        let s = screen_point(ctx.hwnd, p);
        POINTL { x: s.x, y: s.y }
    };
    let before = crate::shell::DROP_RESOLVES.load(std::sync::atomic::Ordering::Relaxed);
    let mut hovered = 0;
    unsafe {
        let mut eff: DROPEFFECT = all;
        let first = pt(over[0]);
        let t = Instant::now();
        let _ = target.DragEnter(&data, keys, first, &mut eff);
        ctx.notes.push(("enter_ms", format!("{:.1}", t.elapsed().as_secs_f64() * 1000.0)));
        let t = Instant::now();
        for &p in &over[1..] {
            eff = all;
            let _ = target.DragOver(keys, pt(p), &mut eff);
            if with_app(|a| a.drop_hover.is_some_and(|(_, row)| row.is_some())).unwrap_or(false) {
                hovered += 1;
            }
        }
        ctx.notes.push(("over_ms", format!("{:.1}", t.elapsed().as_secs_f64() * 1000.0)));
        let resolves = crate::shell::DROP_RESOLVES.load(std::sync::atomic::Ordering::Relaxed) - before;
        ctx.notes.push(("resolves", resolves.to_string()));
        ctx.notes.push(("hovered", format!("{hovered}/{}", over.len() - 1)));
        ctx.notes.push(("over_effect", eff.0.to_string()));
        eff = all;
        let p = pt(drop_at);
        let _ = target.DragOver(keys, p, &mut eff);
        let t = Instant::now();
        let r = target.Drop(&data, keys, p, &mut eff);
        ctx.notes.push(("drop_ms", format!("{:.1}", t.elapsed().as_secs_f64() * 1000.0)));
        ctx.notes.push(("drop_effect", eff.0.to_string()));
        if let Err(e) = r {
            ctx.notes.push(("drop_error", e.message()));
        }
    }
}

/// 放下一般檔案時交給背景檔案作業，Drop 本身應立即返回
fn drop_fast(c: &Ctx) -> Result<(), String> {
    let ms: f64 = c.note("drop_ms").and_then(|v| v.parse().ok()).unwrap_or(f64::MAX);
    if ms > 30.0 { Err(format!("Drop 阻塞 UI 執行緒 {ms:.0} ms")) } else { Ok(()) }
}

// ---------------------------------------------------------------- 測試步驟

/// 測試資料夾的內容（tools/smoke.ps1 建立）：
/// 資料夾A\內層.txt、資料夾B\項目 001–200.txt、a.txt、b.txt、報告 9.txt、報告 10.txt、
/// 拖放來源.txt、.hidden.txt（隱藏）
const VISIBLE: usize = 7;
const B_FILES: usize = 200;

fn steps(ctx: &Ctx) -> Vec<Step> {
    let d = ctx.dir.clone();
    let dir_a = ctx.sub("資料夾A");
    let dir_b = ctx.sub("資料夾B");
    let mut v = Vec::new();

    v.push(step("載入測試資料夾", |_| {}, {
        let d = d.clone();
        move |_, a| {
            at(a, &d)?;
            let n = names(a);
            expect("項目數", n.len(), VISIBLE)?;
            expect("資料夾在前", &n[..2], &["資料夾A".to_string(), "資料夾B".to_string()][..])?;
            let pos = |s: &str| n.iter().position(|x| x == s);
            if pos("報告 9.txt") >= pos("報告 10.txt") {
                return Err(format!("自然排序錯誤：{n:?}"));
            }
            if pos(".hidden.txt").is_some() {
                return Err("顯示了隱藏檔".into());
            }
            Ok(())
        }
    }));
    v.push(step("截圖：清單（深色）", |_| {}, |c, _| screenshot(c, "01-清單-深色")));

    // ------------------------------------------------ 鍵盤
    v.push(step(
        "鍵盤：Home、↓↓ 移動選取",
        |c| {
            key(c, "", VK_HOME);
            key(c, "", VK_DOWN);
            key(c, "", VK_DOWN);
        },
        |_, a| {
            expect("焦點", a.tab().focus, 2)?;
            expect("選取", selected(a), vec![names(a)[2].clone()])
        },
    ));
    v.push(step("鍵盤：Shift+End 範圍選取", |c| key(c, "s", VK_END), |_, a| expect("選取數", selected(a).len(), VISIBLE - 2)));
    v.push(step("鍵盤：Ctrl+A 全選", |c| key(c, "c", letter('a')), |_, a| expect("選取數", selected(a).len(), VISIBLE)));
    v.push(step("鍵盤：Esc 取消選取", |c| key(c, "", VK_ESCAPE), |_, a| expect("選取數", selected(a).len(), 0)));

    v.push(step(
        "篩選：直接輸入文字",
        |c| type_to(c.hwnd, "報告"),
        |_, a| {
            expect("篩選", a.tab().filter.as_str(), "報告")?;
            expect("結果", names(a), vec!["報告 9.txt".to_string(), "報告 10.txt".to_string()])
        },
    ));
    v.push(step("篩選：Backspace 刪字", |c| key(c, "", VK_BACK), |_, a| expect("篩選", a.tab().filter.as_str(), "報")));
    v.push(step("篩選：Esc 清除", |c| key(c, "", VK_ESCAPE), |_, a| {
        expect("篩選", a.tab().filter.as_str(), "")?;
        expect("項目數", names(a).len(), VISIBLE)
    }));

    // ------------------------------------------------ 導覽
    v.push(step(
        "導覽：Enter 進入資料夾",
        |c| {
            key(c, "", VK_HOME);
            key(c, "", VK_RETURN);
        },
        {
            let dir_a = dir_a.clone();
            move |_, a| {
                at(a, &dir_a)?;
                expect("內容", names(a), vec!["內層.txt".to_string()])
            }
        },
    ));
    v.push(step("導覽：Backspace 上一層並選取原資料夾", |c| key(c, "", VK_BACK), {
        let d = d.clone();
        move |_, a| {
            at(a, &d)?;
            expect("選取", selected(a), vec!["資料夾A".to_string()])
        }
    }));
    v.push(step("導覽：Alt+← 上一頁", |c| key(c, "a", VK_LEFT), {
        let dir_a = dir_a.clone();
        move |_, a| at(a, &dir_a)
    }));
    v.push(step("導覽：Alt+→ 下一頁", |c| key(c, "a", VK_RIGHT), {
        let d = d.clone();
        move |_, a| at(a, &d)
    }));

    // ------------------------------------------------ 滑鼠
    v.push(step("滑鼠：點選一列", |c| click(c, "", row_pos("b.txt")), |_, a| {
        expect("選取", selected(a), vec!["b.txt".to_string()])
    }));
    v.push(step("滑鼠：Ctrl+點選加選", |c| click(c, "c", row_pos("a.txt")), |_, a| {
        expect("選取", selected(a), vec!["a.txt".to_string(), "b.txt".to_string()])
    }));
    v.push(step("滑鼠：點名稱欄標頭反向排序", |c| click(c, "", header_pos(0)), |_, a| {
        let s = a.tab().spec;
        expect("排序", (s.key, s.ascending), (SortKey::Name, false))?;
        // 反向時資料夾仍在前，名稱由大到小
        expect("第一項", names(a)[0].as_str(), "資料夾B")
    }));
    v.push(step("滑鼠：再點一次恢復", |c| click(c, "", header_pos(0)), |_, a| {
        let s = a.tab().spec;
        expect("排序", (s.key, s.ascending), (SortKey::Name, true))?;
        expect("第一項", names(a)[0].as_str(), "資料夾A")
    }));
    v.push(step("滑鼠：雙擊進入資料夾", |c| double_click(c, row_pos("資料夾B")), {
        let dir_b = dir_b.clone();
        move |_, a| {
            at(a, &dir_b)?;
            expect("項目數", names(a).len(), B_FILES)
        }
    }));
    v.push(step("滑鼠：滾輪捲動", |c| wheel(c, list_center(), -360), |_, a| {
        let s = a.tab().scroll;
        if s > 0.0 { Ok(()) } else { Err(format!("scroll = {s}")) }
    }));
    v.push(step("鍵盤：End 捲到最後", |c| key(c, "", VK_END), |_, a| {
        expect("焦點", a.tab().focus, B_FILES - 1)?;
        let h = a.layout.panes[a.active_pane].list.h();
        let max = a.max_scroll(a.active_pane, h);
        if (a.tab().scroll - max).abs() < 1.0 { Ok(()) } else { Err(format!("scroll {} ≠ {max}", a.tab().scroll)) }
    }));
    v.push(step("滑鼠：側鍵上一頁", |c| send(c.hwnd, WM_XBUTTONUP, 1 << 16, 0), {
        let d = d.clone();
        move |_, a| at(a, &d)
    }));

    // ------------------------------------------------ 檔案作業（只在測試資料夾內）
    v.push(
        step("新增資料夾：建立並進入命名", |c| key(c, "cs", letter('n')), {
            let p = ctx.sub("新增資料夾");
            move |_, a| match &a.edit {
                Some(e) if matches!(&e.kind, EditKind::Rename { path, .. } if path.eq_ignore_ascii_case(&p)) => {
                    if exists(&p) { Ok(()) } else { Err("磁碟上沒有新資料夾".into()) }
                }
                _ => Err("重新命名框沒有出現".into()),
            }
        })
        .slow(),
    );
    v.push(
        step("新增資料夾：輸入名稱後 Enter", |_| edit_commit("冒煙測試資料夾"), {
            let (old, new) = (ctx.sub("新增資料夾"), ctx.sub("冒煙測試資料夾"));
            move |_, a| {
                if !exists(&new) || exists(&old) {
                    return Err("磁碟上的名稱沒有改變".into());
                }
                expect("選取", selected(a), vec!["冒煙測試資料夾".to_string()])
            }
        })
        .slow(),
    );
    v.push(step(
        "重新命名：F2 只選取主檔名",
        |c| {
            click(c, "", row_pos("a.txt"));
            key(c, "", VK_F2);
        },
        |_, a| match &a.edit {
            Some(e) if matches!(e.kind, EditKind::Rename { .. }) => {
                expect("內容", crate::window::edit_text(e.hwnd).as_str(), "a.txt")
            }
            _ => Err("重新命名框沒有出現".into()),
        },
    ));
    v.push(
        step("重新命名：輸入後 Enter（保留副檔名）", |_| edit_commit("a2"), {
            let (old, new) = (ctx.sub("a.txt"), ctx.sub("a2.txt"));
            move |_, a| {
                if !exists(&new) || exists(&old) {
                    return Err("磁碟上的名稱沒有改變".into());
                }
                expect("選取", selected(a), vec!["a2.txt".to_string()])
            }
        })
        .slow(),
    );
    v.push(step("重新命名：F2 後 Esc 取消", |c| key(c, "", VK_F2), |_, a| {
        if a.edit.is_some() { Ok(()) } else { Err("重新命名框沒有出現".into()) }
    }));
    v.push(step(
        "重新命名：Esc 不改名",
        |_| {
            if let Some(e) = edit_hwnd() {
                key_to(e, "", VK_ESCAPE);
            }
        },
        {
            let p = ctx.sub("a2.txt");
            move |_, a| {
                if a.edit.is_some() {
                    return Err("編輯框沒有關閉".into());
                }
                if exists(&p) { Ok(()) } else { Err("檔案被改名了".into()) }
            }
        },
    ));

    // ------------------------------------------------ 網址列
    v.push(step("網址列：Ctrl+L 開啟編輯", |c| key(c, "c", letter('l')), |_, a| match &a.edit {
        Some(e) if matches!(e.kind, EditKind::Address) => Ok(()),
        _ => Err("網址列編輯框沒有出現".into()),
    }));
    v.push(step(
        "網址列：輸入路徑後 Enter",
        {
            let dir_a = dir_a.clone();
            move |_| {
                // 全選後輸入，取代原本的路徑
                if let Some(e) = edit_hwnd() {
                    key_to(e, "c", letter('a'));
                }
                edit_commit(&dir_a);
            }
        },
        {
            let dir_a = dir_a.clone();
            move |_, a| at(a, &dir_a)
        },
    ));
    v.push(step("網址列：回到測試資料夾", |c| key(c, "a", VK_LEFT), {
        let d = d.clone();
        move |_, a| at(a, &d)
    }));

    // ------------------------------------------------ 分頁、窗格、樹狀列
    v.push(step("分頁：Ctrl+T 新增", |c| key(c, "c", letter('t')), |_, a| {
        let pane = &a.panes[a.active_pane];
        expect("分頁（數量, 使用中）", (pane.tabs.len(), pane.active), (2, 1))
    }));
    v.push(step("分頁：Ctrl+Tab 切換", |c| key(c, "c", VK_TAB), |_, a| expect("使用中分頁", a.panes[a.active_pane].active, 0)));
    v.push(step("分頁：Ctrl+W 關閉", |c| key(c, "c", letter('w')), {
        let d = d.clone();
        move |_, a| {
            expect("分頁數", a.panes[a.active_pane].tabs.len(), 1)?;
            at(a, &d)
        }
    }));
    v.push(step("窗格：Ctrl+\\ 分割", |c| key(c, "c", VK_OEM_5), {
        let d = d.clone();
        move |_, a| {
            expect("（窗格數, 使用中）", (a.panes.len(), a.active_pane), (2, 1))?;
            at(a, &d)
        }
    }));
    // 焦點循環：窗格 1 → … → 最後一個窗格 → 樹狀列 → 窗格 1
    v.push(step("焦點：Tab 從最後窗格移到樹狀列", |c| key(c, "", VK_TAB), |_, a| expect("焦點", a.focus, Focus::Tree)));
    v.push(step("焦點：Tab 回到第一個窗格", |c| key(c, "", VK_TAB), |_, a| {
        expect("（焦點, 使用中窗格）", (a.focus, a.active_pane), (Focus::List, 0))
    }));
    v.push(step("窗格：Ctrl+Shift+W 關閉", |c| key(c, "cs", letter('w')), |_, a| expect("窗格數", a.panes.len(), 1)));
    v.push(step("樹狀列：Ctrl+B 隱藏", |c| key(c, "c", letter('b')), |_, a| expect("顯示", a.tree_visible, false)));
    v.push(step("樹狀列：Ctrl+B 顯示", |c| key(c, "c", letter('b')), |_, a| expect("顯示", a.tree_visible, true)));
    v.push(step("檢視：Ctrl+H 顯示隱藏檔", |c| key(c, "c", letter('h')), |_, a| {
        if names(a).iter().any(|n| n == ".hidden.txt") { Ok(()) } else { Err("沒有出現隱藏檔".into()) }
    }));
    v.push(step("檢視：Ctrl+H 再隱藏", |c| key(c, "c", letter('h')), |_, a| {
        if names(a).iter().any(|n| n == ".hidden.txt") { Err("隱藏檔仍在".into()) } else { Ok(()) }
    }));

    // ------------------------------------------------ 主題
    v.push(step("主題：Ctrl+Shift+T 切換為淺色", |c| key(c, "cs", letter('t')), |_, a| {
        expect("模式", a.theme.mode, Mode::Light)
    }));
    v.push(step("截圖：清單（淺色）", |_| {}, |c, _| {
        screenshot(c, "02-清單-淺色")?;
        let (dark, light) = (luma(c, "01-清單-深色"), luma(c, "02-清單-淺色"));
        if light > dark + 60.0 { Ok(()) } else { Err(format!("淺色畫面的亮度 {light:.0} 沒有明顯高於深色 {dark:.0}")) }
    }));
    v.push(step("主題：切回深色", |c| key(c, "cs", letter('t')), |_, a| expect("模式", a.theme.mode, Mode::Dark)));

    // ------------------------------------------------ 拖放（直接呼叫視窗的 IDropTarget）
    v.push(
        step(
            "拖放：來回經過兩個資料夾後放下（複製）",
            {
                let src = ctx.sub("拖放來源.txt");
                move |c| {
                    let (Some(pa), Some(pb)) = (row_pos("資料夾A"), row_pos("資料夾B")) else {
                        c.notes.push(("drop_error", "找不到資料夾列".into()));
                        return;
                    };
                    let over: Vec<_> = (0..12).map(|i| if i % 2 == 0 { pa } else { pb }).collect();
                    simulate_drop(c, &src, &over, pb, true);
                }
            },
            {
                let (src, copied) = (ctx.sub("拖放來源.txt"), path::join(&dir_b, "拖放來源.txt"));
                move |c, a| {
                    if let Some(e) = c.note("drop_error") {
                        return Err(e.to_string());
                    }
                    // 每次拖曳只解析一次各資料夾（快取）：兩個資料夾 → 2 次
                    expect("解析放置目標次數", c.note("resolves"), Some("2"))?;
                    expect("經過時標示資料夾列", c.note("hovered"), Some("11/11"))?;
                    expect("經過時的效果（複製）", c.note("over_effect"), Some("1"))?;
                    expect("放下後清除標示", a.drop_hover, None)?;
                    expect("回報給來源的效果（複製）", c.note("drop_effect"), Some("1"))?;
                    drop_fast(c)?;
                    if !exists(&copied) {
                        return Err("檔案沒有複製到資料夾B".into());
                    }
                    if exists(&src) { Ok(()) } else { Err("來源檔案不見了（應為複製）".into()) }
                }
            },
        )
        .slow(),
    );

    v.push(
        step(
            "拖放：同磁碟放下（移動）",
            {
                let src = ctx.sub("報告 9.txt");
                move |c| match row_pos("資料夾A") {
                    Some(pa) => simulate_drop(c, &src, &[pa, pa], pa, false),
                    None => c.notes.push(("drop_error", "找不到資料夾列".into())),
                }
            },
            {
                let (src, moved) = (ctx.sub("報告 9.txt"), path::join(&dir_a, "報告 9.txt"));
                move |c, a| {
                    if let Some(e) = c.note("drop_error") {
                        return Err(e.to_string());
                    }
                    // 最佳化移動：目標自己搬移，回報 DROPEFFECT_NONE，來源不會再刪除原檔
                    expect("回報給來源的效果（無）", c.note("drop_effect"), Some("0"))?;
                    drop_fast(c)?;
                    if !exists(&moved) || exists(&src) {
                        return Err("檔案沒有移到資料夾A".into());
                    }
                    if names(a).iter().any(|n| n == "報告 9.txt") { Err("清單沒有更新".into()) } else { Ok(()) }
                }
            },
        )
        .slow(),
    );

    // 狀態列錯誤與 UI 執行緒 I/O 防呆的回報在 finish() 統一檢查
    v.push(step("截圖：結束畫面", |_| {}, |c, _| screenshot(c, "03-結束")));
    v
}
