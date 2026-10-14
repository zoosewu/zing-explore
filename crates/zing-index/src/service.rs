//! Windows 服務：服務控制管理員（SCM）的進入點、安裝與移除。
//!
//! 安裝時把執行檔複製到 %ProgramFiles%\zing-explore（只有系統管理員能寫入；以 LocalSystem 執行的程式
//! 不能放在一般使用者可修改的位置），服務設為延遲自動啟動。安裝與移除需要系統管理員權限，
//! 沒有時以 UAC 重新啟動自己。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_CALL_NOT_IMPLEMENTED, ERROR_CANCELLED, ERROR_SERVICE_DOES_NOT_EXIST, HANDLE, NO_ERROR,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, QueryServiceStatus, RegisterServiceCtrlHandlerExW, SC_HANDLE,
    SC_MANAGER_ALL_ACCESS, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS, SERVICE_AUTO_START,
    SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_CONFIG_DESCRIPTION, SERVICE_CONTROL_INTERROGATE,
    SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_DELAYED_AUTO_START_INFO, SERVICE_DESCRIPTIONW,
    SERVICE_ERROR_NORMAL, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE,
    SERVICE_STATUS_HANDLE, SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
    SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{PCWSTR, PWSTR};
use zing_core::text::wide_z;

use crate::server::{self, Server};

pub const SERVICE_NAME: &str = "zing-index";
const DISPLAY_NAME: &str = "zing-explore 索引服務";
const DESCRIPTION: &str =
    "讀取 NTFS 主檔案表與變更日誌，提供 zing-explore 資料夾大小與子資料夾清單。停用後 zing-explore 改用逐資料夾掃描。";

static SERVER: OnceLock<Arc<Server>> = OnceLock::new();
static STATUS: AtomicUsize = AtomicUsize::new(0);

fn set_status(state: SERVICE_STATUS_CURRENT_STATE, wait_hint: u32) {
    let h = SERVICE_STATUS_HANDLE(STATUS.load(Ordering::Relaxed) as *mut _);
    let accept = if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 };
    let s = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accept,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    unsafe {
        let _ = SetServiceStatus(h, &s);
    }
}

unsafe extern "system" fn handler(control: u32, _: u32, _: *mut core::ffi::c_void, _: *mut core::ffi::c_void) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            set_status(SERVICE_STOP_PENDING, 15_000);
            if let Some(s) = SERVER.get() {
                s.request_stop();
            }
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,
        _ => ERROR_CALL_NOT_IMPLEMENTED.0,
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = wide_z(SERVICE_NAME);
    let Ok(h) = (unsafe { RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(handler), None) }) else {
        return;
    };
    STATUS.store(h.0 as usize, Ordering::Relaxed);
    set_status(SERVICE_START_PENDING, 5_000);
    let server = SERVER.get_or_init(|| Arc::new(Server::default())).clone();
    set_status(SERVICE_RUNNING, 0);
    server.run();
    set_status(SERVICE_STOPPED, 0);
}

/// 由 SCM 啟動時呼叫（`--service`）；直到服務停止才返回
pub fn dispatch() -> Result<(), String> {
    let mut name = wide_z(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR(name.as_mut_ptr()), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }.map_err(|e| e.message())
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut e as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

/// 以系統管理員身分（UAC）重新執行自己並等待結束，回傳結束代碼；使用者取消時回傳 None
pub fn run_elevated(args: &str) -> Result<Option<u32>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let file = wide_z(&exe.to_string_lossy());
    let params = wide_z(args);
    let verb = wide_z("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == ERROR_CANCELLED.to_hresult() {
                return Ok(None);
            }
            return Err(e.message());
        }
        let _ = WaitForSingleObject(info.hProcess, u32::MAX);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        Ok(Some(code))
    }
}

/// 安裝後服務執行檔的位置
pub fn installed_exe() -> PathBuf {
    let base = std::env::var_os("ProgramFiles").map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
    base.join("zing-explore").join("zing-index.exe")
}

struct Sc(SC_HANDLE);
impl Drop for Sc {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn open_service(scm: &Sc) -> Result<Option<Sc>, String> {
    let name = wide_z(SERVICE_NAME);
    match unsafe { OpenServiceW(scm.0, PCWSTR(name.as_ptr()), SERVICE_ALL_ACCESS) } {
        Ok(h) => Ok(Some(Sc(h))),
        Err(e) if e.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => Ok(None),
        Err(e) => Err(e.message()),
    }
}

/// 停止服務並等待（最多 20 秒）
fn stop_service(svc: &Sc) {
    let mut st = SERVICE_STATUS::default();
    unsafe {
        let _ = ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st);
    }
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        if unsafe { QueryServiceStatus(svc.0, &mut st) }.is_err() || st.dwCurrentState == SERVICE_STOPPED {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 安裝（或更新）並啟動服務；需要系統管理員權限
pub fn install() -> Result<(), String> {
    let scm = Sc(unsafe { OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS) }.map_err(|e| e.message())?);
    let existing = open_service(&scm)?;
    if let Some(svc) = &existing {
        stop_service(svc);
    }
    let dest = installed_exe();
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    if me != dest {
        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        // 剛停止的服務程序可能還沒完全結束
        let mut copied = std::fs::copy(&me, &dest);
        for _ in 0..20 {
            if copied.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
            copied = std::fs::copy(&me, &dest);
        }
        copied.map_err(|e| format!("無法複製到 {}：{e}", dest.display()))?;
    }
    let bin = wide_z(&format!("\"{}\" --service", dest.display()));
    let display = wide_z(DISPLAY_NAME);
    let svc = match existing {
        Some(svc) => {
            unsafe {
                ChangeServiceConfigW(
                    svc.0,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(bin.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR(display.as_ptr()),
                )
            }
            .map_err(|e| e.message())?;
            svc
        }
        None => {
            let name = wide_z(SERVICE_NAME);
            Sc(unsafe {
                CreateServiceW(
                    scm.0,
                    PCWSTR(name.as_ptr()),
                    PCWSTR(display.as_ptr()),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(bin.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    // LocalSystem
                    PCWSTR::null(),
                    PCWSTR::null(),
                )
            }
            .map_err(|e| e.message())?)
        }
    };
    let mut desc = wide_z(DESCRIPTION);
    let d = SERVICE_DESCRIPTIONW { lpDescription: PWSTR(desc.as_mut_ptr()) };
    // 延遲自動啟動：開機後等其他服務啟動完才開始，不拖慢開機
    let delayed = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: true.into() };
    unsafe {
        let _ = ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_DESCRIPTION, Some(&d as *const _ as *const _));
        let _ = ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, Some(&delayed as *const _ as *const _));
        StartServiceW(svc.0, None).map_err(|e| format!("服務已安裝，但無法啟動：{}", e.message()))?;
    }
    server::log(false, &format!("已安裝服務：{}", dest.display()));
    Ok(())
}

/// 停止並移除服務，刪除安裝的執行檔、快照與記錄檔；需要系統管理員權限
pub fn uninstall() -> Result<(), String> {
    let scm = Sc(unsafe { OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS) }.map_err(|e| e.message())?);
    if let Some(svc) = open_service(&scm)? {
        stop_service(&svc);
        unsafe { DeleteService(svc.0) }.map_err(|e| e.message())?;
    }
    let dest = installed_exe();
    for _ in 0..20 {
        if !dest.exists() || std::fs::remove_file(&dest).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::fs::remove_dir(dest.parent().unwrap());
    let _ = std::fs::remove_dir_all(server::data_dir());
    Ok(())
}
