//! 顯示用格式化：檔案大小、修改時間、類型。

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

/// 類檔案總管：以 KB 為單位、無條件進位、千分位，例如 `1,234 KB`。
pub fn size_kb(bytes: u64) -> String {
    let kb = bytes.div_ceil(1024);
    let digits = kb.to_string();
    let mut out = String::with_capacity(digits.len() + 4);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out.push_str(" KB");
    out
}

/// 精簡格式，例如 `1.5 MB`，用於狀態列。
pub fn size_short(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

/// FILETIME（UTC）轉本地時間 `2026-10-13 09:32`。0 代表未知，回傳空字串。
pub fn filetime_local(ft: u64) -> String {
    if ft == 0 {
        return String::new();
    }
    let ft = FILETIME { dwLowDateTime: ft as u32, dwHighDateTime: (ft >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err() {
            return String::new();
        }
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute)
}

/// Unix 時間（秒）的本地日期與時間（年, 月, 日, 時, 分）
fn unix_local(secs: u64) -> Option<(u16, u16, u16, u16, u16)> {
    // FILETIME 從 1601 年起算，單位 100 ns
    let ft = (secs + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ft as u32, dwHighDateTime: (ft >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some((local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute))
}

/// 相對的日期＋24 小時制（`今天 14:31`、`昨天 09:05`、`2026-11-20 18:00`）；`then`、`now` 是 Unix 時間（秒）
pub fn relative_local(then: u64, now: u64) -> String {
    let (Some((y, m, d, hh, mm)), Some(today), Some(yesterday)) = (unix_local(then), unix_local(now), unix_local(now.saturating_sub(86_400)))
    else {
        return String::new();
    };
    let time = format!("{hh:02}:{mm:02}");
    if (y, m, d) == (today.0, today.1, today.2) {
        tr!("今天 {time}", time)
    } else if (y, m, d) == (yesterday.0, yesterday.1, yesterday.2) {
        tr!("昨天 {time}", time)
    } else {
        format!("{y:04}-{m:02}-{d:02} {time}")
    }
}

/// 類型欄文字：資料夾 →「資料夾」，其餘為大寫副檔名 +「 檔案」。
pub fn type_label(is_dir: bool, ext: &[u16]) -> String {
    if is_dir {
        return tr!("資料夾").into();
    }
    if ext.is_empty() {
        return tr!("檔案").into();
    }
    tr!("{ext} 檔案", ext = String::from_utf16_lossy(ext).to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size_kb(0), "0 KB");
        assert_eq!(size_kb(1), "1 KB");
        assert_eq!(size_kb(1024 * 1234), "1,234 KB");
        assert_eq!(size_kb(1024 * 1_234_567), "1,234,567 KB");
        assert_eq!(size_short(512), "512 B");
        assert_eq!(size_short(1536), "1.5 KB");
    }

    #[test]
    fn dates() {
        assert_eq!(filetime_local(0), "");
        // 2026-01-01 00:00 UTC
        let s = filetime_local(134_116_992_000_000_000);
        assert!(s.starts_with("2026-01-01") || s.starts_with("2025-12-31"), "{s}");
    }

    #[test]
    fn relative_dates() {
        // 2026-06-15 12:00 UTC：時區最多差 14 小時，本地仍是 6 月 15 日或前後一天的白天
        let noon = 1_781_524_800;
        let today = relative_local(noon, noon);
        assert!(today.starts_with("今天 ") || today.starts_with("Today "), "{today}");
        let yesterday = relative_local(noon - 86_400, noon);
        assert!(yesterday.starts_with("昨天 ") || yesterday.starts_with("Yesterday "), "{yesterday}");
        let older = relative_local(noon - 10 * 86_400, noon);
        assert!(older.starts_with("2026-06-0"), "{older}");
        assert_eq!(today.len() - today.trim_end_matches(char::is_numeric).len(), 2, "24 小時制的分：{today}");
    }
}
