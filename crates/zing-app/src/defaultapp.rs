//! 設為預設的檔案總管：只寫目前使用者的登錄（HKCU\Software\Classes，不需要系統管理員權限），關閉即刪除、還原。
//!
//! - `Directory`、`Drive` 的預設動作改成 `zing-explore`（`"zing.exe" "%1"`）：桌面、開始功能表、其他程式開啟資料夾時。
//! - Win+E：`CLSID\{52205fd8-…}\shell\opennewwindow` 的命令（`DelegateExecute` 設為空字串才會用命令）。
//! - 「在資料夾中顯示」（`SHOpenFolderAndSelectItems`）要找到登記在 ShellWindows 的視窗才會選取檔案：見 shellwin.rs。
//! - zing 是可攜式：啟動時（背景）發現登錄記的 zing.exe 已經不在了，就改成目前的位置（`repair`）。
//!
//! 一律在背景執行緒呼叫（登錄 I/O）；隔離模式（冒煙測試）不呼叫，App 只在記憶體中切換。

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::PCWSTR;
use zing_core::text::wide_z;

use crate::app::App;
use crate::services::Msg;

/// 一般的位置；測試用另一個機碼
pub const CLASSES: &str = "Software\\Classes";
const VERB: &str = "zing-explore";
/// 檔案總管的「此電腦／常用」視窗（Win+E 開的就是它）
const WIN_E: &str = "CLSID\\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}";
const TYPES: [&str; 2] = ["Directory", "Drive"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    /// 登錄指向這個 zing.exe
    On,
    /// 登錄指向另一個位置的 zing.exe
    Other(String),
}

/// 這個程式的完整路徑
pub fn current_exe() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn status(root: &str, exe: &str) -> Status {
    zing_core::guard::io("讀取預設的檔案總管設定");
    let shell = format!("{root}\\Directory\\shell");
    if !get(&shell, "").is_some_and(|v| v.eq_ignore_ascii_case(VERB)) {
        return Status::Off;
    }
    match get(&format!("{shell}\\{VERB}\\command"), "").and_then(|c| command_exe(&c)) {
        Some(p) if p.eq_ignore_ascii_case(exe) => Status::On,
        Some(p) => Status::Other(p),
        None => Status::Off,
    }
}

pub fn enable(root: &str, exe: &str) -> Result<(), String> {
    zing_core::guard::io("設為預設的檔案總管");
    let label = tr!("以 zing-explore 開啟");
    for t in TYPES {
        let shell = format!("{root}\\{t}\\shell");
        set(&format!("{shell}\\{VERB}"), "", label)?;
        set(&format!("{shell}\\{VERB}\\command"), "", &format!("\"{exe}\" \"%1\""))?;
        set(&shell, "", VERB)?;
    }
    let cmd = format!("{root}\\{WIN_E}\\shell\\opennewwindow\\command");
    set(&cmd, "", &format!("\"{exe}\" {}", zing_core::startup::THIS_PC))?;
    set(&cmd, "DelegateExecute", "")?;
    notify(root);
    Ok(())
}

/// 只刪除自己寫的（預設動作已被其他程式改掉就不動它）
pub fn disable(root: &str) -> Result<(), String> {
    zing_core::guard::io("還原預設的檔案總管");
    // Win+E 的命令指向資料夾動作的同一個程式（或名稱是 zing.exe）才是自己寫的
    let ours_exe = get(&format!("{root}\\Directory\\shell\\{VERB}\\command"), "").and_then(|c| command_exe(&c));
    for t in TYPES {
        let shell = format!("{root}\\{t}\\shell");
        if get(&shell, "").is_some_and(|v| v.eq_ignore_ascii_case(VERB)) {
            check(unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(wide_z(&shell).as_ptr()), PCWSTR::null()) })?;
        }
        delete_tree(&format!("{shell}\\{VERB}"))?;
    }
    let win_e = format!("{root}\\{WIN_E}");
    let ours = get(&format!("{win_e}\\shell\\opennewwindow\\command"), "")
        .and_then(|c| command_exe(&c))
        .is_some_and(|p| ours_exe.is_some_and(|o| o.eq_ignore_ascii_case(&p)) || p.to_ascii_lowercase().ends_with("\\zing.exe"));
    if ours {
        delete_tree(&win_e)?;
    }
    notify(root);
    Ok(())
}

/// 啟動時：登錄指向的 zing.exe 已經不在（資料夾搬家、改名）就改成這個；回傳最後的狀態
pub fn repair(root: &str, exe: &str) -> Status {
    match status(root, exe) {
        Status::Other(p) if !std::path::Path::new(&p).exists() => match enable(root, exe) {
            Ok(()) => Status::On,
            Err(_) => Status::Other(p),
        },
        s => s,
    }
}

/// `"C:\…\zing.exe" "%1"` 的程式路徑
fn command_exe(cmd: &str) -> Option<String> {
    let rest = cmd.trim().strip_prefix('"')?;
    let (exe, _) = rest.split_once('"')?;
    Some(exe.to_string()).filter(|e| !e.is_empty())
}

/// 通知 Shell 重新讀取關聯（測試的機碼不通知）
fn notify(root: &str) {
    if root != CLASSES {
        return;
    }
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

fn check(e: WIN32_ERROR) -> Result<(), String> {
    if e.is_ok() || e == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(tr!("無法修改登錄：{e}", e = windows::core::Error::from(e.to_hresult()).message()))
    }
}

/// 字串值（`name` 為空是預設值）
fn get(path: &str, name: &str) -> Option<String> {
    let (p, n) = (wide_z(path), wide_z(name));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()), PCWSTR(n.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size))
    };
    r.is_ok().then(|| {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    })
}

fn set(path: &str, name: &str, value: &str) -> Result<(), String> {
    let (p, n) = (wide_z(path), wide_z(name));
    let mut key = HKEY::default();
    check(unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()), None, None, REG_OPTION_NON_VOLATILE, KEY_READ | KEY_WRITE, None, &mut key, None)
    })?;
    let data: Vec<u8> = wide_z(value).iter().flat_map(|c| c.to_le_bytes()).collect();
    let r = check(unsafe { RegSetValueExW(key, PCWSTR(n.as_ptr()), None, REG_SZ, Some(&data)) });
    unsafe {
        let _ = RegCloseKey(key);
    }
    r
}

fn delete_tree(path: &str) -> Result<(), String> {
    check(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide_z(path).as_ptr())) })
}

impl App {
    /// 在背景讀取登錄的狀態（`repair`：啟動時，順便修正搬家後的位置）；隔離模式不讀
    pub fn refresh_default_app(&mut self, repair: bool) {
        if self.isolated {
            self.default_app.get_or_insert(Status::Off);
            return;
        }
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            let exe = current_exe();
            let status = if repair { self::repair(CLASSES, &exe) } else { status(CLASSES, &exe) };
            n.send(Msg::DefaultApp { result: Ok(status), toggled: false });
        });
    }

    /// 設定頁的開關；隔離模式（冒煙測試）只在記憶體中切換
    pub fn toggle_default_app(&mut self) {
        let Some(current) = self.default_app.clone() else { return };
        let on = current != Status::On;
        if self.isolated {
            self.on_default_app(Ok(if on { Status::On } else { Status::Off }), true);
            return;
        }
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            let exe = current_exe();
            let result = if on { enable(CLASSES, &exe) } else { disable(CLASSES) };
            n.send(Msg::DefaultApp { result: result.map(|()| status(CLASSES, &exe)), toggled: true });
        });
    }

    pub fn on_default_app(&mut self, result: Result<Status, String>, toggled: bool) {
        match result {
            Ok(s) => {
                if toggled {
                    let text = match s {
                        Status::On => tr!("已設為預設的檔案總管：資料夾、磁碟機與 Win+E 會用 zing 開啟"),
                        _ => tr!("已還原為 Windows 檔案總管"),
                    };
                    self.set_status(text.to_string(), false);
                }
                self.default_app = Some(s);
            }
            Err(e) => self.set_status(e, true),
        }
        self.sync_shell_windows();
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 測試寫在自己的機碼下（不影響真正的設定），結束時刪除
    struct Root(String);

    impl Root {
        fn new(name: &str) -> Root {
            Root(format!("Software\\zing-explore-test-{}-{name}\\Classes", std::process::id()))
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let top = self.0.trim_end_matches("\\Classes");
            let _ = delete_tree(top);
            let _ = unsafe { windows::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(wide_z(top).as_ptr())) };
        }
    }

    #[test]
    fn parses_command() {
        assert_eq!(command_exe(r#""C:\a b\zing.exe" "%1""#).as_deref(), Some(r"C:\a b\zing.exe"));
        assert_eq!(command_exe(r#""D:\zing.exe" shell:MyComputerFolder"#).as_deref(), Some(r"D:\zing.exe"));
        assert_eq!(command_exe(r"C:\zing.exe %1"), None);
        assert_eq!(command_exe(r#""""#), None);
    }

    #[test]
    fn enable_disable_and_repair() {
        let root = Root::new("cycle");
        let r = root.0.as_str();
        let exe = r"C:\Tools\zing\zing.exe";
        assert_eq!(status(r, exe), Status::Off);
        enable(r, exe).unwrap();
        assert_eq!(status(r, exe), Status::On);
        assert_eq!(status(r, &exe.to_uppercase()), Status::On, "不分大小寫");
        assert_eq!(get(&format!("{r}\\Drive\\shell\\{VERB}\\command"), "").as_deref(), Some(r#""C:\Tools\zing\zing.exe" "%1""#));
        let win_e = format!("{r}\\{WIN_E}\\shell\\opennewwindow\\command");
        assert_eq!(get(&win_e, "").as_deref(), Some(r#""C:\Tools\zing\zing.exe" shell:MyComputerFolder"#));
        assert_eq!(get(&win_e, "DelegateExecute").as_deref(), Some(""));
        // 另一個位置的 zing.exe：還在就不動，不在了就改成自己
        let other = r"E:\gone\zing.exe";
        assert_eq!(status(r, other), Status::Other(exe.into()));
        let here = current_exe();
        enable(r, other).unwrap();
        assert_eq!(repair(r, &here), Status::On);
        assert_eq!(status(r, &here), Status::On);
        enable(r, &here).unwrap();
        assert_eq!(repair(r, exe), Status::Other(here.clone()), "還在的不動");
        disable(r).unwrap();
        assert_eq!(status(r, &here), Status::Off);
        for t in TYPES {
            assert_eq!(get(&format!("{r}\\{t}\\shell"), ""), None, "{t}");
        }
        assert_eq!(get(&win_e, ""), None);
        disable(r).unwrap();
    }

    #[test]
    fn disable_keeps_other_programs() {
        let root = Root::new("others");
        let r = root.0.as_str();
        enable(r, r"C:\zing\zing.exe").unwrap();
        // 之後另一個程式改了預設動作與 Win+E
        set(&format!("{r}\\Directory\\shell"), "", "openinfiles").unwrap();
        let win_e = format!("{r}\\{WIN_E}\\shell\\opennewwindow\\command");
        set(&win_e, "", r#""C:\Files\Files.exe""#).unwrap();
        assert_eq!(status(r, r"C:\zing\zing.exe"), Status::Off);
        disable(r).unwrap();
        assert_eq!(get(&format!("{r}\\Directory\\shell"), "").as_deref(), Some("openinfiles"));
        assert_eq!(get(&format!("{r}\\Drive\\shell"), ""), None);
        assert_eq!(get(&win_e, "").as_deref(), Some(r#""C:\Files\Files.exe""#));
    }
}
