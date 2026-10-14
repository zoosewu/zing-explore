//! 自動化 UI 冒煙測試（tools/smoke.ps1 使用）。
//!
//! `zing --smoke-out=結果.json --smoke-dir=測試資料夾 [--smoke-shots=截圖資料夾]`
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

use zing_core::path;
use zing_core::folderview::ViewMode;
use zing_core::sort::{SortKey, SortSpec};

/// 1×1 的 PNG（資料夾封面、外部命令產生器的測試圖）
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41,
    0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];
use zing_core::theme::Mode;
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
    GetClientRect, ICON_BIG, KillTimer, SendMessageW, SetTimer, WM_CHAR, WM_GETICON, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_SETTEXT, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONUP,
};

use crate::app::{App, EditKind, Focus, TIMER_SMOKE};
use crate::model::Probe;
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
            let _allow = zing_core::guard::allow_io("冒煙測試的驗證");
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
        let _allow = zing_core::guard::allow_io("冒煙測試寫出報告");
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        let list = |v: &[String]| v.iter().map(|s| format!("\"{}\"", esc(s))).collect::<Vec<_>>().join(", ");
        let violations = zing_core::guard::take_violations();
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

/// Ctrl+滾輪（wParam 帶 MK_CONTROL）
fn ctrl_wheel(ctx: &Ctx, at: Option<(f32, f32)>, delta: i16) {
    let Some(at) = at else { return };
    let pt = screen_point(ctx.hwnd, at);
    let l = ((pt.y as u16 as isize) << 16) | pt.x as u16 as isize;
    send(ctx.hwnd, WM_MOUSEWHEEL, ((delta as u16 as usize) << 16) | MK_CONTROL.0 as usize, l);
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
        let r = a.item_rect(p, i)?;
        // 清單點名稱欄；縮圖點格子中央
        if a.grid(p, pl.list).is_some() {
            Some(((r.l + r.r) / 2.0, (r.t + r.b) / 2.0))
        } else {
            Some((pl.cols[0].0 + 40.0, (r.t + r.b) / 2.0))
        }
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

/// 命令面板第 `i` 個可見列的中心
fn palette_row_pos(i: usize) -> Option<(f32, f32)> {
    with_app(|a| a.palette_layout().and_then(|l| l.rows.get(i).map(|r| (r.l + 40.0, (r.t + r.b) / 2.0)))).flatten()
}

/// 命令面板目前排第一的命令
fn palette_top(a: &App) -> Option<String> {
    let p = a.palette.as_ref()?;
    p.matches.first().map(|&k| p.entries[k].cmd.clone())
}

fn palette_open(a: &App) -> Result<(), String> {
    match (&a.palette, &a.edit) {
        (Some(_), Some(e)) if matches!(e.kind, EditKind::Palette) => Ok(()),
        _ => Err("命令面板沒有開啟".into()),
    }
}

fn palette_closed(a: &App) -> Result<(), String> {
    if a.palette.is_some() || a.edit.is_some() { Err("命令面板沒有關閉".into()) } else { Ok(()) }
}

/// 在命令面板的輸入框按鍵
fn palette_key(vk: VIRTUAL_KEY) {
    if let Some(e) = edit_hwnd() {
        key_to(e, "", vk);
    }
}

/// 預覽處理常式的視窗是否顯示（另一條執行緒的視窗，IsWindowVisible 不會等待它）
fn host_visible(a: &App, want: bool) -> Result<(), String> {
    let visible = a.preview.host.window().is_some_and(|h| unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(h) }.as_bool());
    expect("主機視窗顯示", visible, want)
}

/// 主題編輯面板中某欄位的位置：`value` 為 true 時是數值處（點選項、輸入），否則是名稱處
fn theme_row_pos(label: &str, value: bool) -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let panel = a.layout.theme_panel?;
        let i = zing_core::theme::FIELDS.iter().position(|f| f.label == label)?;
        let k = crate::themeedit::rows().iter().position(|r| *r == crate::themeedit::Row::Field(i))?;
        let pl = a.theme_panel_layout(panel);
        // 先把這一列捲進可見範圍
        let rh = a.row_h();
        let ed = a.theme_editor.as_mut()?;
        let top = k as f32 * rh;
        if top < ed.scroll || top + rh > ed.scroll + pl.list.h() {
            ed.scroll = (top - pl.list.h() / 2.0).max(0.0);
        }
        let scroll = ed.scroll;
        a.invalidate();
        let (row, swatch, v) = a.theme_row_rects(pl.list, k, scroll);
        let y = (row.t + row.b) / 2.0;
        Some(if value { (if swatch.r < v.r { swatch.r + 30.0 } else { v.l + 20.0 }, y) } else { (row.l + 20.0, y) })
    })
    .flatten()
}

fn theme_button_pos(k: usize) -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let panel = a.layout.theme_panel?;
        let b = a.theme_panel_layout(panel).buttons[k];
        Some(((b.l + b.r) / 2.0, (b.t + b.b) / 2.0))
    })
    .flatten()
}

/// 直接執行命令腳本
fn exec(cmd: &str) {
    with_app(|a| a.exec(cmd));
}

fn theme_path(name: &str) -> PathBuf {
    crate::settings::themes_dir().join(format!("{name}.toml"))
}

/// 批次重新命名的兩個輸入框
fn batch_edits() -> (HWND, HWND) {
    with_app(|a| a.batch.as_ref().map(|b| (b.edits[0], b.edits[1]))).flatten().unwrap_or_default()
}

/// 直接設定輸入框內容（會送出 EN_CHANGE）
fn set_text(h: HWND, text: &str) {
    let w = zing_core::text::wide_z(text);
    unsafe {
        SendMessageW(h, WM_SETTEXT, None, Some(LPARAM(w.as_ptr() as isize)));
    }
}

fn palette_type(text: &str) {
    if let Some(e) = edit_hwnd() {
        type_to(e, text);
    }
}

fn list_bottom() -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let pl = &a.layout.panes[a.active_pane];
        ((pl.list.l + pl.list.r) / 2.0, pl.list.b - 10.0)
    })
}

fn list_center() -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let pl = &a.layout.panes[a.active_pane];
        ((pl.list.l + pl.list.r) / 2.0, (pl.list.t + pl.list.b) / 2.0)
    })
}

/// 樹狀列第 `i` 個節點的展開箭頭位置（視窗座標，DIP）
fn tree_arrow_pos(a: &App, i: usize) -> Option<(f32, f32)> {
    let tr = a.layout.tree?;
    let rh = a.row_h();
    let y = tr.t + 4.0 + i as f32 * rh - a.tree.scroll + rh / 2.0;
    if y < tr.t || y > tr.b {
        return None;
    }
    Some((tr.l + 6.0 + a.tree.nodes[i].depth as f32 * 14.0 + 9.0, y))
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
    // 資料物件在真實拖放中由拖曳來源（其他程式）建立，不算 zing-explore 的 UI 執行緒 I/O
    let data = {
        let _allow = zing_core::guard::allow_io("冒煙測試扮演拖曳來源");
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
/// 拖放來源.txt、.hidden.txt（隱藏）、.jobs\大檔案.bin（隱藏資料夾，384 MB）
const VISIBLE: usize = 7;
const B_FILES: usize = 200;
const BIG_BYTES: u64 = 384 * 1024 * 1024;

fn last_job(a: &App) -> Option<&crate::jobs::Job> {
    a.jobs.list.last()
}

fn job_status(a: &App) -> Option<crate::jobs::JobStatus> {
    last_job(a).map(|j| j.status.clone())
}

fn file_size(p: &str) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}

/// 點最後一個工作的暫停（`cancel` 為 false）或取消按鈕
fn click_job_button(c: &Ctx, cancel: bool) {
    let pos = with_app(|a| last_job(a).map(|j| j.id).and_then(|id| a.job_button_pos(id, cancel))).flatten();
    click(c, "", pos);
}

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
    v.push(step("視窗圖示（第一幀之後載入）", |_| {}, |c, _| {
        let icon = unsafe { SendMessageW(c.hwnd, WM_GETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(0))) };
        if icon.0 != 0 { Ok(()) } else { Err("沒有設定視窗圖示".into()) }
    }));
    // ------------------------------------------------ 資料夾大小
    v.push(step("資料夾大小：Ctrl+Shift+S 計算清單中的資料夾", |c| key(c, "cs", letter('s')), {
        let d = d.clone();
        move |_, a| {
            expect("計算中", a.sizes.pending, 0)?;
            let k = crate::sizes::key(&d);
            let get = |n: &str| a.sizes.get(&k, n).map(|i| (i.state, i.size.bytes, i.size.files));
            use crate::sizes::SizeState::Fresh;
            // 資料夾A 的內層.txt 是 "inner" 加換行（7 bytes）；資料夾B 是 200 個空檔案
            expect("資料夾A", get("資料夾A"), Some((Fresh, 7, 1)))?;
            expect("資料夾B", get("資料夾B"), Some((Fresh, 0, 200)))?;
            expect("顯示", a.folder_size_text(&k, "資料夾A").map(|t| t.0), Some("7 B".to_string()))
        }
    }));
    v.push(step("資料夾大小：依大小遞減排序（資料夾也依大小）", |c| key(c, "c", VIRTUAL_KEY(b'4' as u16)), |_, a| {
        let s = a.tab().spec;
        expect("排序", (s.key, s.ascending), (SortKey::Size, false))?;
        expect("前兩項", names(a)[..2].to_vec(), vec!["資料夾A".to_string(), "資料夾B".to_string()])
    }));
    v.push(step("資料夾大小：依大小遞增排序", |c| key(c, "c", VIRTUAL_KEY(b'4' as u16)), |_, a| {
        expect("前兩項", names(a)[..2].to_vec(), vec!["資料夾B".to_string(), "資料夾A".to_string()])
    }));
    v.push(step("資料夾大小：Ctrl+1 回到名稱排序", |c| key(c, "c", VIRTUAL_KEY(b'1' as u16)), |_, a| {
        let s = a.tab().spec;
        expect("排序", (s.key, s.ascending), (SortKey::Name, true))
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

    // ------------------------------------------------ 命令面板
    v.push(step("命令面板：Ctrl+Shift+P 開啟", |c| key(c, "cs", letter('p')), |_, a| {
        palette_open(a)?;
        let n = a.palette.as_ref().map_or(0, |p| p.matches.len());
        if n >= zing_core::palette::BUILTIN.len() { Ok(()) } else { Err(format!("只列出 {n} 個命令")) }
    }));
    v.push(step("命令面板：輸入文字篩選", |_| palette_type("隱藏檔"), |_, a| {
        expect("第一項", palette_top(a).as_deref(), Some("view.togglehidden"))
    }));
    v.push(step("截圖：命令面板", |_| {}, |c, _| screenshot(c, "04-命令面板")));
    v.push(step("命令面板：Enter 執行並關閉", |_| palette_key(VK_RETURN), |_, a| {
        palette_closed(a)?;
        expect("顯示隱藏檔", a.show_hidden, true)
    }));
    v.push(step("命令面板：最近使用的排第一", |c| key(c, "cs", letter('p')), |_, a| {
        palette_open(a)?;
        expect("第一項", palette_top(a).as_deref(), Some("view.togglehidden"))
    }));
    v.push(step("命令面板：再執行一次", |_| palette_key(VK_RETURN), |_, a| {
        palette_closed(a)?;
        expect("顯示隱藏檔", a.show_hidden, false)
    }));
    v.push(step(
        "命令面板：↓↓ 移動選取",
        |c| {
            key(c, "cs", letter('p'));
            palette_key(VK_DOWN);
            palette_key(VK_DOWN);
        },
        |_, a| expect("選取", a.palette.as_ref().map(|p| p.sel), Some(2)),
    ));
    v.push(step("命令面板：Esc 關閉", |_| palette_key(VK_ESCAPE), |_, a| palette_closed(a)));
    v.push(step(
        "命令面板：> 直接執行命令腳本",
        {
            let dir_a = dir_a.clone();
            move |c| {
                key(c, "cs", letter('p'));
                palette_type(&format!(">nav.goto \"{dir_a}\""));
                palette_key(VK_RETURN);
            }
        },
        {
            let dir_a = dir_a.clone();
            move |_, a| {
                palette_closed(a)?;
                at(a, &dir_a)
            }
        },
    ));
    v.push(step("命令面板：回到測試資料夾", |c| key(c, "a", VK_LEFT), {
        let d = d.clone();
        move |_, a| at(a, &d)
    }));
    v.push(step(
        "命令面板：滑鼠點選項目執行",
        |c| {
            key(c, "cs", letter('p'));
            palette_type("新增分頁");
            click(c, "", palette_row_pos(0));
        },
        |_, a| {
            palette_closed(a)?;
            expect("分頁數", a.panes[a.active_pane].tabs.len(), 2)
        },
    ));
    v.push(step("命令面板：關閉新分頁", |c| key(c, "c", letter('w')), |_, a| expect("分頁數", a.panes[a.active_pane].tabs.len(), 1)));
    v.push(step("滑鼠：點選一列（命令面板測試前置）", |c| click(c, "", row_pos("b.txt")), |_, a| {
        expect("選取", selected(a), vec!["b.txt".to_string()])
    }));
    v.push(step(
        "命令面板：點面板外關閉，不影響底下",
        |c| {
            key(c, "cs", letter('p'));
            // 清單底部的空白處（面板下方）：沒有面板時點這裡會取消選取
            click(c, "", list_bottom());
        },
        |_, a| {
            palette_closed(a)?;
            expect("選取", selected(a), vec!["b.txt".to_string()])
        },
    ));
    // 命令開啟自己的編輯框時，面板輸入框失去焦點的通知不可把它取消
    v.push(step(
        "命令面板：執行重新命名，編輯框保持開啟",
        |c| {
            key(c, "cs", letter('p'));
            palette_type("重新命名");
            palette_key(VK_RETURN);
        },
        |c, a| {
            if c.started.elapsed() < Duration::from_millis(300) {
                return Err("等待".into());
            }
            match &a.edit {
                Some(e) if matches!(&e.kind, EditKind::Rename { path, .. } if path.ends_with("b.txt")) => Ok(()),
                _ => Err("重新命名框沒有開啟或被取消".into()),
            }
        },
    ));
    v.push(step("命令面板：取消重新命名", |_| palette_key(VK_ESCAPE), |_, a| {
        if a.edit.is_none() { Ok(()) } else { Err("編輯框沒有關閉".into()) }
    }));

    // ------------------------------------------------ 檔案作業佇列
    let big = ctx.sub(".jobs\\大檔案.bin");
    v.push(step(
        "檔案作業：小作業完成且不顯示面板",
        {
            let (src, dir_b) = (ctx.sub("a2.txt"), dir_b.clone());
            move |_| {
                with_app(|a| a.exec(&format!("copy \"{src}\" --to=\"{dir_b}\"")));
            }
        },
        {
            let copied = path::join(&dir_b, "a2.txt");
            move |_, a| {
                expect("狀態", job_status(a), Some(crate::jobs::JobStatus::Done))?;
                expect("面板", a.jobs.visible, false)?;
                if exists(&copied) { Ok(()) } else { Err("沒有複製".into()) }
            }
        },
    ));
    v.push(
        step(
            "檔案作業：複製大檔案並暫停",
            {
                let (big, dir_a) = (big.clone(), dir_a.clone());
                move |c| {
                    with_app(|a| a.exec(&format!("copy \"{big}\" --to=\"{dir_a}\"")));
                    key(c, "c", letter('j'));
                    click_job_button(c, false);
                }
            },
            |_, a| {
                expect("面板", a.jobs.visible, true)?;
                expect("狀態", job_status(a), Some(crate::jobs::JobStatus::Paused))?;
                let f = last_job(a).map_or(1.0, |j| j.fraction);
                if f < 1.0 { Ok(()) } else { Err("暫停前就完成了".into()) }
            },
        )
        .slow(),
    );
    v.push(step("截圖：檔案作業（暫停）", |_| {}, |c, _| screenshot(c, "06-檔案作業")));
    v.push(
        step(
            "檔案作業：暫停時仍可重新命名（不排在後面）",
            |c| {
                click(c, "", row_pos("b.txt"));
                key(c, "", VK_F2);
                edit_commit("b2");
            },
            {
                let (old, new) = (ctx.sub("b.txt"), ctx.sub("b2.txt"));
                move |_, a| {
                    if !exists(&new) || exists(&old) {
                        return Err("沒有改名".into());
                    }
                    expect("大檔案的工作", job_status(a), Some(crate::jobs::JobStatus::Paused))
                }
            },
        )
        .slow(),
    );
    v.push(
        step("檔案作業：繼續後完成", |c| click_job_button(c, false), {
            let (copied, d) = (path::join(&dir_a, "大檔案.bin"), d.clone());
            move |_, a| {
                expect("狀態", job_status(a), Some(crate::jobs::JobStatus::Done))?;
                expect("大小", file_size(&copied), Some(BIG_BYTES))?;
                // 複製進資料夾A 後，先前算出的大小標為可能過時
                let shown = a.folder_size_text(&crate::sizes::key(&d), "資料夾A").map(|t| t.0);
                if shown.as_deref().is_some_and(|t| t.starts_with('≈')) { Ok(()) } else { Err(format!("資料夾A 顯示 {shown:?}")) }
            }
        })
        .slow(),
    );
    v.push(
        step(
            "檔案作業：開始複製第二個",
            {
                let (big, dir_b) = (big.clone(), dir_b.clone());
                move |_| {
                    // 放慢複製，才能確定在中途取消
                    crate::shell::TEST_PROGRESS_DELAY_MS.store(20, std::sync::atomic::Ordering::Relaxed);
                    with_app(|a| a.exec(&format!("copy \"{big}\" --to=\"{dir_b}\"")));
                }
            },
            |_, a| {
                let j = last_job(a).ok_or("沒有工作")?;
                if j.fraction > 0.02 && j.fraction < 0.9 { Ok(()) } else { Err(format!("進度 {:.2}", j.fraction)) }
            },
        )
        .slow(),
    );
    v.push(
        step("檔案作業：取消進行中的複製", |c| click_job_button(c, true), {
            let copied = path::join(&dir_b, "大檔案.bin");
            move |_, a| {
                expect("狀態", job_status(a), Some(crate::jobs::JobStatus::Cancelled))?;
                crate::shell::TEST_PROGRESS_DELAY_MS.store(0, std::sync::atomic::Ordering::Relaxed);
                // 取消後 Shell 會刪掉複製到一半的檔案
                if exists(&copied) { Err("留下複製到一半的檔案".into()) } else { Ok(()) }
            }
        })
        .slow(),
    );
    v.push(
        step("檔案作業：全部結束後自動收起", |_| {}, |_, a| {
            if a.jobs.list.is_empty() && !a.jobs.visible { Ok(()) } else { Err(format!("還有 {} 個工作", a.jobs.list.len())) }
        })
        .slow(),
    );

    v.push(step(
        "命令面板：設定 position = center 時置中",
        |c| {
            with_app(|a| a.config.palette.position = zing_core::config::PalettePosition::Center);
            key(c, "cs", letter('p'));
        },
        |c, a| {
            palette_open(a)?;
            let lay = a.palette_layout().ok_or("沒有版面")?;
            let (_, h) = a.gfx.size();
            // 以填滿時的高度置中：輸入框在視窗上半部、面板（滿版時）上下留白相同
            let full_b = lay.input.b + 6.0 + crate::palette::MAX_ROWS as f32 * a.row_h() + 8.0;
            let (top_gap, bottom_gap) = (lay.card.t, h - full_b);
            if (top_gap - bottom_gap).abs() > 2.0 {
                return Err(format!("上方留白 {top_gap:.0}，下方 {bottom_gap:.0}"));
            }
            c.notes.push(("palette_input_t", lay.input.t.to_string()));
            Ok(())
        },
    ));
    v.push(step("命令面板：篩選後輸入框不移動", |_| palette_type("重新整理"), |c, a| {
        let lay = a.palette_layout().ok_or("沒有版面")?;
        expect("輸入框位置", Some(lay.input.t.to_string()).as_deref(), c.note("palette_input_t"))?;
        expect("結果列數", lay.rows.len(), 1)
    }));
    v.push(step("截圖：命令面板（置中）", |_| {}, |c, _| screenshot(c, "05-命令面板-置中")));
    v.push(step(
        "命令面板：關閉並還原設定",
        |_| {
            palette_key(VK_ESCAPE);
            with_app(|a| a.config.palette.position = zing_core::config::PalettePosition::Top);
        },
        |_, a| palette_closed(a),
    ));

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

    // ------------------------------------------------ 樹狀列
    v.push(step(
        "樹狀列：可見的節點已預先載入（點擊前就知道能否展開）",
        {
            let d = d.clone();
            move |c| {
                // 前面的步驟進入過子資料夾，目前資料夾的節點已展開：先收合
                let pos = with_app(|a| {
                    a.tree.find(&d).filter(|&i| a.tree.nodes[i].expanded).and_then(|i| tree_arrow_pos(a, i))
                })
                .flatten();
                click(c, "", pos);
            }
        },
        {
            let d = d.clone();
            move |_, a| {
                let i = a.tree.find(&d).ok_or("樹狀列沒有目前資料夾")?;
                let n = &a.tree.nodes[i];
                expect("收合", n.expanded, false)?;
                expect("預先載入", n.probe, Probe::Done)?;
                let names = n.prefetched.as_ref().map(|c| c.names.clone()).unwrap_or_default();
                if !names.iter().any(|x| x == "資料夾A") || !names.iter().any(|x| x == "資料夾B") {
                    return Err(format!("預先載入的子資料夾：{names:?}"));
                }
                if names.iter().any(|x| x == ".jobs") {
                    return Err("包含隱藏資料夾".into());
                }
                expect("箭頭", n.arrow(), true)
            }
        },
    ));
    v.push(step(
        "樹狀列：點箭頭立即展開（不等待列舉）",
        {
            let d = d.clone();
            move |c| {
                let pos = with_app(|a| a.tree.find(&d).and_then(|i| tree_arrow_pos(a, i))).flatten();
                click(c, "", pos);
                // 點擊的訊息處理完時就應該已經展開
                let now = with_app(|a| a.tree.find(&d).is_some_and(|i| a.tree.nodes[i].expanded)).unwrap_or(false);
                c.notes.push(("tree_expand_sync", now.to_string()));
            }
        },
        {
            let (d, dir_a) = (d.clone(), dir_a.clone());
            move |c, a| {
                expect("點擊時已展開", c.note("tree_expand_sync"), Some("true"))?;
                let i = a.tree.find(&d).ok_or("找不到節點")?;
                expect("展開", a.tree.nodes[i].expanded, true)?;
                let k = a.tree.find(&dir_a).ok_or("沒有 資料夾A 子節點")?;
                expect("子節點深度", a.tree.nodes[k].depth, a.tree.nodes[i].depth + 1)
            }
        },
    ));
    v.push(step("樹狀列：沒有子資料夾的節點不顯示箭頭", |_| {}, {
        let dir_a = dir_a.clone();
        move |_, a| {
            let k = a.tree.find(&dir_a).ok_or("沒有 資料夾A 子節點")?;
            let n = &a.tree.nodes[k];
            expect("預先載入", n.probe, Probe::Done)?;
            expect("有子資料夾", n.subdirs, Some(false))?;
            expect("箭頭", n.arrow(), false)
        }
    }));
    v.push(step(
        "樹狀列：再點一次收合",
        {
            let d = d.clone();
            move |c| {
                let pos = with_app(|a| a.tree.find(&d).and_then(|i| tree_arrow_pos(a, i))).flatten();
                click(c, "", pos);
            }
        },
        {
            let (d, dir_a) = (d.clone(), dir_a.clone());
            move |_, a| {
                let i = a.tree.find(&d).ok_or("找不到節點")?;
                expect("收合", a.tree.nodes[i].expanded, false)?;
                expect("子節點", a.tree.find(&dir_a), None)
            }
        },
    ));

    // ------------------------------------------------ 每個資料夾的檢視設定
    v.push(step(
        "資料夾檢視：Ctrl+2 依修改日期排序後進入資料夾A（資料夾A 用預設排序）",
        {
            let dir_a = dir_a.clone();
            move |c| {
                key(c, "c", VIRTUAL_KEY(b'2' as u16));
                with_app(|a| {
                    let p = a.active_pane;
                    a.navigate(p, dir_a.clone(), true);
                });
            }
        },
        {
            let (d, dir_a) = (d.clone(), dir_a.clone());
            move |_, a| {
                at(a, &dir_a)?;
                expect("資料夾A 的排序", a.tab().spec, SortSpec::default())?;
                expect("記住測試資料夾的排序", a.folder_views.get(&d).sort, SortSpec { key: SortKey::Modified, ascending: false })
            }
        },
    ));
    v.push(step("資料夾檢視：回上一頁時恢復記住的排序", |_| {
        with_app(|a| a.go_back());
    }, {
        let d = d.clone();
        move |_, a| {
            at(a, &d)?;
            expect("排序", a.tab().spec, SortSpec { key: SortKey::Modified, ascending: false })
        }
    }));
    v.push(step("資料夾檢視：Ctrl+1 改回名稱排序（與預設相同，不再保存）", |c| key(c, "c", VIRTUAL_KEY(b'1' as u16)), |_, a| {
        expect("排序", a.tab().spec, SortSpec::default())?;
        expect("記錄", a.folder_views.is_empty(), true)
    }));

    // ------------------------------------------------ 縮圖檢視
    v.push(step("縮圖：Ctrl+Shift+3 切換為中縮圖（沒有欄位標題）", |c| key(c, "cs", VIRTUAL_KEY(b'3' as u16)), |_, a| {
        let t = a.tab();
        expect("檢視", (t.mode, t.thumb), (ViewMode::Thumbnails, 128))?;
        let pl = &a.layout.panes[a.active_pane];
        expect("欄位標題高度", pl.header.h(), 0.0)?;
        let g = a.grid(a.active_pane, pl.list).ok_or("沒有格狀版面")?;
        if g.cols < 2 { Err(format!("欄數 {}", g.cols)) } else { Ok(()) }
    }));
    v.push(
        step("縮圖：可見的項目都取得縮圖", |_| {}, |_, a| {
            let n = a.tab().view.len();
            let ready = a.thumbs.ready_count();
            if ready >= n { Ok(()) } else { Err(format!("{ready}/{n} 個縮圖就緒")) }
        })
        .slow(),
    );
    v.push(step("截圖：縮圖檢視", |_| {}, |c, _| screenshot(c, "07-縮圖")));
    v.push(step(
        "縮圖：點格子選取",
        |c| {
            let pos = with_app(|a| {
                a.layout = a.compute_layout();
                a.item_rect(a.active_pane, 2).map(|r| ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0))
            })
            .flatten();
            click(c, "", pos);
        },
        |_, a| {
            let want = names(a)[2].clone();
            expect("選取", selected(a), vec![want])
        },
    ));
    v.push(step(
        "縮圖：方向鍵以格子移動（→ 一格、↓ 一列）",
        |c| {
            key(c, "", VK_HOME);
            key(c, "", VK_RIGHT);
            key(c, "", VK_DOWN);
        },
        |_, a| {
            let p = a.active_pane;
            let cols = a.grid(p, a.layout.panes[p].list).map_or(1, |g| g.cols);
            let want = (1 + cols).min(a.tab().view.len() - 1);
            expect("焦點", a.tab().focus, want)
        },
    ));
    v.push(step("縮圖：Ctrl+滾輪縮小一級", |c| ctrl_wheel(c, list_center(), -120), |_, a| {
        expect("縮圖大小", a.tab().thumb, 128 - crate::views::THUMB_STEP as u16)
    }));
    v.push(
        step(
            "縮圖：資料夾裡有 cover.png 時用它當縮圖",
            {
                let d = d.clone();
                move |_| {
                    let dir = path::join(&d, "封面測試");
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(path::join(&dir, "cover.png"), TINY_PNG);
                }
            },
            {
                let d = d.clone();
                move |_, a| {
                    let px = crate::thumbs::bucket(a.tab().thumb as f32 * a.gfx.scale());
                    let p = path::join(&d, "封面測試");
                    expect("來源", a.thumbs.source(&p, px), Some(crate::thumbs::Source::Cover))
                }
            },
        )
        .slow(),
    );
    v.push(
        step(
            "縮圖：外部命令產生器（設定 [[thumbnailer]]）",
            {
                let d = d.clone();
                move |_| {
                    let cover = path::join(&path::join(&d, "封面測試"), "cover.png");
                    let t = zing_core::config::Thumbnailer {
                        pattern: "*.zt".into(),
                        cmd: format!(r#"cmd /c copy /y "{cover}" "{{out}}""#),
                    };
                    with_app(|a| {
                        a.config.thumbnailers = vec![t.clone()];
                        a.thumbs.set_thumbnailers(vec![t]);
                    });
                    let _ = std::fs::write(path::join(&d, "外部產生.zt"), b"x");
                }
            },
            {
                let d = d.clone();
                move |_, a| {
                    let px = crate::thumbs::bucket(a.tab().thumb as f32 * a.gfx.scale());
                    let p = path::join(&d, "外部產生.zt");
                    expect("來源", a.thumbs.source(&p, px), Some(crate::thumbs::Source::Command))
                }
            },
        )
        .slow(),
    );
    v.push(step(
        "縮圖：Ctrl+Shift+1 回到清單（並清掉測試檔案）",
        {
            let d = d.clone();
            move |c| {
                let _ = std::fs::remove_dir_all(path::join(&d, "封面測試"));
                let _ = std::fs::remove_file(path::join(&d, "外部產生.zt"));
                with_app(|a| {
                    a.config.thumbnailers.clear();
                    a.thumbs.set_thumbnailers(Vec::new());
                });
                // 隔離模式下外部命令的縮圖放在暫存資料夾
                let _ = std::fs::remove_dir_all(crate::thumbs::cache_dir(true));
                key(c, "cs", VIRTUAL_KEY(b'1' as u16));
            }
        },
        |_, a| {
            expect("檢視", a.tab().mode, ViewMode::List)?;
            if a.layout.panes[a.active_pane].header.h() > 0.0 { Ok(()) } else { Err("沒有欄位標題".into()) }
        },
    ));

    // ------------------------------------------------ 預覽窗格
    v.push(step("預覽：Alt+P 開啟預覽窗格", |c| key(c, "a", letter('p')), |_, a| {
        expect("顯示", a.preview.visible, true)?;
        if a.layout.preview.is_some() { Ok(()) } else { Err("版面沒有預覽窗格".into()) }
    }));
    v.push(
        step("預覽：文字檔顯示內容與屬性", |c| click(c, "", row_pos("報告 10.txt")), {
            let d = d.clone();
            move |_, a| {
                let ld = a.preview.loaded.as_ref().ok_or("還沒有內容")?;
                expect("路徑", ld.path.as_str(), path::join(&d, "報告 10.txt").as_str())?;
                match &ld.body {
                    crate::preview::Body::Text { lines, .. } => expect("第一行", lines.first().map(String::as_str), Some("報告 10.txt"))?,
                    _ => return Err("不是文字預覽".into()),
                }
                if ld.props.is_empty() { Err("沒有屬性".into()) } else { Ok(()) }
            }
        })
        .slow(),
    );
    v.push(
        step("預覽：資料夾顯示項目數", |c| click(c, "", row_pos("資料夾A")), |_, a| {
            let ld = a.preview.loaded.as_ref().ok_or("還沒有內容")?;
            if !ld.path.ends_with("資料夾A") {
                return Err(format!("路徑 {}", ld.path));
            }
            match &ld.body {
                crate::preview::Body::Folder { items: Some(n), .. } if *n > 0 => Ok(()),
                _ => Err("沒有項目數".into()),
            }
        })
        .slow(),
    );
    v.push(
        step(
            "預覽：新增圖片檔（清單自動更新）",
            {
                let d = d.clone();
                // 資料夾監看會讓清單自動重新整理
                move |_| {
                    let _ = std::fs::write(path::join(&d, "預覽圖.png"), TINY_PNG);
                }
            },
            |_, a| {
                if a.tab().find_view("預覽圖.png").is_none() {
                    return Err("清單還沒有預覽圖.png".into());
                }
                Ok(())
            },
        )
        .slow(),
    );
    v.push(
        step("預覽：點選圖片", |c| click(c, "", row_pos("預覽圖.png")), |_, a| {
            let ld = a.preview.loaded.as_ref().ok_or("還沒有內容")?;
            if !ld.path.ends_with("預覽圖.png") {
                return Err(format!("路徑 {}", ld.path));
            }
            match &ld.body {
                crate::preview::Body::Image(Some(_)) => Ok(()),
                _ => Err("沒有圖片".into()),
            }
        })
        .slow(),
    );
    v.push(step("截圖：預覽窗格", |_| {}, |c, _| screenshot(c, "08-預覽")));
    v.push(step(
        "預覽：Alt+P 關閉",
        {
            let d = d.clone();
            move |c| {
                key(c, "a", letter('p'));
                let _ = std::fs::remove_file(path::join(&d, "預覽圖.png"));
            }
        },
        |_, a| {
            expect("顯示", a.preview.visible, false)?;
            if a.layout.preview.is_none() { Ok(()) } else { Err("版面仍有預覽窗格".into()) }
        },
    ));

    // ------------------------------------------------ 批次重新命名
    let batch_dir = ctx.sub("批次測試");
    v.push(step(
        "批次：建立測試資料夾並進入",
        {
            let bd = batch_dir.clone();
            move |_| {
                let _ = std::fs::create_dir_all(&bd);
                for n in ["x1.txt", "x2.txt", "y.txt"] {
                    let _ = std::fs::write(path::join(&bd, n), n);
                }
                with_app(|a| {
                    let p = a.active_pane;
                    a.navigate(p, bd.clone(), true);
                });
            }
        },
        {
            let bd = batch_dir.clone();
            move |_, a| {
                at(a, &bd)?;
                expect("項目數", names(a).len(), 3)
            }
        },
    ));
    v.push(step(
        "批次：全選後 F2 開啟批次重新命名",
        |c| {
            key(c, "c", letter('a'));
            key(c, "", VK_F2);
        },
        |_, a| {
            let b = a.batch.as_ref().ok_or("面板沒有開啟")?;
            expect("項目數", b.items.len(), 3)?;
            expect("單檔重新命名", a.edit.is_none(), true)
        },
    ));
    v.push(step(
        "批次：規則運算式＋編號的預覽",
        |_| {
            let (e0, e1) = batch_edits();
            key_to(e0, "a", letter('r'));
            type_to(e0, r"^x(\d)$");
            key_to(e0, "", VK_TAB);
            type_to(e1, "相片 {n:2}");
        },
        |_, a| {
            let b = a.batch.as_ref().ok_or("面板沒有開啟")?;
            expect("規則運算式", b.rule.regex, true)?;
            expect("焦點欄位", b.field, 1)?;
            let news: Vec<String> = b.plan.as_ref().map_err(|e| e.clone())?.iter().map(|p| p.new.clone()).collect();
            expect("新名稱", news, vec!["相片 01.txt".to_string(), "相片 02.txt".into(), "y.txt".into()])
        },
    ));
    v.push(step("截圖：批次重新命名", |_| {}, |c, _| screenshot(c, "09-批次重新命名")));
    v.push(step(
        "批次：新名稱重複時不能執行",
        |_| {
            let (_, e1) = batch_edits();
            set_text(e1, "same");
            key_to(e1, "", VK_RETURN);
        },
        |_, a| {
            let b = a.batch.as_ref().ok_or("面板被關閉了")?;
            expect("擋下", b.blocked, true)?;
            expect("衝突數", b.counts().1, 2)
        },
    ));
    v.push(
        step(
            "批次：Enter 執行改名",
            |_| {
                let (_, e1) = batch_edits();
                set_text(e1, "相片 {n:2}");
                key_to(e1, "", VK_RETURN);
            },
            {
                let bd = batch_dir.clone();
                move |_, a| {
                    expect("面板關閉", a.batch.is_none(), true)?;
                    let has = |n: &str| std::path::Path::new(&path::join(&bd, n)).exists();
                    if !(has("相片 01.txt") && has("相片 02.txt") && has("y.txt") && !has("x1.txt")) {
                        return Err("檔案沒有改名".into());
                    }
                    // 清單也要更新完成，下一步才會選到新名稱
                    if a.tab().find_view("相片 01.txt").is_some() && a.tab().find_view("x1.txt").is_none() {
                        Ok(())
                    } else {
                        Err("清單還沒更新".into())
                    }
                }
            },
        )
        .slow(),
    );
    v.push(
        step(
            "批次：新名稱與舊名稱重疊時經過暫時名稱（01→02、02→03）",
            |c| {
                key(c, "c", letter('a'));
                key(c, "", VK_F2);
                let (e0, e1) = batch_edits();
                key_to(e0, "a", letter('r'));
                type_to(e0, r"^相片 \d+$");
                type_to(e1, "相片 {n:2:2}");
                key_to(e1, "", VK_RETURN);
            },
            {
                let bd = batch_dir.clone();
                move |_, a| {
                    expect("面板關閉", a.batch.is_none(), true)?;
                    let has = |n: &str| std::path::Path::new(&path::join(&bd, n)).exists();
                    let leftovers = std::fs::read_dir(&bd).map(|r| r.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("~zing-")).count()).unwrap_or(0);
                    expect("暫時檔", leftovers, 0)?;
                    if has("相片 02.txt") && has("相片 03.txt") && !has("相片 01.txt") && has("y.txt") {
                        Ok(())
                    } else {
                        Err("連鎖改名不正確".into())
                    }
                }
            },
        )
        .slow(),
    );
    v.push(step(
        "批次：Esc 取消",
        |c| {
            key(c, "c", letter('a'));
            key(c, "", VK_F2);
            let (e0, _) = batch_edits();
            key_to(e0, "", VK_ESCAPE);
        },
        |_, a| expect("面板關閉", a.batch.is_none(), true),
    ));
    v.push(step(
        "批次：回到測試資料夾並清除",
        {
            let (d, bd) = (d.clone(), batch_dir.clone());
            move |_| {
                with_app(|a| {
                    let p = a.active_pane;
                    a.navigate(p, d.clone(), true);
                });
                let _ = std::fs::remove_dir_all(&bd);
            }
        },
        {
            let d = d.clone();
            move |_, a| at(a, &d)
        },
    ));

    // ------------------------------------------------ 主題編輯器
    v.push(step("主題：開啟編輯面板", |_| exec("theme.edit"), |_, a| {
        if a.theme_editor.is_none() || a.layout.theme_panel.is_none() {
            return Err("面板沒有開啟".into());
        }
        expect("目前主題", a.theme_name.as_str(), "dark")
    }));
    v.push(step("主題：滾輪調整列高（即時套用）", |c| wheel(c, theme_row_pos("列高", false), 120), |_, a| {
        expect("列高", a.theme.metrics.row_height, 23.0)
    }));
    v.push(step("主題：點選項切換網址列樣式", |c| click(c, "", theme_row_pos("網址列", true)), |_, a| {
        expect("網址列", a.theme.chrome.address, zing_core::theme::AddressStyle::Breadcrumb)
    }));
    v.push(step(
        "主題：輸入強調色",
        |c| {
            click(c, "", theme_row_pos("強調色", true));
            if let Some(e) = edit_hwnd() {
                set_text(e, "#ff0000");
                key_to(e, "", VK_RETURN);
            }
        },
        |_, a| expect("強調色", a.theme.color.accent, zing_core::theme::Color::rgb(0xff0000)),
    ));
    v.push(step("截圖：主題編輯器", |_| {}, |c, _| screenshot(c, "10-主題編輯器")));
    v.push(
        step(
            "主題：另存新主題（只存差異）",
            |c| {
                click(c, "", theme_button_pos(0));
                if let Some(e) = edit_hwnd() {
                    set_text(e, "smoke-theme");
                    key_to(e, "", VK_RETURN);
                }
            },
            |_, a| {
                expect("主題名稱", a.theme_name.as_str(), "smoke-theme")?;
                let text = std::fs::read_to_string(theme_path("smoke-theme")).map_err(|e| e.to_string())?;
                if !text.contains("#ff0000") || !text.contains("row_height") || text.contains("background") {
                    return Err(format!("主題檔內容：{text}"));
                }
                Ok(())
            },
        )
        .slow(),
    );
    v.push(step(
        "主題：取消時還原到上次儲存",
        |c| {
            wheel(c, theme_row_pos("列高", false), 120);
            click(c, "", theme_button_pos(2));
        },
        |_, a| {
            expect("面板關閉", a.theme_editor.is_none(), true)?;
            expect("列高", a.theme.metrics.row_height, 23.0)
        },
    ));
    v.push(
        step(
            "主題：匯入 .zingtheme",
            |_| {
                let f = std::env::temp_dir().join("smoke-import.zingtheme");
                let _ = std::fs::write(&f, "name = \"smoke-import\"\nstyle = \"fluent\"\n[color]\naccent = \"#00ff00\"\n");
                with_app(|a| a.import_theme_file(f.to_string_lossy().into_owned()));
            },
            |_, a| {
                expect("主題名稱", a.theme_name.as_str(), "smoke-import")?;
                expect("強調色", a.theme.color.accent, zing_core::theme::Color::rgb(0x00ff00))?;
                expect("風格", a.theme.style, zing_core::theme::Style::Fluent)
            },
        )
        .slow(),
    );
    v.push(step("主題：開始監看主題檔（記下目前的修改時間）", |_| with_app(|a| a.watch_theme_file()).unwrap_or(()), |_, a| {
        match &a.theme_watch {
            Some((name, Some(_))) if name == "smoke-import" => Ok(()),
            _ => Err("還沒讀到修改時間".into()),
        }
    }));
    v.push(
        step(
            "主題：文字編輯器存檔後自動重新載入",
            |_| {
                let _ = std::fs::write(theme_path("smoke-import"), "name = \"smoke-import\"\nstyle = \"fluent\"\n[color]\naccent = \"#0000ff\"\n");
            },
            |_, a| expect("強調色", a.theme.color.accent, zing_core::theme::Color::rgb(0x0000ff)),
        )
        .slow(),
    );
    v.push(step(
        "主題：清除測試主題、回到深色",
        |_| {
            with_app(|a| a.stop_theme_watch());
            for n in ["smoke-theme", "smoke-import"] {
                let _ = std::fs::remove_file(theme_path(n));
            }
            let _ = std::fs::remove_file(std::env::temp_dir().join("smoke-import.zingtheme"));
            exec("theme.set dark");
        },
        |_, a| {
            expect("主題名稱", a.theme_name.as_str(), "dark")?;
            expect("列高", a.theme.metrics.row_height, 22.0)
        },
    ));

    // ------------------------------------------------ 預覽處理常式（Windows 的 TXT 預覽處理常式）
    use crate::preview::HostState;
    use std::sync::atomic::Ordering::Relaxed;
    v.push(
        step(
            "預覽處理常式：以獨立視窗顯示",
            |c| {
                crate::prevhost::TEST_FORCE_TEXT.store(true, Relaxed);
                key(c, "a", letter('p'));
                click(c, "", row_pos("報告 10.txt"));
            },
            |_, a| {
                let ld = a.preview.loaded.as_ref().ok_or("還沒有內容")?;
                if ld.handler.is_none() {
                    return Err("找不到 .txt 的預覽處理常式".into());
                }
                expect("狀態", a.preview.host_state, HostState::Shown)?;
                host_visible(a, true)
            },
        )
        .slow(),
    );
    v.push(step("截圖：預覽處理常式", |_| {}, |c, _| screenshot(c, "11-預覽處理常式")));
    v.push(
        step(
            "預覽處理常式：卡住 3 秒後放棄、改顯示縮圖（UI 不受影響）",
            |c| {
                crate::prevhost::TEST_HANG_MS.store(4500, Relaxed);
                click(c, "", row_pos("a2.txt"));
            },
            |_, a| {
                expect("狀態", a.preview.host_state, HostState::Failed)?;
                match a.preview.loaded.as_ref().map(|l| &l.body) {
                    Some(crate::preview::Body::Image(_)) => Ok(()),
                    _ => Err("沒有改顯示縮圖".into()),
                }
            },
        )
        .slow(),
    );
    v.push(
        step("預覽處理常式：之後換新的主機執行緒", |c| click(c, "", row_pos("報告 10.txt")), |_, a| {
            expect("狀態", a.preview.host_state, HostState::Shown)?;
            host_visible(a, true)
        })
        .slow(),
    );
    v.push(step("預覽處理常式：命令面板開啟時暫時隱藏", |c| key(c, "cs", letter('p')), |_, a| {
        expect("狀態", a.preview.host_state, HostState::Suspended)?;
        host_visible(a, false)
    }));
    v.push(
        step("預覽處理常式：命令面板關閉後恢復", |_| palette_key(VK_ESCAPE), |_, a| {
            expect("狀態", a.preview.host_state, HostState::Shown)?;
            host_visible(a, true)
        })
        .slow(),
    );
    v.push(step(
        "預覽處理常式：關閉預覽窗格",
        |c| {
            key(c, "a", letter('p'));
            crate::prevhost::TEST_FORCE_TEXT.store(false, Relaxed);
        },
        |_, a| {
            expect("狀態", a.preview.host_state, HostState::Off)?;
            host_visible(a, false)
        },
    ));

    // 狀態列錯誤與 UI 執行緒 I/O 防呆的回報在 finish() 統一檢查
    v.push(step("截圖：結束畫面", |_| {}, |c, _| screenshot(c, "03-結束")));
    v
}
