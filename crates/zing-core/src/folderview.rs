//! 每個資料夾記住自己的檢視設定：排序、檢視模式、縮圖大小。
//!
//! 以資料夾路徑（不分大小寫）為鍵；與預設相同的設定不保存。最多保留 `MAX` 個最近使用的資料夾，
//! 存成 TSV（`%LOCALAPPDATA%\zing-explore\folders.tsv`，可重建的資料）。

use std::collections::HashMap;

use crate::sort::{SortKey, SortSpec};

/// 最多保留的資料夾數（較久沒用的捨棄）
pub const MAX: usize = 5000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    List,
    Thumbnails,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderView {
    pub sort: SortSpec,
    pub mode: ViewMode,
    /// 縮圖邊長（px）
    pub thumb: u16,
}

impl Default for FolderView {
    fn default() -> Self {
        FolderView { sort: SortSpec::default(), mode: ViewMode::List, thumb: 96 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FolderViews {
    /// 路徑（小寫）→ (設定, 最後使用的時間，Unix 秒)
    map: HashMap<String, (FolderView, u64)>,
}

fn key_name(k: SortKey) -> &'static str {
    match k {
        SortKey::Name => "name",
        SortKey::Modified => "modified",
        SortKey::Type => "type",
        SortKey::Size => "size",
    }
}

impl FolderViews {
    /// 這個資料夾的設定；沒有記錄時為預設
    pub fn get(&self, path: &str) -> FolderView {
        self.map.get(&path.to_lowercase()).map(|e| e.0).unwrap_or_default()
    }

    /// 記下設定；與預設相同時移除記錄。超過上限時捨棄最久沒用的。
    pub fn set(&mut self, path: &str, view: FolderView, now: u64) {
        let k = path.to_lowercase();
        if view == FolderView::default() {
            self.map.remove(&k);
            return;
        }
        self.map.insert(k, (view, now));
        if self.map.len() > MAX {
            let mut whens: Vec<u64> = self.map.values().map(|e| e.1).collect();
            let cut = whens.len() - MAX;
            let (_, &mut nth, _) = whens.select_nth_unstable(cut - 1);
            // 同一秒的可能多刪幾筆，無妨
            self.map.retain(|_, e| e.1 > nth);
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 合併從檔案讀出的記錄：這次執行已經設定過的資料夾不覆蓋
    pub fn merge_loaded(&mut self, loaded: FolderViews) {
        for (k, v) in loaded.map {
            self.map.entry(k).or_insert(v);
        }
    }

    /// 每行：`路徑\t排序欄\t遞增(1/0)\t檢視模式\t縮圖大小\t時間`（路徑不會含 Tab 與換行）
    pub fn to_text(&self) -> String {
        let mut rows: Vec<_> = self.map.iter().collect();
        rows.sort_unstable_by_key(|(_, e)| std::cmp::Reverse(e.1));
        let mut s = String::with_capacity(rows.len() * 64);
        for (path, (v, when)) in rows {
            let mode = if v.mode == ViewMode::Thumbnails { "thumbnails" } else { "list" };
            s.push_str(&format!(
                "{path}\t{}\t{}\t{mode}\t{}\t{when}\n",
                key_name(v.sort.key),
                v.sort.ascending as u8,
                v.thumb
            ));
        }
        s
    }

    /// 解析檔案內容；格式不對的行略過
    pub fn from_text(text: &str) -> Self {
        let mut out = FolderViews::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let [path, key, asc, mode, thumb, when] = f[..] else { continue };
            let (Some(key), Ok(thumb), Ok(when)) = (SortKey::parse(key), thumb.parse::<u16>(), when.parse::<u64>()) else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            let mode = if mode == "thumbnails" { ViewMode::Thumbnails } else { ViewMode::List };
            let view = FolderView { sort: SortSpec { key, ascending: asc == "1" }, mode, thumb: thumb.clamp(32, 512) };
            out.map.insert(path.to_lowercase(), (view, when));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by(key: SortKey, ascending: bool) -> FolderView {
        FolderView { sort: SortSpec { key, ascending }, ..Default::default() }
    }

    #[test]
    fn get_set_case_insensitive_and_default_removed() {
        let mut v = FolderViews::default();
        assert_eq!(v.get(r"C:\a"), FolderView::default());
        v.set(r"C:\Photos", by(SortKey::Modified, false), 10);
        assert_eq!(v.get(r"c:\photos"), by(SortKey::Modified, false));
        // 改回預設就不保存
        v.set(r"C:\PHOTOS", FolderView::default(), 11);
        assert!(v.is_empty());
    }

    #[test]
    fn roundtrip_and_bad_lines() {
        let mut v = FolderViews::default();
        v.set(r"C:\相片 2026", FolderView { mode: ViewMode::Thumbnails, thumb: 160, ..by(SortKey::Size, false) }, 5);
        v.set(r"D:\x", by(SortKey::Type, true), 7);
        let text = v.to_text();
        assert!(text.starts_with(r"d:\x"), "新的在前：{text}");
        let w = FolderViews::from_text(&format!("{text}壞掉的行\nC:\\y\tbogus\t1\tlist\t96\t1\n\n"));
        assert_eq!(w.len(), 2);
        assert_eq!(w.get(r"C:\相片 2026"), FolderView { mode: ViewMode::Thumbnails, thumb: 160, ..by(SortKey::Size, false) });
        assert_eq!(w.get(r"d:\X"), by(SortKey::Type, true));
    }

    #[test]
    fn keeps_most_recent() {
        let mut v = FolderViews::default();
        for i in 0..MAX as u64 + 10 {
            v.set(&format!(r"C:\d{i}"), by(SortKey::Size, true), i);
        }
        assert!(v.len() <= MAX);
        assert_eq!(v.get(r"C:\d0"), FolderView::default());
        assert_eq!(v.get(&format!(r"C:\d{}", MAX + 9)), by(SortKey::Size, true));
    }

    #[test]
    fn merge_keeps_current_session() {
        let mut v = FolderViews::default();
        v.set(r"C:\a", by(SortKey::Size, true), 100);
        let mut loaded = FolderViews::default();
        loaded.set(r"C:\a", by(SortKey::Type, true), 1);
        loaded.set(r"C:\b", by(SortKey::Modified, true), 1);
        v.merge_loaded(loaded);
        assert_eq!(v.get(r"C:\a"), by(SortKey::Size, true));
        assert_eq!(v.get(r"C:\b"), by(SortKey::Modified, true));
    }
}
