//! 資料夾大小：遞迴加總底下所有檔案的大小（邏輯大小）。
//!
//! 在背景執行緒呼叫。不進入資料夾連結（junction、符號連結），避免迴圈與重複計算；
//! 雲端預留位置檔案（OneDrive 等）以邏輯大小計入。無法存取的子資料夾略過並計數。
//! 每 256 筆檢查一次取消旗標；途中定期回報目前的累計值。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Storage::FileSystem::{
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    WIN32_FIND_DATAW,
};
use windows::core::PCWSTR;

use crate::listing::ATTR_DIRECTORY;
use crate::path;

const ATTR_REPARSE_POINT: u32 = 0x400;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirSize {
    pub bytes: u64,
    pub files: u64,
    /// 子資料夾數（不含自己）
    pub dirs: u64,
    /// 無法列舉的資料夾數（沒有權限等）；大於 0 時總和是下限
    pub denied: u32,
}

/// 計算 `root` 的大小；取消時回傳 `None`。`progress` 約每 `interval` 收到一次目前的累計值。
pub fn dir_size(root: &str, cancel: &AtomicBool, interval: Duration, mut progress: impl FnMut(&DirSize)) -> Option<DirSize> {
    crate::guard::io("計算資料夾大小");
    let mut total = DirSize::default();
    let mut stack = vec![path::long_form(root)];
    let mut last = Instant::now();
    let mut seen = 0u32;
    let mut data = WIN32_FIND_DATAW::default();
    while let Some(dir) = stack.pop() {
        let pattern = path::to_wide_z(&path::join(&dir, "*"));
        let handle = unsafe {
            FindFirstFileExW(
                PCWSTR(pattern.as_ptr()),
                FindExInfoBasic,
                &mut data as *mut _ as *mut _,
                FindExSearchNameMatch,
                None,
                FIND_FIRST_EX_LARGE_FETCH,
            )
        };
        let Ok(handle) = handle else {
            total.denied += 1;
            continue;
        };
        loop {
            let len = data.cFileName.iter().position(|&c| c == 0).unwrap_or(data.cFileName.len());
            let name = &data.cFileName[..len];
            let dot = name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16];
            if !dot {
                let attrs = data.dwFileAttributes;
                if attrs & ATTR_DIRECTORY != 0 {
                    // 資料夾連結不進入（可能指回上層造成迴圈，或指向別處而重複計算）
                    if attrs & ATTR_REPARSE_POINT == 0 {
                        total.dirs += 1;
                        stack.push(path::join(&dir, &String::from_utf16_lossy(name)));
                    }
                } else {
                    total.files += 1;
                    total.bytes += ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64;
                }
            }
            seen += 1;
            if seen.is_multiple_of(256) {
                if cancel.load(Ordering::Relaxed) {
                    unsafe {
                        let _ = FindClose(handle);
                    }
                    return None;
                }
                if last.elapsed() >= interval {
                    last = Instant::now();
                    progress(&total);
                }
            }
            if unsafe { FindNextFileW(handle, &mut data) }.is_err() {
                break;
            }
        }
        unsafe {
            let _ = FindClose(handle);
        }
    }
    if cancel.load(Ordering::Relaxed) { None } else { Some(total) }
}

// ---------------------------------------------------------------- 快取檔格式

/// 快取的一筆：完整路徑、大小、計算時間（Unix 秒）
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheLine {
    pub path: String,
    pub size: DirSize,
    pub when: u64,
}

/// 以 Tab 分隔的一行：`路徑\t位元組\t檔案數\t資料夾數\t無法存取數\t時間`（路徑不會含 Tab 與換行）
pub fn to_line(c: &CacheLine) -> String {
    format!("{}\t{}\t{}\t{}\t{}\t{}", c.path, c.size.bytes, c.size.files, c.size.dirs, c.size.denied, c.when)
}

pub fn parse_line(line: &str) -> Option<CacheLine> {
    let mut f = line.split('\t');
    let path = f.next().filter(|p| !p.is_empty())?.to_string();
    let mut num = || f.next()?.parse::<u64>().ok();
    let size = DirSize { bytes: num()?, files: num()?, dirs: num()?, denied: num()? as u32 };
    let when = num()?;
    Some(CacheLine { path, size, when })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("zing-dirsize-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sums_nested_files() {
        let d = temp("nested");
        std::fs::write(d.join("a.bin"), vec![0u8; 1000]).unwrap();
        std::fs::create_dir_all(d.join("sub").join("deep")).unwrap();
        std::fs::write(d.join("sub").join("b.bin"), vec![0u8; 234]).unwrap();
        std::fs::write(d.join("sub").join("deep").join("c.bin"), vec![0u8; 66]).unwrap();
        std::fs::create_dir_all(d.join("empty")).unwrap();
        let cancel = AtomicBool::new(false);
        let s = dir_size(d.to_str().unwrap(), &cancel, Duration::from_secs(60), |_| {}).unwrap();
        assert_eq!(s, DirSize { bytes: 1300, files: 3, dirs: 3, denied: 0 });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn does_not_follow_junctions() {
        let d = temp("junction");
        std::fs::write(d.join("a.bin"), vec![0u8; 10]).unwrap();
        // 指回自己的 junction：若跟進會無限遞迴；建立 junction 不需要系統管理員權限
        let link = d.join("loop");
        let ok = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&d)
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(ok, "無法建立 junction");
        let cancel = AtomicBool::new(false);
        let s = dir_size(d.to_str().unwrap(), &cancel, Duration::from_secs(60), |_| {}).unwrap();
        assert_eq!((s.bytes, s.files, s.dirs), (10, 1, 0));
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cancel_and_missing() {
        let d = temp("cancel");
        for i in 0..600 {
            std::fs::write(d.join(format!("{i}.txt")), b"x").unwrap();
        }
        let cancel = AtomicBool::new(true);
        assert_eq!(dir_size(d.to_str().unwrap(), &cancel, Duration::from_secs(60), |_| {}), None);
        let cancel = AtomicBool::new(false);
        let missing = dir_size(d.join("nope").to_str().unwrap(), &cancel, Duration::from_secs(60), |_| {}).unwrap();
        assert_eq!(missing.denied, 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cache_line_roundtrip() {
        let c = CacheLine {
            path: r"C:\Users\測試 資料夾".into(),
            size: DirSize { bytes: 1 << 40, files: 12, dirs: 3, denied: 1 },
            when: 1_791_800_000,
        };
        assert_eq!(parse_line(&to_line(&c)), Some(c));
        assert_eq!(parse_line("broken\t1\t2"), None);
        assert_eq!(parse_line(""), None);
    }
}
