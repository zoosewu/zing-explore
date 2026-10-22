//! 第三方右鍵選單擴充功能（7-Zip、WinRAR、PowerToys…）：放進精簡右鍵選單的第一層（ctxmenu.rs）。
//!
//! 只在 `ShellHost` 執行緒使用（STA）：預先準備選單時（`shellhost::prepare`）依項目的類型從登錄檔找出擴充功能，
//! 只取 DLL 不在 Windows 資料夾的（Windows 自己的項目在第一層由程式提供，或在「顯示更多選項」中），
//! 逐一建立、`QueryContextMenu` 到各自的選單並計時，讀出項目（文字、可否使用、子選單）送給 UI；
//! 使用者選了項目再以同一個物件 `InvokeCommand`。
//!
//! 緩慢的擴充功能：同一個程序中第二次以後連續兩次超過 `SLOW_MS`（偶發的一次不算），或第一次（含載入 DLL）超過
//! `SLOW_FIRST_MS`，就記在 `%LOCALAPPDATA%\zing-explore\slow-menu-extensions.tsv`（只屬於這台電腦），之後不再放進第一層、
//! 也不載入（設定 `context_menu.show_slow` 可改為照樣顯示）；完整的 Windows 選單中照常有它們。
//! 記錄 `SLOW_EXPIRE` 後失效、重新量（擴充功能更新後可能變快）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_ANY, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::UI::Shell::{
    CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, IContextMenu, IContextMenu2, IShellExtInit,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, HMENU, MENUITEMINFOW, MFS_DISABLED, MFT_SEPARATOR, MIIM_FTYPE,
    MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SW_SHOWNORMAL, WM_INITMENUPOPUP,
};
use windows::core::{GUID, Interface, PCSTR, PCWSTR, PWSTR};
use zing_core::guard;
use zing_core::text::wide_z;

use crate::shell::Pidl;

/// 第二次以後（DLL 已載入）超過這個時間就算緩慢
const SLOW_MS: f64 = 50.0;
/// 第一次（含載入 DLL）超過這個時間就算緩慢
const SLOW_FIRST_MS: f64 = 1000.0;
/// 緩慢的記錄多久後失效、重新量
const SLOW_EXPIRE: u64 = 7 * 24 * 3600;
const ID_LAST: u32 = 0x7fff;
/// windows-rs 沒有匯出（同 shell.rs）
const CMIC_MASK_UNICODE: u32 = 0x4000;

/// 擴充功能加入的一個選單項目（給 UI 顯示）
#[derive(Clone, Debug, PartialEq)]
pub struct ExtItem {
    pub label: String,
    /// `InvokeCommand` 用的 id（子選單、分隔線為 0）
    pub id: u32,
    pub enabled: bool,
    pub separator: bool,
    pub sub: Vec<ExtItem>,
}

/// 一個擴充功能加入的項目
#[derive(Clone, Debug, PartialEq)]
pub struct ExtMenu {
    pub name: String,
    pub items: Vec<ExtItem>,
}

/// 已載入的擴充功能（留在主機執行緒，等使用者選擇）
pub struct LoadedExt {
    cm: IContextMenu,
    menu: HMENU,
}

impl Drop for LoadedExt {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.menu);
        }
    }
}

struct Slow {
    /// 「clsid（小寫）|類型（file、folder、background）」→（名稱, 毫秒, 記錄的時間（Unix 秒））
    map: HashMap<String, (String, u32, u64)>,
    /// 連續超過 `SLOW_MS` 的次數（程序內）
    strikes: HashMap<String, u32>,
    loaded: bool,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

static SLOW: std::sync::LazyLock<Mutex<Slow>> = std::sync::LazyLock::new(|| Mutex::new(Slow { map: HashMap::new(), strikes: HashMap::new(), loaded: false }));

/// 這個程序中已經載入過的擴充功能（第二次起才以 `SLOW_MS` 判斷）
static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn slow_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("zing-explore").join("slow-menu-extensions.tsv"))
}

fn with_slow<R>(persist: bool, f: impl FnOnce(&mut Slow) -> R) -> R {
    let mut s = SLOW.lock().unwrap_or_else(|e| e.into_inner());
    if !s.loaded {
        s.loaded = true;
        if persist && let Some(text) = slow_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            let now = now_secs();
            for line in text.lines() {
                let mut f = line.split('\t');
                if let (Some(c), Some(n), Some(ms), Some(at)) = (f.next(), f.next(), f.next(), f.next()) {
                    let at: u64 = at.parse().unwrap_or(0);
                    if now.saturating_sub(at) < SLOW_EXPIRE {
                        s.map.insert(c.to_ascii_lowercase(), (n.to_string(), ms.parse().unwrap_or(0), at));
                    }
                }
            }
        }
    }
    f(&mut s)
}

fn save_slow(s: &Slow) {
    let Some(p) = slow_path() else { return };
    let mut text = String::new();
    for (c, (n, ms, at)) in &s.map {
        text.push_str(&format!("{c}\t{n}\t{ms}\t{at}\n"));
    }
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, text);
}

struct RegKey(HKEY);

impl RegKey {
    fn open(root: HKEY, path: &str) -> Option<Self> {
        let w = wide_z(path);
        let mut k = HKEY::default();
        unsafe { RegOpenKeyExW(root, PCWSTR(w.as_ptr()), None, KEY_READ, &mut k).ok().ok()? };
        Some(RegKey(k))
    }

    fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut buf = [0u16; 260];
            let mut len = buf.len() as u32;
            if unsafe { RegEnumKeyExW(self.0, i, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) }.is_err() {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        out
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// 字串值（`name` 為空是預設值）
fn reg_string(root: HKEY, path: &str, name: &str) -> Option<String> {
    let (p, n) = (wide_z(path), wide_z(name));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(root, PCWSTR(p.as_ptr()), PCWSTR(n.as_ptr()), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size))
            .ok()
            .ok()?;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len])).filter(|s| !s.is_empty())
}

fn reg_has_value(root: HKEY, path: &str, name: &str) -> bool {
    let (p, n) = (wide_z(path), wide_z(name));
    unsafe { RegGetValueW(root, PCWSTR(p.as_ptr()), PCWSTR(n.as_ptr()), RRF_RT_ANY, None, None, None).is_ok() }
}

/// 項目類型對應的登錄機碼（HKCR 之下）：第一個項目決定，與檔案總管相同
fn type_keys(items: &[String]) -> Vec<String> {
    let Some(first) = items.first() else { return vec!["Directory\\Background".into()] };
    if Path::new(first).is_dir() {
        return vec!["Directory".into(), "Folder".into(), "AllFilesystemObjects".into()];
    }
    let mut keys = vec!["*".to_string(), "AllFilesystemObjects".to_string()];
    if let Some(ext) = Path::new(first).extension().map(|e| format!(".{}", e.to_string_lossy())) {
        if let Some(progid) = reg_string(HKEY_CLASSES_ROOT, &ext, "") {
            keys.push(progid);
        }
        keys.push(format!("SystemFileAssociations\\{ext}"));
        if let Some(perceived) = reg_string(HKEY_CLASSES_ROOT, &ext, "PerceivedType") {
            keys.push(format!("SystemFileAssociations\\{perceived}"));
        }
    }
    keys
}

/// 第三方（DLL 不在 Windows 資料夾）且沒有被封鎖的擴充功能：（機碼, 名稱, CLSID）
fn third_party_handlers(items: &[String]) -> Vec<(String, String, String)> {
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()).to_ascii_lowercase();
    let blocked = |c: &str| {
        let path = "Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Blocked";
        reg_has_value(HKEY_LOCAL_MACHINE, path, c) || reg_has_value(HKEY_CURRENT_USER, path, c)
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for key in type_keys(items) {
        let Some(k) = RegKey::open(HKEY_CLASSES_ROOT, &format!("{key}\\shellex\\ContextMenuHandlers")) else { continue };
        for name in k.subkeys() {
            let clsid = reg_string(HKEY_CLASSES_ROOT, &format!("{key}\\shellex\\ContextMenuHandlers\\{name}"), "")
                .filter(|v| v.starts_with('{'))
                .unwrap_or_else(|| name.clone());
            if !clsid.starts_with('{') || !seen.insert(clsid.to_ascii_lowercase()) || blocked(&clsid) {
                continue;
            }
            let Some(dll) = reg_string(HKEY_CLASSES_ROOT, &format!("CLSID\\{clsid}\\InprocServer32"), "") else { continue };
            let dll = expand_env(&dll).to_ascii_lowercase();
            if dll.trim_matches('"').starts_with(&windir) || !dll.contains('\\') {
                continue;
            }
            out.push((key.clone(), name.trim().to_string(), clsid));
        }
    }
    out
}

fn expand_env(s: &str) -> String {
    let mut out = s.to_string();
    while let Some(a) = out.find('%') {
        let Some(len) = out[a + 1..].find('%') else { break };
        let var = &out[a + 1..a + 1 + len];
        let val = std::env::var(var).unwrap_or_default();
        out.replace_range(a..a + len + 2, &val);
    }
    out
}

/// 讀出選單的項目；子選單先送 WM_INITMENUPOPUP（有些擴充功能這時才加入項目）
unsafe fn read_menu(cm2: Option<&IContextMenu2>, menu: HMENU, depth: u32) -> Vec<ExtItem> {
    let mut out = Vec::new();
    let n = unsafe { GetMenuItemCount(Some(menu)) };
    for i in 0..n.max(0) {
        let mut buf = [0u16; 260];
        let mut mii = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING,
            dwTypeData: PWSTR(buf.as_mut_ptr()),
            cch: (buf.len() - 1) as u32,
            ..Default::default()
        };
        if unsafe { GetMenuItemInfoW(menu, i as u32, true, &mut mii) }.is_err() {
            continue;
        }
        if mii.fType.0 & MFT_SEPARATOR.0 != 0 {
            out.push(ExtItem { label: String::new(), id: 0, enabled: false, separator: true, sub: Vec::new() });
            continue;
        }
        let label = menu_text(&buf[..(mii.cch as usize).min(buf.len())]);
        if label.is_empty() {
            // 自繪的項目沒有文字：無法顯示
            continue;
        }
        let enabled = mii.fState.0 & MFS_DISABLED.0 == 0;
        let mut sub = Vec::new();
        if !mii.hSubMenu.is_invalid() && depth < 4 {
            if let Some(cm2) = cm2 {
                let _ = unsafe { cm2.HandleMenuMsg(WM_INITMENUPOPUP, WPARAM(mii.hSubMenu.0 as usize), LPARAM(i as isize)) };
            }
            sub = unsafe { read_menu(cm2, mii.hSubMenu, depth + 1) };
            if sub.iter().all(|s| s.separator) {
                continue;
            }
        }
        let id = if mii.hSubMenu.is_invalid() { mii.wID } else { 0 };
        out.push(ExtItem { label, id, enabled, separator: false, sub });
    }
    trim_separators(out)
}

/// 去掉頭尾與連續的分隔線
fn trim_separators(items: Vec<ExtItem>) -> Vec<ExtItem> {
    let mut out: Vec<ExtItem> = Vec::new();
    for it in items {
        if it.separator && out.last().is_none_or(|l| l.separator) {
            continue;
        }
        out.push(it);
    }
    while out.last().is_some_and(|l| l.separator) {
        out.pop();
    }
    out
}

/// 選單文字：去掉 & 快速鍵標記（&& 是 &）與 Tab 之後的快速鍵說明
fn menu_text(w: &[u16]) -> String {
    let s = String::from_utf16_lossy(w);
    let mut s: Vec<char> = s.split('\t').next().unwrap_or("").chars().collect();
    // 中日文的快速鍵標記「(&O)」整段去掉
    while let Some(i) = s.windows(4).position(|c| c[0] == '(' && c[1] == '&' && c[3] == ')') {
        s.drain(i..i + 4);
    }
    let s: String = s.into_iter().collect();
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                out.push('&');
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// 載入的結果
#[derive(Default)]
pub struct Loaded {
    pub menus: Vec<ExtMenu>,
    pub exts: Vec<LoadedExt>,
    /// 因為緩慢而沒有放進第一層的擴充功能名稱
    pub hidden: Vec<String>,
    /// 各擴充功能的耗時（名稱, 毫秒；診斷用）
    pub times: Vec<(String, f64)>,
}

/// 載入第三方擴充功能。`show_slow` 為 false 時跳過已知緩慢的；`persist` 為 false（隔離模式）時不讀寫記錄檔
pub fn load(items: &[String], folder: &str, show_slow: bool, persist: bool) -> Loaded {
    guard::io("載入右鍵選單擴充功能");
    let mut out = Loaded::default();
    let handlers = third_party_handlers(items);
    if handlers.is_empty() {
        return out;
    }
    let parent = if items.is_empty() { Pidl::parse(folder).ok() } else { zing_core::path::parent(&items[0]).and_then(|p| Pidl::parse(&p).ok()) };
    let Some(parent) = parent else { return out };
    let data: Option<IDataObject> = if items.is_empty() { None } else { crate::shell::data_object(items).ok() };
    // 同一個擴充功能對不同類型的速度可能差很多（WinRAR 對檔案約 70 ms、對資料夾不到 1 ms）：分開判斷
    let kind = match items.first() {
        None => "background",
        Some(p) if Path::new(p).is_dir() => "folder",
        Some(_) => "file",
    };
    for (key, name, clsid) in handlers {
        let lower = clsid.to_ascii_lowercase();
        let slow_key = format!("{lower}|{kind}");
        if !show_slow && with_slow(persist, |s| s.map.contains_key(&slow_key)) {
            out.hidden.push(name);
            continue;
        }
        let Ok(guid) = GUID::try_from(clsid.trim_matches(|c| c == '{' || c == '}')) else { continue };
        let first = SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(lower.clone());
        let t = Instant::now();
        let result = unsafe { load_one(&guid, &key, parent.0, data.as_ref()) };
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        out.times.push((name.clone(), ms));
        let slow = with_slow(persist, |s| {
            let strikes = s.strikes.entry(slow_key.clone()).or_insert(0);
            *strikes = if !first && ms > SLOW_MS { *strikes + 1 } else { 0 };
            let slow = (first && ms > SLOW_FIRST_MS) || *strikes >= 2;
            if slow {
                s.map.insert(slow_key, (name.clone(), ms as u32, now_secs()));
                if persist {
                    save_slow(s);
                }
            }
            slow
        });
        if slow && !show_slow {
            out.hidden.push(name);
            continue;
        }
        if let Some((ext, items)) = result.filter(|(_, items)| !items.is_empty()) {
            out.menus.push(ExtMenu { name, items });
            out.exts.push(ext);
        }
    }
    out.hidden.sort();
    out.hidden.dedup();
    out
}

unsafe fn load_one(guid: &GUID, key: &str, parent: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST, data: Option<&IDataObject>) -> Option<(LoadedExt, Vec<ExtItem>)> {
    unsafe {
        let init: IShellExtInit = CoCreateInstance(guid, None, CLSCTX_INPROC_SERVER).ok()?;
        let hkey = RegKey::open(HKEY_CLASSES_ROOT, key);
        let hkey = hkey.as_ref().map(|k| k.0);
        match data {
            Some(d) => init.Initialize(Some(parent as *const _), d, hkey).ok()?,
            None => init.Initialize(Some(parent as *const _), None::<&IDataObject>, hkey).ok()?,
        }
        let cm: IContextMenu = init.cast().ok()?;
        let menu = CreatePopupMenu().ok()?;
        let ext = LoadedExt { cm, menu };
        ext.cm.QueryContextMenu(menu, 0, 1, ID_LAST, CMF_NORMAL).ok().ok()?;
        let cm2: Option<IContextMenu2> = ext.cm.cast().ok();
        let items = read_menu(cm2.as_ref(), menu, 0);
        Some((ext, items))
    }
}

/// 執行擴充功能的項目
pub fn invoke(ext: &LoadedExt, id: u32, hwnd: HWND, folder: &str, pt: POINT) -> windows::core::Result<()> {
    let dir = wide_z(folder);
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE,
        hwnd,
        lpVerb: PCSTR((id - 1) as usize as *const u8),
        lpVerbW: PCWSTR((id - 1) as usize as *const u16),
        lpDirectoryW: if folder.is_empty() { PCWSTR::null() } else { PCWSTR(dir.as_ptr()) },
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: pt,
        ..Default::default()
    };
    unsafe { ext.cm.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_text_strips_accelerators() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        assert_eq!(menu_text(&w("&Open")), "Open");
        assert_eq!(menu_text(&w("Save && Exit")), "Save & Exit");
        assert_eq!(menu_text(&w("開啟(&O)")), "開啟");
        assert_eq!(menu_text(&w("Copy\tCtrl+C")), "Copy");
    }

    #[test]
    fn separators_are_trimmed() {
        let sep = || ExtItem { label: String::new(), id: 0, enabled: false, separator: true, sub: Vec::new() };
        let it = |l: &str| ExtItem { label: l.into(), id: 1, enabled: true, separator: false, sub: Vec::new() };
        let r = trim_separators(vec![sep(), it("a"), sep(), sep(), it("b"), sep()]);
        assert_eq!(r.iter().map(|i| i.separator).collect::<Vec<_>>(), [false, true, false]);
    }

    #[test]
    fn env_is_expanded() {
        // SAFETY: 測試中只讀取，設定的是這個測試專用的變數
        unsafe { std::env::set_var("ZING_TEST_EXPAND", "C:\\X") };
        assert_eq!(expand_env("%ZING_TEST_EXPAND%\\a.dll"), "C:\\X\\a.dll");
    }
    /// 這台電腦的第三方擴充功能會放進第一層的項目：`cargo test --release -p zing-app ext_items_here -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn ext_items_here() {
        let _allow = guard::allow_io("測試");
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        let dir = zing_core::testdir::dir().join("zing-ext-items");
        let _ = std::fs::create_dir_all(dir.join("子資料夾"));
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let d = dir.to_string_lossy().into_owned();
        let cases = [vec![dir.join("a.txt").to_string_lossy().into_owned()], vec![dir.join("子資料夾").to_string_lossy().into_owned()], vec![]];
        fn show(items: &[ExtItem], depth: usize) {
            for it in items {
                if it.separator {
                    println!("{}----", "  ".repeat(depth));
                } else {
                    println!("{}{}{}", "  ".repeat(depth), it.label, if it.enabled { "" } else { "（停用）" });
                    show(&it.sub, depth + 1);
                }
            }
        }
        for round in 0..4 {
            for items in &cases {
                let t = Instant::now();
                let r = load(items, &d, false, false);
                println!("== 第 {} 輪 {:?}：{:.1} ms，隱藏 {:?}", round + 1, items.first().map(|s| s.rsplit('\\').next().unwrap_or("")), t.elapsed().as_secs_f64() * 1000.0, r.hidden);
                println!("   {:?}", r.times.iter().map(|(n, ms)| format!("{n} {ms:.1}")).collect::<Vec<_>>());
                if round == 0 {
                    for m in &r.menus {
                        println!("[{}]", m.name);
                        show(&m.items, 1);
                    }
                }
            }
        }
    }
}
