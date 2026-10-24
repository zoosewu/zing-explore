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

/// Lua 冒煙測試用的腳本
const SMOKE_LUA: &str = r#"
--- 冒煙測試命令
function smoke_cmd()
  local names = sel() | map(path.name) | list
  msg("smoke:" .. #names .. ":" .. (names[1] or ""))
end

function smoke_go()
  go(path.join(cwd(), "資料夾A"))
end

function smoke_ask()
  local v = ask("名稱？", "預設")
  msg("答案：" .. tostring(v))
end

function smoke_err()
  error("boom")
end

function smoke_loop()
  while true do end
end

function on_navigate(p)
  msg("nav:" .. path.name(p))
end

-- 型別錯誤的參數傳給 Rust 函式：不可丟錯穿過 Rust（release 會直接結束程序）
function smoke_bad()
  __invoke("nav.refresh", 5, 7)
  __msg({}); __exec(nil); __log(print); __is_dir({})
  msg("bad ok")
end

-- 協程裡的無窮迴圈也受時間上限約束
function smoke_coloop()
  coroutine.wrap(function() while true do end end)()
end

-- 執行外部程式：主控台程式不顯示視窗，wait 取得結束代碼；os.execute、io.popen 的替代版本
function smoke_proc()
  local c = run{ "cmd", "/c", "exit 3", wait = true }
  local _, _, e = os.execute("exit 5")
  local out = io.popen("echo hi"):read("l")
  msg("proc:" .. tostring(c) .. ":" .. tostring(e) .. ":" .. out)
end

-- 選擇資料夾（冒煙測試以 TEST_PICK_FOLDER 代答，不顯示對話方塊）
function smoke_pick()
  msg("pick:" .. tostring(pick_folder("選擇", cwd())))
end

-- 除錯器：在 msg 那一行設中斷點（行號由冒煙測試從這段文字算出）
function smoke_dbg()
  local n = 21
  msg("dbg:" .. n * 2) -- 中斷點
end

-- 全域表設了會報錯的 metatable：Rust 端只做 raw 存取，不受影響
function smoke_strict()
  setmetatable(_G, {
    __index = function(_, k) error("未定義的全域變數：" .. tostring(k), 2) end,
    __newindex = function(_, k) error("禁止新增全域變數：" .. tostring(k), 2) end,
  })
  msg("strict")
end
"#;

/// 冒煙測試的除錯器用戶端：以真的具名管道連到自己（luadebug.rs），依序對話
mod dbg_client {
    use std::io::{Read, Write};
    use std::sync::Mutex;

    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

    pub fn log() -> Vec<String> {
        LOG.lock().unwrap().clone()
    }

    pub fn start() {
        std::thread::spawn(|| {
            if let Err(e) = run() {
                LOG.lock().unwrap().push(format!("錯誤：{e}"));
            }
        });
    }

    fn run() -> std::io::Result<()> {
        let name = crate::luadebug::pipe_name(true);
        let mut pipe = std::fs::OpenOptions::new().read(true).write(true).open(&name)?;
        let line = super::SMOKE_LUA[..super::SMOKE_LUA.find("-- 中斷點").unwrap()].matches('\n').count() + 1;
        let file = crate::script::scripts_dir().join("smoke.lua").to_string_lossy().replace('\\', "\\\\");
        let mut buf = Vec::new();
        let mut seq = 0;
        let mut request = |pipe: &mut std::fs::File, cmd: &str, args: &str| -> std::io::Result<()> {
            seq += 1;
            let body = format!(r#"{{"seq":{seq},"type":"request","command":"{cmd}","arguments":{args}}}"#);
            pipe.write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
        };
        request(&mut pipe, "initialize", r#"{"adapterID":"zing-lua"}"#)?;
        request(&mut pipe, "attach", "{}")?;
        request(&mut pipe, "setBreakpoints", &format!(r#"{{"source":{{"path":"{file}"}},"breakpoints":[{{"line":{line}}}]}}"#))?;
        request(&mut pipe, "configurationDone", "{}")?;
        until(&mut pipe, &mut buf, |m| m.contains(r#""command":"configurationDone""#))?;
        // 等冒煙測試執行命令後停在中斷點
        until(&mut pipe, &mut buf, |m| m.contains(r#""event":"stopped""#))?;
        request(&mut pipe, "stackTrace", r#"{"threadId":1}"#)?;
        until(&mut pipe, &mut buf, |m| m.contains(r#""command":"stackTrace""#))?;
        request(&mut pipe, "evaluate", r#"{"expression":"n * 2","frameId":1,"context":"repl"}"#)?;
        until(&mut pipe, &mut buf, |m| m.contains(r#""command":"evaluate""#))?;
        request(&mut pipe, "continue", r#"{"threadId":1}"#)?;
        until(&mut pipe, &mut buf, |m| m.contains(r#""command":"continue""#))?;
        request(&mut pipe, "disconnect", "{}")?;
        until(&mut pipe, &mut buf, |m| m.contains(r#""command":"disconnect""#))?;
        Ok(())
    }

    /// 讀取訊息（記錄下來）直到符合條件
    fn until(pipe: &mut std::fs::File, buf: &mut Vec<u8>, done: impl Fn(&str) -> bool) -> std::io::Result<()> {
        loop {
            while let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..end]).to_string();
                let len: usize = head.trim_start_matches("Content-Length:").trim().parse().unwrap_or(0);
                if buf.len() < end + 4 + len {
                    break;
                }
                let body = String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).to_string();
                buf.drain(..end + 4 + len);
                let hit = done(&body);
                LOG.lock().unwrap().push(body);
                if hit {
                    return Ok(());
                }
            }
            let mut chunk = [0u8; 4096];
            let n = pipe.read(&mut chunk)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }
}

/// 狀態列目前的訊息
fn status(a: &App) -> Option<String> {
    a.status.as_ref().map(|s| s.0.clone())
}

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
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_LBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardState, SetKeyboardState, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F2, VK_HOME,
    VK_LEFT, VK_MENU, VK_OEM_5, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, ICON_BIG, KillTimer, SendMessageW, SetTimer, WM_CHAR, WM_GETICON, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETTEXT, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONUP,
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
    /// 這一步故意觸發的錯誤，不算狀態列錯誤
    expect_error: bool,
}

impl Step {
    fn expect_error(mut self) -> Self {
        self.expect_error = true;
        self
    }

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
    Step {
        name: name.into(),
        act: Some(Box::new(act)),
        check: Box::new(check),
        timeout: Duration::from_secs(4),
        expect_error: false,
    }
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
    /// 防呆回報（I/O、卡頓），附上當時進行中的步驟
    violations: Vec<String>,
    /// 上一個計時器週期處理的步驟（卡頓是在訊息處理完後才回報的）
    last_step: String,
    /// 測試開始前使用者的剪貼簿
    clip: ClipBackup,
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
    crate::window::TEST_NO_HISTORY.store(true, std::sync::atomic::Ordering::Relaxed);
    let clip = harness_io(clip_backup);
    let driver = Driver {
        clip,
        steps: steps(&ctx).into(),
        ctx,
        out: smoke.out,
        results: Vec::new(),
        errors: Vec::new(),
        last_status: String::new(),
        violations: Vec::new(),
        last_step: String::new(),
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
        for v in zing_core::guard::take_violations() {
            self.violations.push(format!("{v}（步驟：{}）", self.last_step));
        }
        if let Some(step) = self.steps.front() {
            self.last_step.clone_from(&step.name);
        }
        let Some(step) = self.steps.front_mut() else { return true };
        if let Some(act) = step.act.take() {
            // 動作與檢查分在不同的計時器週期，讓動作引發的訊息（繪製、背景結果）先處理
            self.ctx.started = Instant::now();
            act(&mut self.ctx);
            return false;
        }
        let ctx = &mut self.ctx;
        let (errors, last) = (&mut self.errors, &mut self.last_status);
        let expect_error = step.expect_error;
        let r = {
            // 檢查會讀磁碟、寫截圖；被測的程式碼在動作中執行，不受影響。
            // 測試資料夾、截圖可能在慢的硬碟上：檢查花的時間不算 UI 停頓（動作仍受卡頓監看）
            let _allow = zing_core::guard::allow_io("冒煙測試的驗證");
            crate::window::exclude_from_stall(|| with_app(|a| {
                if let Some((text, true, _)) = &a.status
                    && text != last
                {
                    *last = text.clone();
                    if !expect_error {
                        errors.push(text.clone());
                    }
                }
                (step.check)(ctx, a)
            }))
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

    fn finish(mut self) {
        let _allow = zing_core::guard::allow_io("冒煙測試寫出報告");
        // 還原使用者的剪貼簿：剪貼簿上還是測試寫入的內容（帶測試標記）才還原，不論中途有沒有步驟失敗；
        // 沒有標記但已改變表示使用者自己複製了東西，不動
        let ours = unsafe { IsClipboardFormatAvailable(crate::window::test_clipboard_format()) }.is_ok();
        let restored = if ours {
            if harness_io(|| clip_restore(self.ctx.hwnd, &self.clip)) { "restored" } else { "restore_failed" }
        } else if unsafe { GetClipboardSequenceNumber() } == self.clip.seq {
            "untouched"
        } else {
            "changed_by_user"
        };
        self.ctx.notes.push(("clipboard", restored.into()));
        if restored == "restore_failed" {
            self.errors.push("無法還原使用者的剪貼簿（Win+V 的剪貼簿歷程中仍有原本的內容）".into());
        }
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        let list = |v: &[String]| v.iter().map(|s| format!("\"{}\"", esc(s))).collect::<Vec<_>>().join(", ");
        let mut violations = self.violations.clone();
        violations.extend(zing_core::guard::take_violations());
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
        let _ = harness_io(|| std::fs::write(&self.out, json));
    }
}

// ---------------------------------------------------------------- 使用者的剪貼簿

/// 測試開始前的剪貼簿：每個能以記憶體區塊複製的格式（文字、檔案清單、DIB 圖片、HTML…）
struct ClipBackup {
    formats: Vec<(u32, Vec<u8>)>,
    /// 備份時的序號
    seq: u32,
}

/// GDI 控制代碼等不是記憶體區塊的格式：無法複製（點陣圖另有 CF_DIB 可以還原）
fn clip_skip(f: u32) -> bool {
    matches!(f, 2 | 3 | 9 | 14 | 0x80 | 0x82 | 0x83 | 0x8E) || (0x300..=0x3FF).contains(&f)
}

/// 開啟剪貼簿（別的程式正開著時稍候重試）
fn clip_open(owner: Option<HWND>) -> bool {
    for _ in 0..20 {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn clip_backup() -> ClipBackup {
    let mut formats = Vec::new();
    let seq = unsafe { GetClipboardSequenceNumber() };
    if !clip_open(None) {
        return ClipBackup { formats, seq };
    }
    let ours = crate::window::test_clipboard_format();
    unsafe {
        let mut f = 0;
        loop {
            f = EnumClipboardFormats(f);
            if f == 0 {
                break;
            }
            if clip_skip(f) || f == ours {
                continue;
            }
            let Ok(h) = GetClipboardData(f) else { continue };
            let mem = windows::Win32::Foundation::HGLOBAL(h.0);
            let p = GlobalLock(mem) as *const u8;
            if p.is_null() {
                continue;
            }
            formats.push((f, std::slice::from_raw_parts(p, GlobalSize(mem)).to_vec()));
            let _ = GlobalUnlock(mem);
        }
        let _ = CloseClipboard();
    }
    ClipBackup { formats, seq }
}

/// 放回備份的內容（不再進入剪貼簿歷程：原本的內容已經在歷程裡）
fn clip_restore(owner: HWND, b: &ClipBackup) -> bool {
    if !clip_open(Some(owner)) {
        return false;
    }
    unsafe {
        let ok = EmptyClipboard().is_ok();
        for (f, data) in &b.formats {
            let _ = crate::window::put_clipboard(*f, data);
        }
        if !b.formats.is_empty() {
            crate::window::mark_no_history();
        }
        let _ = CloseClipboard();
        ok
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

/// 設定執行緒鍵盤狀態中的一個鍵（跨越多個訊息按住修飾鍵）
fn set_key(vk: VIRTUAL_KEY, down: bool) {
    unsafe {
        let mut state = [0u8; 256];
        let _ = GetKeyboardState(&mut state);
        state[vk.0 as usize] = if down { 0x80 } else { 0 };
        let _ = SetKeyboardState(&state);
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

/// 按住左鍵從 `from` 拖到 `to` 再放開
fn drag(ctx: &Ctx, from: Option<(f32, f32)>, to: Option<(f32, f32)>) {
    let (Some(from), Some(to)) = (from, to) else { return };
    let h = ctx.hwnd;
    send(h, WM_MOUSEMOVE, 0, mouse_lparam(h, from));
    send(h, WM_LBUTTONDOWN, MK_LBUTTON.0 as usize, mouse_lparam(h, from));
    send(h, WM_MOUSEMOVE, MK_LBUTTON.0 as usize, mouse_lparam(h, to));
    send(h, WM_LBUTTONUP, 0, mouse_lparam(h, to));
}

fn right_click(ctx: &Ctx, at: Option<(f32, f32)>) {
    let Some(at) = at else { return };
    let (h, l) = (ctx.hwnd, mouse_lparam(ctx.hwnd, at));
    send(h, WM_MOUSEMOVE, 0, l);
    send(h, WM_RBUTTONDOWN, MK_RBUTTON.0 as usize, l);
    send(h, WM_RBUTTONUP, 0, l);
}

fn hover_at(ctx: &Ctx, at: Option<(f32, f32)>) {
    let Some(at) = at else { return };
    send(ctx.hwnd, WM_MOUSEMOVE, 0, mouse_lparam(ctx.hwnd, at));
}

/// 命令列上某個命令的按鈕中心
fn bar_button_pos(cmd: &str) -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let bl = a.layout.cmdbar.as_ref()?;
        let (_, r, _) = bl.buttons.iter().find(|(i, _, _)| a.config.cmdbar[*i].cmd == cmd)?;
        Some(((r.l + r.r) / 2.0, (r.t + r.b) / 2.0))
    })
    .flatten()
}

/// 開啟中的選單第 `level` 層、名稱為 `label` 的項目中心
fn menu_item_pos(level: usize, label: &str) -> Option<(f32, f32)> {
    with_app(|a| {
        let m = a.menu.as_ref()?;
        let lv = m.levels.get(level)?;
        let i = lv.items.iter().position(|it| it.label == label)?;
        let mut y = lv.rect.t + 4.0;
        for it in &lv.items[..i] {
            y += if it.separator { 9.0 } else { (a.row_h() + 4.0).max(26.0) };
        }
        Some(((lv.rect.l + lv.rect.r) / 2.0, y + (a.row_h() + 4.0).max(26.0) / 2.0))
    })
    .flatten()
}

/// 快速鍵面板中某個命令的列，`part` 從該列的各部分中選一個點
fn keys_pos(cmd: &str, part: usize) -> Option<(f32, f32)> {
    use crate::keyspanel::Row;
    with_app(|a| {
        a.layout = a.compute_layout();
        let k = a.keys_rows().iter().position(|r| matches!(r, Row::Cmd(c, _) if c == cmd))?;
        let r = a.side_reveal(k)?;
        let (_, p) = a.key_parts(cmd, r).into_iter().find(|(i, _)| *i == part)?;
        Some(((p.l + p.r) / 2.0, (p.t + p.b) / 2.0))
    })
    .flatten()
}

/// 快速鍵面板：衝突列或搜尋列的部分
fn keys_special_pos(conflict: bool, part: usize) -> Option<(f32, f32)> {
    use crate::keyspanel::Row;
    with_app(|a| {
        a.layout = a.compute_layout();
        let panel = a.layout.side_panel?;
        let (_, list, _, _) = a.keys_layout(panel);
        let scroll = a.keys_panel()?.scroll;
        let r = if conflict {
            let k = a.keys_rows().iter().position(|r| *r == Row::Conflict)?;
            a.conflict_parts(a.keys_row_rect(list, k, scroll))[part]
        } else {
            a.search_parts(a.keys_row_rect(list, 0, scroll))[part]
        };
        Some(((r.l + r.r) / 2.0, (r.t + r.b) / 2.0))
    })
    .flatten()
}

fn keys_button_pos(k: usize) -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let (_, _, _, b) = a.keys_layout(a.layout.side_panel?);
        Some(((b[k].l + b[k].r) / 2.0, (b[k].t + b[k].b) / 2.0))
    })
    .flatten()
}

/// 自訂命令列面板中可加入的命令（`avail`）或命令列上的按鈕的列，`part` 為 None 時是整列
fn bar_edit_pos(cmd: &str, avail: bool, part: Option<usize>) -> Option<(f32, f32)> {
    use crate::cmdbaredit::Row;
    with_app(|a| {
        a.layout = a.compute_layout();
        let rows = a.cmdbar_edit_rows();
        let k = rows.iter().position(|r| match r {
            Row::Avail(c, _) => avail && c == cmd,
            Row::Item(i) => !avail && a.config.cmdbar[*i].cmd == cmd,
            _ => false,
        })?;
        let r = a.side_reveal(k)?;
        let p = match part {
            Some(p) => a.bar_parts(&rows[k], r)[p],
            None => crate::gfx::Rect::new(r.l + 60.0, r.t, r.l + 80.0, r.b),
        };
        Some(((p.l + p.r) / 2.0, (p.t + p.b) / 2.0))
    })
    .flatten()
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
    palette_key_mods("", vk);
}

fn palette_key_mods(mods: &str, vk: VIRTUAL_KEY) {
    if let Some(e) = edit_hwnd() {
        key_to(e, mods, vk);
    }
}

/// 預覽處理常式的視窗是否顯示（另一條執行緒的視窗，IsWindowVisible 不會等待它）
fn host_visible(a: &App, want: bool) -> Result<(), String> {
    let visible = a.preview.host.window().is_some_and(|h| unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(h) }.as_bool());
    expect("主機視窗顯示", visible, want)
}

/// 主題編輯面板中某一列的位置（先捲進可見範圍）；`pick` 從這一列的各個位置中選一個點
fn theme_pos(want: crate::themepanel::Row, pick: impl Fn(&crate::themepanel::RowRects) -> (f32, f32)) -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let k = a.theme_rows().iter().position(|r| *r == want)?;
        a.side_reveal(k)?;
        let list = a.theme_panel_layout(a.layout.side_panel?).list;
        Some(pick(&a.theme_row_rects(list, k, a.side_scroll())))
    })
    .flatten()
}

fn theme_field(label: &str) -> usize {
    zing_core::theme::FIELDS.iter().position(|f| f.label == label).expect("沒有這個欄位")
}

fn mid(r: crate::gfx::Rect) -> (f32, f32) {
    ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0)
}

/// 主題編輯面板中某欄位的位置：`value` 為 true 時是數值處（色碼、選單、數字），否則是名稱處
fn theme_row_pos(label: &str, value: bool) -> Option<(f32, f32)> {
    let i = theme_field(label);
    theme_pos(crate::themepanel::Row::Field(i), move |r| {
        let y = (r.row.t + r.row.b) / 2.0;
        match (value, zing_core::theme::FIELDS[i].kind) {
            (false, _) => (r.row.l + 20.0, y),
            (true, zing_core::theme::FieldKind::Color) => mid(r.text),
            (true, zing_core::theme::FieldKind::Number { .. }) => (r.plus.r + 12.0, y),
            (true, _) => (r.value.l + 20.0, y),
        }
    })
}

/// 設定頁中某個設定的列（`option` 為 Some 時是展開的第幾個選項）；`value` 為 true 時是數值處
fn settings_row_pos(key: &str, option: Option<usize>, value: bool) -> Option<(f32, f32)> {
    use crate::prefspage::Row;
    let i = zing_core::prefs::PREFS.iter().position(|p| p.key == key)?;
    settings_pos(
        match option {
            Some(o) => Row::Option(i, o),
            None => Row::Field(i),
        },
        value,
    )
}

/// 設定頁的分類清單中的一個分類
fn settings_category_pos(group: &str) -> Option<(f32, f32)> {
    let g = zing_core::prefs::groups().iter().position(|g| *g == group)?;
    settings_pos(crate::prefspage::Row::Category(g), false)
}

/// 設定頁的標題列（在分類中時是「‹ 分類」，點了回到分類清單）
fn settings_title_pos() -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let t = a.settings_layout(a.layout.side_panel?).title;
        Some((t.l + 30.0, (t.t + t.b) / 2.0))
    })
    .flatten()
}

fn settings_pos(want: crate::prefspage::Row, value: bool) -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let k = a.settings_page()?.rows().iter().position(|r| *r == want)?;
        a.side_reveal(k)?;
        let list = a.settings_layout(a.layout.side_panel?).list;
        let (row, v) = a.settings_row_rects(list, k, a.side_scroll());
        let y = (row.t + row.b) / 2.0;
        Some(if value { (v.l + 20.0, y) } else { (row.l + 20.0, y) })
    })
    .flatten()
}

/// 展開的選單中第 o 個選項
fn theme_option_pos(t: crate::themepanel::Target, o: usize) -> Option<(f32, f32)> {
    theme_pos(crate::themepanel::Row::Option(t, o), |r| (r.value.l + 20.0, (r.row.t + r.row.b) / 2.0))
}

fn theme_button_pos(k: usize) -> Option<(f32, f32)> {
    with_app(|a| {
        a.layout = a.compute_layout();
        let panel = a.layout.side_panel?;
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

/// 全選後輸入（取代命令面板輸入框的內容）
fn palette_set(text: &str) {
    if let Some(e) = edit_hwnd() {
        key_to(e, "c", letter('a'));
        type_to(e, text);
    }
}

/// 搜尋結果的名稱（依顯示順序）
fn search_names(a: &App) -> Result<Vec<String>, String> {
    let p = a.palette.as_ref().ok_or("面板已關閉")?;
    let r = p.search.as_ref().ok_or("不是搜尋模式")?;
    expect("狀態", &r.state, &crate::search::State::Done)?;
    Ok(p.matches.iter().map(|&k| path::file_name(&r.items[k].path).to_string()).collect())
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
/// 測試工具自己的檔案 I/O（見 `window::exclude_from_stall`）
fn harness_io<R>(f: impl FnOnce() -> R) -> R {
    let _allow = zing_core::guard::allow_io("冒煙測試工具的檔案");
    crate::window::exclude_from_stall(f)
}

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
    v.push(step(
        "篩選：輸入法組字視窗對齊篩選標籤的文字結尾",
        |c| {
            use windows::Win32::UI::WindowsAndMessaging::{WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION};
            send(c.hwnd, WM_IME_STARTCOMPOSITION, 0, 0);
            send(c.hwnd, WM_IME_ENDCOMPOSITION, 0, 0);
        },
        |_, a| {
            use windows::Win32::UI::Input::Ime::{COMPOSITIONFORM, ImmGetCompositionWindow, ImmGetContext, ImmReleaseContext};
            let chip = a.layout.filter_chip.ok_or("沒有篩選標籤")?;
            let s = a.gfx.scale();
            let mut cf = COMPOSITIONFORM::default();
            unsafe {
                let himc = ImmGetContext(a.hwnd);
                if himc.0.is_null() {
                    return Ok(()); // 沒有輸入法（IMM 停用）：不檢查
                }
                let _ = ImmGetCompositionWindow(himc, &mut cf);
                let _ = ImmReleaseContext(a.hwnd, himc);
            }
            let (x, y) = (cf.ptCurrentPos.x as f32 / s, cf.ptCurrentPos.y as f32 / s);
            // 在標籤內、「篩選：報告」之後，垂直在標籤範圍內
            if x < chip.l + 60.0 || x > chip.r - 20.0 || y < chip.t - 4.0 || y > chip.b {
                return Err(format!("組字視窗在 ({x:.0}, {y:.0})，標籤 {chip:?}"));
            }
            Ok(())
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
            // 窗格變窄時整欄收起（類型欄先收），留下的欄位不縮小
            for pl in &a.compute_layout().panes {
                let w: Vec<f32> = pl.cols[1..].iter().map(|(l, r)| r - l).collect();
                for (i, &x) in w.iter().enumerate() {
                    if x != 0.0 && (x - a.cols[i]).abs() > 0.5 {
                        return Err(format!("欄寬 {w:?}，設定 {:?}", a.cols));
                    }
                }
            }
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
        if n >= zing_core::builtin::palette().count() { Ok(()) } else { Err(format!("只列出 {n} 個命令")) }
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

    // ------------------------------------------------ 全磁碟搜尋（隔離模式不用索引服務：以 TEST_RESULTS 提供結果）
    v.push(step(
        "搜尋：Ctrl+F 開啟搜尋面板",
        {
            let (d, dir_a) = (d.clone(), dir_a.clone());
            move |c| {
                use zing_core::indexproto::SearchItem;
                // 修改時間：2024-05-10、2023-01-01（FILETIME）
                let (may, jan) = (133_597_728_000_000_000, 133_170_048_000_000_000);
                let item = |p: String, dir: bool, hidden: bool, size: u64, modified: u64| SearchItem {
                    path: p,
                    size,
                    dir,
                    hidden,
                    modified,
                };
                *crate::search::TEST_RESULTS.lock().unwrap() = Some(vec![
                    item(path::join(&dir_a, "內層.txt"), false, false, 5, may),
                    item(path::join(&d, "隱藏的內層.txt"), false, true, 5, may),
                    item(path::join(&d, "報告 9.txt"), false, false, 3 << 20, jan),
                    item(path::join(&d, "ÉTÉ.JPG"), false, false, 2 << 20, may),
                    item(path::join(&d, "第10章.md"), false, false, 1, jan),
                    item(path::join(&d, "第2章.md"), false, false, 2, may),
                    item(dir_a.clone(), true, false, 5, 0),
                ]);
                key(c, "c", letter('f'));
            }
        },
        |_, a| expect("搜尋模式", a.palette.as_ref().is_some_and(|p| p.search.is_some()), true),
    ));
    v.push(step("搜尋：輸入名稱列出結果（隱藏項目依設定略過）", |_| palette_type("內層"), |_, a| {
        let p = a.palette.as_ref().ok_or("面板已關閉")?;
        let r = p.search.as_ref().ok_or("不是搜尋模式")?;
        expect("狀態", &r.state, &crate::search::State::Done)?;
        let names: Vec<&str> = p.matches.iter().map(|&k| path::file_name(&r.items[k].path)).collect();
        expect("結果", names, vec!["內層.txt"])
    }));
    v.push(step("截圖：全磁碟搜尋", |_| {}, |c, _| screenshot(c, "11-全磁碟搜尋")));
    v.push(step("搜尋：篩選大小、依大小排序", |_| palette_set("file: size:>1mb sort:size"), |_, a| {
        expect("結果", search_names(a)?, vec!["報告 9.txt".to_string(), "ÉTÉ.JPG".to_string()])
    }));
    v.push(step("搜尋：副檔名、修改日期、非 ASCII 不分大小寫", |_| palette_set("été ext:jpg dm:2024-05"), |_, a| {
        expect("結果", search_names(a)?, vec!["ÉTÉ.JPG".to_string()])
    }));
    v.push(step(
        "搜尋：比對完整路徑、排除",
        {
            let dir_a = dir_a.clone();
            move |_| palette_set(&format!("\"{}\\\" !報告", path::file_name(&dir_a)))
        },
        |_, a| expect("結果", search_names(a)?, vec!["內層.txt".to_string()]),
    ));
    // 排序按鍵改寫查詢：Ctrl+1 名稱（自然排序，第2章在第10章前）、再按一次反向、Ctrl+5 日期、Ctrl+0 取消
    let sort_step = |name: &'static str, digit: u8, query: &'static str, want: [&'static str; 2]| {
        step(name, move |_| palette_key_mods("c", VIRTUAL_KEY(digit as u16)), move |_, a| {
            expect("查詢", a.palette.as_ref().map(|p| p.query.as_str()), Some(query))?;
            expect("結果", search_names(a)?, want.map(String::from).to_vec())
        })
    };
    v.push(step("搜尋：名稱有數字", |_| palette_set("章"), |_, a| expect("結果數", search_names(a)?.len(), 2)));
    v.push(sort_step("搜尋：Ctrl+1 依名稱自然排序", b'1', "章 sort:name", ["第2章.md", "第10章.md"]));
    v.push(sort_step("搜尋：再按 Ctrl+1 反向", b'1', "章 sort:name-desc", ["第10章.md", "第2章.md"]));
    v.push(sort_step("搜尋：Ctrl+5 依日期（新的在前）", b'5', "章 sort:date", ["第2章.md", "第10章.md"]));
    v.push(step("搜尋：Ctrl+0 取消排序", |_| palette_key_mods("c", VIRTUAL_KEY(b'0' as u16)), |_, a| {
        expect("查詢", a.palette.as_ref().map(|p| p.query.as_str()), Some("章"))
    }));
    v.push(step("搜尋：無法解讀的詞不查詢並提示", |_| palette_set("size:abc"), |_, a| {
        let r = a.palette.as_ref().and_then(|p| p.search.as_ref()).ok_or("不是搜尋模式")?;
        expect("狀態", &r.state, &crate::search::State::Invalid("size:abc".into()))
    }));
    v.push(step("搜尋：回到名稱查詢", |_| palette_set("內層"), |_, a| {
        expect("結果", search_names(a)?, vec!["內層.txt".to_string()])
    }));
    v.push(step("搜尋：Enter 前往所在的資料夾並選取", |_| palette_key(VK_RETURN), {
        let dir_a = dir_a.clone();
        move |_, a| {
            palette_closed(a)?;
            at(a, &dir_a)?;
            expect("選取", selected(a), vec!["內層.txt".to_string()])
        }
    }));
    v.push(step("搜尋：回到測試資料夾", |c| key(c, "a", VK_LEFT), {
        let d = d.clone();
        move |_, a| at(a, &d)
    }));
    v.push(step(
        "搜尋：搜尋資料夾",
        |c| {
            key(c, "c", letter('f'));
            palette_type("資料夾A");
        },
        |_, a| {
            let p = a.palette.as_ref().ok_or("面板已關閉")?;
            let r = p.search.as_ref().ok_or("不是搜尋模式")?;
            expect("狀態", &r.state, &crate::search::State::Done)?;
            expect("結果數", p.matches.len(), 1)
        },
    ));
    // Enter 會到上一層並選取它；Ctrl+Enter 進入資料夾
    // 右 Ctrl 一直按著到確認處理完（Enter 的確認是 post 的，真實鍵盤此時 Ctrl 仍按著）；只設右 Ctrl，左右都要認得
    v.push(step("搜尋：右 Ctrl+Enter 開啟（資料夾就進入）", |_| {
        set_key(VK_RCONTROL, true);
        if let Some(e) = edit_hwnd() {
            key_to(e, "", VK_RETURN);
        }
    }, {
        let dir_a = dir_a.clone();
        move |_, a| {
            // 計時器訊息排在 post 的確認之後：檢查時確認已處理完，先放開（失敗時也不會一直按著）
            set_key(VK_RCONTROL, false);
            palette_closed(a)?;
            at(a, &dir_a)
        }
    }));
    v.push(step(
        "搜尋：回到測試資料夾（結束）",
        |c| {
            *crate::search::TEST_RESULTS.lock().unwrap() = None;
            key(c, "a", VK_LEFT);
        },
        {
            let d = d.clone();
            move |_, a| at(a, &d)
        },
    ));
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
    v.push(step("截圖：重新命名", |_| {}, |c, _| screenshot(c, "12-重新命名")));
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
                if !(j.fraction > 0.02 && j.fraction < 0.9) {
                    return Err(format!("進度 {:.2}", j.fraction));
                }
                // 另一條執行緒算出來源的總大小後，以進度換算速度
                expect("總大小", j.total, Some(BIG_BYTES))?;
                if j.speed().is_some_and(|v| v > 0.0) { Ok(()) } else { Err("沒有速度".into()) }
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
    // ------------------------------------------------ 快速鍵編輯面板（隔離模式不寫 keys.toml）
    let kc = |s: &str| zing_core::keymap::KeyChord::parse(s).unwrap();
    v.push(step("快速鍵：開啟編輯面板", |_| exec("keys.edit"), |_, a| {
        expect("面板", a.keys_panel().is_some(), true)?;
        expect("位置", a.layout.side_panel.is_some(), true)
    }));
    v.push(step("快速鍵：搜尋「重新整理」", |_| with_app(|a| a.keys_query("重新整理".into())).unwrap_or(()), |_, a| {
        let rows = a.keys_rows();
        let found = rows.iter().any(|r| matches!(r, crate::keyspanel::Row::Cmd(c, _) if c == "nav.refresh"));
        expect("找到", found, true)?;
        expect("沒有不相關的", rows.iter().any(|r| matches!(r, crate::keyspanel::Row::Cmd(c, _) if c == "file.copy")), false)
    }));
    v.push(step("快速鍵：點 ＋ 開始錄製", |c| click(c, "", keys_pos("nav.refresh", crate::keyspanel::PART_ADD)), |_, a| {
        expect("錄製中", a.keys_panel().and_then(|k| k.recording.clone()).as_deref(), Some("nav.refresh"))
    }));
    v.push(step("截圖：快速鍵編輯面板（錄製中）", |_| {}, |c, _| screenshot(c, "13-快速鍵編輯器")));
    v.push(step("快速鍵：按下新的組合鍵（保留原本的 F5）", |c| key(c, "cs", VIRTUAL_KEY(0x78)), move |_, a| {
        expect("錄製結束", a.keys_capturing(), false)?;
        expect("新按鍵", a.config.command_for(kc("Ctrl+Shift+F9")), Some("nav.refresh"))?;
        expect("原本的 F5", a.config.command_for(kc("F5")), Some("nav.refresh"))
    }));
    v.push(step("快速鍵：已被使用的按鍵先詢問", |c| {
        click(c, "", keys_pos("nav.refresh", crate::keyspanel::PART_ADD));
        key(c, "c", letter('c'));
    }, move |_, a| {
        expect("衝突", a.keys_panel().and_then(|k| k.conflict.as_ref()).map(|x| x.2.clone()).as_deref(), Some("file.copy"))?;
        expect("還沒搶走", a.config.command_for(kc("Ctrl+C")), Some("file.copy"))
    }));
    v.push(step("快速鍵：取代", |c| click(c, "", keys_special_pos(true, crate::keyspanel::PART_REPLACE)), move |_, a| {
        expect("Ctrl+C", a.config.command_for(kc("Ctrl+C")), Some("nav.refresh"))?;
        expect("衝突列消失", a.keys_panel().is_some_and(|k| k.conflict.is_none()), true)
    }));
    v.push(step("快速鍵：點按鍵方塊移除 Ctrl+Shift+F9", |c| click(c, "", keys_pos("nav.refresh", 1)), move |_, a| {
        expect("Ctrl+Shift+F9", a.config.command_for(kc("Ctrl+Shift+F9")), None)?;
        expect("F5", a.config.command_for(kc("F5")), Some("nav.refresh"))
    }));
    v.push(step("快速鍵：↺ 還原這個命令", |c| click(c, "", keys_pos("nav.refresh", crate::keyspanel::PART_RESET)), move |_, a| {
        expect("Ctrl+C 不再是重新整理", a.config.command_for(kc("Ctrl+C")), None)?;
        expect("F5", a.config.command_for(kc("F5")), Some("nav.refresh"))
    }));
    v.push(step("快速鍵：依按鍵搜尋 Ctrl+A", |c| {
        with_app(|a| a.keys_query(String::new()));
        click(c, "", keys_special_pos(false, crate::keyspanel::PART_BY_KEY));
        key(c, "c", letter('a'));
    }, |_, a| {
        let cmds: Vec<String> = a.keys_rows().into_iter().filter_map(|r| match r {
            crate::keyspanel::Row::Cmd(c, _) => Some(c),
            _ => None,
        }).collect();
        expect("結果", cmds, vec!["sel.all".to_string()])
    }));
    v.push(step("快速鍵：全部還原後 Esc 關閉", |c| {
        click(c, "", keys_button_pos(0));
        key(c, "", VK_ESCAPE);
    }, |_, a| {
        expect("面板關閉", a.keys_panel().is_none(), true)?;
        let sorted = |v: &[(zing_core::keymap::KeyChord, String)]| {
            let mut v: Vec<(String, String)> = v.iter().map(|(k, c)| (k.to_string(), c.clone())).collect();
            v.sort();
            v
        };
        if sorted(&a.config.keys) == sorted(&a.config.config_keys) { Ok(()) } else { Err("按鍵沒有全部還原".into()) }
    }));

    // ------------------------------------------------ 命令列（工具列下方）、下拉選單、提示
    v.push(step("命令列：預設顯示，沒有選取時剪下是灰色", |_| exec("sel.none"), |_, a| {
        expect("命令列", a.layout.cmdbar.as_ref().is_some_and(|b| !b.buttons.is_empty()), true)?;
        expect("剪下", a.cmd_disabled("file.cut").is_some(), true)?;
        expect("新增", a.cmd_disabled("file.newfolder").is_none(), true)
    }));
    v.push(step("命令列：點「檢視▾」開啟選單", |c| click(c, "", bar_button_pos(zing_core::cmdbar::MENU_VIEW)), |_, a| {
        let m = a.menu.as_ref().ok_or("選單沒有開啟")?;
        let labels: Vec<&str> = m.levels[0].items.iter().map(|i| i.label.as_str()).collect();
        expect("有中縮圖", labels.contains(&"中縮圖"), true)?;
        // 目前的檢視打勾
        expect("清單打勾", m.levels[0].items.iter().any(|i| i.label == "清單" && i.checked), true)
    }));
    v.push(step("截圖：命令列與選單", |_| {}, |c, _| screenshot(c, "18-命令列選單")));
    v.push(step("命令列：選「中縮圖」", |c| click(c, "", menu_item_pos(0, "中縮圖")), |_, a| {
        expect("選單關閉", a.menu.is_none(), true)?;
        expect("縮圖", (a.tab().mode, a.tab().thumb), (zing_core::folderview::ViewMode::Thumbnails, 128))
    }));
    v.push(step("命令列：「⋯」的子選單", |c| {
        // 回到預設的檢視（不留下這個資料夾的檢視記錄）
        with_app(|a| {
            let (p, t) = (a.active_pane, a.panes[a.active_pane].active);
            let d = zing_core::folderview::FolderView::default();
            let tab = a.tab_mut();
            tab.mode = d.mode;
            tab.thumb = d.thumb;
            a.remember_view(p, t);
            a.after_view_change(p, t);
        });
        click(c, "", bar_button_pos(zing_core::cmdbar::MENU_MORE));
        hover_at(c, menu_item_pos(0, "選取"));
    }, |_, a| {
        let m = a.menu.as_ref().ok_or("選單沒有開啟")?;
        expect("子選單", m.levels.len(), 2)?;
        expect("反向選取", m.levels[1].items.iter().any(|i| i.label == "反向選取"), true)
    }));
    v.push(step("命令列：子選單項目右鍵「加到命令列」", |c| {
        right_click(c, menu_item_pos(1, "反向選取"));
        click(c, "", menu_item_pos(0, "加到命令列"));
    }, |_, a| {
        expect("選單關閉", a.menu.is_none(), true)?;
        expect("加在「⋯」前面", a.config.cmdbar.iter().rev().nth(1).map(|i| i.cmd.as_str()), Some("sel.invert"))
    }));
    v.push(step("命令列：右鍵「從命令列移除」", |c| {
        right_click(c, bar_button_pos("sel.invert"));
        click(c, "", menu_item_pos(0, "從命令列移除"));
    }, |_, a| expect("已移除", a.config.cmdbar.iter().any(|i| i.cmd == "sel.invert"), false)));
    // 真的游標不在視窗上，系統隨後會送 WM_MOUSELEAVE 收起提示：在動作中記下當時的提示
    v.push(step("提示：停在複製按鈕上顯示名稱與快速鍵，移到貼上立即切換", |c| {
        let shown = || with_app(|a| a.tip.as_ref().filter(|t| t.shown).map(|t| t.text.joined())).flatten().unwrap_or_default();
        hover_at(c, bar_button_pos("file.copy"));
        let before = shown();
        with_app(|a| a.on_tip_timer());
        let copy = shown();
        hover_at(c, bar_button_pos("file.paste"));
        let paste = shown();
        c.notes.push(("tip_before_timer", before));
        c.notes.push(("tip_copy", copy));
        c.notes.push(("tip_paste", paste));
    }, |c, _| {
        expect("計時前不顯示", c.note("tip_before_timer"), Some(""))?;
        let copy = c.note("tip_copy").unwrap_or_default();
        if !(copy.contains("複製") && copy.contains("Ctrl+C") && copy.contains("請先選取項目")) {
            return Err(format!("複製的提示：{copy}"));
        }
        expect("貼上", c.note("tip_paste").is_some_and(|t| t.starts_with("貼上")), true)
    }));
    // 截圖：不送滑鼠訊息（不會收到 WM_MOUSELEAVE），直接設定游標所在的按鈕
    v.push(step("截圖：提示（不能用的原因在第二行）", |_| {
        with_app(|a| {
            a.layout = a.compute_layout();
            let k = a.layout.cmdbar.as_ref().and_then(|b| b.buttons.iter().position(|(i, _, _)| a.config.cmdbar[*i].cmd == "file.delete"));
            if let Some(k) = k {
                let hit = crate::view::Hit::CmdButton(k);
                a.update_tip(&hit);
                a.hover = hit;
                a.on_tip_timer();
            }
        });
    }, |c, a| {
        expect("提示", a.tip.as_ref().is_some_and(|t| t.shown && t.text.detail.is_some()), true)?;
        screenshot(c, "19-提示")
    }));
    // 右鍵選單本身無法在程序內測試（會搶前景）：確認游標停住後 ShellHost 真的預先建好選單
    let prepared_before = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let before = prepared_before.clone();
    let mut s = step("右鍵選單：游標停在一列後預先準備選單", move |c| {
        before.set(crate::shellhost::PREPARED_COUNT.load(std::sync::atomic::Ordering::Relaxed));
        hover_at(c, row_pos("a2.txt"));
        with_app(|a| a.on_menu_prepare_timer());
        let target = with_app(|a| a.menu_prepared.as_ref().map(|t| t.items.join("|"))).flatten().unwrap_or_default();
        c.notes.push(("menu_prepared", target));
    }, move |c, a| {
        expect("項目", c.note("menu_prepared").is_some_and(|t| t.ends_with("a2.txt")), true)?;
        // 第三方擴充功能（這台電腦可能沒有）：至少要回報過這一列的結果
        if !a.menu_exts.as_ref().is_some_and(|m| m.items.first().is_some_and(|i| i.ends_with("a2.txt"))) {
            return Err("還沒收到擴充功能的項目".into());
        }
        let now = crate::shellhost::PREPARED_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        if now <= prepared_before.get() {
            return Err("ShellHost 還沒建好選單".into());
        }
        Ok(())
    });
    // 第一次要載入 Shell 擴充功能，可能要數秒
    s.timeout = Duration::from_secs(15);
    v.push(s);
    // 精簡的右鍵選單是程式自己畫的，可以測；「顯示更多選項」會開 Windows 的選單（搶前景），不在這裡測
    v.push(step("右鍵選單：在檔案上按右鍵立即顯示精簡選單", |c| right_click(c, row_pos("a2.txt")), |_, a| {
        let m = a.menu.as_ref().ok_or("選單沒開")?;
        let labels: Vec<&str> = m.levels[0].items.iter().map(|i| i.label.as_str()).collect();
        for want in ["開啟", "開啟方式…", "複製路徑", "內容", "顯示更多選項"] {
            if !labels.contains(&want) {
                return Err(format!("缺少「{want}」：{labels:?}"));
            }
        }
        expect("已選取", a.tab().selected_entries().len(), 1)?;
        expect("完整選單待命", a.pending_shell_menu.is_some(), true)
    }));
    v.push(step("截圖：精簡右鍵選單", |_| {}, |c, _| screenshot(c, "21-右鍵選單")));
    v.push(step("右鍵選單：點「重新命名」開始就地編輯", |c| click(c, "", menu_item_pos(0, "重新命名")), |_, a| {
        expect("選單關閉", a.menu.is_none(), true)?;
        match &a.edit {
            Some(e) if matches!(&e.kind, EditKind::Rename { path, .. } if path.ends_with("a2.txt")) => Ok(()),
            _ => Err("沒有進入重新命名".into()),
        }
    }));
    v.push(step("右鍵選單：Esc 取消重新命名", |_| {
        if let Some(e) = edit_hwnd() {
            key_to(e, "", VK_ESCAPE);
        }
    }, |_, a| expect("編輯結束", a.edit.is_none(), true)));
    v.push(step("右鍵選單：空白處按右鍵有「新增」子選單", |c| {
        let at = with_app(|a| {
            a.layout = a.compute_layout();
            let pl = &a.layout.panes[a.active_pane];
            Some((pl.list.l + 60.0, pl.list.b - 10.0))
        })
        .flatten();
        right_click(c, at);
        hover_at(c, menu_item_pos(0, "新增"));
    }, |_, a| {
        let m = a.menu.as_ref().ok_or("選單沒開")?;
        expect("沒有選取", a.tab().selected_entries().len(), 0)?;
        expect("子選單", m.levels.get(1).is_some_and(|l| l.items.iter().any(|i| i.label == "資料夾")), true)
    }));
    v.push(step("右鍵選單：Esc 關閉", |c| {
        key(c, "", VK_ESCAPE);
        key(c, "", VK_ESCAPE);
    }, |_, a| expect("關閉", a.menu.is_none(), true)));
    v.push(step("命令列：視窗太窄時移進「⋯」", |c| hover_at(c, Some((5.0, 400.0))), |_, a| {
        let narrow = a.layout_cmdbar(crate::gfx::Rect::new(0.0, 0.0, 180.0, 30.0));
        expect("有移出的按鈕", narrow.overflow.is_empty(), false)?;
        expect("「⋯」還在", narrow.buttons.iter().any(|(i, _, _)| a.config.cmdbar[*i].cmd == zing_core::cmdbar::MENU_MORE), true)?;
        expect("提示消失", a.tip.is_none(), true)
    }));
    v.push(step("自訂命令列：開啟面板，加入「反向選取」", |c| {
        exec("cmdbar.edit");
        click(c, "", bar_edit_pos("sel.invert", true, None));
    }, |_, a| {
        expect("面板", a.cmdbar_editor().is_some(), true)?;
        expect("已加入", a.config.cmdbar.iter().any(|i| i.cmd == "sel.invert"), true)
    }));
    v.push(step("自訂命令列：↑ 往前移", |c| click(c, "", bar_edit_pos("sel.invert", false, Some(crate::cmdbaredit::PART_UP))), |_, a| {
        let n = a.config.cmdbar.len();
        expect("位置", a.config.cmdbar.iter().position(|i| i.cmd == "sel.invert"), Some(n - 3))
    }));
    v.push(step("自訂命令列：展開後選圖示", |c| {
        click(c, "", bar_edit_pos("sel.invert", false, None));
        let at = with_app(|a| {
            a.layout = a.compute_layout();
            let panel = a.layout.side_panel?;
            let rows = a.cmdbar_edit_rows();
            let k = rows.iter().position(|r| matches!(r, crate::cmdbaredit::Row::Icons(_, 0)))?;
            let (_, list, _) = a.cmdbar_edit_layout(panel);
            let r = a.bar_row_rect(list, k, a.cmdbar_editor()?.scroll);
            let cell = a.bar_parts(&rows[k], r)[2];
            Some(((cell.l + cell.r) / 2.0, (cell.t + cell.b) / 2.0))
        })
        .flatten();
        click(c, "", at);
    }, |_, a| {
        let it = a.config.cmdbar.iter().find(|i| i.cmd == "sel.invert").ok_or("沒有按鈕")?;
        expect("圖示", it.icon.as_str(), &format!("glyph:{}", zing_core::cmdbar::GLYPHS[2].0))
    }));
    v.push(step("截圖：自訂命令列", |_| {}, |c, _| screenshot(c, "20-自訂命令列")));
    v.push(step("自訂命令列：搜尋「終端機」後點結果加入", |c| {
        with_app(|a| a.bar_query("終端機".into()));
        let rows = with_app(|a| a.cmdbar_edit_rows()).unwrap_or_default();
        let only = rows.iter().filter(|r| matches!(r, crate::cmdbaredit::Row::Avail(..))).count();
        c.notes.push(("bar_search_count", only.to_string()));
        click(c, "", bar_edit_pos(zing_core::cmdbar::TERMINAL, true, None));
    }, |c, a| {
        expect("只列出符合的", c.note("bar_search_count"), Some("1"))?;
        expect("已加入", a.config.cmdbar.iter().any(|i| i.cmd == zing_core::cmdbar::TERMINAL), true)
    }));
    v.push(step("自訂命令列：點 × 清除搜尋", |c| {
        let at = with_app(|a| {
            a.layout = a.compute_layout();
            let s = a.bar_search_rect(a.layout.side_panel?);
            Some((s.r - s.h() / 2.0, (s.t + s.b) / 2.0))
        })
        .flatten();
        click(c, "", at);
    }, |_, a| {
        expect("清除", a.cmdbar_editor().map(|e| e.query.as_str()), Some(""))?;
        expect("又看到目前的按鈕", a.cmdbar_edit_rows().iter().any(|r| matches!(r, crate::cmdbaredit::Row::Item(_))), true)
    }));
    v.push(step("自訂命令列：還原預設後 Esc 關閉", |c| {
        with_app(|a| {
            a.config.cmdbar = zing_core::cmdbar::defaults();
            a.cmdbar_changed();
        });
        key(c, "", VK_ESCAPE);
    }, |_, a| {
        expect("面板關閉", a.cmdbar_editor().is_none(), true)?;
        expect("預設", a.config.cmdbar == zing_core::cmdbar::defaults(), true)
    }));

    // ------------------------------------------------ 鍵盤操作命令列（F10）
    let f10 = VIRTUAL_KEY(0x79);
    v.push(step("命令列鍵盤：F10 移到第一個按鈕並顯示提示", move |c| key(c, "", f10), |_, a| {
        let first = a.layout.cmdbar.as_ref().and_then(|b| b.buttons.first()).map(|b| b.0);
        expect("焦點", a.bar_focus, first)?;
        expect("是「新增」", a.bar_focus.map(|i| a.config.cmdbar[i].cmd.as_str()), Some(zing_core::cmdbar::MENU_NEW))?;
        expect("提示", a.tip.as_ref().is_some_and(|t| t.shown), true)
    }));
    v.push(step("命令列鍵盤：→ 略過不能用的按鈕", |c| key(c, "", VK_RIGHT), |_, a| {
        let i = a.bar_focus.ok_or("沒有焦點")?;
        let it = &a.config.cmdbar[i];
        expect("不是分隔線", it.is_separator(), false)?;
        expect("可以用", it.is_menu() || a.cmd_disabled(&it.cmd).is_none(), true)?;
        expect("移動了", it.cmd != zing_core::cmdbar::MENU_NEW, true)
    }));
    v.push(step("命令列鍵盤：Home 後 Enter 開啟「新增」選單並選取第一項", |c| {
        key(c, "", VK_HOME);
        key(c, "", VK_RETURN);
    }, |_, a| {
        let m = a.menu.as_ref().ok_or("選單沒開")?;
        expect("第一項", m.levels[0].hover, Some(0))
    }));
    v.push(step("命令列鍵盤：選單開著時 ← 換到「⋯」並開啟", |c| key(c, "", VK_LEFT), |_, a| {
        expect("焦點", a.bar_focus.map(|i| a.config.cmdbar[i].cmd.as_str()), Some(zing_core::cmdbar::MENU_MORE))?;
        expect("選單", a.menu.is_some(), true)
    }));
    v.push(step("命令列鍵盤：Esc 關閉選單、留在命令列，再按 Esc 離開", |c| {
        key(c, "", VK_ESCAPE);
        let still = with_app(|a| a.menu.is_none() && a.bar_focus.is_some()).unwrap_or(false);
        c.notes.push(("bar_after_esc", still.to_string()));
        key(c, "", VK_ESCAPE);
    }, |c, a| {
        expect("第一次 Esc", c.note("bar_after_esc"), Some("true"))?;
        expect("離開", a.bar_focus, None)?;
        expect("提示消失", a.tip.is_none(), true)
    }));

    // ------------------------------------------------ 介面語言（隔離模式預設繁體中文；切換後檢查幾處文字並截圖）
    let lang_step = |lang: zing_core::i18n::Lang, refresh: &'static str, fav: &'static str, shot: &'static str| {
        step(
            match lang {
                zing_core::i18n::Lang::En => "語言：切換成英文",
                _ => "語言：切換成日文",
            },
            move |c| {
                with_app(|a| a.set_language(lang));
                key(c, "cs", letter('p'));
            },
            move |c, a| {
                palette_open(a)?;
                let p = a.palette.as_ref().ok_or("面板已關閉")?;
                let title = p.entries.iter().find(|e| e.cmd == "nav.refresh").map(|e| e.title.as_str());
                expect("命令面板的標題", title, Some(refresh))?;
                expect("樹狀列", a.tree.nodes.first().map(|n| n.name.as_str()), Some(fav))?;
                screenshot(c, shot)
            },
        )
    };
    v.push(lang_step(zing_core::i18n::Lang::En, "Refresh", "Favorites", "14-英文介面"));
    v.push(step("語言：關閉命令面板", |_| palette_key(VK_ESCAPE), |_, a| palette_closed(a)));
    v.push(lang_step(zing_core::i18n::Lang::Ja, "最新の情報に更新", "お気に入り", "15-日文介面"));
    v.push(step(
        "語言：回到繁體中文",
        |_| {
            palette_key(VK_ESCAPE);
            with_app(|a| a.set_language(zing_core::i18n::Lang::ZhTw));
        },
        |_, a| {
            palette_closed(a)?;
            expect("樹狀列", a.tree.nodes.first().map(|n| n.name.as_str()), Some("我的最愛"))
        },
    ));

    // ------------------------------------------------ 設定頁（隔離模式只改記憶體中的設定，不寫 config.toml）
    use crate::prefspage::Row;
    let category = |a: &App| a.settings_page().and_then(|p| p.category);
    let group_index = |g: &str| zing_core::prefs::groups().iter().position(|x| *x == g);
    v.push(step("設定：Ctrl+, 開啟設定頁（分類清單）", |c| key(c, "c", VIRTUAL_KEY(0xBC)), move |_, a| {
        expect("設定頁", a.settings_page().is_some(), true)?;
        expect("面板位置", a.layout.side_panel.is_some(), true)?;
        expect("分類清單", category(a), None)?;
        expect("第一列", a.settings_page().and_then(|p| p.rows().first().copied()), Some(Row::Category(0)))
    }));
    v.push(step("截圖：設定的分類", |_| {}, |c, _| screenshot(c, "22-設定分類")));
    v.push(step("設定：點「一般」", |c| click(c, "", settings_category_pos("一般")), move |_, a| {
        expect("分類", category(a), group_index("一般"))?;
        expect("只有這個分類的項目", a.settings_page().is_some_and(|p| p.rows().contains(&Row::Field(3))), false)
    }));
    v.push(step("設定：點開關顯示隱藏檔案", |c| click(c, "", settings_row_pos("show_hidden", None, true)), |_, a| {
        expect("顯示隱藏檔案", a.show_hidden, true)?;
        expect("設定", a.config.show_hidden, true)
    }));
    v.push(step(
        "設定：輸入啟動時開啟的資料夾",
        |c| {
            click(c, "", settings_row_pos("start_path", None, true));
            edit_commit("C:\\Windows");
        },
        |_, a| expect("start_path", a.config.start_path.as_str(), "C:\\Windows"),
    ));
    v.push(step("設定：點標題回到分類清單", |c| click(c, "", settings_title_pos()), move |_, a| expect("分類清單", category(a), None)));
    v.push(step("設定：點「外觀」", |c| click(c, "", settings_category_pos("外觀")), move |_, a| {
        expect("分類", category(a), group_index("外觀"))
    }));
    v.push(step("設定：展開樹狀列位置", |c| click(c, "", settings_row_pos("layout.tree", None, false)), |_, a| {
        let i = zing_core::prefs::PREFS.iter().position(|p| p.key == "layout.tree");
        expect("展開", a.settings_page().and_then(|p| p.expanded), i)
    }));
    v.push(step("截圖：設定頁", |_| {}, |c, _| screenshot(c, "16-設定頁")));
    v.push(step("設定：選右側", |c| click(c, "", settings_row_pos("layout.tree", Some(1), true)), |_, a| {
        expect("樹狀列", a.tree_side, zing_core::config::TreeSide::Right)?;
        expect("收起", a.settings_page().and_then(|p| p.expanded), None)?;
        expect("版面", a.layout.tree.zip(a.layout.panes.first()).is_some_and(|(t, p)| t.l > p.list.l), true)
    }));
    v.push(step("設定：Esc 回到分類清單", |c| key(c, "", VK_ESCAPE), move |_, a| {
        expect("設定頁", a.settings_page().is_some(), true)?;
        expect("分類清單", category(a), None)
    }));
    // 緩慢的擴充功能表格：隔離模式的記錄只在記憶體中，放一個假的
    const SLOW_CLSID: &str = "{5a1e7e57-0000-4000-8000-00000000c0de}";
    let slow_row = || {
        with_app(|a| a.settings_page().and_then(|p| p.rows().into_iter().find(|r| matches!(r, Row::Slow(..)))))
            .flatten()
    };
    let allowed = |a: &App| a.config.context_menu.allow_slow.iter().any(|c| c == SLOW_CLSID);
    v.push(step(
        "設定：「右鍵選單」列出緩慢的擴充功能",
        |c| {
            crate::shellext::test_mark_slow(SLOW_CLSID, "SlowTest", "file", 120);
            crate::shellext::test_mark_slow(SLOW_CLSID, "SlowTest", "background", 80);
            click(c, "", settings_category_pos("右鍵選單"));
        },
        move |_, a| {
            expect("分類", category(a), group_index("右鍵選單"))?;
            let page = a.settings_page().ok_or("設定頁")?;
            let e = page.slow.iter().find(|e| e.clsid == SLOW_CLSID).ok_or("表格中沒有 SlowTest")?;
            expect("類型", e.kinds.as_slice(), ["檔案", "空白處"].as_slice())?;
            expect("耗時", e.ms, 120)
        },
    ));
    v.push(step(
        "設定：表格中切換為放在第一層",
        move |c| {
            let k = with_app(|a| a.settings_page().and_then(|p| p.slow.iter().position(|e| e.clsid == SLOW_CLSID))).flatten();
            let row = slow_row().and_then(|r| match r {
                Row::Slow(i, _) => Some(Row::Slow(i, k?)),
                _ => None,
            });
            click(c, "", row.and_then(|r| settings_pos(r, true)));
        },
        move |_, a| expect("allow_slow", allowed(a), true),
    ));
    v.push(step("截圖：右鍵選單的設定", |_| {}, |c, _| screenshot(c, "23-右鍵選單設定")));
    v.push(step("設定：關閉「隱藏緩慢的擴充功能項目」", |c| click(c, "", settings_row_pos("context_menu.hide_slow", None, true)), |_, a| {
        expect("hide_slow", a.config.context_menu.hide_slow, false)
    }));
    v.push(step(
        "設定：表格變成灰階、不能切換",
        move |c| click(c, "", slow_row().and_then(|r| settings_pos(r, true))),
        move |_, a| {
            expect("可切換", a.slow_table_enabled(), false)?;
            expect("allow_slow 不變", allowed(a), true)
        },
    ));
    v.push(step("截圖：緩慢項目的表格（灰階）", |_| {}, |c, _| screenshot(c, "24-緩慢項目灰階")));
    v.push(step(
        "設定：還原後 Esc 兩次關閉",
        |c| {
            with_app(|a| {
                for (key, value) in [
                    ("show_hidden", "false"),
                    ("layout.tree", "left"),
                    ("start_path", ""),
                    ("context_menu.hide_slow", "true"),
                    ("context_menu.allow_slow", ""),
                ] {
                    let i = zing_core::prefs::PREFS.iter().position(|p| p.key == key).unwrap();
                    a.set_pref(i, value.into());
                }
            });
            key(c, "", VK_ESCAPE);
            key(c, "", VK_ESCAPE);
        },
        |_, a| {
            expect("設定頁", a.settings_page().is_none(), true)?;
            expect("樹狀列", a.tree_side, zing_core::config::TreeSide::Left)?;
            expect("顯示隱藏檔案", a.show_hidden, false)?;
            expect("start_path", a.config.start_path.as_str(), "")?;
            expect("hide_slow", a.config.context_menu.hide_slow, true)?;
            expect("allow_slow", a.config.context_menu.allow_slow.len(), 0)
        },
    ));
    let gear = || with_app(|a| {
        a.layout = a.compute_layout();
        let r = a.layout.settings_btn;
        ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0)
    });
    v.push(step("設定：點工具列的齒輪開啟", move |c| click(c, "", gear()), |_, a| expect("設定頁", a.settings_page().is_some(), true)));
    v.push(step("設定：再點一次關閉", move |c| click(c, "", gear()), |_, a| expect("設定頁", a.settings_page().is_none(), true)));
    // 重新載入設定（config.reload）與設定頁走同一條套用路徑（App::apply_config）
    let reload_with = |f: fn(&mut zing_core::config::Config)| {
        with_app(|a| {
            let mut cfg = a.config.clone();
            // 重新載入以設定檔的主題為準：沿用目前的主題，不影響後面的步驟
            cfg.theme = a.theme_name.clone();
            f(&mut cfg);
            let theme = a.theme.clone();
            a.on_service_msg(crate::services::Msg::ConfigLoaded { config: Box::new(cfg), theme, warnings: Vec::new() });
        });
    };
    v.push(step(
        "設定：重新載入改了 show_hidden 的設定檔",
        move |_| {
            with_app(|a| a.menu_prepared = Some(Default::default()));
            reload_with(|c| {
                c.show_hidden = true;
                c.context_menu.hide_slow = false;
            });
        },
        |_, a| {
            expect("顯示隱藏檔案", a.show_hidden, true)?;
            expect("右鍵選單重新準備", a.menu_prepared.is_none(), true)
        },
    ));
    v.push(step(
        "設定：重新載入還原",
        move |_| reload_with(|c| {
            c.show_hidden = false;
            c.context_menu.hide_slow = true;
        }),
        |_, a| {
            expect("顯示隱藏檔案", a.show_hidden, false)?;
            expect("hide_slow", a.config.context_menu.hide_slow, true)
        },
    ));

    // position = middle：整個面板永遠置中，輸入框（子視窗）跟著移動
    let centered = |a: &App| -> Result<f32, String> {
        let lay = a.palette_layout().ok_or("沒有版面")?;
        let (_, h) = a.gfx.size();
        let (top_gap, bottom_gap) = (lay.card.t, h - lay.card.b);
        if (top_gap - bottom_gap).abs() > 2.0 {
            return Err(format!("上方留白 {top_gap:.0}，下方 {bottom_gap:.0}"));
        }
        // 輸入框子視窗與面板的輸入區對齊
        let hwnd = a.edit.as_ref().ok_or("沒有輸入框")?.hwnd;
        let mut wr = windows::Win32::Foundation::RECT::default();
        let mut pt = windows::Win32::Foundation::POINT::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut wr);
            pt.y = (wr.top + wr.bottom) / 2;
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(a.hwnd, &mut pt);
        }
        let cy = (lay.input.t + lay.input.b) / 2.0 * a.gfx.scale();
        if (pt.y as f32 - cy).abs() > 2.0 {
            return Err(format!("輸入框中心 {}，輸入區中心 {cy:.0}", pt.y));
        }
        Ok(lay.input.t)
    };
    v.push(step(
        "命令面板：position = middle 時整個面板置中",
        |c| {
            with_app(|a| a.config.palette.position = zing_core::config::PalettePosition::Middle);
            key(c, "cs", letter('p'));
        },
        move |c, a| {
            palette_open(a)?;
            let t = centered(a)?;
            c.notes.push(("palette_middle_t", t.to_string()));
            Ok(())
        },
    ));
    v.push(step("命令面板：篩選後仍置中（輸入框跟著移動）", |_| palette_type("重新整理"), move |c, a| {
        let t = centered(a)?;
        if c.note("palette_middle_t") == Some(t.to_string().as_str()) { Err("輸入框沒有移動".into()) } else { Ok(()) }
    }));
    v.push(step(
        "命令面板：關閉並還原設定（middle）",
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
    // 在別處（不經 zing）新增、刪除子資料夾：展開中的節點自動更新
    let outside = path::join(&d, "樹狀列外部新增");
    v.push(step(
        "樹狀列：在別處新增子資料夾，展開中的節點自動出現",
        {
            let outside = outside.clone();
            // 在背景做：測試資料夾可能在慢的硬碟上
            move |_| drop(std::thread::spawn(move || std::fs::create_dir(&outside)))
        },
        {
            let (d, outside) = (d.clone(), outside.clone());
            move |_, a| {
                let k = a.tree.find(&outside).ok_or("樹狀列還沒有新的子資料夾")?;
                expect("子節點深度", a.tree.nodes[k].depth, a.tree.nodes[a.tree.find(&d).ok_or("找不到節點")?].depth + 1)
            }
        },
    ));
    v.push(step(
        "樹狀列：在別處刪除子資料夾，節點自動移除",
        {
            let outside = outside.clone();
            move |_| drop(std::thread::spawn(move || std::fs::remove_dir(&outside)))
        },
        move |_, a| expect("節點", a.tree.find(&outside), None),
    ));
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

    // 收合中的節點不在資料夾監看裡：改名後要丟掉預先載入的舊名稱；我的最愛跟著改
    let renamed_b = path::join(&d, "資料夾B改");
    v.push(step(
        "樹狀列：改名資料夾後收合中的節點與我的最愛更新",
        {
            let dir_b = dir_b.clone();
            move |_| {
                with_app(|a| {
                    a.exec(&format!("favorites.add --path=\"{dir_b}\""));
                    a.submit_op(crate::shell::FileOp::Rename { path: dir_b.clone(), new_name: "資料夾B改".into() });
                });
            }
        },
        {
            let (d, renamed_b) = (d.clone(), renamed_b.clone());
            move |_, a| {
                let i = a.tree.find(&d).ok_or("找不到節點")?;
                let names = a.tree.nodes[i].prefetched.as_ref().map(|c| c.names.clone()).unwrap_or_default();
                if !names.iter().any(|n| n == "資料夾B改") || names.iter().any(|n| n == "資料夾B") {
                    return Err(format!("預先載入的子資料夾：{names:?}"));
                }
                expect("我的最愛", a.favorites.contains(&renamed_b), true)
            }
        },
    ));
    v.push(step(
        "樹狀列：改回原名（清掉我的最愛）",
        {
            let renamed_b = renamed_b.clone();
            move |_| {
                with_app(|a| {
                    a.submit_op(crate::shell::FileOp::Rename { path: renamed_b.clone(), new_name: "資料夾B".into() });
                });
            }
        },
        {
            let dir_b = dir_b.clone();
            move |_, a| {
                if !exists(&dir_b) || !a.favorites.contains(&dir_b) {
                    return Err("沒有改回".into());
                }
                Ok(())
            }
        },
    ));
    v.push(step(
        "樹狀列：移除測試用的我的最愛",
        {
            let dir_b = dir_b.clone();
            move |_| {
                with_app(|a| a.exec(&format!("favorites.remove --path=\"{dir_b}\"")));
            }
        },
        {
            let dir_b = dir_b.clone();
            move |_, a| expect("我的最愛", a.favorites.contains(&dir_b), false)
        },
    ));

    // ------------------------------------------------ 我的最愛的順序
    let fav_order = |a: &App| -> Vec<String> {
        a.tree.nodes.iter().filter(|n| n.kind == crate::model::NodeKind::Favorite).map(|n| n.name.clone()).collect()
    };
    v.push(step(
        "我的最愛：加入兩個資料夾",
        {
            let (dir_a, dir_b) = (dir_a.clone(), dir_b.clone());
            move |_| {
                with_app(|a| {
                    a.exec(&format!("favorites.add --path=\"{dir_a}\""));
                    a.exec(&format!("favorites.add --path=\"{dir_b}\""));
                });
            }
        },
        // 重建樹狀列後要再展開到目前資料夾（完成時焦點會移到它）
        move |_, a| {
            let names = fav_order(a);
            expect("順序", &names[names.len() - 2..], &["資料夾A".to_string(), "資料夾B".to_string()][..])?;
            expect("展開到目前資料夾", a.tree.reveal.is_none(), true)
        },
    ));
    v.push(step(
        "我的最愛：樹狀列 Ctrl+↑ 上移",
        {
            let dir_b = dir_b.clone();
            move |c| {
                with_app(|a| {
                    a.focus = Focus::Tree;
                    let k = a.tree.nodes.iter().position(|n| n.kind == crate::model::NodeKind::Favorite && n.path == dir_b);
                    a.tree.focus = k.unwrap_or(0);
                });
                key(c, "c", VIRTUAL_KEY(0x26));
            }
        },
        move |_, a| {
            let names = fav_order(a);
            expect("順序", &names[names.len() - 2..], &["資料夾B".to_string(), "資料夾A".to_string()][..])?;
            expect("焦點", a.tree.nodes.get(a.tree.focus).map(|n| n.name.as_str()), Some("資料夾B"))?;
            expect("記在工作階段", a.session_snapshot().favorite_order.len(), a.favorites.len())
        },
    ));
    v.push(step(
        "我的最愛：favorites.move 下移",
        {
            let dir_b = dir_b.clone();
            move |_| {
                with_app(|a| a.exec(&format!("favorites.move --by=1 --path=\"{dir_b}\"")));
            }
        },
        move |_, a| {
            let names = fav_order(a);
            expect("順序", &names[names.len() - 2..], &["資料夾A".to_string(), "資料夾B".to_string()][..])
        },
    ));
    v.push(step(
        "我的最愛：移除測試項目",
        {
            let (dir_a, dir_b) = (dir_a.clone(), dir_b.clone());
            move |_| {
                with_app(|a| {
                    a.exec(&format!("favorites.remove --path=\"{dir_a}\""));
                    a.exec(&format!("favorites.remove --path=\"{dir_b}\""));
                    a.focus = Focus::List;
                });
            }
        },
        move |_, a| {
            if fav_order(a).iter().any(|n| n.starts_with("資料夾")) { Err("沒有移除".into()) } else { Ok(()) }
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
                    let _ = harness_io(|| std::fs::create_dir_all(&dir));
                    let _ = harness_io(|| std::fs::write(path::join(&dir, "cover.png"), TINY_PNG));
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
                    let _ = harness_io(|| std::fs::write(path::join(&d, "外部產生.zt"), b"x"));
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
    v.push(
        step(
            "縮圖：手動指定（thumb.set，優先於其他來源）",
            {
                let d = d.clone();
                move |_| {
                    let (file, img) = (path::join(&d, "自訂縮圖.zt"), path::join(&d, "自訂縮圖來源.png"));
                    let _ = harness_io(|| std::fs::write(&file, b"x"));
                    let _ = harness_io(|| std::fs::write(&img, TINY_PNG));
                    with_app(|a| a.exec(&format!("thumb.set --path=\"{file}\" --image=\"{img}\"")));
                }
            },
            {
                let d = d.clone();
                move |_, a| {
                    let px = crate::thumbs::bucket(a.tab().thumb as f32 * a.gfx.scale());
                    let p = path::join(&d, "自訂縮圖.zt");
                    expect("來源", a.thumbs.source(&p, px), Some(crate::thumbs::Source::Custom))?;
                    expect("右鍵選單可移除", a.thumbs.is_custom(&p), true)
                }
            },
        )
        .slow(),
    );
    v.push(
        step(
            "縮圖：改名後手動縮圖仍有效（以檔案 ID 為鍵）",
            {
                let d = d.clone();
                move |_| {
                    let (old, new) = (path::join(&d, "自訂縮圖.zt"), path::join(&d, "自訂縮圖2.zt"));
                    let _ = harness_io(|| std::fs::rename(&old, &new));
                }
            },
            {
                let d = d.clone();
                move |_, a| {
                    let px = crate::thumbs::bucket(a.tab().thumb as f32 * a.gfx.scale());
                    expect("來源", a.thumbs.source(&path::join(&d, "自訂縮圖2.zt"), px), Some(crate::thumbs::Source::Custom))
                }
            },
        )
        .slow(),
    );
    v.push(
        step(
            "縮圖：移除手動縮圖（thumb.clear）",
            {
                let d = d.clone();
                move |_| {
                    with_app(|a| a.exec(&format!("thumb.clear --path=\"{}\"", path::join(&d, "自訂縮圖2.zt"))));
                }
            },
            {
                let d = d.clone();
                move |_, a| {
                    let px = crate::thumbs::bucket(a.tab().thumb as f32 * a.gfx.scale());
                    let src = a.thumbs.source(&path::join(&d, "自訂縮圖2.zt"), px);
                    if src.is_some() && src != Some(crate::thumbs::Source::Custom) { Ok(()) } else { Err(format!("來源 {src:?}")) }
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
                with_app(|a| {
                    a.config.thumbnailers.clear();
                    a.thumbs.set_thumbnailers(Vec::new());
                });
                // 清理在背景做：測試資料夾可能在慢的硬碟上，刪除會被卡頓監看當成 UI 停頓
                // （隔離模式下外部命令的縮圖放在暫存資料夾）
                let (cache, custom) = (crate::thumbs::cache_dir(true), crate::thumbs::custom_dir(true));
                std::thread::spawn(move || {
                    let _ = std::fs::remove_dir_all(path::join(&d, "封面測試"));
                    let _ = std::fs::remove_file(path::join(&d, "外部產生.zt"));
                    let _ = std::fs::remove_file(path::join(&d, "自訂縮圖2.zt"));
                    let _ = std::fs::remove_file(path::join(&d, "自訂縮圖來源.png"));
                    let _ = std::fs::remove_dir_all(cache);
                    let _ = std::fs::remove_dir_all(custom);
                });
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
                    let _ = harness_io(|| std::fs::write(path::join(&d, "預覽圖.png"), TINY_PNG));
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
                let _ = harness_io(|| std::fs::remove_file(path::join(&d, "預覽圖.png")));
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
                let _ = harness_io(|| std::fs::create_dir_all(&bd));
                for n in ["x1.txt", "x2.txt", "y.txt"] {
                    let _ = harness_io(|| std::fs::write(path::join(&bd, n), n));
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
    // 系統選單本身無法在程序內測試：檢查自訂項目，並直接執行它
    v.push(step(
        "批次：多選時右鍵選單有「批次重新命名」",
        |c| {
            key(c, "c", letter('a'));
            let has = with_app(|a| {
                let (items, folder) = (a.tab().target_paths(), a.tab().path.clone());
                let (extras, _) = a.entry_menu_extras(&items, &folder);
                let has = extras.iter().any(|(id, _)| *id == crate::app::MENU_BATCH_RENAME);
                a.on_menu_extra(crate::app::MENU_BATCH_RENAME, String::new());
                has
            });
            c.notes.push(("ctx_batch", has.unwrap_or(false).to_string()));
        },
        |c, a| {
            expect("選單項目", c.note("ctx_batch"), Some("true"))?;
            expect("面板開啟", a.batch.as_ref().map(|b| b.items.len()), Some(3))
        },
    ));
    v.push(step("批次：關閉面板", |_| with_app(|a| a.close_batch()).unwrap_or(()), |_, a| expect("面板關閉", a.batch.is_none(), true)));
    v.push(step(
        "批次：回到測試資料夾並清除",
        {
            let (d, bd) = (d.clone(), batch_dir.clone());
            move |_| {
                with_app(|a| {
                    let p = a.active_pane;
                    a.navigate(p, d.clone(), true);
                });
                let _ = harness_io(|| std::fs::remove_dir_all(&bd));
            }
        },
        {
            let d = d.clone();
            move |_, a| at(a, &d)
        },
    ));

    // ------------------------------------------------ 主題編輯器
    v.push(step("主題：開啟編輯面板", |_| exec("theme.edit"), |_, a| {
        if a.theme_editor().is_none() || a.layout.side_panel.is_none() {
            return Err("面板沒有開啟".into());
        }
        expect("目前主題", a.theme_name.as_str(), "dark")
    }));
    v.push(step("主題：滾輪調整列高（即時套用）", |c| wheel(c, theme_row_pos("列高", false), 120), |_, a| {
        expect("列高", a.theme.metrics.row_height, 23.0)
    }));
    v.push(step("主題：展開網址列的選單", |c| click(c, "", theme_row_pos("網址列", true)), |_, a| {
        let t = crate::themepanel::Target::Field(theme_field("網址列"));
        expect("展開", a.theme_editor().and_then(|e| e.expanded), Some(t))
    }));
    v.push(step(
        "主題：選麵包屑",
        |c| click(c, "", theme_option_pos(crate::themepanel::Target::Field(theme_field("網址列")), 1)),
        |_, a| {
            expect("網址列", a.theme.chrome.address, zing_core::theme::AddressStyle::Breadcrumb)?;
            expect("收起", a.theme_editor().and_then(|e| e.expanded), None)
        },
    ));
    v.push(step("主題：按 ＋ 加大圓角", |c| click(c, "", theme_pos(crate::themepanel::Row::Field(theme_field("圓角")), |r| mid(r.plus))), |_, a| {
        expect("圓角", a.theme.metrics.radius, 1.0)
    }));
    v.push(step("主題：拖曳滑桿到最右", |c| {
        let row = crate::themepanel::Row::Field(theme_field("圓角"));
        let at = theme_pos(row, |r| mid(r.track));
        let end = theme_pos(row, |r| (r.track.r + 30.0, (r.row.t + r.row.b) / 2.0));
        drag(c, at, end);
    }, |_, a| expect("圓角", a.theme.metrics.radius, 16.0)));
    v.push(step("主題：↺ 還原圓角", |c| click(c, "", theme_pos(crate::themepanel::Row::Field(theme_field("圓角")), |r| mid(r.tail))), |_, a| {
        expect("圓角", a.theme.metrics.radius, 0.0)
    }));
    v.push(step(
        "主題：點色票改資料夾顏色",
        |c| {
            click(c, "", theme_row_pos("資料夾", false));
            let t = crate::themepanel::Target::Field(theme_field("資料夾"));
            click(c, "", theme_pos(crate::themepanel::Row::Swatches(t, 0), |r| (r.row.l + 8.0 + 5.0 * r.row.h() + (r.row.h() - 6.0) / 2.0, (r.row.t + r.row.b) / 2.0)));
        },
        |_, a| expect("資料夾", a.theme.color.folder, zing_core::theme::Color::rgb(zing_core::theme::SWATCHES[5])),
    ));
    v.push(step("截圖：主題編輯器的色票", |_| {}, |c, _| screenshot(c, "17-主題色票")));
    v.push(step("主題：游標停在「次要區域」上說明用在哪裡", |c| {
        hover_at(c, theme_row_pos("次要區域", false));
        with_app(|a| a.on_tip_timer());
        let tip = with_app(|a| a.tip.as_ref().filter(|t| t.shown).map(|t| t.text.joined())).flatten().unwrap_or_default();
        c.notes.push(("theme_tip", tip));
    }, |c, _| {
        let tip = c.note("theme_tip").unwrap_or_default();
        if !(tip.starts_with("次要區域") && tip.contains("工具列")) {
            return Err(format!("提示：{tip}"));
        }
        Ok(())
    }));
    v.push(step(
        "主題：新增「圖片」的檔案顏色",
        |c| {
            let _ = with_app(|a| a.theme_escape());
            click(c, "", theme_pos(crate::themepanel::Row::AddRule, |r| (r.row.l + 20.0, (r.row.t + r.row.b) / 2.0)));
            click(c, "", theme_option_pos(crate::themepanel::Target::AddRule, 0));
        },
        |_, a| {
            let rules = &a.theme.color.file_rule;
            expect("規則數", rules.len(), 1)?;
            expect("樣式", rules[0].pattern.as_str(), zing_core::theme::FILE_PRESETS[0].1)?;
            expect("顏色規則已套用", a.rules.len(), 1)
        },
    ));
    v.push(step("主題：換成淺色（保留尺寸、網址列與檔案顏色）", |c| {
        click(c, "", theme_pos(crate::themepanel::Row::Mode, |r| (r.value.l + 20.0, (r.row.t + r.row.b) / 2.0)));
        click(c, "", theme_option_pos(crate::themepanel::Target::Mode, 1));
    }, |_, a| {
        expect("深淺色", a.theme.mode, zing_core::theme::Mode::Light)?;
        expect("背景", a.theme.color.background, zing_core::theme::Theme::light().color.background)?;
        expect("網址列", a.theme.chrome.address, zing_core::theme::AddressStyle::Breadcrumb)?;
        expect("列高", a.theme.metrics.row_height, 23.0)?;
        expect("檔案顏色", a.theme.color.file_rule.len(), 1)
    }));
    v.push(step("主題：換回深色", |c| {
        click(c, "", theme_pos(crate::themepanel::Row::Mode, |r| (r.value.l + 20.0, (r.row.t + r.row.b) / 2.0)));
        click(c, "", theme_option_pos(crate::themepanel::Target::Mode, 0));
    }, |_, a| expect("深淺色", a.theme.mode, zing_core::theme::Mode::Dark)));
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
            click(c, "", theme_button_pos(1));
        },
        |_, a| {
            expect("面板關閉", a.theme_editor().is_none(), true)?;
            expect("列高", a.theme.metrics.row_height, 23.0)
        },
    ));
    v.push(
        step(
            "主題：匯入 .zingtheme",
            |_| {
                let f = zing_core::testdir::dir().join("smoke-import.zingtheme");
                let _ = harness_io(|| std::fs::write(&f, "name = \"smoke-import\"\nstyle = \"fluent\"\n[color]\naccent = \"#00ff00\"\n"));
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
                let _ = harness_io(|| std::fs::write(theme_path("smoke-import"), "name = \"smoke-import\"\nstyle = \"fluent\"\n[color]\naccent = \"#0000ff\"\n"));
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
                let _ = harness_io(|| std::fs::remove_file(theme_path(n)));
            }
            let _ = harness_io(|| std::fs::remove_file(zing_core::testdir::dir().join("smoke-import.zingtheme")));
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

    // ------------------------------------------------ Lua 腳本
    v.push(
        step(
            "Lua：載入腳本（全域函式成為命令，標題取自 --- 註解）",
            |_| {
                let dir = crate::script::scripts_dir();
                let _ = harness_io(|| std::fs::create_dir_all(&dir));
                let _ = harness_io(|| std::fs::write(dir.join("smoke.lua"), SMOKE_LUA));
                exec("script.reload");
            },
            |_, a| {
                let want = ("smoke_cmd".to_string(), "冒煙測試命令".to_string());
                if !a.scripts.commands.contains(&want) {
                    return Err(format!("命令：{:?}", a.scripts.commands));
                }
                expect("on_navigate", a.scripts.on_navigate, true)?;
                let meta = crate::script::scripts_dir().join(".zing").join("zing.d.lua");
                if meta.exists() { Ok(()) } else { Err("沒有寫入自動補全定義".into()) }
            },
        )
        .slow(),
    );
    v.push(
        step(
            "Lua：執行自訂命令（sel、管線、msg）",
            |c| {
                click(c, "", row_pos("a2.txt"));
                exec("smoke_cmd");
            },
            |_, a| expect("狀態列", status(a).as_deref(), Some("smoke:1:a2.txt")),
        )
        .slow(),
    );
    v.push(
        step(
            "Lua：命令面板 = 運算式（惰性串流）",
            |c| {
                key(c, "cs", letter('p'));
                // 一次設定整段文字：逐字輸入在 debug build 單一計時器週期內會超過卡頓門檻
                if let Some(e) = edit_hwnd() {
                    set_text(e, "= seq({1,2,3,4}) | filter(function(x) return x % 2 == 0 end) | map(function(x) return x * 10 end) | list");
                }
                palette_key(VK_RETURN);
            },
            |_, a| expect("狀態列", status(a).as_deref(), Some("= { 20, 40 }")),
        )
        .slow(),
    );

    // ------------------------------------------------ 剪貼簿（開始前已備份使用者的剪貼簿，結束時還原；測試內容不進入剪貼簿歷程）
    const CLIP_TEXT: &str = "zing 冒煙測試：剪貼簿\n第二行";
    let palette_eval = |expr: String| {
        move |c: &mut Ctx| {
            key(c, "cs", letter('p'));
            if let Some(e) = edit_hwnd() {
                set_text(e, &expr);
            }
            palette_key(VK_RETURN);
        }
    };
    let clip_is = |want: String| move |_: &mut Ctx, _: &mut App| expect("剪貼簿", crate::window::clipboard_text(), Some(want.clone()));
    v.push(
        step("剪貼簿：Lua clipboard() 寫入", palette_eval(format!("= clipboard({CLIP_TEXT:?})")), {
            // 剪貼簿裡是 Windows 的換行
            let check = clip_is(CLIP_TEXT.replace('\n', "\r\n"));
            move |c, a| {
                expect("狀態列", status(a).as_deref(), Some("= true"))?;
                check(c, a)
            }
        })
        .slow(),
    );
    v.push(
        step("剪貼簿：Lua clipboard() 讀回（換行為 \\n）", palette_eval(format!("= clipboard() == {CLIP_TEXT:?}")), |_, a| {
            expect("狀態列", status(a).as_deref(), Some("= true"))
        })
        .slow(),
    );
    v.push(
        step(
            "剪貼簿：複製路徑（選取的項目）",
            |_| {
                with_app(|a| {
                    a.exec("sel.all");
                    a.exec("file.copypath");
                });
            },
            {
                let d = d.clone();
                move |c, a| {
                    at(a, &d)?;
                    // 多個項目以換行分隔
                    let want = a.tab().target_paths().join("\r\n");
                    if a.tab().target_paths().len() < 2 {
                        return Err("全選後選取的項目少於兩個".into());
                    }
                    clip_is(want)(c, a)
                }
            },
        )
        .slow(),
    );
    v.push(
        step("Lua：go() 與 on_navigate 事件", |_| exec("smoke_go"), {
            let dir_a = dir_a.clone();
            move |_, a| {
                at(a, &dir_a)?;
                expect("狀態列", status(a).as_deref(), Some("nav:資料夾A"))
            }
        })
        .slow(),
    );
    v.push(step("Lua：回到測試資料夾", |_| exec("nav.back"), {
        let d = d.clone();
        move |_, a| at(a, &d)
    }));
    v.push(step("Lua：ask() 顯示輸入框", |_| exec("smoke_ask"), |_, a| {
        if a.ask.is_some() && a.edit.is_some() { Ok(()) } else { Err(format!("沒有輸入框：ask={} 狀態={:?}", a.ask.is_some(), status(a))) }
    }));
    v.push(
        step(
            "Lua：ask() 回答後腳本繼續",
            |_| {
                if let Some(e) = edit_hwnd() {
                    set_text(e, "abc");
                    key_to(e, "", VK_RETURN);
                }
            },
            |_, a| {
                expect("輸入框", a.ask.is_none(), true)?;
                expect("狀態列", status(a).as_deref(), Some("答案：abc"))
            },
        )
        .slow(),
    );
    // 命令腳本的 {ask:提示}：同樣的提示只問一次，回答後才執行；Esc 取消整個腳本
    v.push(step("命令：{ask:} 先詢問", |_| exec("filter.set --text={ask:篩選}*\nfilter.set --text={ask:篩選}"), |_, a| {
        expect("提示", a.ask.as_ref().map(|q| q.0.as_str()), Some("篩選"))?;
        expect("還沒執行", a.tab().filter.as_str(), "")
    }));
    v.push(step(
        "命令：{ask:} 回答後執行",
        |_| {
            if let Some(e) = edit_hwnd() {
                set_text(e, "報告");
                key_to(e, "", VK_RETURN);
            }
        },
        |_, a| {
            expect("輸入框", a.ask.is_none(), true)?;
            expect("篩選", a.tab().filter.as_str(), "報告")
        },
    ));
    v.push(step("命令：{ask:} Esc 取消", |_| {
        exec("filter.set --text={ask:另一個}");
        palette_key(VK_ESCAPE);
    }, |_, a| {
        expect("輸入框", a.ask.is_none(), true)?;
        expect("篩選不變", a.tab().filter.as_str(), "報告")
    }));
    v.push(step("命令：清除篩選", |_| exec("filter.clear"), |_, a| expect("篩選", a.tab().filter.as_str(), "")));
    v.push(
        step("Lua：錯誤顯示在狀態列並記錄行號", |_| exec("smoke_err"), |_, a| {
            let st = a.status.as_ref().map(|s| (s.0.clone(), s.1));
            match st {
                Some((m, true)) if m.contains("boom") => {}
                other => return Err(format!("狀態列：{other:?}")),
            }
            let log = std::fs::read_to_string(crate::script::scripts_dir().join("error.log")).unwrap_or_default();
            if log.contains("smoke.lua:") { Ok(()) } else {             Err("error.log 沒有檔名與行號".into()) }
        })
        .slow()
        .expect_error(),
    );
    v.push(
        step(
            "Lua：無窮迴圈超過時間上限被中止（UI 不受影響）",
            |_| {
                crate::script::TEST_CALL_LIMIT_MS.store(400, std::sync::atomic::Ordering::Relaxed);
                with_app(|a| a.status = None);
                exec("smoke_loop");
            },
            |_, a| {
                let st = a.status.as_ref().map(|s| s.0.clone()).unwrap_or_default();
                if st.contains("中止") { Ok(()) } else { Err(format!("狀態列：{st}")) }
            },
        )
        .slow()
        .expect_error(),
    );
    v.push(
        step(
            "Lua：協程裡的無窮迴圈也會被中止",
            |_| {
                with_app(|a| a.status = None);
                exec("smoke_coloop");
            },
            |_, a| {
                let st = a.status.as_ref().map(|s| s.0.clone()).unwrap_or_default();
                if st.contains("中止") { Ok(()) } else { Err(format!("狀態列：{st}")) }
            },
        )
        .slow()
        .expect_error(),
    );
    v.push(
        step("Lua：型別錯誤的參數傳給內部函式不會中止程序", |_| exec("smoke_bad"), |_, a| {
            expect("狀態列", status(a).as_deref(), Some("bad ok"))
        })
        .slow(),
    );
    v.push(
        step("Lua：執行外部程式（不顯示主控台視窗，取得結束代碼與輸出）", |_| exec("smoke_proc"), |_, a| {
            expect("狀態列", status(a).as_deref(), Some("proc:3:5:hi"))
        })
        .slow(),
    );
    v.push(
        step(
            "Lua：pick_folder() 選擇資料夾",
            {
                let dir_a = dir_a.clone();
                move |_| {
                    *crate::script::TEST_PICK_FOLDER.lock().unwrap() = Some(dir_a.clone());
                    exec("smoke_pick");
                }
            },
            {
                let dir_a = dir_a.clone();
                move |_, a| expect("狀態列", status(a), Some(format!("pick:{dir_a}")))
            },
        )
        .slow(),
    );
    v.push(
        step("Lua 除錯：連線並設定中斷點", |_| dbg_client::start(), |_, _| {
            let log = dbg_client::log();
            if log.iter().any(|m| m.contains(r#""command":"setBreakpoints""#) && m.contains(r#""verified":true"#)) {
                Ok(())
            } else {
                Err(format!("除錯器回應：{log:?}"))
            }
        })
        .slow(),
    );
    v.push(
        step(
            "Lua 除錯：命令停在中斷點，求值後繼續（暫停期間畫面照常）",
            |_| exec("smoke_dbg"),
            |_, a| {
                let log = dbg_client::log();
                let stopped = log.iter().any(|m| m.contains(r#""event":"stopped""#) && m.contains(r#""reason":"breakpoint""#));
                let frame = log.iter().any(|m| m.contains(r#""name":"smoke_dbg""#) && m.contains("smoke.lua"));
                let value = log.iter().any(|m| m.contains(r#""result":"42""#));
                if !(stopped && frame && value) {
                    return Err(format!("停住 {stopped}、堆疊 {frame}、求值 {value}：{log:?}"));
                }
                expect("狀態列", status(a).as_deref(), Some("dbg:42"))
            },
        )
        .slow(),
    );
    v.push(
        step(
            "Lua：全域表設了 metatable 後仍可呼叫命令",
            |_| exec("smoke_strict"),
            |_, a| expect("狀態列", status(a).as_deref(), Some("strict")),
        )
        .slow(),
    );
    v.push(
        step(
            "Lua：（同上）再執行命令",
            |c| {
                click(c, "", row_pos("a2.txt"));
                exec("smoke_cmd");
            },
            |_, a| expect("狀態列", status(a).as_deref(), Some("smoke:1:a2.txt")),
        )
        .slow(),
    );
    v.push(
        step(
            "Lua：移除測試腳本並重新載入",
            |_| {
                crate::script::TEST_CALL_LIMIT_MS.store(0, std::sync::atomic::Ordering::Relaxed);
                let dir = crate::script::scripts_dir();
                let _ = harness_io(|| std::fs::remove_file(dir.join("smoke.lua")));
                let _ = harness_io(|| std::fs::remove_file(dir.join("error.log")));
                let _ = harness_io(|| std::fs::remove_file(dir.join("zing.log")));
                exec("script.reload");
            },
            |_, a| {
                if a.scripts.has("smoke_cmd") { Err("測試命令還在".into()) } else { Ok(()) }
            },
        )
        .slow(),
    );

    // 狀態列錯誤與 UI 執行緒 I/O 防呆的回報在 finish() 統一檢查
    v.push(step("截圖：結束畫面", |_| {}, |c, _| screenshot(c, "03-結束")));
    v
}
