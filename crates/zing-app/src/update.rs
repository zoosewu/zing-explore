//! 檢查新版本與自動更新（版號與提示規則見 `zing_core::update`）。
//!
//! - 檢查：啟動時（第一幀之後）在背景執行緒讀紀錄（`%LOCALAPPDATA%\zing-explore\update.tsv`），距上次超過 24 小時
//!   才以 WinHTTP 向 GitHub 查最新版（`/releases/latest` 不跟隨轉址，從 Location 讀出標籤），否則沿用紀錄中的最新版。
//!   值得提示時狀態列右側常駐「有新版本」，點了開選單（安裝、版本說明、略過這個版本、不再檢查）。
//!   設定 `update.check` 關閉時不檢查；`update.check` 命令立即檢查，有任何較新的版本都提示（含修補版、略過的版本）。
//! - 安裝：下載這個版本的壓縮檔與 .sha256 到 `%LOCALAPPDATA%\zing-explore\update`，核對雜湊後以系統的 tar.exe
//!   解壓縮（只用相對路徑：tar 以 ANSI 字碼頁讀參數，使用者名稱可能不是 ASCII），再把每個檔案先複製成程式旁的
//!   `<名稱>.new`；全部成功後把現有的檔案改名成 `<名稱>.old`（執行中的 exe 可以改名、不能覆蓋）、`.new` 改成正式的名稱，
//!   途中失敗就還原。下次啟動時刪除 `.old`（其他還在執行的 zing 用著的刪不掉，留到再下次）。
//!   zing-index.exe 一起換新；新版 zing 啟動時依 `SERVICE_VERSION` 以 UAC 更新已安裝的服務（index.rs）。
//! - 重新啟動：照一般方式關閉視窗（儲存工作階段），訊息迴圈結束後才啟動新的 zing.exe（`relaunch`），
//!   新程序讀到的才是存好的工作階段。
//! - 網路與檔案都在背景執行緒；WinHTTP 與 bcrypt.dll 延遲載入（build.rs），不影響啟動時間。
//! - 隔離模式（效能、冒煙測試）不連網、不寫檔；冒煙測試以 `TEST_LATEST` 提供最新版。

use std::ffi::c_void;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use zing_core::text::wide_z;
use zing_core::update::{self as upd, State, Version};

use crate::app::App;
use crate::gfx::Rect;
use crate::menu::{Action, MenuItem};
use crate::services::{Msg, Show};

/// 冒煙測試：有值時不連網，以這個標籤當作最新版
#[cfg(feature = "smoke")]
pub static TEST_LATEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 訊息迴圈結束後要啟動新版（`relaunch`）
static RESTART: AtomicBool = AtomicBool::new(false);

/// 程式旁會被更新換掉的檔案的前綴：清理 `.old`、`.new` 時只動這些，不碰使用者放在旁邊的其他檔案
const OWN_FILES: &[&str] = &["zing", "LICENSE"];

#[derive(Default)]
pub enum Update {
    #[default]
    None,
    /// 有新版本，等使用者決定
    Available { tag: String, version: Version },
    Downloading { tag: String, version: Version, percent: u32, cancel: Arc<AtomicBool> },
    /// 已換好檔案，重新啟動後生效
    Ready { tag: String, version: Version },
}

/// 狀態列提示的選單項目
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Install,
    Notes,
    Skip,
    Disable,
    Cancel,
    Restart,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn local_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("zing-explore"))
}

fn state_path() -> Option<PathBuf> {
    local_dir().map(|d| d.join("update.tsv"))
}

fn read_state() -> State {
    zing_core::guard::io("讀取更新檢查紀錄");
    state_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| State::parse(&s)).unwrap_or_default()
}

fn write_state(s: &State) {
    zing_core::guard::io("寫入更新檢查紀錄");
    if let Some(p) = state_path() {
        let _ = p.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(p, s.to_text());
    }
}

/// 背景：清掉上次更新留下的 `.old`／`.new`，再依紀錄或向 GitHub 取得最新版的標籤。
/// `force`：不管距上次多久都連線。回傳（最新版或錯誤, 略過的版本）
fn check(force: bool) -> (Result<Option<String>, String>, Option<Version>) {
    cleanup(&crate::settings::app_dir());
    let mut st = read_state();
    let now = now_secs();
    if !force && !st.due(now) {
        return (Ok(st.latest.clone()), st.skipped);
    }
    match http::latest_tag() {
        Ok(tag) => {
            st.last_check = now;
            st.latest = tag.clone();
            write_state(&st);
            (Ok(tag), st.skipped)
        }
        // 失敗時不記時間：下次啟動再試
        Err(e) => (Err(e), st.skipped),
    }
}

fn own_file(name: &str) -> bool {
    OWN_FILES.iter().any(|p| name.starts_with(p))
}

/// 刪除程式旁上次更新留下的 `<名稱>.old`、沒有完成的 `<名稱>.new`
fn cleanup(dir: &Path) {
    zing_core::guard::io("清除上次更新留下的檔案");
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if own_file(&name) && (name.ends_with(".old") || name.ends_with(".new")) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// 背景：下載、核對、解壓縮並換掉程式旁的檔案。Ok(false) 表示途中取消
fn install(tag: &str, cancel: &AtomicBool, progress: &dyn Fn(u32)) -> Result<bool, String> {
    zing_core::guard::io("下載並安裝更新");
    let work = local_dir().ok_or_else(|| tr!("找不到 %LOCALAPPDATA%").to_string())?.join("update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("files")).map_err(|e| tr!("無法建立 {dir}：{e}", dir = work.display(), e))?;
    let result = (|| {
        let sha = http::get_text(&upd::asset_url(tag, &format!("{}.sha256", upd::ASSET)))?;
        let want = upd::parse_sha256(&sha).ok_or_else(|| tr!("雜湊檔的格式不對").to_string())?;
        let Some(got) = http::download(&upd::asset_url(tag, upd::ASSET), &work.join("update.zip"), cancel, progress)? else {
            return Ok(false);
        };
        if got != want {
            return Err(tr!("下載的檔案與雜湊不符，已放棄").into());
        }
        extract(&work)?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        replace_files(&work.join("files"), &crate::settings::app_dir())?;
        Ok(true)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// 以系統的 tar.exe 把 `work\update.zip` 解到 `work\files`（只用相對路徑）
fn extract(work: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let tar = root.join("System32").join("tar.exe");
    let out = std::process::Command::new(&tar)
        .args(["-xf", "update.zip", "-C", "files"])
        .current_dir(work)
        .creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0)
        .output()
        .map_err(|e| tr!("無法執行 {tar}：{e}", tar = tar.display(), e))?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(tr!("解壓縮失敗：{msg}", msg))
    }
}

/// `src` 的每個檔案換掉 `dst` 中的同名檔案（見模組說明）
fn replace_files(src: &Path, dst: &Path) -> Result<(), String> {
    let mut names: Vec<String> = std::fs::read_dir(src)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    if !names.iter().any(|n| n.eq_ignore_ascii_case("zing.exe")) {
        return Err(tr!("壓縮檔中沒有 zing.exe").into());
    }
    let with = |n: &str, ext: &str| dst.join(format!("{n}.{ext}"));
    let cannot_write = |e: std::io::Error| tr!("無法寫入 {dir}：{e}", dir = dst.display(), e);
    // 先全部複製成 .new：空間不足、沒有權限都在這一步發現，還沒動到現有的檔案
    for n in &names {
        if let Err(e) = std::fs::copy(src.join(n), with(n, "new")) {
            for n in &names {
                let _ = std::fs::remove_file(with(n, "new"));
            }
            return Err(cannot_write(e));
        }
    }
    // 換上：現有的改名成 .old，.new 改成正式的名稱；失敗就把已換的換回來
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let undo = |done: &[(PathBuf, Option<PathBuf>)]| {
        for (target, old) in done.iter().rev() {
            let _ = std::fs::remove_file(target);
            if let Some(o) = old {
                let _ = std::fs::rename(o, target);
            }
        }
        for n in &names {
            let _ = std::fs::remove_file(with(n, "new"));
        }
    };
    for n in &names {
        let target = dst.join(n);
        let old = if target.exists() {
            let o = free_old_name(dst, n);
            if let Err(e) = std::fs::rename(&target, &o) {
                undo(&done);
                return Err(cannot_write(e));
            }
            Some(o)
        } else {
            None
        };
        if let Err(e) = std::fs::rename(with(n, "new"), &target) {
            if let Some(o) = &old {
                let _ = std::fs::rename(o, &target);
            }
            undo(&done);
            return Err(cannot_write(e));
        }
        done.push((target, old));
    }
    Ok(())
}

/// `<名稱>.old`；還在使用中刪不掉（其他 zing 執行中）時改用 `<名稱>.1.old`…
fn free_old_name(dir: &Path, name: &str) -> PathBuf {
    for i in 0..10 {
        let p = if i == 0 { dir.join(format!("{name}.old")) } else { dir.join(format!("{name}.{i}.old")) };
        if !p.exists() || std::fs::remove_file(&p).is_ok() {
            return p;
        }
    }
    dir.join(format!("{name}.{}.old", now_secs()))
}

/// 訊息迴圈結束後（main.rs）：使用者選了重新啟動就啟動新的 zing.exe
pub fn relaunch() {
    if RESTART.load(Ordering::Relaxed) {
        let _ = std::process::Command::new(crate::settings::app_dir().join("zing.exe")).spawn();
    }
}

impl App {
    /// 啟動時（第一幀之後）：清理上次更新留下的檔案，依設定檢查新版本
    pub fn startup_update(&mut self) {
        if self.isolated {
            return;
        }
        let enabled = self.config.update.check;
        let n = self.notifier.clone();
        std::thread::spawn(move || {
            if enabled {
                let (latest, skipped) = check(false);
                n.send(Msg::UpdateChecked { latest, skipped, manual: false });
            } else {
                cleanup(&crate::settings::app_dir());
            }
        });
    }

    /// 命令 update.check：立即檢查
    pub fn check_update_now(&mut self) {
        match &self.update {
            Update::Downloading { .. } => return self.set_status(tr!("正在下載更新"), false),
            Update::Ready { version, .. } => return self.set_status(tr!("已更新到 {version}，重新啟動後生效", version), false),
            _ => {}
        }
        let n = self.notifier.clone();
        #[cfg(feature = "smoke")]
        if let Some(tag) = TEST_LATEST.lock().unwrap().clone() {
            n.send(Msg::UpdateChecked { latest: Ok(Some(tag)), skipped: None, manual: true });
            return;
        }
        if self.isolated {
            return self.set_status(tr!("效能與冒煙測試不檢查更新"), false);
        }
        self.set_status(tr!("正在檢查更新…"), false);
        std::thread::spawn(move || {
            let (latest, skipped) = check(true);
            n.send(Msg::UpdateChecked { latest, skipped, manual: true });
        });
    }

    pub fn on_update_checked(&mut self, latest: Result<Option<String>, String>, skipped: Option<Version>, manual: bool) {
        if matches!(self.update, Update::Downloading { .. } | Update::Ready { .. }) {
            return;
        }
        // 自動檢查之後關掉了設定：不再提示
        if !manual && !self.config.update.check {
            return;
        }
        let current = Version::current();
        let latest = match latest {
            Ok(tag) => tag.and_then(|t| Version::parse(&t).map(|v| (t, v))),
            Err(e) => {
                if manual {
                    self.set_status(tr!("無法檢查更新：{e}", e), true);
                }
                return;
            }
        };
        match latest {
            // 手動檢查：有任何較新的版本都提示
            Some((tag, version)) if (manual && version > current) || upd::worth_prompting(current, version, skipped) => {
                if manual {
                    self.set_status(tr!("有新版本 {version}：點狀態列右側的提示安裝", version), false);
                }
                self.update = Update::Available { tag, version };
            }
            _ => {
                if manual {
                    self.set_status(tr!("已是最新版本（{current}）", current), false);
                }
                self.update = Update::None;
            }
        }
        self.invalidate();
    }

    /// 狀態列右側的提示（沒有新版本時為 None）
    pub fn update_indicator_text(&self) -> Option<String> {
        match &self.update {
            Update::None => None,
            Update::Available { version, .. } => Some(tr!("有新版本 {version}", version)),
            Update::Downloading { percent, .. } => Some(tr!("正在下載更新 {percent}%", percent)),
            Update::Ready { version, .. } => Some(tr!("已更新到 {version}：重新啟動", version)),
        }
    }

    /// 點狀態列的提示：開啟選單
    pub fn open_update_menu(&mut self, anchor: Rect) {
        let item = |label: String, c: Choice| MenuItem::new(label, Action::Update(c));
        let items = match &self.update {
            Update::None => return,
            Update::Available { version, .. } => vec![
                item(tr!("下載並安裝 {version}", version), Choice::Install),
                item(tr!("版本說明").into(), Choice::Notes),
                MenuItem::separator(),
                item(tr!("略過這個版本").into(), Choice::Skip),
                item(tr!("不再檢查更新").into(), Choice::Disable),
            ],
            Update::Downloading { .. } => vec![item(tr!("取消更新").into(), Choice::Cancel)],
            Update::Ready { .. } => vec![item(tr!("立即重新啟動").into(), Choice::Restart), item(tr!("版本說明").into(), Choice::Notes)],
        };
        self.open_menu(items, anchor);
    }

    pub fn update_choice(&mut self, c: Choice) {
        let (tag, version) = match &self.update {
            Update::Available { tag, version } | Update::Downloading { tag, version, .. } | Update::Ready { tag, version } => {
                (tag.clone(), *version)
            }
            Update::None => return,
        };
        match c {
            Choice::Install => self.start_install(tag, version),
            Choice::Notes => crate::services::launch(&self.notifier, upd::release_page(&tag), None, None, None, Show::Normal),
            Choice::Skip => {
                self.update = Update::None;
                self.set_status(tr!("已略過 {version}，之後的功能更新照常提示", version), false);
                if !self.isolated {
                    std::thread::spawn(move || {
                        let mut st = read_state();
                        st.skipped = Some(version);
                        write_state(&st);
                    });
                }
            }
            Choice::Disable => {
                self.update = Update::None;
                if let Some(i) = zing_core::prefs::PREFS.iter().position(|p| p.key == "update.check") {
                    self.set_pref(i, "false".into());
                }
                self.set_status(tr!("已關閉更新檢查（可在設定頁重新開啟）"), false);
            }
            Choice::Cancel => {
                if let Update::Downloading { cancel, .. } = &self.update {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            Choice::Restart => self.restart_for_update(),
        }
        self.invalidate();
    }

    fn start_install(&mut self, tag: String, version: Version) {
        if self.isolated {
            return self.set_status(tr!("效能與冒煙測試不安裝更新"), false);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.update = Update::Downloading { tag: tag.clone(), version, percent: 0, cancel: cancel.clone() };
        let n = self.notifier.clone();
        std::thread::spawn(move || {
            let progress = |percent| n.send(Msg::UpdateProgress { percent });
            let result = install(&tag, &cancel, &progress);
            n.send(Msg::UpdateInstalled(result));
        });
    }

    pub fn on_update_progress(&mut self, p: u32) {
        if let Update::Downloading { percent, .. } = &mut self.update {
            *percent = p;
            self.invalidate();
        }
    }

    pub fn on_update_installed(&mut self, result: Result<bool, String>) {
        let Update::Downloading { tag, version, .. } = std::mem::take(&mut self.update) else { return };
        match result {
            Ok(true) => {
                self.set_status(tr!("已安裝 {version}，重新啟動後生效", version), false);
                self.update = Update::Ready { tag, version };
            }
            Ok(false) => {
                self.set_status(tr!("已取消更新"), false);
                self.update = Update::Available { tag, version };
            }
            Err(e) => {
                self.set_status(tr!("更新失敗：{e}", e), true);
                self.update = Update::Available { tag, version };
            }
        }
        self.invalidate();
    }

    /// 照一般方式關閉視窗（儲存工作階段），結束後啟動新版（`relaunch`）
    fn restart_for_update(&mut self) {
        if self.jobs.active() > 0 {
            return self.set_status(tr!("還有檔案作業尚未完成，完成後再重新啟動"), true);
        }
        RESTART.store(true, Ordering::Relaxed);
        unsafe {
            use windows::Win32::Foundation::{LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

/// 以 WinHTTP 取得 GitHub 的資料（只用 https；系統的 Proxy 設定）
mod http {
    use super::*;
    use windows::Win32::Networking::WinHttp::*;
    use windows::Win32::Security::Cryptography::{
        BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptHashData,
    };
    use windows::core::{PCWSTR, w};

    /// 壓縮檔、文字檔的大小上限（防止伺服器送來不合理的內容）
    const MAX_DOWNLOAD: u64 = 64 << 20;
    const MAX_TEXT: usize = 64 << 10;

    struct Handle(*mut c_void);

    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    /// 送出的請求（三個控制代碼依建立的反順序關閉）
    struct Request {
        req: Handle,
        _connect: Handle,
        _session: Handle,
        status: u32,
    }

    fn last_error() -> String {
        let e = windows::core::Error::from_thread();
        // WinHTTP 的錯誤碼（12xxx）系統訊息表沒有，常見的自己翻
        match e.code().0 as u32 & 0xFFFF {
            12002 => tr!("連線逾時").into(),
            12007 => tr!("找不到伺服器（沒有網路？）").into(),
            12029 | 12030 => tr!("無法連線到伺服器").into(),
            code => {
                let m = e.message();
                if m.is_empty() { tr!("WinHTTP 錯誤 {code}", code) } else { m }
            }
        }
    }

    fn check(h: *mut c_void) -> Result<Handle, String> {
        if h.is_null() { Err(last_error()) } else { Ok(Handle(h)) }
    }

    fn open(url: &str, follow: bool) -> Result<Request, String> {
        let rest = url.strip_prefix("https://").ok_or_else(|| tr!("只支援 https：{url}", url))?;
        let (host, path) = match rest.split_once('/') {
            Some((h, p)) => (h, format!("/{p}")),
            None => (rest, "/".to_string()),
        };
        let agent = wide_z(&format!("zing-explore/{}", env!("CARGO_PKG_VERSION")));
        let (host, path) = (wide_z(host), wide_z(&path));
        unsafe {
            let session =
                check(WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0))?;
            let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 30_000);
            let connect = check(WinHttpConnect(session.0, PCWSTR(host.as_ptr()), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
            let req = check(WinHttpOpenRequest(
                connect.0,
                w!("GET"),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ))?;
            if !follow {
                let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes();
                WinHttpSetOption(Some(req.0), WINHTTP_OPTION_REDIRECT_POLICY, Some(&never)).map_err(|_| last_error())?;
            }
            WinHttpSendRequest(req.0, None, None, 0, 0, 0).map_err(|_| last_error())?;
            WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|_| last_error())?;
            let status = number_header(&req.0, WINHTTP_QUERY_STATUS_CODE).unwrap_or(0) as u32;
            Ok(Request { req, _connect: connect, _session: session, status })
        }
    }

    fn number_header(req: &*mut c_void, query: u32) -> Option<u64> {
        // 數字形式的標頭是 32 位元；Content-Length 可能超過，改讀字串
        let s = string_header(req, query)?;
        s.trim().parse().ok()
    }

    fn string_header(req: &*mut c_void, query: u32) -> Option<String> {
        let mut buf = vec![0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        unsafe {
            WinHttpQueryHeaders(*req, query, PCWSTR::null(), Some(buf.as_mut_ptr() as *mut c_void), &mut len, std::ptr::null_mut()).ok()?;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize / 2]))
    }

    /// 讀一段內容到 `buf`，回傳讀到的位元組數（0 表示結束）
    fn read(req: &Request, buf: &mut [u8]) -> Result<usize, String> {
        let mut n = 0u32;
        unsafe { WinHttpReadData(req.req.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut n) }.map_err(|_| last_error())?;
        Ok(n as usize)
    }

    fn bad_status(status: u32) -> String {
        tr!("伺服器回應 HTTP {status}", status)
    }

    /// 最新版的標籤；還沒有任何 Release 時為 None
    pub fn latest_tag() -> Result<Option<String>, String> {
        zing_core::guard::io("向 GitHub 查詢最新版");
        let r = open(&upd::latest_url(), false)?;
        match r.status {
            301 | 302 | 303 | 307 | 308 => {
                let loc = string_header(&r.req.0, WINHTTP_QUERY_LOCATION).unwrap_or_default();
                Ok(upd::tag_from_location(&loc).map(String::from))
            }
            200 | 404 => Ok(None),
            s => Err(bad_status(s)),
        }
    }

    pub fn get_text(url: &str) -> Result<String, String> {
        zing_core::guard::io("下載文字檔");
        let r = open(url, true)?;
        if r.status != 200 {
            return Err(bad_status(r.status));
        }
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = read(&r, &mut buf)?;
            if n == 0 || out.len() > MAX_TEXT {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// 下載到 `path`，回傳內容的 SHA-256；取消時為 None。`progress` 收到 0–100（每變一個百分點一次）
    pub fn download(url: &str, path: &Path, cancel: &AtomicBool, progress: &dyn Fn(u32)) -> Result<Option<[u8; 32]>, String> {
        zing_core::guard::io("下載更新");
        let r = open(url, true)?;
        if r.status != 200 {
            return Err(bad_status(r.status));
        }
        let total = number_header(&r.req.0, WINHTTP_QUERY_CONTENT_LENGTH).unwrap_or(0);
        if total > MAX_DOWNLOAD {
            return Err(tr!("檔案太大（{total} 位元組）", total));
        }
        let mut file = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
        let hash = Sha256::new().ok_or_else(|| tr!("無法計算雜湊").to_string())?;
        let mut buf = vec![0u8; 64 << 10];
        let (mut got, mut last) = (0u64, u32::MAX);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let n = read(&r, &mut buf)?;
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > MAX_DOWNLOAD {
                return Err(tr!("檔案太大（{total} 位元組）", total = got));
            }
            hash.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            let pct = (got * 100).checked_div(total).map_or(0, |p| p.min(100) as u32);
            if pct != last {
                last = pct;
                progress(pct);
            }
        }
        if total > 0 && got != total {
            return Err(tr!("下載不完整（{got}／{total} 位元組）", got, total));
        }
        file.flush().map_err(|e| e.to_string())?;
        Ok(Some(hash.finish()))
    }

    struct Sha256(BCRYPT_HASH_HANDLE);

    impl Sha256 {
        fn new() -> Option<Self> {
            let mut h = BCRYPT_HASH_HANDLE::default();
            unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &mut h, None, None, 0) }.ok().ok()?;
            Some(Sha256(h))
        }

        fn update(&self, data: &[u8]) {
            unsafe {
                let _ = BCryptHashData(self.0, data, 0);
            }
        }

        fn finish(self) -> [u8; 32] {
            let mut out = [0u8; 32];
            unsafe {
                let _ = BCryptFinishHash(self.0, &mut out, 0);
            }
            out
        }
    }

    impl Drop for Sha256 {
        fn drop(&mut self) {
            unsafe {
                let _ = BCryptDestroyHash(self.0);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sha256_known_value() {
            let h = Sha256::new().unwrap();
            h.update(b"ab");
            h.update(b"c");
            let hex: String = h.finish().iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        }

        /// 實際連到 GitHub：`cargo test -p zing-app latest_release -- --ignored --nocapture`
        #[test]
        #[ignore]
        fn latest_release() {
            println!("{:?}", latest_tag());
            // 有 Release 的專案：不跟隨轉址，讀出標籤
            let r = open("https://github.com/LuaLS/lua-language-server/releases/latest", false).unwrap();
            let loc = string_header(&r.req.0, WINHTTP_QUERY_LOCATION).unwrap_or_default();
            println!("{} {loc}", r.status);
            assert!(upd::tag_from_location(&loc).is_some(), "{loc}");
            // 跟隨轉址（github.com → raw.githubusercontent.com）並讀完內容
            let text = get_text(&format!("https://github.com/{}/raw/master/LICENSE", upd::REPO)).unwrap();
            assert!(text.contains("MIT License"), "{text}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = zing_core::testdir::dir().join(format!("zing-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("app")).unwrap();
        d
    }

    fn read(p: PathBuf) -> String {
        std::fs::read_to_string(p).unwrap_or_default()
    }

    #[test]
    fn replaces_files_and_keeps_old() {
        let d = dir("replace");
        let (src, app) = (d.join("src"), d.join("app"));
        for (n, s) in [("zing.exe", "new exe"), ("zing-index.exe", "new index"), ("LICENSE", "new license")] {
            std::fs::write(src.join(n), s).unwrap();
        }
        std::fs::write(app.join("zing.exe"), "old exe").unwrap();
        std::fs::write(app.join("config.toml"), "mine").unwrap();
        // 上次留下、刪不掉的 .old 不影響（這裡刪得掉）
        std::fs::write(app.join("zing.exe.old"), "older").unwrap();
        replace_files(&src, &app).unwrap();
        assert_eq!(read(app.join("zing.exe")), "new exe");
        assert_eq!(read(app.join("zing-index.exe")), "new index");
        assert_eq!(read(app.join("LICENSE")), "new license");
        assert_eq!(read(app.join("zing.exe.old")), "old exe");
        assert!(!app.join("zing.exe.new").exists());
        cleanup(&app);
        assert!(!app.join("zing.exe.old").exists());
        assert_eq!(read(app.join("config.toml")), "mine", "其他檔案不動");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn extracts_zip_with_system_tar() {
        let d = dir("extract");
        let work = d.join("work");
        std::fs::create_dir_all(work.join("files")).unwrap();
        std::fs::write(d.join("src").join("zing.exe"), "exe").unwrap();
        std::fs::write(d.join("src").join("LICENSE"), "license").unwrap();
        // 和 Compress-Archive 一樣：檔案在壓縮檔的最上層
        let tar = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\tar.exe");
        let zip = work.join("update.zip");
        let ok = std::process::Command::new(tar)
            .arg("-a")
            .arg("-cf")
            .arg(&zip)
            .args(["zing.exe", "LICENSE"])
            .current_dir(d.join("src"))
            .status()
            .unwrap()
            .success();
        assert!(ok);
        extract(&work).unwrap();
        assert_eq!(read(work.join(r"files\zing.exe")), "exe");
        assert_eq!(read(work.join(r"files\LICENSE")), "license");
        std::fs::write(&zip, "not a zip").unwrap();
        assert!(extract(&work).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_archive_without_exe() {
        let d = dir("noexe");
        let (src, app) = (d.join("src"), d.join("app"));
        std::fs::write(src.join("LICENSE"), "x").unwrap();
        std::fs::write(app.join("zing.exe"), "old").unwrap();
        assert!(replace_files(&src, &app).is_err());
        assert_eq!(read(app.join("zing.exe")), "old");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn failed_swap_restores_files() {
        let d = dir("rollback");
        let (src, app) = (d.join("src"), d.join("app"));
        std::fs::write(src.join("LICENSE"), "new license").unwrap();
        std::fs::write(src.join("zing.exe"), "new exe").unwrap();
        std::fs::write(app.join("LICENSE"), "old license").unwrap();
        std::fs::write(app.join("zing.exe"), "old exe").unwrap();
        // zing.exe 被開著且不允許共用：改名失敗，已換上的 LICENSE 要換回來
        use std::os::windows::fs::OpenOptionsExt;
        let locked = std::fs::OpenOptions::new().read(true).share_mode(0).open(app.join("zing.exe")).unwrap();
        assert!(replace_files(&src, &app).is_err());
        drop(locked);
        assert_eq!(read(app.join("zing.exe")), "old exe");
        assert_eq!(read(app.join("LICENSE")), "old license");
        assert!(!app.join("LICENSE.new").exists() && !app.join("zing.exe.new").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cleanup_only_touches_own_files() {
        let d = dir("cleanup");
        let app = d.join("app");
        for n in ["zing.exe.old", "zing-index.exe.1.old", "LICENSE.new", "notes.old", "my.new"] {
            std::fs::write(app.join(n), "x").unwrap();
        }
        cleanup(&app);
        let mut left: Vec<String> =
            std::fs::read_dir(&app).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["my.new", "notes.old"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
