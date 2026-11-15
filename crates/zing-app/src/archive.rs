//! 壓縮檔當資料夾瀏覽（規則與 zip 的讀取在 `zing_core::archive`）：列出壓縮檔裡的資料夾、唯讀的限制。
//!
//! zip 讀中央目錄（快取最近一個），其他格式經 Shell 的命名空間（Windows 內建的壓縮資料夾）列舉。開啟、複製、拖出
//! 裡面的項目交給 Shell（`SHParseDisplayName` 認得 `專案.zip\src\a.rs` 這種路徑）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2, SHCreateItemFromParsingName, SIGDN_PARENTRELATIVEPARSING,
};
use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_STREAM};
use windows::core::{GUID, Interface, PCWSTR};
use zing_core::archive::{ZipIndex, is_zip, splits};
use zing_core::enumerate::{EnumError, enumerate};
use zing_core::listing::{ATTR_DIRECTORY, Listing};
use zing_core::text::wide_z;

use crate::shell::FileOp;

/// 列出資料夾；壓縮檔裡的路徑改讀壓縮檔（在背景執行緒呼叫）
pub fn list_folder(path: &str, dirs_only: bool, cancel: &AtomicBool, progress: impl FnMut(&Listing)) -> Result<Listing, EnumError> {
    for (arc, inner) in splits(path) {
        zing_core::guard::io("確認壓縮檔");
        // 名稱像壓縮檔的資料夾照一般資料夾列舉
        if !std::path::Path::new(&zing_core::path::long_form(arc)).is_file() {
            continue;
        }
        return if is_zip(arc) { zip_index(arc)?.list(inner, dirs_only, cancel) } else { shell_list(path, dirs_only, cancel) };
    }
    enumerate(path, dirs_only, cancel, progress)
}

/// 最近讀過的 zip 目錄（在壓縮檔裡換資料夾不必重讀）：以路徑、大小、修改時間判斷是否過時
static ZIP_CACHE: Mutex<Option<CachedZip>> = Mutex::new(None);

/// (壓縮檔的路徑, 大小, 修改時間, 目錄)
type CachedZip = (String, u64, std::time::SystemTime, Arc<ZipIndex>);

fn zip_index(arc: &str) -> Result<Arc<ZipIndex>, EnumError> {
    let meta = std::fs::metadata(zing_core::path::long_form(arc)).map_err(|_| EnumError::NotFound)?;
    let stamp = (meta.len(), meta.modified().unwrap_or(std::time::UNIX_EPOCH));
    if let Ok(c) = ZIP_CACHE.lock()
        && let Some((p, len, at, idx)) = c.as_ref()
        && p.eq_ignore_ascii_case(arc)
        && (*len, *at) == stamp
    {
        return Ok(idx.clone());
    }
    let idx = Arc::new(ZipIndex::read(arc)?);
    if let Ok(mut c) = ZIP_CACHE.lock() {
        *c = Some((arc.to_string(), stamp.0, stamp.1, idx.clone()));
    }
    Ok(idx)
}

/// System.Size、System.DateModified
const PKEY_SIZE: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac), pid: 12 };
const PKEY_DATE_MODIFIED: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac), pid: 14 };

/// 經 Shell 的命名空間列舉（7z、rar、tar…）
fn shell_list(path: &str, dirs_only: bool, cancel: &AtomicBool) -> Result<Listing, EnumError> {
    zing_core::guard::io("經 Shell 列出壓縮檔");
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let result = unsafe { shell_list_inner(path, dirs_only, cancel) };
    if init.is_ok() {
        unsafe { CoUninitialize() };
    }
    result
}

unsafe fn shell_list_inner(path: &str, dirs_only: bool, cancel: &AtomicBool) -> Result<Listing, EnumError> {
    let w = wide_z(path);
    let folder: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.map_err(|_| EnumError::NotFound)?;
    let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.map_err(|e| EnumError::Other(e.message()))?;
    let mut out = Listing::default();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(EnumError::Cancelled);
        }
        let mut next = [None];
        let mut fetched = 0;
        if unsafe { items.Next(&mut next, Some(&mut fetched)) }.is_err() || fetched == 0 {
            break;
        }
        let Some(item) = next[0].take() else { break };
        // 壓縮檔裡的壓縮檔在 Shell 也是資料夾（同時是資料流），當成檔案
        let attrs = unsafe { item.GetAttributes(SFGAO_FOLDER | SFGAO_STREAM) }.unwrap_or_default();
        let dir = attrs.0 & SFGAO_FOLDER.0 != 0 && attrs.0 & SFGAO_STREAM.0 == 0;
        if dirs_only && !dir {
            continue;
        }
        let Ok(name) = (unsafe { item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING) }) else { continue };
        let text = unsafe { name.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(name.0 as *const _)) };
        let item2 = item.cast::<IShellItem2>().ok();
        let size = if dir { 0 } else { item2.as_ref().and_then(|i| unsafe { i.GetUInt64(&PKEY_SIZE) }.ok()).unwrap_or(0) };
        let modified = item2
            .as_ref()
            .and_then(|i| unsafe { i.GetFileTime(&PKEY_DATE_MODIFIED) }.ok())
            .map_or(0, |f| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64);
        out.push_str(&text, if dir { ATTR_DIRECTORY } else { 0 }, size, modified);
    }
    Ok(out)
}

/// 作業會改變壓縮檔的內容（放進、刪除、改名、移出）：壓縮檔是唯讀的
pub fn op_writes_archive(op: &FileOp) -> bool {
    let inside = |p: &String| zing_core::archive::is_inside(p) && !is_archive_itself(p);
    match op {
        FileOp::Transfer { sources, dest, mov } => inside(dest) || (*mov && sources.iter().any(inside)),
        FileOp::Delete { paths, .. } => paths.iter().any(inside),
        FileOp::Rename { path, .. } => inside(path),
        FileOp::NewFolder { dir } | FileOp::NewFile { dir } | FileOp::BatchRename { dir, .. } => zing_core::archive::is_inside(dir),
        FileOp::Restore { .. } => false,
    }
}

/// 路徑是壓縮檔本身（`D:\a.zip`）：對它的刪除、改名、移動是一般的檔案作業
fn is_archive_itself(p: &str) -> bool {
    splits(p).next().is_some_and(|(_, inner)| inner.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changes_inside_an_archive_are_refused() {
        let t = |sources: &[&str], dest: &str, mov| FileOp::Transfer { sources: sources.iter().map(|s| s.to_string()).collect(), dest: dest.into(), mov };
        assert!(!op_writes_archive(&t(&[r"D:\a.zip\x.txt"], r"D:\out", false)), "複製出來可以");
        assert!(op_writes_archive(&t(&[r"D:\a.zip\x.txt"], r"D:\out", true)), "移出來會刪除壓縮檔裡的項目");
        assert!(op_writes_archive(&t(&[r"D:\x.txt"], r"D:\a.zip\src", false)));
        assert!(!op_writes_archive(&t(&[r"D:\a.zip"], r"D:\out", true)), "移動壓縮檔本身可以");
        assert!(op_writes_archive(&FileOp::Delete { paths: vec![r"D:\a.zip\x".into()], permanent: false }));
        assert!(!op_writes_archive(&FileOp::Delete { paths: vec![r"D:\a.zip".into()], permanent: false }));
        assert!(op_writes_archive(&FileOp::Rename { path: r"D:\a.7z\x".into(), new_name: "y".into() }));
        assert!(op_writes_archive(&FileOp::NewFolder { dir: r"D:\a.zip".into() }));
        assert!(!op_writes_archive(&FileOp::NewFolder { dir: r"D:\資料".into() }));
    }
}
