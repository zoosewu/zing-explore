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

use mlua::{FromLuaMulti, IntoLuaMulti, Lua, LuaOptions, StdLib, Table, Value};
use zing_core::command::{Arg, Invocation, quote_arg};
use zing_core::scriptapi;

use crate::services::{self, Msg, Notifier, Show};

const PRELUDE: &str = include_str!("prelude.lua");
const DEBUGGER: &str = include_str!("debugger.lua");
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
    /// 除錯器（luadebug.rs）收到的 DAP 訊息
    Debug(String),
    /// 除錯器的連線中斷
    DebugEnd,
}

/// 腳本執行緒的工作佇列。除錯器暫停時只取除錯訊息，其他工作留到繼續之後
struct Queue {
    rx: Receiver<Job>,
    pending: std::cell::RefCell<std::collections::VecDeque<Job>>,
}

impl Queue {
    fn next(&self) -> Option<Job> {
        let job = self.pending.borrow_mut().pop_front();
        job.or_else(|| self.rx.recv().ok())
    }

    /// 下一個除錯訊息。`block` 時等到有訊息（暫停中）；否則只看已收到的（執行中），沒有時回傳 None。
    /// 連線中斷：等待時回傳 None，不等待時回傳空字串。縮圖規則直接回答 None（縮圖執行緒在等）
    fn debug_message(&self, block: bool) -> Option<String> {
        loop {
            let job = if block { self.rx.recv().ok()? } else { self.rx.try_recv().ok()? };
            match job {
                Job::Debug(m) => return Some(m),
                Job::DebugEnd => return if block { None } else { Some(String::new()) },
                Job::Thumb { reply, .. } => {
                    let _ = reply.send(None);
                }
                other => self.pending.borrow_mut().push_back(other),
            }
        }
    }
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
    /// 隔離模式（冒煙測試）：除錯器的管道名稱加上程序 ID
    isolated: bool,
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
            let dbg = tx.clone();
            crate::luadebug::start(crate::luadebug::pipe_name(self.isolated), move |m| {
                let _ = dbg.send(m.map_or(Job::DebugEnd, Job::Debug));
            });
            *THUMB_TX.lock().unwrap() = Some(tx.clone());
            self.tx = Some(tx);
        }
        let _ = self.tx.as_ref().unwrap().send(job);
    }

    pub fn load(&mut self, n: &Notifier, isolated: bool) {
        self.isolated = isolated;
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
    let q = std::rc::Rc::new(Queue { rx, pending: Default::default() });
    while let Some(job) = q.next() {
        match job {
            Job::Load(dir) => {
                PIPES.with(|p| p.borrow_mut().1.clear());
                THUMB_HOOK.store(false, Ordering::Relaxed);
                // 除錯器的連線狀態（中斷點）帶到新的 Lua 狀態
                let debug_state = match lua.as_ref().and_then(|l| debugger_call(l, "export", Value::Nil)) {
                    Some(Value::String(s)) => Some(s.to_string_lossy()),
                    _ => None,
                };
                let (state, loaded) = load(&dir, &n, &deadline, &q);
                THUMB_HOOK.store(state.as_ref().is_some_and(|l| has_fn(l, "thumbnail")), Ordering::Relaxed);
                if let (Some(l), Some(s)) = (&state, debug_state)
                    && let Ok(s) = l.create_string(&s)
                {
                    debugger_call(l, "import", Value::String(s));
                }
                lua = state;
                n.send(Msg::ScriptsLoaded(loaded));
            }
            Job::Debug(m) => {
                if let Some(l) = &lua {
                    deadline.set(call_limit());
                    let m = l.create_string(&m).map(Value::String).unwrap_or(Value::Nil);
                    debugger_call(l, "handle", m);
                }
            }
            Job::DebugEnd => {
                if let Some(l) = &lua {
                    debugger_call(l, "handle", Value::Nil);
                }
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
                    n.status(tr!("Lua 腳本尚未載入"), true);
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

/// 呼叫 debugger.lua 的函式（__debugger.名稱）
fn debugger_call(l: &Lua, name: &str, arg: Value) -> Option<Value> {
    let d: Table = l.globals().raw_get("__debugger").ok()?;
    let f: mlua::Function = d.raw_get(name).ok()?;
    f.call(arg).ok()
}

fn has_fn(l: &Lua, name: &str) -> bool {
    matches!(l.globals().raw_get::<Value>(name), Ok(Value::Function(_)))
}

/// 以 __call（xpcall + 呼叫堆疊）呼叫全域函式
fn call_named(l: &Lua, name: &str, arg: Value) -> Result<(), String> {
    let f: Value = l.globals().raw_get(name).map_err(|e| e.to_string())?;
    if !matches!(f, Value::Function(_)) {
        return Err(tr!("找不到 Lua 函式「{name}」", name));
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
        _ => tr!("（非字串的錯誤值）").into(),
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
    n.status(tr!("Lua：{first}", first), true);
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

/// Rust 提供給 Lua 的函式一律經過這裡（clippy.toml 禁止直接呼叫 `create_function`）：
/// 參數只能是轉換不會失敗的 `Value` 組合（`SafeArgs`），回傳值不含錯誤，Lua 的錯誤就不會從 Rust 框架丟出（見檔案開頭）
#[allow(clippy::disallowed_methods)]
fn lua_fn<A: SafeArgs, R: IntoLuaMulti>(lua: &Lua, f: impl Fn(&Lua, A) -> R + 'static) -> mlua::Result<mlua::Function> {
    lua.create_function(move |l, a: A| Ok(f(l, a)))
}

/// 從 Lua 轉換時不會失敗的參數型別（缺少的參數是 nil，多的忽略）
trait SafeArgs: FromLuaMulti {}
impl SafeArgs for () {}
impl SafeArgs for Value {}
impl SafeArgs for (Value, Value) {}
impl SafeArgs for (Value, Value, Value) {}

fn new_lua(n: &Notifier, deadline: &Deadline, q: &std::rc::Rc<Queue>) -> mlua::Result<Lua> {
    // 腳本是使用者自己放的檔案，可信度與設定檔相同：開放完整的標準函式庫
    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::new()) };
    let g = lua.globals();
    let tx = n.clone();
    g.raw_set(
        "__invoke",
        lua_fn(&lua, move |_, (name, pos, flags): (Value, Value, Value)| {
            let mut args: Vec<Arg> = seq_texts(&pos).into_iter().map(Arg::Positional).collect();
            if let Value::Table(t) = flags {
                for (k, v) in t.pairs::<Value, Value>().flatten() {
                    let v = text(&v);
                    args.push(Arg::Flag(text(&k), (!v.is_empty()).then_some(v)));
                }
            }
            tx.send(Msg::ScriptRun(vec![Invocation { name: text(&name), args }]));
        })?,
    )?;
    let tx = n.clone();
    g.raw_set("__msg", lua_fn(&lua, move |_, v: Value| tx.status(text(&v), false))?)?;
    let tx = n.clone();
    g.raw_set("__exec", lua_fn(&lua, move |_, v: Value| tx.send(Msg::ScriptExec(text(&v))))?)?;
    // ask()、pick_folder()：等使用者回答（不耗 CPU，不計入時間上限）
    let tx = n.clone();
    g.raw_set(
        "__ask",
        lua_fn(&lua, move |_, (prompt, default): (Value, Value)| {
            let (reply, rx) = sync_channel(1);
            tx.send(Msg::ScriptAsk { prompt: text(&prompt), default: text(&default), reply });
            rx.recv().ok().flatten()
        })?,
    )?;
    let tx = n.clone();
    g.raw_set(
        "__pick_folder",
        lua_fn(&lua, move |_, (title, default): (Value, Value)| {
            let (reply, rx) = sync_channel(1);
            tx.send(Msg::ScriptPickFolder { title: text(&title), default: text(&default), reply });
            rx.recv().ok().flatten()
        })?,
    )?;
    g.raw_set("__log", lua_fn(&lua, |_, v: Value| append(&scripts_dir().join("zing.log"), &text(&v)))?)?;
    let tx = n.clone();
    g.raw_set(
        "__clipboard_set",
        lua_fn(&lua, move |_, v: Value| {
            let (reply, rx) = sync_channel(1);
            tx.send(Msg::ScriptClipboard { text: lf_to_crlf(&text(&v)), reply });
            // ShellHost 忙碌（卡住的 Shell 擴充功能）時不無限等待
            rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false)
        })?,
    )?;
    add_system_fns(&lua)?;
    add_process_fns(&lua)?;
    // 時間上限：prelude 的鉤子每執行一段指令問一次，超過時由 Lua 丟出錯誤
    let d = deadline.0.clone();
    g.raw_set("__expired", lua_fn(&lua, move |_, ()| thread_cpu() > d.load(Ordering::Relaxed))?)?;
    g.raw_set("__commands", lua.create_sequence_from(scriptapi::COMMANDS.iter().map(|c| c.name))?)?;
    drop(g);
    deadline.set(CALL_LIMIT);
    lua.load(PRELUDE).set_name("@prelude.lua").exec()?;
    add_debug_fns(&lua, q.clone(), crate::luadebug::send)?;
    Ok(lua)
}

/// 檔案系統與剪貼簿：__is_dir、__stat、__clipboard_get（寫入剪貼簿需要 UI，見 new_lua）
fn add_system_fns(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.raw_set("__is_dir", lua_fn(lua, |_, v: Value| Path::new(&text(&v)).is_dir())?)?;
    g.raw_set("__stat", lua_fn(lua, |l, v: Value| stat(l, &text(&v)))?)?;
    g.raw_set("__clipboard_get", lua_fn(lua, |_, ()| crate::window::clipboard_text().map(|s| s.replace("\r\n", "\n")))?)?;
    // prelude.lua、debugger.lua 的訊息翻譯（zing_core::i18n）
    g.raw_set("__tr", lua_fn(lua, |_, v: Value| zing_core::i18n::tr_str(&text(&v)).into_owned())?)?;
    Ok(())
}

/// stat(path)：大小、時間（Unix 秒數，可直接給 os.date）、屬性；失敗時回傳 nil 與錯誤訊息
fn stat(l: &Lua, p: &str) -> (Option<Table>, Option<String>) {
    use std::os::windows::fs::MetadataExt;
    zing_core::guard::io("Lua stat()");
    let m = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(e) => return (None, Some(e.to_string())),
    };
    let secs = |t: std::io::Result<std::time::SystemTime>| {
        t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64)
    };
    let attrs = m.file_attributes();
    let t = (|| -> mlua::Result<Table> {
        let t = l.create_table()?;
        t.raw_set("size", if m.is_dir() { 0 } else { m.len() as i64 })?;
        t.raw_set("modified", secs(m.modified()))?;
        t.raw_set("created", secs(m.created()))?;
        t.raw_set("accessed", secs(m.accessed()))?;
        t.raw_set("is_dir", m.is_dir())?;
        t.raw_set("readonly", attrs & 0x1 != 0)?;
        t.raw_set("hidden", attrs & 0x2 != 0)?;
        t.raw_set("system", attrs & 0x4 != 0)?;
        Ok(t)
    })();
    match t {
        Ok(t) => (Some(t), None),
        Err(e) => (None, Some(e.to_string())),
    }
}

/// 剪貼簿用 Windows 的換行
fn lf_to_crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// 除錯器（debugger.lua）與它的傳輸函式。`send` 送出一個 DAP 訊息（測試用自己的版本）
fn add_debug_fns(lua: &Lua, q: std::rc::Rc<Queue>, send: impl Fn(&str) -> bool + 'static) -> mlua::Result<()> {
    let g = lua.globals();
    g.raw_set("__dbg_send", lua_fn(lua, move |_, v: Value| send(&text(&v)))?)?;
    let q2 = q.clone();
    g.raw_set("__dbg_wait", lua_fn(lua, move |_, ()| q2.debug_message(true))?)?;
    g.raw_set("__dbg_poll", lua_fn(lua, move |_, ()| q.debug_message(false))?)?;
    g.raw_set("__scripts_dir", scripts_dir().to_string_lossy().as_ref())?;
    g.raw_set("__prelude_source", PRELUDE)?;
    drop(g);
    lua.load(DEBUGGER).set_name("=debugger").exec()
}

/// io.popen 開啟的程序（腳本執行緒）。重新載入腳本時全部放掉
struct Pipe {
    child: std::process::Child,
    out: Option<std::io::BufReader<std::process::ChildStdout>>,
    inp: Option<std::process::ChildStdin>,
}

thread_local! {
    static PIPES: std::cell::RefCell<(i64, std::collections::HashMap<i64, Pipe>)> = Default::default();
}

fn with_pipe<R>(id: &Value, f: impl FnOnce(&mut Pipe) -> R) -> Option<R> {
    let Value::Integer(id) = id else { return None };
    PIPES.with(|p| p.borrow_mut().1.get_mut(id).map(f))
}

/// io.popen 的 read：格式同 Lua（"l"、"L"、"a"、"n"、位元組數）；文字模式，\r\n 轉成 \n
fn pipe_read(l: &Lua, r: &mut std::io::BufReader<std::process::ChildStdout>, fmt: &Value) -> Value {
    use std::io::{BufRead, Read};
    let string = |b: Vec<u8>| l.create_string(b).map_or(Value::Nil, Value::String);
    let count = match fmt {
        Value::Integer(n) => Some((*n).max(0) as u64),
        Value::Number(n) => Some(n.max(0.0) as u64),
        _ => None,
    };
    if let Some(n) = count {
        let mut b = Vec::new();
        let eof = r.fill_buf().map_or(true, |x| x.is_empty());
        if eof || r.by_ref().take(n).read_to_end(&mut b).is_err() {
            return Value::Nil;
        }
        return string(b);
    }
    let f = text(fmt);
    match f.trim_start_matches('*').chars().next().unwrap_or('l') {
        'a' => {
            let mut b = Vec::new();
            let _ = r.read_to_end(&mut b);
            string(crlf_to_lf(&b))
        }
        'n' => {
            let mut tok = Vec::new();
            while let Ok(buf) = r.fill_buf() {
                let Some(&c) = buf.first() else { break };
                if c.is_ascii_whitespace() && !tok.is_empty() {
                    break;
                }
                if !c.is_ascii_whitespace() {
                    tok.push(c);
                }
                r.consume(1);
            }
            let t = String::from_utf8_lossy(&tok);
            t.parse::<i64>().map(Value::Integer).or_else(|_| t.parse::<f64>().map(Value::Number)).unwrap_or(Value::Nil)
        }
        c => {
            let mut b = Vec::new();
            if r.read_until(b'\n', &mut b).map_or(true, |n| n == 0) {
                return Value::Nil;
            }
            let newline = b.last() == Some(&b'\n');
            if newline {
                b.pop();
                if b.last() == Some(&b'\r') {
                    b.pop();
                }
                if c == 'L' {
                    b.push(b'\n');
                }
            }
            string(b)
        }
    }
}

fn crlf_to_lf(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    for (i, &c) in b.iter().enumerate() {
        if !(c == b'\r' && b.get(i + 1) == Some(&b'\n')) {
            out.push(c);
        }
    }
    out
}

/// run{ wait = true }、os.execute、io.popen 用的程序函式（測試也用真的實作）
fn add_process_fns(lua: &Lua) -> mlua::Result<()> {
    use std::process::Stdio;
    let g = lua.globals();
    // run{ ..., wait = true }：在腳本執行緒啟動並等程式結束（等待不耗 CPU，不計入時間上限）。回傳結束代碼，無法啟動時為 nil 與錯誤訊息
    g.raw_set(
        "__spawn",
        lua_fn(lua, |_, (pos, console, dir): (Value, Value, Value)| {
            let args = seq_texts(&pos);
            let Some((program, rest)) = args.split_first() else {
                return (None, Some(tr!("run 需要程式名稱").to_string()));
            };
            let params = rest.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" ");
            let how = if matches!(console, Value::Boolean(true)) { Show::Normal } else { Show::HideConsole };
            match services::start(program, Some(&params), Some(&text(&dir)), None, how, Default::default()) {
                Ok(s) => (s.wait().map(i64::from), None),
                Err(e) => (None, Some(tr!("無法執行 {program}：{e}", program, e))),
            }
        })?,
    )?;
    // os.execute：經 cmd.exe 執行、不顯示主控台視窗。回傳結束代碼，無法啟動時為 nil 與錯誤訊息
    g.raw_set(
        "__shell",
        lua_fn(lua, |_, (line, dir): (Value, Value)| {
            zing_core::guard::io("Lua 執行命令");
            let mut c = services::shell_command(&text(&line), Some(&text(&dir)));
            match c.stdout(Stdio::null()).stderr(Stdio::null()).status() {
                Ok(s) => (s.code().map(i64::from), None),
                Err(e) => (None, Some(e.to_string())),
            }
        })?,
    )?;
    // io.popen：mode 為 "r" 時讀取命令的輸出（邊執行邊讀），"w" 時寫入命令的輸入。回傳代號，無法啟動時為 nil 與錯誤訊息
    g.raw_set(
        "__popen_open",
        lua_fn(lua, |_, (line, mode, dir): (Value, Value, Value)| {
            zing_core::guard::io("Lua 執行命令");
            let write = text(&mode) == "w";
            let mut c = services::shell_command(&text(&line), Some(&text(&dir)));
            c.stderr(Stdio::null());
            if write {
                c.stdin(Stdio::piped()).stdout(Stdio::null());
            } else {
                c.stdout(Stdio::piped());
            }
            match c.spawn() {
                Ok(mut child) => {
                    let out = child.stdout.take().map(std::io::BufReader::new);
                    let inp = child.stdin.take();
                    let id = PIPES.with(|p| {
                        let mut p = p.borrow_mut();
                        p.0 += 1;
                        let id = p.0;
                        p.1.insert(id, Pipe { child, out, inp });
                        id
                    });
                    (Some(id), None)
                }
                Err(e) => (None, Some(e.to_string())),
            }
        })?,
    )?;
    g.raw_set(
        "__popen_read",
        lua_fn(lua, |l, (id, fmt): (Value, Value)| {
            with_pipe(&id, |p| p.out.as_mut().map_or(Value::Nil, |r| pipe_read(l, r, &fmt))).unwrap_or(Value::Nil)
        })?,
    )?;
    g.raw_set(
        "__popen_write",
        lua_fn(lua, |_, (id, data): (Value, Value)| {
            use std::io::Write;
            let bytes = match &data {
                Value::String(s) => s.as_bytes().to_vec(),
                v => text(v).into_bytes(),
            };
            with_pipe(&id, |p| p.inp.as_mut().is_some_and(|w| w.write_all(&bytes).is_ok())).unwrap_or(false)
        })?,
    )?;
    // 關閉：wait 時等命令結束並回傳結束代碼；否則（被回收時）只放掉，不等待
    g.raw_set(
        "__popen_close",
        lua_fn(lua, |_, (id, wait): (Value, Value)| {
            let Value::Integer(key) = id else { return None };
            let mut p = PIPES.with(|p| p.borrow_mut().1.remove(&key))?;
            if !matches!(wait, Value::Boolean(true)) {
                return None;
            }
            drop(p.inp.take());
            drop(p.out.take());
            p.child.wait().ok().and_then(|s| s.code()).map(i64::from)
        })?,
    )?;
    Ok(())
}

fn load(dir: &Path, n: &Notifier, deadline: &Deadline, q: &std::rc::Rc<Queue>) -> (Option<Lua>, Loaded) {
    let mut loaded = Loaded { commands: Vec::new(), on_navigate: false, errors: Vec::new() };
    let _ = std::fs::create_dir_all(dir.join(".zing"));
    write_if_changed(&dir.join(".zing").join("zing.d.lua"), &scriptapi::luals_meta(zing_core::i18n::lang()));
    // 沒有或仍是舊版 zing 寫的（使用者沒改過）時寫入新版
    let luarc = dir.join(".luarc.json");
    let current = std::fs::read_to_string(&luarc).ok().map(|s| s.replace("\r\n", "\n"));
    if current.as_deref().is_none_or(|c| scriptapi::LUARC_OLD.contains(&c)) {
        let _ = std::fs::write(&luarc, scriptapi::LUARC);
    }
    let launch = dir.join(".vscode").join("launch.json");
    if !launch.exists() {
        let _ = std::fs::create_dir_all(dir.join(".vscode"));
        let _ = std::fs::write(launch, scriptapi::LAUNCH_JSON);
    }
    let lua = match new_lua(n, deadline, q) {
        Ok(l) => l,
        Err(e) => {
            loaded.errors.push(tr!("內建函式庫載入失敗：{e}", e));
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
                loaded.errors.push(tr!("{name}：{e}", name, e));
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
            Some(e) => self.set_status(tr!("Lua 腳本有 {errors} 個錯誤：{first}", errors, first = e.lines().next().unwrap_or("")), true),
            None if n > 0 => self.set_status(tr!("已載入 {n} 個 Lua 命令", n), false),
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

    pub fn open_ask(&mut self, prompt: String, default: String, reply: crate::app::AskReply) {
        // 前一個沒回答的視為取消
        if let Some((_, crate::app::AskReply::Script(old))) = self.ask.take() {
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
        crate::window::focus_main(self.hwnd);
        match self.ask.take() {
            Some((_, crate::app::AskReply::Script(reply))) => {
                let _ = reply.send(if commit { text } else { None });
            }
            Some((prompt, crate::app::AskReply::Command { script, mut answers })) => match (commit, text) {
                (true, Some(t)) => {
                    answers.push((prompt, t));
                    self.exec_with(&script, answers);
                }
                _ => self.set_status(tr!("已取消命令"), false),
            },
            None => {}
        }
        self.invalidate();
    }

    /// pick_folder()：在 UI 執行緒的模態動作中顯示（見 window.rs 的 run_modal）
    pub fn open_pick_folder(&mut self, title: String, default: String, reply: SyncSender<Option<String>>) {
        self.close_palette();
        self.modal = Some(crate::app::Modal::PickFolder { title, default, reply });
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

/// 冒煙測試：下一個資料夾選擇對話方塊直接回答這個路徑（不顯示對話方塊）
#[cfg(feature = "smoke")]
pub static TEST_PICK_FOLDER: Mutex<Option<String>> = Mutex::new(None);

/// 選擇資料夾的對話方塊（模態，UI 執行緒）；取消時為 None
pub fn folder_dialog(owner: windows::Win32::Foundation::HWND, title: &str, default: &str) -> Option<String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
        SIGDN_FILESYSPATH,
    };
    use windows::core::PCWSTR;
    use zing_core::text::wide_z;
    #[cfg(feature = "smoke")]
    if let Some(p) = TEST_PICK_FOLDER.lock().ok().and_then(|mut t| t.take()) {
        return Some(p);
    }
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = d.GetOptions().unwrap_or_default();
        let _ = d.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST);
        if !title.is_empty() {
            let t = wide_z(title);
            let _ = d.SetTitle(PCWSTR(t.as_ptr()));
        }
        if !default.is_empty() {
            let p = wide_z(default);
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(p.as_ptr()), None) {
                let _ = d.SetFolder(&item);
            }
        }
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

// 測試在 unwind 下執行，直接操作 Lua 狀態不受 clippy.toml 的限制
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::new()) };
        let g = lua.globals();
        for name in ["__invoke", "__msg", "__exec", "__ask", "__pick_folder", "__log", "__is_dir", "__expired"] {
            g.set(name, lua.create_function(|_, _: mlua::MultiValue| Ok(())).unwrap()).unwrap();
        }
        add_system_fns(&lua).unwrap();
        add_process_fns(&lua).unwrap();
        g.set("__commands", lua.create_sequence_from(scriptapi::COMMANDS.iter().map(|c| c.name)).unwrap()).unwrap();
        drop(g);
        lua.load(PRELUDE).set_name("@prelude.lua").exec().unwrap();
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

    /// io.popen：邊執行邊讀取、各種讀取格式、寫入模式、結束狀態
    #[test]
    fn popen_streams() {
        let _allow = zing_core::guard::allow_io("測試");
        let l = lua();
        // 第一行在命令結束（約 2 秒）前就讀得到
        let t = std::time::Instant::now();
        let first = run(&l, "pf = io.popen('echo first& ping -n 3 127.0.0.1 >nul& echo second& exit 2'); return pf:read()");
        assert_eq!(first, "first");
        assert!(t.elapsed() < std::time::Duration::from_millis(1500), "{:?}", t.elapsed());
        assert_eq!(run(&l, "local s = pf:read('L'); local ok, kind, code = pf:close(); return s .. tostring(ok) .. kind .. code"), "second\nnilexit2");
        assert_eq!(run(&l, "local a, b, c = io.popen('echo 12 3.5'):read('n', 'n', 'n'); return a + b .. tostring(c)"), "15.5nil");
        assert_eq!(run(&l, "local f = io.popen('echo ab& echo cd'); return f:read(1) .. '|' .. f:read('a')"), "a|b\ncd\n");
        assert_eq!(run(&l, "local f <close> = io.popen('echo x'); return f:read()"), "x");
        assert_eq!(run(&l, "return select(2, pcall(io.popen, 'echo', 'rw')):match('模式') ~= nil"), "true");
        // 寫入模式：寫進命令的標準輸入
        let out = zing_core::testdir::dir().join(format!("zing-popen-{}.txt", std::process::id()));
        let lua_path = out.display().to_string().replace('\\', "\\\\");
        let src = format!("local f = io.popen('findstr /r \"^\" > \"{lua_path}\"', 'w'); f:write('甲\\n', 2, '\\n'); return tostring(f:close())");
        assert_eq!(run(&l, &src), "true");
        let got = std::fs::read_to_string(&out).unwrap().replace("\r\n", "\n");
        let _ = std::fs::remove_file(&out);
        assert_eq!(got, "甲\n2\n");
    }

    /// 除錯器：以預先排好的 DAP 請求驅動（暫停時依序取用），驗證中斷點、條件、堆疊、變數、求值、單步、錯誤與執行中暫停
    #[test]
    fn debugger_session() {
        let (tx, rx) = channel();
        let q = std::rc::Rc::new(Queue { rx, pending: Default::default() });
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        // 每次暫停（送出 stopped 事件）時才放入那次要用的請求，執行中的輪詢不會提早取走
        let batches = std::rc::Rc::new(std::cell::RefCell::new(std::collections::VecDeque::<Vec<String>>::new()));
        let l = lua();
        let (out, pending, feed) = (sent.clone(), batches.clone(), tx.clone());
        add_debug_fns(&l, q, move |m| {
            out.borrow_mut().push(m.to_string());
            if m.contains(r#""event":"stopped""#)
                && let Some(batch) = pending.borrow_mut().pop_front()
            {
                for r in batch {
                    feed.send(Job::Debug(r)).unwrap();
                }
            }
            true
        })
        .unwrap();
        let req = |seq: u32, cmd: &str, args: &str| format!(r#"{{"seq":{seq},"type":"request","command":"{cmd}","arguments":{args}}}"#);
        let file = scripts_dir().join("dbgtest.lua").to_string_lossy().replace('\\', "\\\\");
        let idle = |m: String| {
            let m = l.create_string(&m).unwrap();
            debugger_call(&l, "handle", Value::String(m));
        };
        idle(req(1, "initialize", "{}"));
        idle(req(2, "attach", "{}"));
        idle(req(3, "setBreakpoints", &format!(r#"{{"source":{{"path":"{file}"}},"breakpoints":[{{"line":4,"condition":"i == 3"}},{{"line":7}}]}}"#)));
        l.load(
            "function target(n)\n  local total = 0\n  for i = 1, n do\n    total = total + i\n  end\n  local t = { name = 'zing', list = { 1, 2 } }\n  local r = helper(total, t)\n  r = r + 1\n  return r\nend\nfunction helper(x, t)\n  return x * 2\nend\nfunction boom() error('壞了') end\n",
        )
        .set_name("@dbgtest.lua")
        .exec()
        .unwrap();
        // 每次暫停時的請求
        batches.borrow_mut().extend([
            vec![
                req(10, "stackTrace", r#"{"threadId":1}"#),
                req(11, "scopes", r#"{"frameId":1}"#),
                req(12, "variables", r#"{"variablesReference":1}"#),
                req(13, "evaluate", r#"{"expression":"total + i","frameId":1,"context":"watch"}"#),
                req(14, "continue", "{}"),
            ],
            vec![req(20, "stepIn", "{}")],
            vec![req(21, "stackTrace", r#"{"threadId":1}"#), req(22, "stepOut", "{}")],
            vec![req(23, "evaluate", r#"{"expression":"r","frameId":1}"#), req(24, "continue", "{}")],
            vec![req(30, "stackTrace", r#"{"threadId":1}"#), req(31, "continue", "{}")],
            vec![req(41, "continue", "{}")],
        ]);
        assert_eq!(run(&l, "local ok, r = __call(target, 3); return tostring(ok) .. r"), "true13");
        assert_eq!(run(&l, "local ok, e = __call(boom); return tostring(ok) .. tostring(e:find('壞了') ~= nil)"), "falsetrue");
        // 執行中收到暫停請求（指令計數鉤子輪詢時處理）
        tx.send(Job::Debug(req(40, "pause", "{}"))).unwrap();
        assert_eq!(run(&l, "local ok, r = __call(function() local x = 0 for i = 1, 200000 do x = x + i end return x end); return r"), "20000100000");
        idle(req(50, "disconnect", "{}"));

        let sent = sent.borrow();
        // 對某個請求的回應（JSON 欄位順序不固定）
        let resp = |n: u32| {
            let (a, b) = (format!(r#""request_seq":{n},"#), format!(r#""request_seq":{n}}}"#));
            sent.iter().find(|m| m.contains(&a) || m.contains(&b)).cloned().unwrap_or_else(|| panic!("找不到 {n} 的回應：{sent:#?}"))
        };
        let has = |m: &str, parts: &[&str]| {
            for p in parts {
                assert!(m.contains(p), "{p} 不在 {m}");
            }
        };
        has(&resp(1), &[r#""supportsConditionalBreakpoints":true"#]);
        has(&resp(3), &[r#""verified":true"#, r#""line":4"#, r#""line":7"#]);
        let stops: Vec<&String> = sent.iter().filter(|m| m.contains(r#""event":"stopped""#)).collect();
        let reasons: Vec<&str> = stops
            .iter()
            .map(|m| ["breakpoint", "step", "exception", "pause"].into_iter().find(|r| m.contains(&format!(r#""reason":"{r}""#))).unwrap_or("?"))
            .collect();
        assert_eq!(reasons, ["breakpoint", "breakpoint", "step", "step", "exception", "pause"], "{stops:#?}");
        has(stops[4], &["壞了"]);
        // 條件中斷點只在 i == 3 時停：堆疊頂端是 target 第 4 行
        has(&resp(10), &[r#""name":"target""#, r#""line":4"#, "dbgtest.lua"]);
        has(&resp(12), &[r#""name":"i""#, r#""value":"3""#, r#""name":"total""#, r#""name":"n""#]);
        has(&resp(13), &[r#""result":"6""#]);
        // 單步進入 helper，再跳出回到 target 的下一行
        has(&resp(21), &[r#""name":"helper""#, r#""line":12"#]);
        has(&resp(23), &[r#""result":"12""#]);
        has(&resp(30), &[r#""name":"boom""#]);
    }

    #[test]
    fn debugger_json() {
        let l = lua();
        let (_tx, rx) = channel();
        add_debug_fns(&l, std::rc::Rc::new(Queue { rx, pending: Default::default() }), |_| true).unwrap();
        let src = r#"local d = __debugger.decode('{"a":[1,2.5,{"b":"\\u4e2d\\ud83d\\ude00\\n"}],"c":null,"d":true,"e":[]}')
            return d.a[1] .. '|' .. d.a[2] .. '|' .. d.a[3].b .. '|' .. tostring(d.c) .. tostring(d.d) .. #d.e .. '|' .. __debugger.encode({ x = 'a"\n' })"#;
        assert_eq!(run(&l, src), "1|2.5|中😀\n|niltrue0|{\"x\":\"a\\\"\\n\"}");
    }

    /// 載入範例腳本：is_dir 查真的檔案系統，msg 存在 last_msg，命令（__invoke）記在 __calls（「名稱|位置參數;…|to」），
    /// ask()、pick_folder() 依序回答 __answers
    fn example_lua(name: &str, src: &str) -> Lua {
        let l = lua();
        let g = l.globals();
        // 範例寫在 %LOCALAPPDATA% 的資料（復原記錄）改放測試資料夾，不碰使用者的資料
        let local = zing_core::testdir::dir().join(format!("zing-localappdata-{name}"));
        let _ = std::fs::remove_dir_all(&local);
        g.set("__test_localappdata", local.to_string_lossy().into_owned()).unwrap();
        g.set("__is_dir", l.create_function(|_, p: String| Ok(Path::new(&p).is_dir())).unwrap()).unwrap();
        g.set("__msg", l.create_function(|l, m: String| l.globals().set("last_msg", m)).unwrap()).unwrap();
        drop(g);
        l.load(
            "local getenv = os.getenv
             os.getenv = function(k) if k == 'LOCALAPPDATA' then return __test_localappdata end; return getenv(k) end
             function __ask() return table.remove(__answers, 1) end; __pick_folder = __ask
             __calls = {}
             function __clipboard_set(t) __clip = t; return true end
             function __invoke(name, pos, flags)
               __calls[#__calls + 1] = name .. '|' .. table.concat(pos, ';') .. '|' .. tostring(flags.to)
             end",
        )
        .exec()
        .unwrap();
        l.load(src).set_name(format!("@{name}")).exec().unwrap();
        l
    }

    /// 目前資料夾與選取項目（dir 裡的名稱）
    fn set_ctx(l: &Lua, dir: &Path, sel: &[&str]) {
        let ctx = l.create_table().unwrap();
        ctx.set("cwd", dir.to_string_lossy().as_ref()).unwrap();
        ctx.set("sel", sel.iter().map(|n| dir.join(n).to_string_lossy().into_owned()).collect::<Vec<_>>()).unwrap();
        l.globals().set("__ctx", ctx).unwrap();
    }

    /// 執行命令並回傳最後的 msg
    fn example_run(l: &Lua, code: &str, answers: &[&str]) -> String {
        l.globals().set("last_msg", "").unwrap();
        l.globals().set("__answers", answers.to_vec()).unwrap();
        l.load(code).exec().unwrap();
        l.globals().get("last_msg").unwrap()
    }

    fn example_dir(name: &str) -> std::path::PathBuf {
        let dir = zing_core::testdir::dir().join(format!("zing-{name}-測試-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// examples/scripts/rename_pattern.lua：樣式取代、保留副檔名、名稱衝突、只改大小寫、錯誤的樣式、復原
    #[test]
    fn example_rename_pattern_script() {
        let _allow = zing_core::guard::allow_io("測試");
        let dir = example_dir("rename");
        for n in ["IMG_0012.jpg", "IMG_0013.JPG", "相片 0013.JPG", "說明.txt", "abc.txt"] {
            std::fs::write(dir.join(n), n).unwrap();
        }
        std::fs::create_dir(dir.join("IMG_資料夾.v2")).unwrap();
        let l = example_lua("rename_pattern.lua", include_str!("../../../examples/scripts/rename_pattern.lua"));
        let names = || {
            let mut v: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        };
        let before = names();

        // 主檔名：副檔名保留原本的大小寫；與既有的「相片 0013.JPG」衝突時加序號；資料夾的名稱整個套用
        set_ctx(&l, &dir, &["IMG_0012.jpg", "IMG_0013.JPG", "說明.txt", "IMG_資料夾.v2"]);
        let m = example_run(&l, "rename_pattern()", &["^IMG_(.+)", "相片 %1", "1", "是"]);
        assert!(m.contains("已重新命名 3 個"), "{m}");
        assert_eq!(names(), ["abc.txt", "相片 0012.jpg", "相片 0013 (2).JPG", "相片 0013.JPG", "相片 資料夾.v2", "說明.txt"]);

        // 復原
        let m = example_run(&l, "rename_undo()", &[]);
        assert!(m.contains("已復原 3 個"), "{m}");
        assert_eq!(names(), before);
        assert!(example_run(&l, "rename_undo()", &[]).contains("沒有可以復原"));

        // 只改大小寫、完整名稱
        set_ctx(&l, &dir, &["abc.txt"]);
        let m = example_run(&l, "rename_pattern()", &["abc%.txt", "ABC.TXT", "2", "是"]);
        assert!(m.contains("已重新命名 1 個"), "{m}");
        assert!(names().contains(&"ABC.TXT".to_string()));
        example_run(&l, "rename_undo()", &[]);
        assert!(names().contains(&"abc.txt".to_string()));

        // 錯誤的樣式、沒有變更、取消
        let m = example_run(&l, "rename_pattern()", &["[abc", "x", "1"]);
        assert!(m.contains("樣式有誤"), "{m}");
        assert!(example_run(&l, "rename_pattern()", &["zzz", "x", "1"]).contains("沒有名稱需要變更"));
        example_run(&l, "rename_pattern()", &["abc", "x", "1", "否"]);
        assert_eq!(names(), before);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// examples/scripts/checksum.lua：以真的 certutil 計算（含空檔案、中文檔名），再驗證（相符、不符、找不到）
    #[test]
    fn example_checksum_script() {
        let _allow = zing_core::guard::allow_io("測試");
        let dir = example_dir("sha256");
        std::fs::write(dir.join("甲 1.txt"), "abc").unwrap();
        std::fs::write(dir.join("空.txt"), "").unwrap();
        std::fs::create_dir(dir.join("子資料夾")).unwrap();
        let l = example_lua("checksum.lua", include_str!("../../../examples/scripts/checksum.lua"));

        set_ctx(&l, &dir, &["甲 1.txt", "空.txt", "子資料夾"]);
        let m = example_run(&l, "sha256_create()", &["校驗.sha256"]);
        assert!(m.contains("2 個檔案"), "{m}");
        let sums = std::fs::read_to_string(dir.join("校驗.sha256")).unwrap();
        assert_eq!(
            sums,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  甲 1.txt\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  空.txt\n"
        );

        set_ctx(&l, &dir, &["校驗.sha256", "甲 1.txt"]);
        let m = example_run(&l, "sha256_verify()", &[]);
        assert!(m.contains("驗證通過：2 個"), "{m}");

        std::fs::write(dir.join("甲 1.txt"), "abd").unwrap();
        std::fs::remove_file(dir.join("空.txt")).unwrap();
        let m = example_run(&l, "sha256_verify()", &[]);
        assert!(m.contains("0 個相符、1 個不符、1 個找不到"), "{m}");

        // 只有一個檔案：雜湊也放到剪貼簿
        set_ctx(&l, &dir, &["甲 1.txt"]);
        let m = example_run(&l, "sha256_create()", &["單一.sha256"]);
        assert!(m.contains("已複製到剪貼簿"), "{m}");
        let clip: String = l.load("return __clip").eval().unwrap();
        let written = std::fs::read_to_string(dir.join("單一.sha256")).unwrap();
        assert_eq!(written, format!("{clip}  甲 1.txt\n"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// examples/scripts/organize.lua：依副檔名分組、建立子資料夾、每組一個移動命令，資料夾不移動
    #[test]
    fn example_organize_script() {
        let _allow = zing_core::guard::allow_io("測試");
        let dir = example_dir("organize");
        for n in ["a.JPG", "b.png", "c.pdf", "d.xyz", "e.mp4"] {
            std::fs::write(dir.join(n), "").unwrap();
        }
        std::fs::create_dir(dir.join("資料夾")).unwrap();
        let l = example_lua("organize.lua", include_str!("../../../examples/scripts/organize.lua"));
        set_ctx(&l, &dir, &["a.JPG", "b.png", "c.pdf", "d.xyz", "e.mp4", "資料夾"]);
        let m = example_run(&l, "organize_by_type()", &[&dir.to_string_lossy()]);
        assert!(m.contains("5 個檔案到 4 個分類"), "{m}");
        let d = dir.to_string_lossy();
        let calls: Vec<String> = l.load("return __calls").eval().unwrap();
        assert_eq!(
            calls,
            [
                format!(r"copy|{d}\a.JPG;{d}\b.png|{d}\圖片"),
                format!(r"copy|{d}\e.mp4|{d}\影片"),
                format!(r"copy|{d}\c.pdf|{d}\文件"),
                format!(r"copy|{d}\d.xyz|{d}\其他"),
            ]
        );
        for k in ["圖片", "影片", "文件", "其他"] {
            assert!(dir.join(k).is_dir(), "{k}");
        }
        assert!(!dir.join("音樂").exists());
        // 沒有選取檔案
        set_ctx(&l, &dir, &["資料夾"]);
        assert!(example_run(&l, "organize_by_type()", &[]).contains("請先選取"));

        // 依修改日期（stat）：舊檔案在 2020-03，其他是這個月
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_584_230_400); // 2020-03-15
        std::fs::File::options().write(true).open(dir.join("c.pdf")).unwrap().set_modified(old).unwrap();
        l.load("__calls = {}").exec().unwrap();
        set_ctx(&l, &dir, &["a.JPG", "c.pdf", "資料夾"]);
        let m = example_run(&l, "organize_by_date()", &[&dir.to_string_lossy()]);
        assert!(m.contains("2 個檔案到 2 個月份資料夾"), "{m}");
        let now: String = l.load("return os.date('%Y-%m')").eval().unwrap();
        let calls: Vec<String> = l.load("return __calls").eval().unwrap();
        assert_eq!(calls, [format!(r"copy|{d}\c.pdf|{d}\2020-03"), format!(r"copy|{d}\a.JPG|{d}\{now}")]);
        assert!(dir.join("2020-03").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// examples/scripts/folder_view.lua：相片資料夾切換成縮圖、資料夾封面
    #[test]
    fn example_folder_view_script() {
        let _allow = zing_core::guard::allow_io("測試");
        let dir = example_dir("folderview");
        let album = dir.join("專輯");
        std::fs::create_dir(&album).unwrap();
        std::fs::write(album.join("cover.jpg"), "").unwrap();
        std::fs::create_dir(dir.join("空的")).unwrap();
        let l = example_lua("folder_view.lua", include_str!("../../../examples/scripts/folder_view.lua"));

        l.load(r#"on_navigate([[D:\Photos\2024]]); on_navigate([[C:\Users\a\Pictures]]); on_navigate([[D:\相片]]); on_navigate([[D:\Code\photoshop]])"#)
            .exec()
            .unwrap();
        let calls: Vec<String> = l.load("return __calls").eval().unwrap();
        assert_eq!(calls, ["view.mode|thumbnails|nil"; 3]);

        let thumb = |p: &Path| -> Option<String> { l.load(format!("return thumbnail([[{}]])", p.display())).eval().unwrap() };
        assert_eq!(thumb(&album), Some(album.join("cover.jpg").to_string_lossy().into_owned()));
        assert_eq!(thumb(&dir.join("空的")), None);
        assert_eq!(thumb(&album.join("cover.jpg")), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// examples/scripts/extract.lua：以真的 tar.exe 解壓縮（中文路徑、子資料夾、損壞的檔案）
    #[test]
    fn example_extract_script() {
        let _allow = zing_core::guard::allow_io("測試");
        let dir = zing_core::testdir::dir().join(format!("zing-extract-測試-{}", std::process::id()));
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

        let l = example_lua("extract.lua", include_str!("../../../examples/scripts/extract.lua"));
        set_ctx(&l, &dir, &["壓縮 1.zip", "資料.tar.gz", "壞掉.zip", "說明.txt"]);
        let run = |answers: [&str; 2]| example_run(&l, "extract_to()", &answers);

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

    /// stat()：大小、時間（給 os.date）、屬性；不存在時回傳 nil 與錯誤訊息
    #[test]
    fn stat_files() {
        let _allow = zing_core::guard::allow_io("測試");
        let dir = example_dir("stat");
        let f = dir.join("資料 1.bin");
        std::fs::write(&f, [0u8; 1234]).unwrap();
        std::process::Command::new("attrib").arg("+R").arg("+H").arg(&f).status().unwrap();
        let l = lua();
        l.globals().set("F", f.to_string_lossy().as_ref()).unwrap();
        l.globals().set("D", dir.to_string_lossy().as_ref()).unwrap();
        assert_eq!(
            run(&l, "local s = stat(F); return { s.size, s.is_dir, s.readonly, s.hidden, s.system, os.date('%Y', s.modified) == os.date('%Y') }"),
            "{ 1234, false, true, true, false, true }"
        );
        assert_eq!(run(&l, "local s = stat(D); return { s.size, s.is_dir, math.abs(s.created - os.time()) < 600 }"), "{ 0, true, true }");
        assert_eq!(run(&l, "local s, e = stat(path.join(D, '不存在')); return { s == nil, type(e) }"), "{ true, string }");
        std::process::Command::new("attrib").arg("-R").arg("-H").arg(&f).status().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clipboard_newlines() {
        assert_eq!(lf_to_crlf("a\nb\r\nc"), "a\r\nb\r\nc");
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
