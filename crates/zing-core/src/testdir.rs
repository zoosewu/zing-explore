//! 測試用的暫存資料夾：環境變數 `ZING_TEST_DIR`，沒有設定時用系統的暫存資料夾。
//! 單元測試、冒煙測試、效能測試（`tools/*.ps1` 也讀同一個變數）與隔離模式的快取都放在這裡。

use std::path::PathBuf;

pub fn dir() -> PathBuf {
    std::env::var_os("ZING_TEST_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| std::fs::create_dir_all(p).is_ok())
        .unwrap_or_else(std::env::temp_dir)
}
