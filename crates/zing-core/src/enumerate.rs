//! 資料夾列舉：`FindFirstFileExW` + `FindExInfoBasic` + `FIND_FIRST_EX_LARGE_FETCH`。
//!
//! 在背景執行緒呼叫。每列舉一段就回報一次目前結果，讓 UI 能先顯示首屏；
//! 每 256 筆檢查一次取消旗標，換資料夾時可立即中止。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND};
use windows::Win32::Storage::FileSystem::{
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchLimitToDirectories, FindExSearchNameMatch,
    FindFirstFileExW, FindNextFileW, GetLogicalDriveStringsW, WIN32_FIND_DATAW,
};
use windows::core::PCWSTR;

use crate::listing::{ATTR_DIRECTORY, Listing};
use crate::path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumError {
    Cancelled,
    NotFound,
    AccessDenied,
    Other(String),
}

impl std::fmt::Display for EnumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("已取消"),
            Self::NotFound => f.write_str("找不到這個資料夾"),
            Self::AccessDenied => f.write_str("沒有存取權限"),
            Self::Other(m) => f.write_str(m),
        }
    }
}

/// 第一次回報的時機：滿這麼多筆或經過這麼久（取先到者）
const FIRST_BATCH: usize = 1000;
const FIRST_DELAY: Duration = Duration::from_millis(30);
const NEXT_DELAY: Duration = Duration::from_millis(250);

/// 列舉 `dir`。`dir` 為空字串時列出磁碟機（「此電腦」）。
///
/// `progress` 會在列舉途中收到目前為止的結果（不保證有呼叫）；最終結果由回傳值提供。
pub fn enumerate(
    dir: &str,
    dirs_only: bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Listing),
) -> Result<Listing, EnumError> {
    if dir.is_empty() {
        return Ok(drives());
    }
    crate::guard::io("列舉資料夾");
    let pattern = path::to_wide_z(&path::join(&path::long_form(dir), "*"));
    let mut data = WIN32_FIND_DATAW::default();
    let op = if dirs_only { FindExSearchLimitToDirectories } else { FindExSearchNameMatch };
    let handle = unsafe {
        FindFirstFileExW(
            PCWSTR(pattern.as_ptr()),
            FindExInfoBasic,
            &mut data as *mut _ as *mut _,
            op,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    let handle = match handle {
        Ok(h) => h,
        Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => return Ok(Listing::default()),
        Err(e) => return Err(map_err(e)),
    };

    let mut listing = Listing::with_capacity(256);
    let start = Instant::now();
    let mut last_report: Option<Instant> = None;
    let result = loop {
        push(&mut listing, &data, dirs_only);
        if listing.len().is_multiple_of(256) {
            if cancel.load(Ordering::Relaxed) {
                break Err(EnumError::Cancelled);
            }
            let now = Instant::now();
            let due = match last_report {
                None => listing.len() >= FIRST_BATCH || now - start >= FIRST_DELAY,
                Some(t) => now - t >= NEXT_DELAY,
            };
            if due {
                progress(&listing);
                last_report = Some(now);
            }
        }
        if let Err(e) = unsafe { FindNextFileW(handle, &mut data) } {
            if e.code() == ERROR_NO_MORE_FILES.to_hresult() {
                break Ok(());
            }
            break Err(map_err(e));
        }
    };
    unsafe {
        let _ = FindClose(handle);
    }
    result.map(|()| listing)
}

#[inline]
fn push(listing: &mut Listing, d: &WIN32_FIND_DATAW, dirs_only: bool) {
    let len = d.cFileName.iter().position(|&c| c == 0).unwrap_or(d.cFileName.len());
    let name = &d.cFileName[..len];
    // 略過 "." 與 ".."
    if name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16] {
        return;
    }
    let attrs = d.dwFileAttributes;
    if dirs_only && attrs & ATTR_DIRECTORY == 0 {
        return;
    }
    let size = ((d.nFileSizeHigh as u64) << 32) | d.nFileSizeLow as u64;
    let modified = ((d.ftLastWriteTime.dwHighDateTime as u64) << 32) | d.ftLastWriteTime.dwLowDateTime as u64;
    listing.push(name, attrs, size, modified);
}

fn map_err(e: windows::core::Error) -> EnumError {
    let code = e.code();
    if code == ERROR_PATH_NOT_FOUND.to_hresult() {
        EnumError::NotFound
    } else if code == ERROR_ACCESS_DENIED.to_hresult() {
        EnumError::AccessDenied
    } else {
        EnumError::Other(e.message())
    }
}

/// 列出磁碟機根目錄，例如 `C:\`。
pub fn drives() -> Listing {
    let mut buf = [0u16; 512];
    let n = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    let mut listing = Listing::default();
    for d in buf[..n.min(buf.len())].split(|&c| c == 0).filter(|s| !s.is_empty()) {
        listing.push(d, ATTR_DIRECTORY, 0, 0);
    }
    listing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_temp_dir() {
        let dir = std::env::temp_dir().join(format!("zing-enum-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for i in 0..1500 {
            std::fs::write(dir.join(format!("f{i}.txt")), b"x").unwrap();
        }
        let cancel = AtomicBool::new(false);
        let mut reports = 0;
        let l = enumerate(dir.to_str().unwrap(), false, &cancel, |_| reports += 1).unwrap();
        assert_eq!(l.len(), 1501);
        assert!(reports >= 1);
        let d = enumerate(dir.to_str().unwrap(), true, &cancel, |_| {}).unwrap();
        assert_eq!(d.len(), 1);
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(enumerate(dir.to_str().unwrap(), false, &cancel, |_| {}).unwrap_err(), EnumError::Cancelled);
        assert_eq!(
            enumerate(dir.join("nope").to_str().unwrap(), false, &AtomicBool::new(false), |_| {}).unwrap_err(),
            EnumError::NotFound
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!drives().is_empty());
    }
}
