//! Lua 腳本（Lua 5.4，mlua）。API 說明見 docs/lua.md（由 `zing_core::scriptapi` 產生）。
//!
//! - 腳本在專用的執行緒執行，UI 只送工作、不等待；腳本要求的動作（命令、訊息、詢問）以訊息送回 UI。
//! - 呼叫時一併傳入當下狀態的快照（目前資料夾、選取、焦點）。
//! - 每次執行有 CPU 時間上限（命令 30 秒、縮圖規則 0.3 秒），超過時以指令計數鉤子中止。
//!   以腳本執行緒的 CPU 時間計算：等待 `ask()`、外部程式（`os.execute`）、檔案 I/O 不計，只攔下失控的迴圈。
//! - 全域函式就是命令（標題取自上方的 `---` 註解）；`on_navigate`、`thumbnail` 是事件。
//!
//! release build 是 `panic = "abort"`：Lua 的錯誤（Windows 上是 SEH 展開的 longjmp）只要穿過 Rust 的堆疊框架，
//! 程序就會直接結束。因此錯誤只能在 Lua 內部丟出、由 Lua 內部（pcall）接住：
//! - Rust 提供的函式參數一律收 `Value`、不回傳 `Err`（型別不對時當成空值）；
//! - 時間上限的鉤子是 prelude 裡的 Lua 函式（`debug.sethook`），不用 mlua 的 `set_hook`；
//! - Rust 端讀寫全域變數用 `raw_get`／`raw_set`，不觸發腳本設的 metatable。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mlua::{Lua, LuaOptions, StdLib, Table, Value};
use zing_core::command::{Arg, Invocation, quote_arg};
use zing_core::scriptapi;

use crate::services::{self, Msg, Notifier, Show};

const PRELUDE: &str = include_str!("prelude.lua");
/// 一次命令、事件、運算式的 CPU 時間上限
const CALL_LIMIT: Duration = Duration::from_secs(30);
/// 縮圖規則的 CPU 時間上限（每個縮圖呼叫一次）
const THUMB_LIMIT: Duration = Duration::from_millis(300);
const HOOK_NAMES: [&str; 2] = ["on_navigate", "thumbnail"];

/// 冒煙測試：縮短命令的時間上限
#[cfg(feature = "smoke")]
pub static TEST_CALL_LIMIT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 呼叫時的狀態快照
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub cwd: String,
    pub sel: Vec<String>,
    pub focus: Option<String>,
}

enum Job {
    Load(PathBuf),
    Call { name: String, ctx: Ctx },
    Event { hook: &'static str, arg: String, ctx: Ctx },
    Eval { expr: String, ctx: Ctx },
    Thumb { path: String, reply: SyncSender<Option<String>> },
}

/// 載入完成的結果
pub struct Loaded {
    /// (命令名稱, 標題)
    pub commands: Vec<(String, String)>,
    pub on_navigate: bool,
    pub errors: Vec<String>,
}

/// UI 端：腳本執行緒的入口與已載入的命令
#[derive(Default)]
pub struct Scripts {
    tx: Option<Sender<Job>>,
    pub commands: Vec<(String, String)>,
    pub on_navigate: bool,
    pub loaded: bool,
}

/// 縮圖執行緒用：腳本有 `thumbnail` 時才送出請求
static THUMB_HOOK: AtomicBool = AtomicBool::new(false);
static THUMB_TX: Mutex<Option<Sender<Job>>> = Mutex::new(None);

/// 縮圖規則：回傳要當縮圖的圖片路徑（沒有規則、逾時或回傳 nil 時為 None）。在縮圖背景執行緒呼叫
pub fn thumbnail_rule(path: &str) -> Option<String> {
    if !THUMB_HOOK.load(Ordering::Relaxed) {
        return None;
    }
    let tx = THUMB_TX.lock().ok()?.clone()?;
    let (reply, rx) = sync_channel(1);
    tx.send(Job::Thumb { path: path.to_string(), reply }).ok()?;
    rx.recv_timeout(THUMB_LIMIT + Duration::from_millis(200)).ok().flatten()
}

pub fn scripts_dir() -> PathBuf {
    crate::settings::app_dir().join("scripts")
}

impl Scripts {
    fn send(&mut self, n: &Notifier, job: Job) {
        if self.tx.is_none() {
            let (tx, rx) = channel();
            let n = n.clone();
            let _ = std::thread::Builder::new().name("zing-script".into()).spawn(move || worker(rx, n));
            *THUMB_TX.lock().unwrap() = Some(tx.clone());
            self.tx = Some(tx);
        }
        let _ = self.tx.as_ref().unwrap().send(job);
    }

    pub fn load(&mut self, n: &Notifier) {
        self.send(n, Job::Load(scripts_dir()));
    }

    pub fn has(&self, name: &str) -> bool {
        self.commands.iter().any(|(c, _)| c == name)
    }

    pub fn call(&mut self, n: &Notifier, name: &str, ctx: Ctx) {
        self.send(n, Job::Call { name: name.to_string(), ctx });
    }

    pub fn navigate(&mut self, n: &Notifier, path: &str, ctx: Ctx) {
        if self.on_navigate {
            self.send(n, Job::Event { hook: "on_navigate", arg: path.to_string(), ctx });
        }
    }

    pub fn eval(&mut self, n: &Notifier, expr: &str, ctx: Ctx) {
        self.send(n, Job::Eval { expr: expr.to_string(), ctx });
    }

    pub fn on_loaded(&mut self, l: Loaded) {
        self.commands = l.commands;
        self.on_navigate = l.on_navigate;
        self.loaded = true;
    }
}

// ---------------------------------------------------------------- 腳本執行緒

/// CPU 時間上限（腳本執行緒的 CPU 時間，100 ns 單位）；鉤子每執行一段指令檢查一次。
/// 設定與檢查都在腳本執行緒上
struct Deadline(Arc<AtomicU64>);

/// 目前執行緒用掉的 CPU 時間（核心 + 使用者，100 ns 單位）
fn thread_cpu() -> u64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let ft = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    match unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) } {
        Ok(()) => ft(k) + ft(u),
        Err(_) => 0,
    }
}

impl Deadline {
    fn set(&self, limit: Duration) {
        self.0.store(thread_cpu() + (limit.as_nanos() / 100) as u64, Ordering::Relaxed);
    }
}

fn call_limit() -> Duration {
    #[cfg(feature = "smoke")]
    {
        let ms = TEST_CALL_LIMIT_MS.load(Ordering::Relaxed);
        if ms > 0 {
            return Duration::from_millis(ms);
        }
    }
    CALL_LIMIT
}

fn worker(rx: Receiver<Job>, n: Notifier) {
    // run{ wait = true } 可能經 ShellExecuteEx 啟動（文件、視窗程式），需要 STA
    unsafe {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
    let mut lua: Option<Lua> = None;
    let deadline = Deadline(Arc::new(AtomicU64::new(u64::MAX)));
    for job in rx {
        match job {
            Job::Load(dir) => {
                THUMB_HOOK.store(false, Ordering::Relaxed);
                let (state, loaded) = load(&dir, &n, &deadline);
                THUMB_HOOK.store(state.as_ref().is_some_and(|l| has_fn(l, "thumbnail")), Ordering::Relaxed);
                lua = state;
                n.send(Msg::ScriptsLoaded(loaded));
            }
            Job::Call { name, ctx } => {
                if let Some(l) = &lua {
                    deadline.set(call_limit());
                    set_ctx(l, &ctx);
                    if let Err(e) = call_named(l, &name, Value::Nil) {
                        report(&n, &e);
                    }
                }
            }
            Job::Event { hook, arg, ctx } => {
                if let Some(l) = &lua {
                    deadline.set(call_limit());
                    set_ctx(l, &ctx);
                    let arg = l.create_string(&arg).map(Value::String).unwrap_or(Value::Nil);
                    if let Err(e) = call_named(l, hook, arg) {
                        report(&n, &e);
                    }
                }
            }
            Job::Thumb { path, reply } => {
                let result = lua.as_ref().and_then(|l| {
                    deadline.set(THUMB_LIMIT);
                    let f: mlua::Function = l.globals().raw_get("thumbnail").ok()?;
                    match f.call::<Option<String>>(path) {
                        Ok(v) => v.filter(|s| !s.is_empty()),
                        Err(e) => {
                            report(&n, &e.to_string());
                            None
                        }
                    }
                });
                let _ = reply.send(result);
            }
            Job::Eval { expr, ctx } => {
                let Some(l) = &lua else {
                    n.status("Lua 腳本尚未載入", true);
                    continue;
                };
                deadline.set(call_limit());
                set_ctx(l, &ctx);
                match eval(l, &expr) {
                    Ok(text) => n.status(format!("= {text}"), false),
                    Err(e) => report(&n, &e),
                }
            }
        }
    }
}

fn has_fn(l: &Lua, name: &str) -> bool {
    matches!(l.globals().raw_get::<Value>(name), Ok(Value::Function(_)))
}

/// 以 __call（xpcall + 呼叫堆疊）呼叫全域函式
fn call_named(l: &Lua, name: &str, arg: Value) -> Result<(), String> {
    let f: Value = l.globals().raw_get(name).map_err(|e| e.to_string())?;
    if !matches!(f, Value::Function(_)) {
        return Err(format!("找不到 Lua 函式「{name}」"));
    }
    let call: mlua::Function = l.globals().raw_get("__call").map_err(|e| e.to_string())?;
    let (ok, err): (bool, Value) = call.call((f, arg)).map_err(|e| e.to_string())?;
    if ok { Ok(()) } else { Err(err_text(err)) }
}

/// Lua 錯誤值的文字：字串、Rust 產生的錯誤（例如時間上限）或其他值
fn err_text(v: Value) -> String {
    match v {
        Value::Error(e) => e.to_string(),
        Value::String(_) | Value::Integer(_) | Value::Number(_) => text(&v),
        _ => "（非字串的錯誤值）".into(),
    }
}

/// Rust 函式的參數轉成文字：字串、數字照轉，其他型別當成空字串（不丟錯，見檔案開頭）
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string_lossy(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// 表格的連續整數鍵（raw，不觸發 metatable）
fn seq_texts(v: &Value) -> Vec<String> {
    let Value::Table(t) = v else { return Vec::new() };
    (1..=t.raw_len()).map_while(|i| t.raw_get::<Value>(i).ok().filter(|x| !x.is_nil())).map(|x| text(&x)).collect()
}

fn eval(l: &Lua, expr: &str) -> Result<String, String> {
    let chunk = l.load(format!("return {expr}")).set_name("=REPL").into_function().or_else(|_| l.load(expr).set_name("=REPL").into_function());
    let f = chunk.map_err(|e| e.to_string())?;
    let call: mlua::Function = l.globals().raw_get("__call").map_err(|e| e.to_string())?;
    let (ok, v): (bool, Value) = call.call(f).map_err(|e| e.to_string())?;
    if !ok {
        return Err(err_text(v));
    }
    let show: mlua::Function = l.globals().raw_get("__show").map_err(|e| e.to_string())?;
    show.call::<String>(v).map_err(|e| e.to_string())
}

fn set_ctx(l: &Lua, ctx: &Ctx) {
    let build = || -> mlua::Result<Table> {
        let t = l.create_table()?;
        t.raw_set("cwd", ctx.cwd.as_str())?;
        t.raw_set("sel", l.create_sequence_from(ctx.sel.iter().map(String::as_str))?)?;
        if let Some(f) = &ctx.focus {
            t.raw_set("focus", f.as_str())?;
        }
        Ok(t)
    };
    if let Ok(t) = build() {
        let _ = l.globals().raw_set("__ctx", t);
    }
}

/// 錯誤：狀態列顯示第一行，scripts\error.log 記錄全文（含呼叫堆疊）
fn report(n: &Notifier, e: &str) {
    let first = e.lines().next().unwrap_or(e);
    n.status(format!("Lua：{first}"), true);
    append(&scripts_dir().join("error.log"), e);
}

fn append(file: &Path, text: &str) {
    use std::io::Write;
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {text}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        );
    }
}

/// 函式上方的 `---` 註解當作命令面板的標題
fn titles(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut doc: Option<String> = None;
    for line in src.lines() {
        let t = line.trim();
        if let Some(d) = t.strip_prefix("---") {
            if doc.is_none() {
                doc = Some(d.trim().to_string());
            }
            continue;
        }
        let name = t
            .strip_prefix("function ")
            .and_then(|r| r.split('(').next())
            .or_else(|| t.split_once("= function").map(|(n, _)| n))
            .map(str::trim)
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        if let (Some(n), Some(d)) = (name, doc.take()) {
            out.push((n.to_string(), d));
        }
        doc = None;
    }
    out
}

fn new_lua(n: &Notifier, deadline: &Deadline) -> mlua::Result<Lua> {
    // 腳本是使用者自己放的檔案，可信度與設定檔相同：開放完整的標準函式庫
    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::new()) };
    let g = lua.globals();
    // 以下函式的參數都收 Value、不回傳 Err（見檔案開頭）
    let tx = n.clone();
    g.raw_set(
        "__invoke",
        lua.create_function(move |_, (name, pos, flags): (Value, Value, Value)| {
            let mut args: Vec<Arg> = seq_texts(&pos).into_iter().map(Arg::Positional).collect();
            if let Value::Table(t) = flags {
                for (k, v) in t.pairs::<Value, Value>().flatten() {
                    let v = text(&v);
                    args.push(Arg::Flag(text(&k), (!v.is_empty()).then_some(v)));
                }
            }
            tx.send(Msg::ScriptRun(vec![Invocation { name: text(&name), args }]));
            Ok(())
        })?,
    )?;
    let tx = n.clone();
    g.raw_set("__msg", lua.create_function(move |_, v: Value| {
        tx.status(text(&v), false);
        Ok(())
    })?)?;
    let tx = n.clone();
    g.raw_set("__exec", lua.create_function(move |_, v: Value| {
        tx.send(Msg::ScriptExec(text(&v)));
        Ok(())
    })?)?;
    let tx = n.clone();
    g.raw_set(
        "__ask",
        lua.create_function(move |_, (prompt, default): (Value, Value)| {
            let (reply, rx) = sync_channel(1);
            tx.send(Msg::ScriptAsk { prompt: text(&prompt), default: text(&default), reply });
            // 等使用者回答（不耗 CPU，不計入時間上限）
            Ok(rx.recv().ok().flatten())
        })?,
    )?;
    g.raw_set("__log", lua.create_function(|_, v: Value| {
        append(&scripts_dir().join("zing.log"), &text(&v));
        Ok(())
    })?)?;
    g.raw_set("__is_dir", lua.create_function(|_, v: Value| Ok(Path::new(&text(&v)).is_dir()))?)?;
    add_process_fns(&lua)?;
    // 時間上限：prelude 的鉤子每執行一段指令問一次，超過時由 Lua 丟出錯誤
    let d = deadline.0.clone();
    g.raw_set("__expired", lua.create_function(move |_, ()| Ok(thread_cpu() > d.load(Ordering::Relaxed)))?)?;
    g.raw_set("__commands", lua.create_sequence_from(scriptapi::COMMANDS.iter().map(|c| c.name))?)?;
    drop(g);
    deadline.set(CALL_LIMIT);
    lua.load(PRELUDE).set_name("@prelude.lua").exec()?;
    Ok(lua)
}

/// run{ wait = true }、os.execute、io.popen 用的程序函式（測試也用真的實作）
fn add_process_fns(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // run{ ..., wait = true }：在腳本執行緒啟動並等程式結束（等待不耗 CPU，不計入時間上限）。回傳結束代碼，無法啟動時為 nil 與錯誤訊息
    g.raw_set(
        "__spawn",
        lua.create_function(|_, (pos, console, dir): (Value, Value, Value)| {
            let args = seq_texts(&pos);
            let Some((program, rest)) = args.split_first() else {
                return Ok((None, Some("run 需要程式名稱".to_string())));
            };
            let params = rest.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" ");
            let how = if matches!(console, Value::Boolean(true)) { Show::Normal } else { Show::HideConsole };
            match services::start(program, Some(&params), Some(&text(&dir)), None, how, Default::default()) {
                Ok(s) => Ok((s.wait().map(i64::from), None)),
                Err(e) => Ok((None, Some(format!("無法執行 {program}：{e}")))),
            }
        })?,
    )?;
    // os.execute、io.popen 的替代：經 cmd.exe 執行、不顯示主控台視窗。回傳 (結束代碼, 輸出)，無法啟動時為 (nil, 錯誤訊息)
    g.raw_set(
        "__shell",
        lua.create_function(|l, (line, capture, dir): (Value, Value, Value)| {
            zing_core::guard::io("Lua 執行命令");
            let mut c = services::shell_command(&text(&line), Some(&text(&dir)));
            let r = if matches!(capture, Value::Boolean(true)) {
                c.stderr(std::process::Stdio::null()).output().map(|o| (o.status.code(), Some(o.stdout)))
            } else {
                c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| (s.code(), None))
            };
            Ok(match r {
                Ok((code, out)) => (code.map(|c| Value::Integer(c.into())), out.and_then(|o| l.create_string(o).ok()).map(Value::String)),
                Err(e) => (None, l.create_string(e.to_string()).ok().map(Value::String)),
            })
        })?,
    )?;
    Ok(())
}

fn load(dir: &Path, n: &Notifier, deadline: &Deadline) -> (Option<Lua>, Loaded) {
    let mut loaded = Loaded { commands: Vec::new(), on_navigate: false, errors: Vec::new() };
    let _ = std::fs::create_dir_all(dir.join(".zing"));
    write_if_changed(&dir.join(".zing").join("zing.d.lua"), &scriptapi::luals_meta());
    if !dir.join(".luarc.json").exists() {
        let _ = std::fs::write(dir.join(".luarc.json"), scriptapi::LUARC);
    }
    let lua = match new_lua(n, deadline) {
        Ok(l) => l,
        Err(e) => {
            loaded.errors.push(format!("內建函式庫載入失敗：{e}"));
            return (None, loaded);
        }
    };
    let baseline: std::collections::HashSet<String> =
        lua.globals().pairs::<Value, Value>().flatten().map(|(k, _)| text(&k)).collect();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lua"))).collect())
        .unwrap_or_default();
    files.sort();
    let mut docs = Vec::new();
    for f in &files {
        let name = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                loaded.errors.push(format!("{name}：{e}"));
                continue;
            }
        };
        docs.extend(titles(&src));
        deadline.set(CALL_LIMIT);
        if let Err(e) = lua.load(src.as_str()).set_name(format!("@{name}")).exec() {
            loaded.errors.push(e.to_string());
        }
    }
    for (k, v) in lua.globals().pairs::<Value, Value>().flatten() {
        let Value::String(k) = k else { continue };
        let k = k.to_string_lossy();
        if matches!(v, Value::Function(_)) && !baseline.contains(&k) && !k.starts_with("__") && !HOOK_NAMES.contains(&k.as_str())
        {
            let title = docs.iter().find(|(n, _)| *n == k).map_or_else(|| k.clone(), |(_, t)| t.clone());
            loaded.commands.push((k, title));
        }
    }
    loaded.commands.sort();
    loaded.on_navigate = has_fn(&lua, "on_navigate");
    for e in &loaded.errors {
        append(&dir.join("error.log"), e);
    }
    (Some(lua), loaded)
}

fn write_if_changed(p: &Path, text: &str) {
    if std::fs::read_to_string(p).ok().as_deref() != Some(text) {
        let _ = std::fs::write(p, text);
    }
}

// ---------------------------------------------------------------- UI 端

const ASK_W: f32 = 560.0;

impl crate::app::App {
    /// 呼叫腳本時的狀態快照
    pub fn script_ctx(&self) -> Ctx {
        let tab = self.tab();
        Ctx {
            cwd: tab.path.clone(),
            sel: tab.selected_entries().into_iter().map(|e| tab.full_path(e)).collect(),
            focus: tab.focused_entry().filter(|_| !tab.view.is_empty()).map(|e| tab.full_path(e)),
        }
    }

    pub fn on_scripts_loaded(&mut self, l: Loaded) {
        let (n, errors) = (l.commands.len(), l.errors.len());
        let first = l.errors.first().cloned();
        self.scripts.on_loaded(l);
        match first {
            Some(e) => self.set_status(format!("Lua 腳本有 {errors} 個錯誤：{}", e.lines().next().unwrap_or("")), true),
            None if n > 0 => self.set_status(format!("已載入 {n} 個 Lua 命令"), false),
            None => {}
        }
    }

    /// ask()：畫面中央的輸入框
    pub fn ask_layout(&self) -> Option<(crate::gfx::Rect, crate::gfx::Rect, crate::gfx::Rect)> {
        self.ask.as_ref()?;
        let (w, h) = self.gfx.size();
        let width = ASK_W.min(w - 32.0).max(240.0);
        let rh = self.row_h();
        let input_h = (rh + 10.0).max(30.0);
        let card_h = 14.0 + rh + 8.0 + input_h + 14.0;
        let l = ((w - width) / 2.0).round();
        let t = ((h - card_h) / 2.5).max(6.0).round();
        let card = crate::gfx::Rect::new(l, t, l + width, t + card_h);
        let prompt = crate::gfx::Rect::new(l + 14.0, t + 14.0, l + width - 14.0, t + 14.0 + rh);
        let input = crate::gfx::Rect::new(l + 14.0, prompt.b + 8.0, l + width - 14.0, prompt.b + 8.0 + input_h);
        Some((card, prompt, input))
    }

    pub fn open_ask(&mut self, prompt: String, default: String, reply: SyncSender<Option<String>>) {
        // 前一個沒回答的視為取消
        if let Some((_, old)) = self.ask.take() {
            let _ = old.send(None);
        }
        self.close_palette();
        self.ask = Some((prompt, reply));
        if let Some((_, _, input)) = self.ask_layout() {
            let len = default.encode_utf16().count();
            crate::window::begin_edit(self, input.inset(10.0, 0.0), &default, len, crate::app::EditKind::Ask);
        }
        self.invalidate();
    }

    pub fn finish_ask(&mut self, commit: bool, text: Option<String>) {
        if let Some(e) = self.edit.take() {
            crate::window::destroy_edit(e.hwnd);
        }
        if let Some((_, reply)) = self.ask.take() {
            let _ = reply.send(if commit { text } else { None });
        }
        crate::window::focus_main(self.hwnd);
        self.invalidate();
    }

    pub fn paint_ask(&self) {
        let (Some((prompt, _)), Some((card, pr, input))) = (self.ask.as_ref(), self.ask_layout()) else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius + 2.0;
        let shadow = zing_core::theme::Color { r: 0, g: 0, b: 0, a: 48 };
        g.fill_round(crate::gfx::Rect::new(card.l - 2.0, card.t, card.r + 2.0, card.b + 4.0), radius + 2.0, shadow);
        g.fill_round(card, radius, c.surface);
        g.stroke_round(card, radius, c.border, 1.0);
        g.text(prompt, crate::gfx::Font::Text, pr, c.text);
        g.fill_round(input, self.theme.metrics.radius, c.background);
        g.stroke_round(input, self.theme.metrics.radius, c.accent, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::new()) };
        let g = lua.globals();
        for name in ["__invoke", "__msg", "__exec", "__ask", "__log", "__is_dir", "__expired"] {
            g.set(name, lua.create_function(|_, _: mlua::MultiValue| Ok(())).unwrap()).unwrap();
        }
        add_process_fns(&lua).unwrap();
        g.set("__commands", lua.create_sequence_from(scriptapi::COMMANDS.iter().map(|c| c.name)).unwrap()).unwrap();
        drop(g);
        lua.load(PRELUDE).exec().unwrap();
        lua
    }

    fn run(l: &Lua, src: &str) -> String {
        let show: mlua::Function = l.globals().get("__show").unwrap();
        let v: Value = l.load(src).eval().unwrap();
        show.call(v).unwrap()
    }

    #[test]
    fn lazy_pipeline_and_composition() {
        let l = lua();
        assert_eq!(run(&l, "return seq({1,2,3,4,5}) | filter(function(x) return x % 2 == 1 end) | map(function(x) return x * 10 end) | list"), "{ 10, 30, 50 }");
        // 惰性：只計算需要的項目
        assert_eq!(run(&l, "local n = 0; local s = seq({1,2,3,4}):map(function(x) n = n + 1; return x end); local _ = s | take(2) | list; return n"), "2");
        // Seq 可以重複使用
        assert_eq!(run(&l, "local s = seq({3,1,2}) | sort_by(function(x) return x end); return (s | count) + (s | first)"), "4");
        assert_eq!(run(&l, "return compose(string.upper, path.stem)(\"C:\\\\a\\\\photo.JPG\")"), "PHOTO");
        assert_eq!(run(&l, "return pipe(\"a.TXT\", path.ext, string.upper)"), "TXT");
        assert_eq!(run(&l, "return partial(path.join, \"D:\\\\\")(\"x\")"), "D:\\x");
        assert_eq!(run(&l, "return { 1, 2, 3 } | reduce(function(a, x) return a + x end, 0)"), "6");
        assert_eq!(run(&l, "return seq({1,5,2,7}) | take_while(function(x) return x < 6 end) | skip(1) | list"), "{ 5, 2 }");
    }

    #[test]
    fn immutability() {
        let l = lua();
        assert_eq!(run(&l, "local t = freeze({1, 2}); return pcall(function() t[1] = 9 end)"), "false");
        assert_eq!(run(&l, "local t = seq({1,2}) | list; return #t .. ',' .. t[2]"), "2,2");
        assert_eq!(run(&l, "__ctx = { cwd = 'C:\\\\', sel = {'a','b'} }; return pcall(function() sel()[1] = 'x' end)"), "false");
    }

    #[test]
    fn paths() {
        let l = lua();
        assert_eq!(run(&l, r#"return path.name("C:\\a\\b.tar.gz") .. "|" .. path.stem("b.tar.gz") .. "|" .. path.ext("x.JPG") .. "|" .. path.dir("C:\\a\\b") .. "|" .. path.dir("C:\\a")"#), r"b.tar.gz|b.tar|jpg|C:\a|C:\");
        assert_eq!(run(&l, r#"return path.ext("README") .. "|" .. path.ext(".gitignore")"#), "|");
    }

    #[test]
    fn commands_and_namespaces() {
        let l = lua();
        // sel、filter 同時是函式與命令的命名空間
        assert_eq!(run(&l, "return type(sel.all) .. type(filter.set) .. type(nav.up) .. type(copy)"), "functionfunctionfunctionfunction");
        assert_eq!(run(&l, "__ctx = { cwd = '', sel = {'x'} }; return #sel()"), "1");
        assert_eq!(run(&l, "return { 1, 2 } | filter(function(x) return x > 1 end) | count"), "1");
    }

    #[test]
    fn every_prelude_global_is_documented() {
        let l = lua();
        let documented: std::collections::HashSet<&str> = scriptapi::GLOBALS
            .iter()
            .chain(scriptapi::PRELUDE)
            .chain(scriptapi::COMMANDS)
            .map(|s| s.name.split('.').next().unwrap())
            .collect();
        let std_names = [
            "_G", "_VERSION", "assert", "collectgarbage", "coroutine", "debug", "dofile", "error", "getmetatable", "io",
            "ipairs", "load", "loadfile", "math", "next", "os", "package", "pairs", "pcall", "print", "rawequal", "rawget",
            "rawlen", "rawset", "require", "select", "setmetatable", "string", "table", "tonumber", "tostring", "type",
            "utf8", "warn", "xpcall",
        ];
        for (k, _) in l.globals().pairs::<String, Value>().flatten() {
            if k.starts_with("__") || std_names.contains(&k.as_str()) {
                continue;
            }
            assert!(documented.contains(k.as_str()), "內建函式庫的全域「{k}」沒有寫在 scriptapi.rs");
        }
        for s in scriptapi::GLOBALS.iter().chain(scriptapi::PRELUDE) {
            let v: Value = l.load(format!("return {}", s.name)).eval().unwrap();
            assert!(!matches!(v, Value::Nil), "scriptapi.rs 的「{}」在內建函式庫中不存在", s.name);
        }
    }

    /// run{ wait }、os.execute、io.popen（不顯示主控台視窗的版本）
    #[test]
    fn processes_without_console() {
        let l = lua();
        assert_eq!(run(&l, "return run{ 'cmd', '/c', 'exit 4', wait = true }"), "4");
        assert_eq!(run(&l, "local ok, kind, code = os.execute('exit 5'); return tostring(ok) .. kind .. code"), "nilexit5");
        assert_eq!(run(&l, "return (os.execute('exit 0'))"), "true");
        assert_eq!(run(&l, "return os.execute()"), "true");
        let popen = "local f = io.popen('echo a& echo b'); local r = f:read('l') .. '|' .. f:read('a'); return r .. tostring(f:close())";
        assert_eq!(run(&l, popen), "a|b\ntrue");
        assert_eq!(run(&l, "local n = 0; for _ in io.popen('echo 1& echo 2'):lines() do n = n + 1 end; return n"), "2");
        assert_eq!(run(&l, "local c, e = run{ '不存在的程式-zing', wait = true }; return tostring(c) .. (e and 'err' or '')"), "nilerr");
    }

    /// examples/scripts/extract.lua：以真的 tar.exe 解壓縮（中文路徑、子資料夾、損壞的檔案）
    #[test]
    fn example_extract_script() {
        let _allow = zing_core::guard::allow_io("測試");
        let dir = std::env::temp_dir().join(format!("zing-extract-測試-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("來源");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("甲.txt"), "a").unwrap();
        std::fs::write(src.join("乙.txt"), "b").unwrap();
        let tar = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap()).join(r"System32\tar.exe");
        let pack = |out: &str, file: &str| {
            let st = std::process::Command::new(&tar).arg("-a").arg("-cf").arg(dir.join(out)).arg("-C").arg(&src).arg(file).status();
            assert!(st.unwrap().success(), "建立 {out} 失敗");
        };
        pack("壓縮 1.zip", "甲.txt");
        pack("資料.tar.gz", "乙.txt");
        std::fs::write(dir.join("壞掉.zip"), "not a zip").unwrap();
        std::fs::write(dir.join("說明.txt"), "x").unwrap();

        let l = lua();
        let g = l.globals();
        g.set("__is_dir", l.create_function(|_, p: String| Ok(Path::new(&p).is_dir())).unwrap()).unwrap();
        g.set("__msg", l.create_function(|l, m: String| l.globals().set("last_msg", m)).unwrap()).unwrap();
        // ask() 依序回答 __answers 的內容
        l.load("function __ask() return table.remove(__answers, 1) end").exec().unwrap();
        l.load(include_str!("../../../examples/scripts/extract.lua")).set_name("@extract.lua").exec().unwrap();
        let sel = ["壓縮 1.zip", "資料.tar.gz", "壞掉.zip", "說明.txt"].map(|n| dir.join(n).to_string_lossy().into_owned());
        let ctx = l.create_table().unwrap();
        ctx.set("cwd", dir.to_string_lossy().as_ref()).unwrap();
        ctx.set("sel", sel.to_vec()).unwrap();
        g.set("__ctx", ctx).unwrap();
        drop(g);
        let run = |answers: [&str; 2]| -> String {
            l.globals().set("__answers", answers.to_vec()).unwrap();
            l.load("extract_to()").exec().unwrap();
            l.globals().get("last_msg").unwrap()
        };

        // 每個壓縮檔各自的子資料夾；損壞的檔案回報失敗，其他照常完成
        let out = dir.join("輸出");
        let m = run([&out.to_string_lossy(), "是"]);
        assert!(m.contains("1 個失敗") && m.contains("壞掉.zip"), "{m}");
        assert!(out.join("壓縮 1").join("甲.txt").exists());
        assert!(out.join("資料").join("乙.txt").exists());

        // 直接解壓縮到目標資料夾
        let flat = dir.join("平放");
        run([&flat.to_string_lossy(), "否"]);
        assert!(flat.join("甲.txt").exists() && flat.join("乙.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_titles_from_doc_comments() {
        let t = titles("--- 整理相片\nfunction sort_photos()\nend\nlocal function helper() end\n--- 另一個\nother = function() end\nfunction no_doc() end");
        assert_eq!(t, vec![("sort_photos".to_string(), "整理相片".to_string()), ("other".to_string(), "另一個".to_string())]);
    }

    #[test]
    fn app_commands_match_scriptapi() {
        // app.rs 的命令分派（"名稱" =>）與 scriptapi::COMMANDS 必須一致，文件才不會漏
        let src = include_str!("app.rs").replace("\r\n", "\n");
        let start = src.find("fn run(&mut self, inv: &Invocation)").unwrap();
        let body = &src[start..];
        let end = body.find("\n    }\n").unwrap();
        let mut found = std::collections::BTreeSet::new();
        for line in body[..end].lines() {
            let t = line.trim_start();
            if !t.starts_with('"') || !t.contains("=>") {
                continue;
            }
            for part in t.split("=>").next().unwrap().split('|') {
                let name = part.trim().trim_matches('"');
                if !name.is_empty() && !name.contains(' ') {
                    found.insert(name.to_string());
                }
            }
        }
        let want: std::collections::BTreeSet<String> = scriptapi::COMMANDS.iter().map(|c| c.name.to_string()).collect();
        assert_eq!(found, want, "app.rs 的命令與 scriptapi::COMMANDS 不一致");
    }
}
