//! 檢查新版本與自動更新，依 zoo-release 的規範（docs/UPDATE.md、docs/UI.md）；判斷與驗證用 `zoo_update`。
//!
//! - 檢查：讀 GitHub 上最新 Release 的 `latest.json`（`/releases/latest/download/`，不經 GitHub API）。
//!   第一個視窗在第一幀之後讀紀錄（`%LOCALAPPDATA%\zing-explore\update.json`，`zoo_update::UpdateState`）並清掉上次
//!   更新留下的檔案；啟動 1 分鐘後、之後每小時看一次，距上次成功檢查滿 24 小時才連線（`TIMER_UPDATE`）。
//!   設定 `update.check`（「關於」頁的「自動檢查更新」）關閉時只有手動檢查。自動檢查失敗不顯示，下次再試。
//! - 提示：主版或次版較新、沒有略過時，主畫面上方（命令列下方）顯示橫幅（`update_banner`）：版本說明、略過此版本
//!   （記下 主.次）、更新、✕（這次執行不再顯示）；下載中、已下載、失敗也顯示在橫幅。修補版只顯示在「關於」頁。
//!   鍵盤：命令列按 Tab（沒有命令列時 F10）進入橫幅，←→ 移動、Enter 執行、Esc 離開（`banner_key`）。
//! - 「關於」頁（設定的最後一頁，prefspage.rs）：名稱、版本與「檢查更新」，狀態（`about_status`）與它的按鈕。
//! - 安裝：重新讀 `latest.json`，版本必須比目前新（不降版）、下載網址必須是本 repo 的 Release；下載到記憶體，
//!   驗證簽章（必須）與 sha256，寫成 `%LOCALAPPDATA%\zing-explore\update\update.zip`，以系統的 tar.exe 解壓縮
//!   （只用相對路徑：tar 以 ANSI 字碼頁讀參數，使用者名稱可能不是 ASCII），再把每個檔案先複製成程式旁的
//!   `<名稱>.new`；全部成功後把現有的檔案改名成 `<名稱>.old`（執行中的 exe 可以改名、不能覆蓋）、`.new` 改成正式的名稱，
//!   途中失敗就還原。下次啟動時刪除 `.old`（其他還在執行的 zing 用著的刪不掉，留到再下次）。
//!   zing-index.exe 一起換新；新版 zing 啟動時依 `SERVICE_VERSION` 以 UAC 更新已安裝的服務（index.rs）。
//!   `latest.json` 沒有這個平台的檔案時改為開啟 Release 頁面。
//! - 重新啟動：照一般方式關閉視窗（儲存工作階段），訊息迴圈結束後才啟動新的 zing.exe（`relaunch`），
//!   新程序讀到的才是存好的工作階段。
//! - 網路與檔案都在背景執行緒；WinHTTP 延遲載入（build.rs），不影響啟動時間。
//! - 隔離模式（效能、冒煙測試）不連網、不寫檔；冒煙測試以 `TEST_LATEST` 提供最新版。

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use zing_core::text::wide_z;
use zoo_update::{Manifest, UpdateState, Version, keys};

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::services::{Msg, Show};
use crate::view::Hit;

/// 本專案的 GitHub repo（Release、latest.json、連結）
pub const REPO: &str = "zoosewu/zing-explore";
/// 「關於」頁顯示的授權
pub const LICENSE: &str = "AGPL-3.0";
/// 啟動後多久第一次自動檢查；之後每隔多久看一次是否已滿 24 小時
pub const FIRST_CHECK_MS: u32 = 60_000;
pub const RECHECK_MS: u32 = 60 * 60_000;

/// 冒煙測試：有值時不連網，以這個版本（`99.1.0`）當作最新版
#[cfg(feature = "smoke")]
pub static TEST_LATEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 訊息迴圈結束後要啟動新版（`relaunch`）
static RESTART: AtomicBool = AtomicBool::new(false);

/// 程式旁會被更新換掉的檔案的前綴：清理 `.old`、`.new` 時只動這些，不碰使用者放在旁邊的其他檔案
const OWN_FILES: &[&str] = &["zing", "LICENSE"];

/// 這個執行檔的版本
pub fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml 的版號格式")
}

/// 下載與安裝的進度
#[derive(Default)]
pub enum Update {
    #[default]
    Idle,
    Downloading { version: Version, percent: u32, cancel: Arc<AtomicBool> },
    /// 已換好檔案，重新啟動後生效
    Ready { version: Version },
    Failed { version: Option<Version>, reason: String },
}

/// 檢查的狀態（「關於」頁）
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckStatus {
    #[default]
    Idle,
    Checking,
    /// 手動檢查失敗（自動檢查失敗不顯示）
    Failed,
}

/// 橫幅與「關於」頁的按鈕
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    /// 版本說明（Release 頁面）
    Notes,
    /// 略過此版本（記下 主.次）
    Skip,
    Install,
    /// ✕：這次執行期間不再顯示
    Later,
    Cancel,
    Restart,
    /// 手動下載（Release 頁面）
    Manual,
    /// 檢查更新
    Check,
    /// 檢查失敗後重試
    Retry,
}

impl UpdateAction {
    /// 按鈕的文字（原文）
    pub fn label(self) -> &'static str {
        match self {
            UpdateAction::Notes => tr_key!("版本說明"),
            UpdateAction::Skip => tr_key!("略過此版本"),
            UpdateAction::Install => tr_key!("更新"),
            UpdateAction::Later => tr_key!("稍後"),
            UpdateAction::Cancel => tr_key!("取消"),
            UpdateAction::Restart => tr_key!("立即重新啟動"),
            UpdateAction::Manual => tr_key!("手動下載"),
            UpdateAction::Check => tr_key!("檢查更新"),
            UpdateAction::Retry => tr_key!("重試"),
        }
    }

    /// 主要按鈕（強調色）
    pub fn primary(self) -> bool {
        matches!(self, UpdateAction::Install | UpdateAction::Restart)
    }
}

/// 主畫面的橫幅：文字、進度（下載中）、按鈕、是否有 ✕
pub struct Banner {
    pub text: String,
    pub progress: Option<u32>,
    pub actions: Vec<UpdateAction>,
    pub close: bool,
}

/// 「關於」頁的狀態列：文字、接在後面的次要文字、是否為錯誤、進度（下載中）、按鈕
#[derive(Default)]
pub struct AboutStatus {
    pub text: String,
    pub detail: Option<String>,
    pub error: bool,
    pub progress: Option<u32>,
    pub actions: Vec<UpdateAction>,
}

/// 安裝的結果
pub enum Installed {
    Done(Version),
    Canceled,
    /// latest.json 沒有這個平台的檔案：改為開啟 Release 頁面
    NoAsset(Version),
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn local_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("zing-explore"))
}

fn state_path() -> Option<PathBuf> {
    local_dir().map(|d| d.join("update.json"))
}

fn read_state() -> UpdateState {
    zing_core::guard::io("讀取更新檢查紀錄");
    // v0.6 以前的紀錄（格式不同、略過的是整個版本號）：不沿用，下次檢查重新記
    if let Some(old) = local_dir().map(|d| d.join("update.tsv")) {
        let _ = std::fs::remove_file(old);
    }
    state_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write_state(s: &UpdateState) {
    zing_core::guard::io("寫入更新檢查紀錄");
    if let (Some(p), Ok(text)) = (state_path(), serde_json::to_string_pretty(s)) {
        let _ = p.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(p, text);
    }
}

/// `zoo_update` 的錯誤 → 介面上的原因（docs/UI.md 的用詞）
fn reason(e: zoo_update::Error) -> String {
    use zoo_update::Error;
    match e {
        Error::Signature | Error::SignatureFormat | Error::PublicKey => tr!("簽章驗證失敗").into(),
        Error::Sha256 => tr!("下載的檔案與雜湊不符").into(),
        Error::Url(url) => tr!("下載網址不是 zing-explore 的 Release：{url}", url),
        Error::Manifest(_) => tr!("latest.json 的格式不對").into(),
        Error::Io(e) => e,
    }
}

fn fetch_manifest() -> Result<Manifest, String> {
    let text = http::get_bytes(&zoo_update::manifest_url(REPO))?;
    Manifest::parse(&String::from_utf8_lossy(&text)).map_err(reason)
}

/// 背景：連線檢查，成功時記下時間與最新版。回傳（紀錄, 錯誤）
fn check() -> (UpdateState, Option<String>) {
    let mut st = read_state();
    match fetch_manifest() {
        Ok(m) => {
            st.record_check(now_secs(), &m);
            write_state(&st);
            (st, None)
        }
        Err(e) => (st, Some(e)),
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

/// 背景：讀 latest.json、下載、驗證、解壓縮並換掉 `dst`（程式旁）的檔案
fn install(dst: &Path, cancel: &AtomicBool, progress: &dyn Fn(u32)) -> Result<Installed, String> {
    zing_core::guard::io("下載並安裝更新");
    let manifest = fetch_manifest()?;
    let version = manifest.version().ok_or_else(|| tr!("latest.json 的格式不對").to_string())?;
    if version <= current() {
        return Err(tr!("{version} 沒有比目前的版本新", version = format!("v{version}")));
    }
    let Some(asset) = manifest.asset(zoo_update::current_platform()) else { return Ok(Installed::NoAsset(version)) };
    asset.check_url(REPO).map_err(reason)?;
    let Some(data) = http::download(&asset.url, cancel, progress)? else { return Ok(Installed::Canceled) };
    asset.verify(&data, keys::ALL).map_err(reason)?;
    let work = local_dir().ok_or_else(|| tr!("找不到 %LOCALAPPDATA%").to_string())?.join("update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("files")).map_err(|e| tr!("無法建立 {dir}：{e}", dir = work.display(), e))?;
    let result = (|| {
        std::fs::write(work.join("update.zip"), &data).map_err(|e| tr!("無法建立 {dir}：{e}", dir = work.display(), e))?;
        extract(&work)?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(Installed::Canceled);
        }
        replace_files(&work.join("files"), dst)?;
        Ok(Installed::Done(version))
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
    /// 第一個視窗的第一幀之後：讀紀錄、清理上次更新留下的檔案，1 分鐘後第一次自動檢查（`on_update_timer`）
    pub fn startup_update(&mut self) {
        if self.isolated {
            return;
        }
        let n = self.notifier.clone();
        std::thread::spawn(move || {
            cleanup(&crate::settings::app_dir());
            n.send(Msg::UpdateChecked { state: Box::new(read_state()), error: None, manual: false });
        });
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), crate::app::TIMER_UPDATE, FIRST_CHECK_MS, None);
        }
    }

    /// `TIMER_UPDATE`：之後每小時看一次；開著自動檢查、距上次成功檢查滿 24 小時才連線
    pub fn on_update_timer(&mut self) {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), crate::app::TIMER_UPDATE, RECHECK_MS, None);
        }
        let due = self.update_state.as_ref().is_some_and(|s| UpdateState { auto_check: self.config.update.check, ..s.clone() }.check_due(now_secs()));
        if !due || self.update_check == CheckStatus::Checking || !matches!(self.update, Update::Idle) {
            return;
        }
        let n = self.notifier.clone();
        std::thread::spawn(move || {
            let (state, error) = check();
            n.send(Msg::UpdateChecked { state: Box::new(state), error, manual: false });
        });
    }

    /// 立即檢查（命令 update.check、「關於」頁的「檢查更新」「重試」）：結果顯示在「關於」頁
    pub fn check_update_now(&mut self) {
        if self.update_check == CheckStatus::Checking || matches!(self.update, Update::Downloading { .. } | Update::Ready { .. }) {
            return;
        }
        let n = self.notifier.clone();
        #[cfg(feature = "smoke")]
        if let Some(v) = TEST_LATEST.lock().unwrap().clone() {
            let mut state = self.update_state.clone().unwrap_or_default();
            state.record_check(now_secs(), &Manifest { version: v, ..Default::default() });
            n.send(Msg::UpdateChecked { state: Box::new(state), error: None, manual: true });
            self.update_check = CheckStatus::Checking;
            return self.invalidate();
        }
        if self.isolated {
            self.update_check = CheckStatus::Failed;
            return self.invalidate();
        }
        self.update_check = CheckStatus::Checking;
        self.invalidate();
        std::thread::spawn(move || {
            let (state, error) = check();
            n.send(Msg::UpdateChecked { state: Box::new(state), error, manual: true });
        });
    }

    pub fn on_update_checked(&mut self, state: UpdateState, error: Option<String>, manual: bool) {
        // 略過的版本可能在檢查途中改了：以畫面上的為準
        let skipped = self.update_state.as_ref().and_then(|s| s.skipped.clone());
        self.update_state = Some(UpdateState { skipped: skipped.or(state.skipped.clone()), ..state });
        if manual {
            self.update_check = if error.is_some() { CheckStatus::Failed } else { CheckStatus::Idle };
            // 手動檢查的失敗記在「關於」頁；原因只在狀態列（UI.md 的狀態只寫「無法檢查更新」）
            if let Some(e) = error {
                self.set_status(tr!("無法檢查更新：{e}", e), true);
            }
        }
        self.invalidate();
    }

    /// 最新版（紀錄中的；還沒有紀錄時為 None）
    pub fn latest_version(&self) -> Option<Version> {
        self.update_state.as_ref().and_then(|s| s.latest_version())
    }

    /// 主畫面的橫幅；不顯示時為 None
    pub fn update_banner(&self) -> Option<Banner> {
        let banner = |text: String, actions: Vec<UpdateAction>, close: bool| Some(Banner { text, progress: None, actions, close });
        match &self.update {
            Update::Downloading { version, percent, .. } => Some(Banner {
                text: tr!("正在下載 {version}", version = format!("v{version}")),
                progress: Some(*percent),
                actions: vec![UpdateAction::Cancel],
                close: false,
            }),
            Update::Ready { version } => {
                banner(tr!("{version} 已下載，重新啟動後完成更新", version = format!("v{version}")), vec![UpdateAction::Restart], false)
            }
            Update::Failed { reason, .. } if !self.banner_dismissed => {
                banner(tr!("更新失敗：{reason}", reason), vec![UpdateAction::Manual], true)
            }
            Update::Failed { .. } => None,
            Update::Idle => {
                let st = self.update_state.as_ref()?;
                let v = st.latest_version()?;
                (self.config.update.check && !self.banner_dismissed && st.should_show_banner(current())).then_some(())?;
                banner(
                    tr!("有新版本 {version}", version = format!("v{v}")),
                    vec![UpdateAction::Notes, UpdateAction::Skip, UpdateAction::Install],
                    true,
                )
            }
        }
    }

    /// 「關於」頁的狀態列
    pub fn about_status(&self) -> AboutStatus {
        let status = |text: String, error: bool, actions: Vec<UpdateAction>| AboutStatus { text, error, actions, ..Default::default() };
        match &self.update {
            // 與橫幅相同的進度
            Update::Downloading { version, percent, .. } => {
                return AboutStatus {
                    text: tr!("正在下載 {version}", version = format!("v{version}")),
                    progress: Some(*percent),
                    actions: vec![UpdateAction::Cancel],
                    ..Default::default()
                };
            }
            Update::Ready { version } => {
                return status(tr!("{version} 已下載，重新啟動後完成更新", version = format!("v{version}")), false, vec![UpdateAction::Restart]);
            }
            Update::Failed { reason, .. } => return status(tr!("更新失敗：{reason}", reason), true, vec![UpdateAction::Manual]),
            Update::Idle => {}
        }
        match self.update_check {
            CheckStatus::Checking => return status(tr!("檢查中…").into(), false, Vec::new()),
            CheckStatus::Failed => return status(tr!("無法檢查更新").into(), true, vec![UpdateAction::Retry]),
            CheckStatus::Idle => {}
        }
        let Some(st) = &self.update_state else { return AboutStatus::default() };
        match st.availability(current()).version() {
            Some(v) => status(tr!("有新版本 {version}", version = format!("v{v}")), false, vec![UpdateAction::Notes, UpdateAction::Install]),
            None => match st.last_check {
                // 上次檢查的時間以次要的顏色接在後面
                Some(t) => AboutStatus {
                    text: tr!("已是最新版").into(),
                    detail: Some(format!("（{}）", tr!("上次檢查：{time}", time = zing_core::format::relative_local(t, now_secs())))),
                    ..Default::default()
                },
                None => AboutStatus::default(),
            },
        }
    }

    /// 「檢查更新」按鈕可以按（檢查中、下載中、已下載時停用）
    pub fn can_check_update(&self) -> bool {
        self.update_check != CheckStatus::Checking && matches!(self.update, Update::Idle | Update::Failed { .. })
    }

    pub fn update_action(&mut self, a: UpdateAction) {
        let latest = self.latest_version();
        match a {
            UpdateAction::Notes | UpdateAction::Manual => {
                let version = match &self.update {
                    Update::Failed { version, .. } => *version,
                    _ => latest,
                };
                crate::services::launch(&self.notifier, zoo_update::release_page_url(REPO, version), None, None, None, Show::Normal);
            }
            UpdateAction::Skip => {
                let Some(v) = latest else { return };
                let Some(st) = self.update_state.as_mut() else { return };
                st.skip(v);
                if !self.isolated {
                    let skipped = st.skipped.clone();
                    std::thread::spawn(move || {
                        let mut st = read_state();
                        st.skipped = skipped;
                        write_state(&st);
                    });
                }
            }
            UpdateAction::Install => self.start_install(),
            UpdateAction::Later => {
                self.banner_dismissed = true;
                if matches!(self.update, Update::Failed { .. }) {
                    self.update = Update::Idle;
                }
            }
            UpdateAction::Cancel => {
                if let Update::Downloading { cancel, .. } = &self.update {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            UpdateAction::Restart => self.restart_for_update(),
            UpdateAction::Check | UpdateAction::Retry => self.check_update_now(),
        }
        self.leave_banner_if_gone();
        self.invalidate();
    }

    fn start_install(&mut self) {
        if matches!(self.update, Update::Downloading { .. } | Update::Ready { .. }) {
            return;
        }
        if self.isolated {
            return self.set_status(tr!("效能與冒煙測試不安裝更新"), false);
        }
        let Some(version) = self.latest_version() else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        self.update = Update::Downloading { version, percent: 0, cancel: cancel.clone() };
        let n = self.notifier.clone();
        std::thread::spawn(move || {
            let progress = |percent| n.send(Msg::UpdateProgress { percent });
            let result = install(&crate::settings::app_dir(), &cancel, &progress);
            n.send(Msg::UpdateInstalled(result));
        });
    }

    pub fn on_update_progress(&mut self, p: u32) {
        if let Update::Downloading { percent, .. } = &mut self.update {
            *percent = p;
            self.invalidate();
        }
    }

    pub fn on_update_installed(&mut self, result: Result<Installed, String>) {
        let Update::Downloading { version, .. } = std::mem::take(&mut self.update) else { return };
        self.update = match result {
            Ok(Installed::Done(v)) => Update::Ready { version: v },
            Ok(Installed::Canceled) => Update::Idle,
            Ok(Installed::NoAsset(v)) => {
                crate::services::launch(&self.notifier, zoo_update::release_page_url(REPO, Some(v)), None, None, None, Show::Normal);
                Update::Idle
            }
            Err(reason) => {
                self.banner_dismissed = false;
                Update::Failed { version: Some(version), reason }
            }
        };
        self.leave_banner_if_gone();
        self.invalidate();
    }

    /// 照一般方式關閉視窗（儲存工作階段），結束後啟動新版（`relaunch`）
    fn restart_for_update(&mut self) {
        if self.jobs.active() > 0 {
            return self.set_status(tr!("還有檔案作業尚未完成，完成後再重新啟動"), true);
        }
        RESTART.store(true, Ordering::Relaxed);
        // 所有視窗照一般方式關閉，最後一個結束後才啟動新版
        crate::hub::close_all();
    }

    pub fn banner_height(&self) -> f32 {
        (self.row_h() + 14.0).round()
    }

    /// 橫幅的版面：文字在左，按鈕由右往左排（主要按鈕在最右、✕ 之前），下載中時文字後面是進度條
    pub fn layout_banner(&self, bar: Rect, b: &Banner) -> crate::view::BannerLayout {
        let h = (bar.h() - 10.0).max(18.0);
        let (t, bt) = (bar.t + (bar.h() - h) / 2.0, bar.t + (bar.h() + h) / 2.0);
        let mut right = bar.r - 8.0;
        let close = b.close.then(|| {
            let r = Rect::new(right - h, t, right, bt);
            right -= h + 6.0;
            r
        });
        let mut targets: Vec<Rect> = b
            .actions
            .iter()
            .rev()
            .map(|a| {
                let w = self.gfx.measure(zing_core::i18n::tr(a.label()), Font::Text) + 24.0;
                let r = Rect::new(right - w, t, right, bt);
                right -= w + 6.0;
                r
            })
            .collect();
        targets.reverse();
        targets.extend(close);
        let progress = b.progress.map(|_| {
            let l = 16.0 + self.gfx.measure(&b.text, Font::Text) + 16.0;
            Rect::new(l, bar.t + bar.h() / 2.0 - 3.0, (l + 180.0).min(right - 60.0), bar.t + bar.h() / 2.0 + 3.0)
        });
        crate::view::BannerLayout { bar, targets, progress }
    }

    pub fn paint_banner(&self, lay: &crate::view::Layout) {
        let (Some(bl), Some(b)) = (lay.banner.as_ref(), self.update_banner()) else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius.max(4.0);
        // 強調色（失敗時錯誤色）的淡色（與選取相同的做法：疊在背景上，文字維持一般的顏色）
        g.fill(bl.bar, c.background);
        // 失敗時用錯誤色的淡色
        let tint = if matches!(self.update, Update::Failed { .. }) { c.error } else { c.accent };
        g.fill(bl.bar, tint.with_alpha(0x2a));
        g.hline(0.0, bl.bar.r, bl.bar.b - 1.0, c.border);
        let text_r = bl.targets.first().map_or(bl.bar.r, |t| t.l - 8.0);
        g.text(&b.text, Font::Text, Rect::new(bl.bar.l + 16.0, bl.bar.t, text_r, bl.bar.b), c.text);
        if let (Some(p), Some(pct)) = (bl.progress, b.progress) {
            g.fill_round(p, 3.0, c.border);
            g.fill_round(Rect::new(p.l, p.t, p.l + p.w() * pct.min(100) as f32 / 100.0, p.b), 3.0, c.accent);
            g.text(&format!("{pct}%"), Font::Small, Rect::new(p.r + 8.0, bl.bar.t, p.r + 60.0, bl.bar.b), c.text_secondary);
        }
        for (k, r) in bl.targets.iter().enumerate() {
            let hot = self.hover == Hit::Banner(k);
            match b.actions.get(k) {
                Some(a) => self.paint_button(*r, a.label(), a.primary(), hot),
                None => {
                    if hot {
                        g.fill_round(*r, radius, c.hover);
                    }
                    g.text(crate::gfx::glyph::CLOSE, Font::GlyphSmall, *r, c.text_secondary);
                }
            }
            if self.banner_focus == Some(k) {
                g.stroke_round(r.inset(-2.0, -2.0), radius, c.accent, 2.0);
            }
        }
    }

    /// 點了橫幅的第 k 個目標（按鈕或 ✕）
    pub fn banner_click(&mut self, k: usize) {
        let Some(b) = self.update_banner() else { return };
        self.update_action(b.actions.get(k).copied().unwrap_or(UpdateAction::Later));
    }

    /// 命令列按 Tab、沒有命令列時按 F10：鍵盤焦點移到橫幅的第一個按鈕；沒有橫幅時回傳 false
    pub fn enter_banner(&mut self) -> bool {
        if self.update_banner().is_none() {
            return false;
        }
        self.banner_focus = Some(0);
        self.invalidate();
        true
    }

    fn leave_banner_if_gone(&mut self) {
        let n = self.update_banner().map_or(0, |b| b.actions.len() + b.close as usize);
        if self.banner_focus.is_some_and(|f| f >= n) {
            self.banner_focus = (n > 0).then_some(0);
        }
    }

    /// 焦點在橫幅時的按鍵（目標依序是按鈕、✕）；不是橫幅的按鍵就離開、回傳 false（照常處理）
    pub fn banner_key(&mut self, vk: u16) -> bool {
        let Some(b) = self.update_banner() else {
            self.banner_focus = None;
            return false;
        };
        let n = b.actions.len() + b.close as usize;
        let f = self.banner_focus.unwrap_or(0).min(n.saturating_sub(1));
        match vk {
            0x25 => self.banner_focus = Some((f + n - 1) % n),
            0x27 => self.banner_focus = Some((f + 1) % n),
            0x24 => self.banner_focus = Some(0),
            0x23 => self.banner_focus = Some(n - 1),
            0x0D | 0x20 => {
                let a = b.actions.get(f).copied().unwrap_or(UpdateAction::Later);
                self.update_action(a);
            }
            0x1B | 0x79 | 0x09 => self.banner_focus = None,
            _ => {
                self.banner_focus = None;
                self.invalidate();
                return false;
            }
        }
        self.invalidate();
        true
    }
}

/// 以 WinHTTP 取得 GitHub 的資料（只用 https；跟隨轉址；系統的 Proxy 設定）
pub mod http {
    use super::*;
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::{PCWSTR, w};

    /// 下載的大小上限（防止伺服器送來不合理的內容）
    const MAX_DOWNLOAD: u64 = 64 << 20;
    const MAX_SMALL: usize = 4 << 20;

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
        // WinHTTP 的錯誤碼（12xxx）系統訊息表沒有：連不上的都是「無法連線」（docs/UI.md 的用詞）
        match e.code().0 as u32 & 0xFFFF {
            12002 | 12007 | 12029 | 12030 => tr!("無法連線").into(),
            code => {
                let m = e.message();
                if m.is_empty() { tr!("WinHTTP 錯誤 {code}", code) } else { m }
            }
        }
    }

    fn check(h: *mut c_void) -> Result<Handle, String> {
        if h.is_null() { Err(last_error()) } else { Ok(Handle(h)) }
    }

    fn open(url: &str) -> Result<Request, String> {
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

    /// 小檔案（latest.json、專案清單、封面）的內容
    pub fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
        zing_core::guard::io("下載檔案");
        let r = open(url)?;
        if r.status != 200 {
            return Err(bad_status(r.status));
        }
        let mut out = Vec::new();
        let mut buf = [0u8; 16 << 10];
        loop {
            let n = read(&r, &mut buf)?;
            if n == 0 {
                break;
            }
            if out.len() + n > MAX_SMALL {
                return Err(tr!("檔案太大（{total} 位元組）", total = out.len() + n));
            }
            out.extend_from_slice(&buf[..n]);
        }
        Ok(out)
    }

    /// 下載到記憶體；取消時為 None。`progress` 收到 0–100（每變一個百分點一次）
    pub fn download(url: &str, cancel: &AtomicBool, progress: &dyn Fn(u32)) -> Result<Option<Vec<u8>>, String> {
        zing_core::guard::io("下載更新");
        let r = open(url)?;
        if r.status != 200 {
            return Err(bad_status(r.status));
        }
        let total = number_header(&r.req.0, WINHTTP_QUERY_CONTENT_LENGTH).unwrap_or(0);
        if total > MAX_DOWNLOAD {
            return Err(tr!("檔案太大（{total} 位元組）", total));
        }
        let mut out = Vec::with_capacity(total as usize);
        let mut buf = vec![0u8; 64 << 10];
        let mut last = u32::MAX;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let n = read(&r, &mut buf)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            let got = out.len() as u64;
            if got > MAX_DOWNLOAD {
                return Err(tr!("檔案太大（{total} 位元組）", total = got));
            }
            let pct = (got * 100).checked_div(total).map_or(0, |p| p.min(100) as u32);
            if pct != last {
                last = pct;
                progress(pct);
            }
        }
        if total > 0 && out.len() as u64 != total {
            return Err(tr!("下載不完整（{got}／{total} 位元組）", got = out.len(), total));
        }
        Ok(Some(out))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 實際連到 GitHub：`cargo test -p zing-app latest_release -- --ignored --nocapture`
        #[test]
        #[ignore]
        fn latest_release() {
            // 跟隨轉址（github.com → objects.githubusercontent.com）並讀完內容
            let m = fetch_manifest().expect("讀取 latest.json");
            println!("最新版 {}：{:?}", m.version, m.platforms.keys().collect::<Vec<_>>());
            assert!(m.version().is_some(), "{}", m.version);
            let asset = m.asset(zoo_update::current_platform()).expect("沒有這個平台的檔案");
            asset.check_url(REPO).unwrap();
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

    /// 從 GitHub 下載最新的 Release 安裝到暫存資料夾（舊的 zing.exe 改名成 .old）：latest.json、下載、
    /// 簽章與雜湊、解壓縮、替換整條路。目前的版本要比最新版舊（測試的建置是 Cargo.toml 的版本）。
    /// `cargo test -p zing-app install_latest_release -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn install_latest_release() {
        let d = dir("install");
        let app = d.join("app");
        std::fs::write(app.join("zing.exe"), "舊版").unwrap();
        let cancel = AtomicBool::new(false);
        let last = std::cell::Cell::new(0);
        match install(&app, &cancel, &|p| last.set(p)) {
            Ok(Installed::Done(v)) => println!("已安裝 {v}"),
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("沒有安裝"),
        }
        let exe = std::fs::read(app.join("zing.exe")).unwrap();
        assert_eq!(&exe[..2], b"MZ", "換上的不是執行檔");
        assert!(exe.len() > 1 << 20, "執行檔太小：{}", exe.len());
        assert_eq!(read(app.join("zing.exe.old")), "舊版");
        assert!(app.join("zing-index.exe").exists(), "沒有 zing-index.exe");
        println!("最後的進度：{}%；換上 {} 位元組", last.get(), exe.len());
        let _ = std::fs::remove_dir_all(&d);
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

    #[test]
    fn current_version_parses() {
        assert_eq!(current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn error_reasons_use_ui_wording() {
        assert_eq!(reason(zoo_update::Error::Signature), tr!("簽章驗證失敗"));
        assert_eq!(reason(zoo_update::Error::SignatureFormat), tr!("簽章驗證失敗"));
    }
}
