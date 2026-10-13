//! Windows Shell 整合：檔案作業、剪貼簿、右鍵選單、拖放、內容對話方塊。
//!
//! 檔案作業（IFileOperation）在專用的 STA 執行緒執行：複製／移動／刪除排成一條佇列依序執行
//! （進度、暫停、取消見 jobs.rs），重新命名與新增資料夾走另一條，不會被大量複製擋住；
//! 右鍵選單與拖放必須在 UI 執行緒（它們有自己的模態訊息迴圈），
//! 呼叫端要在不持有 App 借用的狀態下呼叫，讓重入的視窗訊息能正常處理。

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use fx_core::{guard, path};
use fx_core::text::wide_z;
use windows::Win32::Foundation::{HGLOBAL, HWND, LPARAM, LRESULT, POINT, POINTL, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropSource, IDropTarget,
    IDropTarget_Impl, OleGetClipboard, OleSetClipboard, ReleaseStgMedium,
};
use windows::Win32::System::SystemServices::{MK_RBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, CLSID_DragDropHelper, CMF_CANRENAME, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE,
    CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, DragQueryFileW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
    FOF_NOCONFIRMMKDIR, FOF_RENAMEONCOLLISION, FOF_SILENT, FOF_WANTNUKEWARNING, FOFX_RECYCLEONDELETE, FileOperation, GCS_VERBW, HDROP, IContextMenu,
    IContextMenu2, IContextMenu3, IDropTargetHelper, IFileOperation, IFileOperationProgressSink,
    IFileOperationProgressSink_Impl, IShellFolder, SIGDN_NORMALDISPLAY,
    IShellItem, IShellItemArray, ILFindLastID, ILFree, SHBindToObject, SHBindToParent,
    SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHMultiFileProperties,
    SHOP_FILEPATH, SHObjectProperties, SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, MF_SEPARATOR, MF_STRING, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_DRAWITEM,
    WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
};
use windows::core::{HRESULT, Interface, PCSTR, PCWSTR, PSTR, Ref, Result, implement};

use crate::jobs::{JobCtl, JobEvent, JobId};

use crate::services::{Msg, Notifier, Select};

const CMIC_MASK_UNICODE: u32 = 0x4000;
const CMIC_MASK_SHIFT_DOWN: u32 = 0x1000_0000;
const CMIC_MASK_CONTROL_DOWN: u32 = 0x4000_0000;
const ERROR_CANCELLED_HR: i32 = 0x800704C7_u32 as i32;

// ---------------------------------------------------------------- PIDL 工具

/// 擁有所有權的絕對 PIDL
struct Pidl(*mut ITEMIDLIST);

impl Pidl {
    fn parse(p: &str) -> Result<Self> {
        guard::io("解析 Shell 路徑");
        let w = wide_z(p);
        let mut pidl = std::ptr::null_mut();
        unsafe { SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None)? };
        Ok(Self(pidl))
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { ILFree(Some(self.0)) };
    }
}

fn shell_item(p: &str) -> Result<IShellItem> {
    guard::io("建立 Shell 項目");
    let w = wide_z(p);
    unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }
}

fn item_array(paths: &[String]) -> Result<IShellItemArray> {
    let pidls = paths.iter().map(|p| Pidl::parse(p)).collect::<Result<Vec<_>>>()?;
    let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0 as *const _).collect();
    unsafe { SHCreateShellItemArrayFromIDLists(&raw) }
}

pub fn data_object(paths: &[String]) -> Result<IDataObject> {
    unsafe { item_array(paths)?.BindToHandler(None, &BHID_DataObject) }
}

// ---------------------------------------------------------------- 檔案作業

pub enum FileOp {
    Transfer { sources: Vec<String>, dest: String, mov: bool },
    Delete { paths: Vec<String>, permanent: bool },
    Rename { path: String, new_name: String },
    NewFolder { dir: String },
}

impl FileOp {
    /// 可能耗時、要顯示在作業佇列的作業
    pub fn is_long(&self) -> bool {
        matches!(self, FileOp::Transfer { .. } | FileOp::Delete { .. })
    }
}

type Queued = (FileOp, Option<(JobId, Arc<JobCtl>)>);

pub struct FileOps {
    /// 複製、移動、刪除：依序執行
    queue: Sender<Queued>,
    /// 重新命名、新增資料夾：不排在大量複製後面
    quick: Sender<Queued>,
    next_id: JobId,
}

impl FileOps {
    pub fn new(n: Notifier) -> Self {
        Self { queue: spawn_worker("fx-fileops", n.clone()), quick: spawn_worker("fx-fileops-quick", n), next_id: 1 }
    }

    /// 送出作業。耗時的作業回傳工作編號與控制旗標，交給作業佇列介面顯示
    pub fn submit(&mut self, op: FileOp) -> Option<(JobId, Arc<JobCtl>)> {
        if !op.is_long() {
            let _ = self.quick.send((op, None));
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let ctl = Arc::new(JobCtl::default());
        let _ = self.queue.send((op, Some((id, ctl.clone()))));
        Some((id, ctl))
    }
}

fn spawn_worker(name: &str, n: Notifier) -> Sender<Queued> {
    let (tx, rx) = channel::<Queued>();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            }
            for (op, job) in rx {
                if let Some((id, ctl)) = &job
                    && ctl.cancel.load(Ordering::Relaxed)
                {
                    // 排隊中就取消了
                    n.send(Msg::Job { id: *id, event: JobEvent::Finished { error: None, cancelled: true } });
                    continue;
                }
                let (dirs, select) = affected(&op);
                let sink: Option<IFileOperationProgressSink> =
                    job.as_ref().map(|(id, ctl)| Progress::new(*id, ctl.clone(), n.clone()).into());
                if let Some((id, _)) = &job {
                    n.send(Msg::Job { id: *id, event: JobEvent::Running });
                }
                let result = perform(n.hwnd(), op, sink.as_ref());
                // 使用者取消時 Shell 回傳的錯誤碼不一定（ERROR_CANCELLED、E_ABORT、COPYENGINE_E_USER_CANCELLED）
                let user_cancelled = job.as_ref().is_some_and(|(_, ctl)| ctl.cancel.load(Ordering::Relaxed));
                let cancelled = matches!(&result, Err(e) if user_cancelled || is_cancel_code(e.code()));
                let (error, select) = match result {
                    Ok(Some(name)) => (None, select.map(|s| Select { name, ..s })),
                    Ok(None) => (None, select),
                    Err(_) if cancelled => (None, None),
                    Err(e) => (Some(e.message()), None),
                };
                if let Some((id, _)) = &job {
                    n.send(Msg::Job { id: *id, event: JobEvent::Finished { error: error.clone(), cancelled } });
                }
                // 作業佇列介面會顯示錯誤，狀態列不再重複
                let error = if job.is_some() { None } else { error };
                n.send(Msg::FileOpDone { dirs, error, select });
            }
        })
        .expect("無法建立檔案作業執行緒");
    tx
}

// ---------------------------------------------------------------- 進度、暫停、取消

fn is_cancel_code(hr: HRESULT) -> bool {
    const E_ABORT: i32 = 0x80004004_u32 as i32;
    const COPYENGINE_E_USER_CANCELLED: i32 = 0x80270000_u32 as i32;
    matches!(hr.0, ERROR_CANCELLED_HR | E_ABORT | COPYENGINE_E_USER_CANCELLED)
}

/// 進度回呼：Shell 在執行作業的執行緒上同步呼叫。暫停時在回呼中等待，取消時回傳錯誤讓 Shell 中止作業
#[implement(IFileOperationProgressSink)]
struct Progress {
    id: JobId,
    ctl: Arc<JobCtl>,
    n: Notifier,
    state: RefCell<ProgressState>,
}

struct ProgressState {
    last_sent: Instant,
    fraction: f32,
    current: String,
    done: u32,
}

/// 進度訊息的最短間隔
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// 冒煙測試：每次進度回呼延遲幾毫秒，讓複製慢到可以在中途取消
#[cfg(feature = "smoke")]
pub static TEST_PROGRESS_DELAY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Progress {
    fn new(id: JobId, ctl: Arc<JobCtl>, n: Notifier) -> Self {
        let state = ProgressState { last_sent: Instant::now() - PROGRESS_INTERVAL, fraction: 0.0, current: String::new(), done: 0 };
        Self { id, ctl, n, state: RefCell::new(state) }
    }

    fn send(&self, event: JobEvent) {
        self.n.send(Msg::Job { id: self.id, event });
    }

    /// 暫停就等到繼續或取消；取消回傳 ERROR_CANCELLED
    fn gate(&self) -> Result<()> {
        if self.ctl.pause.load(Ordering::Relaxed) && !self.ctl.cancel.load(Ordering::Relaxed) {
            self.send(JobEvent::Paused);
            while self.ctl.pause.load(Ordering::Relaxed) && !self.ctl.cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(30));
            }
            self.send(JobEvent::Running);
        }
        if self.ctl.cancel.load(Ordering::Relaxed) {
            return Err(HRESULT(ERROR_CANCELLED_HR).into());
        }
        Ok(())
    }

    fn report(&self, force: bool) {
        let mut st = self.state.borrow_mut();
        if !force && st.last_sent.elapsed() < PROGRESS_INTERVAL {
            return;
        }
        st.last_sent = Instant::now();
        self.send(JobEvent::Progress { fraction: st.fraction, current: st.current.clone(), done: st.done });
    }

    fn begin_item(&self, item: Ref<IShellItem>) -> Result<()> {
        if let Some(name) = item.as_ref().and_then(display_name) {
            self.state.borrow_mut().current = name;
        }
        self.report(false);
        self.gate()
    }

    fn end_item(&self) -> Result<()> {
        self.state.borrow_mut().done += 1;
        self.report(false);
        Ok(())
    }
}

fn display_name(item: &IShellItem) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

impl IFileOperationProgressSink_Impl for Progress_Impl {
    fn StartOperations(&self) -> Result<()> {
        self.gate()
    }
    fn FinishOperations(&self, _hr: HRESULT) -> Result<()> {
        self.state.borrow_mut().fraction = 1.0;
        self.report(true);
        Ok(())
    }
    fn PreRenameItem(&self, _f: u32, _item: Ref<IShellItem>, _name: &PCWSTR) -> Result<()> {
        self.gate()
    }
    fn PostRenameItem(&self, _f: u32, _item: Ref<IShellItem>, _name: &PCWSTR, _hr: HRESULT, _new: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn PreMoveItem(&self, _f: u32, item: Ref<IShellItem>, _dest: Ref<IShellItem>, _name: &PCWSTR) -> Result<()> {
        self.begin_item(item)
    }
    fn PostMoveItem(&self, _f: u32, _item: Ref<IShellItem>, _dest: Ref<IShellItem>, _name: &PCWSTR, _hr: HRESULT, _new: Ref<IShellItem>) -> Result<()> {
        self.end_item()
    }
    fn PreCopyItem(&self, _f: u32, item: Ref<IShellItem>, _dest: Ref<IShellItem>, _name: &PCWSTR) -> Result<()> {
        self.begin_item(item)
    }
    fn PostCopyItem(&self, _f: u32, _item: Ref<IShellItem>, _dest: Ref<IShellItem>, _name: &PCWSTR, _hr: HRESULT, _new: Ref<IShellItem>) -> Result<()> {
        self.end_item()
    }
    fn PreDeleteItem(&self, _f: u32, item: Ref<IShellItem>) -> Result<()> {
        self.begin_item(item)
    }
    fn PostDeleteItem(&self, _f: u32, _item: Ref<IShellItem>, _hr: HRESULT, _new: Ref<IShellItem>) -> Result<()> {
        self.end_item()
    }
    fn PreNewItem(&self, _f: u32, _dest: Ref<IShellItem>, _name: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn PostNewItem(&self, _f: u32, _dest: Ref<IShellItem>, _name: &PCWSTR, _tpl: &PCWSTR, _attrs: u32, _hr: HRESULT, _new: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, total: u32, so_far: u32) -> Result<()> {
        #[cfg(feature = "smoke")]
        std::thread::sleep(Duration::from_millis(TEST_PROGRESS_DELAY_MS.load(Ordering::Relaxed)));
        if total > 0 {
            self.state.borrow_mut().fraction = (so_far as f32 / total as f32).clamp(0.0, 1.0);
        }
        self.report(false);
        self.gate()
    }
    fn ResetTimer(&self) -> Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> Result<()> {
        Ok(())
    }
}

fn affected(op: &FileOp) -> (Vec<String>, Option<Select>) {
    let parent = |p: &String| path::parent(p).unwrap_or_default();
    match op {
        FileOp::Transfer { sources, dest, mov } => {
            let mut dirs = vec![dest.clone()];
            if *mov {
                dirs.extend(sources.iter().map(parent));
            }
            (dirs, None)
        }
        FileOp::Delete { paths, .. } => (paths.iter().map(parent).collect(), None),
        FileOp::Rename { path, new_name } => {
            let dir = parent(path);
            (vec![dir.clone()], Some(Select { dir, name: new_name.clone(), rename: false }))
        }
        FileOp::NewFolder { dir } => {
            (vec![dir.clone()], Some(Select { dir: dir.clone(), name: String::new(), rename: true }))
        }
    }
}

/// 執行一個作業。新資料夾回傳實際建立的名稱。`sink` 為進度回呼（作業佇列的工作才有）
fn perform(hwnd: isize, op: FileOp, sink: Option<&IFileOperationProgressSink>) -> Result<Option<String>> {
    unsafe {
        let fo: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        fo.SetOwnerWindow(HWND(hwnd as _))?;
        // 有自己的進度介面時，不顯示 Shell 的進度視窗（衝突、確認對話框仍由 Shell 顯示）
        let silent = if sink.is_some() { FOF_SILENT } else { Default::default() };
        if let Some(s) = sink {
            fo.Advise(s)?;
        }
        let none_sink = None::<&IFileOperationProgressSink>;
        let mut created = None;
        match op {
            FileOp::Transfer { sources, dest: dest_path, mov } => {
                let same_dir = |s: &String| path::parent(s).is_some_and(|p| p.eq_ignore_ascii_case(&dest_path));
                // 移到原本的資料夾等於沒動作，直接略過
                let sources: Vec<&String> = sources.iter().filter(|s| !(mov && same_dir(s))).collect();
                if sources.is_empty() {
                    return Ok(None);
                }
                // 複製到同一個資料夾時自動改名（「名稱 - 複製」），與檔案總管相同
                let mut flags = FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR | silent;
                if !mov && sources.iter().any(|s| same_dir(s)) {
                    flags |= FOF_RENAMEONCOLLISION;
                }
                fo.SetOperationFlags(flags)?;
                let dest = shell_item(&dest_path)?;
                for s in sources {
                    let item = shell_item(s)?;
                    if mov {
                        fo.MoveItem(&item, &dest, PCWSTR::null(), none_sink)?;
                    } else {
                        fo.CopyItem(&item, &dest, PCWSTR::null(), none_sink)?;
                    }
                }
            }
            FileOp::Delete { paths, permanent } => {
                fo.SetOperationFlags(silent | if permanent {
                    FOF_WANTNUKEWARNING
                } else {
                    FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION
                })?;
                for p in &paths {
                    fo.DeleteItem(&shell_item(p)?, none_sink)?;
                }
            }
            FileOp::Rename { path, new_name } => {
                fo.SetOperationFlags(FOF_ALLOWUNDO)?;
                let name = wide_z(&new_name);
                fo.RenameItem(&shell_item(&path)?, PCWSTR(name.as_ptr()), none_sink)?;
            }
            FileOp::NewFolder { dir } => {
                fo.SetOperationFlags(FOF_ALLOWUNDO)?;
                let name = unique_name(&dir, "新增資料夾");
                let name_w = wide_z(&name);
                fo.NewItem(&shell_item(&dir)?, FILE_ATTRIBUTE_DIRECTORY.0, PCWSTR(name_w.as_ptr()), PCWSTR::null(), none_sink)?;
                created = Some(name);
            }
        }
        fo.PerformOperations()?;
        if fo.GetAnyOperationsAborted()?.as_bool() {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(ERROR_CANCELLED_HR)));
        }
        Ok(created)
    }
}

fn unique_name(dir: &str, base: &str) -> String {
    let exists = |n: &str| unsafe { GetFileAttributesW(PCWSTR(wide_z(&path::join(dir, n)).as_ptr())) != INVALID_FILE_ATTRIBUTES };
    if !exists(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} ({i})")).find(|n| !exists(n)).unwrap()
}

// ---------------------------------------------------------------- 剪貼簿

fn preferred_drop_effect_format() -> u16 {
    unsafe { RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect")) as u16 }
}

fn performed_drop_effect_format() -> u16 {
    unsafe { RegisterClipboardFormatW(windows::core::w!("Performed DropEffect")) as u16 }
}

fn logical_performed_drop_effect_format() -> u16 {
    unsafe { RegisterClipboardFormatW(windows::core::w!("Logical Performed DropEffect")) as u16 }
}

/// 在資料物件上設定一個 DWORD 格式（拖放效果的回報）
fn set_dword(obj: &IDataObject, cf: u16, value: u32) {
    unsafe {
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, 4) else { return };
        let p = GlobalLock(mem) as *mut u32;
        if !p.is_null() {
            *p = value;
            let _ = GlobalUnlock(mem);
        }
        let medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: mem },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        };
        let _ = obj.SetData(&hglobal_format(cf), &medium, true);
    }
}

/// 一般檔案系統路徑（不含資源回收筒內部項目，那需要 Shell 的還原邏輯）
fn is_plain_file(p: &str) -> bool {
    let b = p.as_bytes();
    let absolute = (b.len() >= 3 && b[1] == b':' && b[2] == b'\\') || p.starts_with(r"\\");
    absolute && !p.to_ascii_lowercase().contains(r"\$recycle.bin\")
}

fn hglobal_format(cf: u16) -> FORMATETC {
    FORMATETC { cfFormat: cf, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

/// 把檔案放上剪貼簿（與檔案總管互通）。`cut` 為剪下。
pub fn set_clipboard(paths: &[String], cut: bool) -> Result<()> {
    let obj = data_object(paths)?;
    unsafe {
        let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY | DROPEFFECT_LINK };
        let mem = GlobalAlloc(GMEM_MOVEABLE, 4)?;
        let p = GlobalLock(mem) as *mut u32;
        if !p.is_null() {
            *p = effect.0;
            let _ = GlobalUnlock(mem);
        }
        let medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: mem },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        };
        obj.SetData(&hglobal_format(preferred_drop_effect_format()), &medium, true)?;
        OleSetClipboard(&obj)
    }
}

pub fn clear_clipboard() {
    unsafe {
        let _ = OleSetClipboard(None);
    }
}

/// 讀取剪貼簿上的檔案清單與是否為剪下
pub fn read_clipboard() -> Option<(Vec<String>, bool)> {
    guard::io("讀取剪貼簿");
    unsafe {
        let obj = OleGetClipboard().ok()?;
        let paths = hdrop_paths(&obj)?;
        let cut = obj
            .GetData(&hglobal_format(preferred_drop_effect_format()))
            .ok()
            .map(|mut m| {
                let effect = read_u32(m.u.hGlobal);
                ReleaseStgMedium(&mut m);
                effect & DROPEFFECT_MOVE.0 != 0
            })
            .unwrap_or(false);
        Some((paths, cut))
    }
}

unsafe fn read_u32(mem: HGLOBAL) -> u32 {
    unsafe {
        let p = GlobalLock(mem) as *const u32;
        if p.is_null() {
            return 0;
        }
        let v = *p;
        let _ = GlobalUnlock(mem);
        v
    }
}

fn hdrop_paths(obj: &IDataObject) -> Option<Vec<String>> {
    unsafe {
        let mut medium = obj.GetData(&hglobal_format(CF_HDROP.0)).ok()?;
        let hdrop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(hdrop, i, Some(&mut buf));
            out.push(String::from_utf16_lossy(&buf[..len]));
        }
        ReleaseStgMedium(&mut medium);
        Some(out)
    }
}

// ---------------------------------------------------------------- 右鍵選單

thread_local! {
    static ACTIVE_MENU: RefCell<(Option<IContextMenu2>, Option<IContextMenu3>)> = const { RefCell::new((None, None)) };
}

/// 右鍵選單開啟期間，把「傳送到」「開啟檔案」等子選單的繪製訊息轉給 Shell。
pub fn handle_menu_msg(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
    ACTIVE_MENU.with(|m| {
        let m = m.borrow();
        if let Some(cm3) = &m.1 {
            let mut result = LRESULT(0);
            if unsafe { cm3.HandleMenuMsg2(msg, wparam, lparam, Some(&mut result)) }.is_ok() {
                return Some(result);
            }
        }
        if let Some(cm2) = &m.0
            && unsafe { cm2.HandleMenuMsg(msg, wparam, lparam) }.is_ok() {
                return Some(LRESULT(0));
            }
        None
    })
}

pub enum MenuResult {
    None,
    /// 使用者選了「重新命名」：由程式自己的就地編輯處理
    Rename,
    Invoked,
    /// 選了程式自己加入的項目（`extras` 中的 id）
    Extra(u32),
}

/// 程式自訂項目的 id 範圍，與 Shell 項目（1..0x7FFF）分開
pub const EXTRA_ID_BASE: u32 = 0x8000;

/// 顯示 Shell 右鍵選單。`items` 為空時顯示 `folder` 的背景選單（新增、貼上…）；
/// 兩者皆空時只顯示 `extras`。`extras` 放在選單最上方，以分隔線隔開。
///
/// 在 Shell 主機執行緒呼叫（見 shellhost.rs）：`hwnd` 是該執行緒的 message-only 視窗，
/// 作為 Shell 物件與叫用命令的擁有者。`before_show` 在選單出現前呼叫，回傳選單的擁有者視窗
/// （None 表示取消）；`after_close` 在選單關閉、叫用命令前呼叫。
#[allow(clippy::too_many_arguments)]
pub fn context_menu(
    hwnd: HWND,
    items: &[String],
    folder: &str,
    pt: POINT,
    extras: &[(u32, String)],
    extended: bool,
    ctrl: bool,
    before_show: &mut dyn FnMut() -> Option<HWND>,
    after_close: &dyn Fn(),
) -> Result<MenuResult> {
    unsafe {
        let mut _keep = Vec::new();
        let cm: Option<IContextMenu> = if items.is_empty() {
            if folder.is_empty() {
                None
            } else {
                let pidl = Pidl::parse(folder)?;
                let sf: IShellFolder = SHBindToObject(None, pidl.0, None)?;
                Some(sf.CreateViewObject(hwnd)?)
            }
        } else {
            let pidls = items.iter().map(|p| Pidl::parse(p)).collect::<Result<Vec<_>>>()?;
            let parent: IShellFolder = SHBindToParent(pidls[0].0, None)?;
            let children: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| ILFindLastID(p.0) as *const _).collect();
            let cm = parent.GetUIObjectOf(hwnd, &children, None)?;
            _keep = pidls;
            Some(cm)
        };
        if cm.is_none() && extras.is_empty() {
            return Ok(MenuResult::None);
        }

        let menu = CreatePopupMenu()?;
        for (id, label) in extras {
            let w = wide_z(label);
            AppendMenuW(menu, MF_STRING, *id as usize, PCWSTR(w.as_ptr()))?;
        }
        if let Some(cm) = &cm {
            if !extras.is_empty() {
                AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
            }
            let flags = CMF_NORMAL | if items.is_empty() { 0 } else { CMF_CANRENAME } | if extended { CMF_EXTENDEDVERBS } else { 0 };
            cm.QueryContextMenu(menu, extras.len() as u32 + 1, 1, EXTRA_ID_BASE - 1, flags).ok()?;
            ACTIVE_MENU.with(|m| *m.borrow_mut() = (cm.cast().ok(), cm.cast().ok()));
        }
        let Some(owner) = before_show() else {
            ACTIVE_MENU.with(|m| *m.borrow_mut() = (None, None));
            let _ = DestroyMenu(menu);
            return Ok(MenuResult::None);
        };
        let cmd = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, owner, None).0 as u32;
        ACTIVE_MENU.with(|m| *m.borrow_mut() = (None, None));
        after_close();

        let result = match &cm {
            _ if cmd == 0 => MenuResult::None,
            _ if cmd >= EXTRA_ID_BASE => MenuResult::Extra(cmd),
            None => MenuResult::None,
            Some(cm) if !items.is_empty() && verb(cm, cmd - 1).as_deref() == Some("rename") => MenuResult::Rename,
            Some(cm) => {
                let dir = wide_z(folder);
                let mut mask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
                if ctrl {
                    mask |= CMIC_MASK_CONTROL_DOWN;
                }
                if extended {
                    mask |= CMIC_MASK_SHIFT_DOWN;
                }
                let info = CMINVOKECOMMANDINFOEX {
                    cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                    fMask: mask,
                    hwnd,
                    lpVerb: PCSTR((cmd - 1) as usize as *const u8),
                    lpVerbW: PCWSTR((cmd - 1) as usize as *const u16),
                    lpDirectoryW: if folder.is_empty() { PCWSTR::null() } else { PCWSTR(dir.as_ptr()) },
                    nShow: SW_SHOWNORMAL.0,
                    ptInvoke: pt,
                    ..Default::default()
                };
                cm.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)?;
                MenuResult::Invoked
            }
        };
        let _ = DestroyMenu(menu);
        Ok(result)
    }
}

fn verb(cm: &IContextMenu, id: u32) -> Option<String> {
    let mut buf = [0u16; 64];
    unsafe {
        cm.GetCommandString(id as usize, GCS_VERBW, None, PSTR(buf.as_mut_ptr() as *mut u8), buf.len() as u32).ok()?;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]).to_ascii_lowercase())
}

pub fn properties(hwnd: HWND, paths: &[String]) {
    unsafe {
        if let [one] = paths {
            let w = wide_z(one);
            let _ = SHObjectProperties(Some(hwnd), SHOP_FILEPATH, PCWSTR(w.as_ptr()), PCWSTR::null());
        } else if let Ok(obj) = data_object(paths) {
            let _ = SHMultiFileProperties(&obj, 0);
        }
    }
}

// ---------------------------------------------------------------- 拖放

/// 從程式拖出檔案（模態，直到放開滑鼠）
pub fn drag_out(hwnd: HWND, paths: &[String]) -> Result<DROPEFFECT> {
    // COM 規定拖曳來源必須在視窗所屬的 UI 執行緒
    let _allow = guard::allow_io("拖曳來源受 COM 限制必須在 UI 執行緒");
    let obj = data_object(paths)?;
    unsafe { SHDoDragDrop(Some(hwnd), &obj, None::<&IDropSource>, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK) }
}

/// 解析過幾次放置目標（冒煙測試用來確認每次拖曳的快取）
pub static DROP_RESOLVES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 取得某資料夾在 Shell 中的放置目標，讓拖放行為與檔案總管完全一致
fn shell_drop_target(hwnd: HWND, folder: &str) -> Option<IDropTarget> {
    DROP_RESOLVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // 放置目標的回呼由 COM 在 UI 執行緒呼叫，無法移到背景；每次拖曳已快取
    let _allow = guard::allow_io("放置目標受 COM 限制必須在 UI 執行緒");
    unsafe {
        let pidl = Pidl::parse(folder).ok()?;
        let mut last = std::ptr::null_mut();
        let parent: IShellFolder = SHBindToParent(pidl.0, Some(&mut last)).ok()?;
        parent.GetUIObjectOf(hwnd, &[last as *const _], None).ok()
    }
}

/// 由視窗層提供：螢幕座標 → 放置目標資料夾；以及拖曳中的游標位置（None 表示離開）
pub type ResolveFn = fn(POINT) -> Option<String>;
pub type HoverFn = fn(Option<POINT>);
/// 放下檔案時交給背景檔案作業：(來源, 目標資料夾, 是否移動)
pub type TransferFn = fn(Vec<String>, String, bool);

#[implement(IDropTarget)]
pub struct DropTarget {
    hwnd: HWND,
    resolve: ResolveFn,
    hover: HoverFn,
    transfer: TransferFn,
    state: RefCell<DropState>,
}

#[derive(Default)]
struct DropState {
    data: Option<IDataObject>,
    folder: Option<String>,
    inner: Option<IDropTarget>,
    helper: Option<IDropTargetHelper>,
    /// 這次拖曳已解析過的放置目標：游標來回移動時不重複解析路徑
    cache: std::collections::HashMap<String, Option<IDropTarget>>,
    /// 右鍵拖曳：放下時由 Shell 顯示選單
    right_button: bool,
}

impl DropTarget {
    pub fn new(hwnd: HWND, resolve: ResolveFn, hover: HoverFn, transfer: TransferFn) -> Self {
        let helper = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
        Self { hwnd, resolve, hover, transfer, state: RefCell::new(DropState { helper, ..Default::default() }) }
    }

    /// 游標移到不同資料夾時，切換轉發對象
    fn retarget(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let p = POINT { x: pt.x, y: pt.y };
        let folder = (self.resolve)(p);
        let mut st = self.state.borrow_mut();
        if folder != st.folder {
            if let Some(inner) = st.inner.take() {
                unsafe {
                    let _ = inner.DragLeave();
                }
            }
            st.folder = folder.clone();
            st.inner = folder.and_then(|f| {
                let hwnd = self.hwnd;
                st.cache.entry(f).or_insert_with_key(|f| shell_drop_target(hwnd, f)).clone()
            });
            if let (Some(inner), Some(data)) = (&st.inner, &st.data) {
                unsafe { inner.DragEnter(data, keys, *pt, effect)? };
            } else {
                unsafe { *effect = DROPEFFECT_NONE };
            }
        } else if let Some(inner) = &st.inner {
            unsafe { inner.DragOver(keys, *pt, effect)? };
        } else {
            unsafe { *effect = DROPEFFECT_NONE };
        }
        Ok(())
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        {
            let mut st = self.state.borrow_mut();
            st.data = data.cloned();
            st.folder = None;
            st.inner = None;
            st.cache.clear();
            st.right_button = keys.0 & MK_RBUTTON.0 != 0;
            if let (Some(h), Some(d)) = (&st.helper, &st.data) {
                unsafe {
                    let _ = h.DragEnter(self.hwnd, d, &POINT { x: pt.x, y: pt.y }, *effect);
                }
            }
        }
        self.retarget(keys, pt, effect)?;
        (self.hover)(Some(POINT { x: pt.x, y: pt.y }));
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.retarget(keys, pt, effect)?;
        if let Some(h) = &self.state.borrow().helper {
            unsafe {
                let _ = h.DragOver(&POINT { x: pt.x, y: pt.y }, *effect);
            }
        }
        (self.hover)(Some(POINT { x: pt.x, y: pt.y }));
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        let mut st = self.state.borrow_mut();
        if let Some(inner) = st.inner.take() {
            unsafe {
                let _ = inner.DragLeave();
            }
        }
        if let Some(h) = &st.helper {
            unsafe {
                let _ = h.DragLeave();
            }
        }
        st.data = None;
        st.folder = None;
        st.cache.clear();
        drop(st);
        (self.hover)(None);
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.retarget(keys, pt, effect)?;
        let mut st = self.state.borrow_mut();
        if let Some(h) = &st.helper {
            unsafe {
                let _ = h.Drop(data.as_ref(), &POINT { x: pt.x, y: pt.y }, *effect);
            }
        }
        let inner = st.inner.take();
        let folder = st.folder.take();
        let right_button = st.right_button;
        st.data = None;
        st.cache.clear();
        drop(st);
        (self.hover)(None);
        match (inner, data.as_ref()) {
            (Some(inner), Some(d)) => {
                // 一般檔案的複製／移動交給背景檔案作業：Shell 的 Drop 會在 UI 執行緒同步複製（約 120–190 ms）。
                // 效果沿用 Shell 在最後一次 DragOver 決定的（修飾鍵、同磁碟移動／跨磁碟複製，與檔案總管相同）。
                let chosen = unsafe { *effect };
                let transfer = (chosen == DROPEFFECT_COPY || chosen == DROPEFFECT_MOVE) && !right_button;
                if let Some(dest) = folder.filter(|_| transfer)
                    && let Some(paths) = hdrop_paths(d).filter(|p| !p.is_empty() && p.iter().all(|p| is_plain_file(p)))
                {
                    unsafe {
                        let _ = inner.DragLeave();
                    }
                    let mov = chosen == DROPEFFECT_MOVE;
                    if mov {
                        // 最佳化移動：已由目標搬移，告知來源不要再刪除原檔
                        set_dword(d, performed_drop_effect_format(), DROPEFFECT_NONE.0);
                        set_dword(d, logical_performed_drop_effect_format(), DROPEFFECT_MOVE.0);
                        unsafe { *effect = DROPEFFECT_NONE };
                    }
                    (self.transfer)(paths, dest, mov);
                    return Ok(());
                }
                // 其他情況（右鍵選單、建立捷徑、虛擬檔案、資源回收筒）仍交給 Shell，同步執行，COM 規定必須在 UI 執行緒
                crate::window::exempt_stall();
                unsafe { inner.Drop(d, keys, *pt, effect) }
            }
            _ => {
                unsafe { *effect = DROPEFFECT_NONE };
                Ok(())
            }
        }
    }
}
