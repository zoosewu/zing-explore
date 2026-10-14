//! 每個資料夾記住自己的檢視設定（`zing_core::folderview`）：進入資料夾時套用，變更排序時記下。
//!
//! 檔案在 %LOCALAPPDATA%\zing-explore\folders.tsv（可重建的系統資料，見 CLAUDE.md），第一幀之後在背景讀取，
//! 變更時在背景寫入。隔離模式（效能、冒煙測試）不讀寫檔案，記憶體內照常運作。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use zing_core::folderview::{FolderViews, ViewMode};

use crate::app::App;
use crate::services::Msg;

/// Ctrl+滾輪的縮圖大小範圍與每格的變化量
pub const THUMB_MIN: u16 = 48;
pub const THUMB_MAX: u16 = 256;
pub const THUMB_STEP: i32 = 16;

fn file() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("zing-explore").join("folders.tsv"))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl App {
    /// 記下分頁目前資料夾的檢視設定並在背景儲存
    pub fn remember_view(&mut self, p: usize, t: usize) {
        let tab = &self.panes[p].tabs[t];
        if tab.path.is_empty() {
            return;
        }
        let mut view = self.folder_views.get(&tab.path);
        view.sort = tab.spec;
        view.mode = tab.mode;
        view.thumb = tab.thumb;
        let path = tab.path.clone();
        self.folder_views.set(&path, view, now_secs());
        self.save_folder_views();
    }

    fn save_folder_views(&self) {
        if self.isolated {
            return;
        }
        let Some(file) = file() else { return };
        let text = self.folder_views.to_text();
        self.pool.spawn(move || {
            zing_core::guard::io("儲存資料夾檢視設定");
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = file.with_extension("tsv.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, file);
            }
        });
    }

    /// 切換檢視：list、small、medium、large（縮圖 64／128／192）或 thumbnails（沿用目前的縮圖大小）
    pub fn set_view_mode(&mut self, p: usize, mode: &str) -> Result<(), String> {
        let (m, size) = match mode {
            "list" => (ViewMode::List, None),
            "small" => (ViewMode::Thumbnails, Some(64)),
            "medium" => (ViewMode::Thumbnails, Some(128)),
            "large" => (ViewMode::Thumbnails, Some(192)),
            "thumbnails" => (ViewMode::Thumbnails, None),
            other => return Err(format!("未知的檢視：{other}（list、small、medium、large、thumbnails）")),
        };
        let t = self.panes[p].active;
        let tab = &mut self.panes[p].tabs[t];
        tab.mode = m;
        if let Some(s) = size {
            tab.thumb = s;
        }
        self.after_view_change(p, t);
        Ok(())
    }

    /// 縮圖大小加減（Ctrl+滾輪）：清單放大時進入縮圖，縮圖縮到最小以下時回到清單
    pub fn zoom_thumbs(&mut self, p: usize, delta: i32) {
        let t = self.panes[p].active;
        let tab = &mut self.panes[p].tabs[t];
        match tab.mode {
            ViewMode::List if delta > 0 => {
                tab.mode = ViewMode::Thumbnails;
                tab.thumb = THUMB_MIN;
            }
            ViewMode::List => return,
            ViewMode::Thumbnails => {
                let next = tab.thumb as i32 + delta;
                if next < THUMB_MIN as i32 {
                    tab.mode = ViewMode::List;
                } else {
                    tab.thumb = next.min(THUMB_MAX as i32) as u16;
                }
            }
        }
        self.after_view_change(p, t);
    }

    fn after_view_change(&mut self, p: usize, t: usize) {
        self.remember_view(p, t);
        self.layout = self.compute_layout();
        let max = self.max_scroll(p, self.layout.panes.get(p).map_or(400.0, |pl| pl.list.h()));
        let tab = &mut self.panes[p].tabs[t];
        tab.scroll = tab.scroll.min(max);
        self.ensure_visible(p);
        self.invalidate();
    }

    /// 第一幀之後在背景讀取
    pub fn load_folder_views(&self) {
        if self.isolated {
            return;
        }
        let Some(file) = file() else { return };
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            zing_core::guard::io("讀取資料夾檢視設定");
            if let Ok(text) = std::fs::read_to_string(file) {
                let v = FolderViews::from_text(&text);
                if !v.is_empty() {
                    n.send(Msg::FolderViewsLoaded(v));
                }
            }
        });
    }

    /// 讀取完成：併入（這次執行已設定的優先），已開啟、仍是預設排序的分頁改用記住的排序
    pub fn on_folder_views_loaded(&mut self, v: FolderViews) {
        self.folder_views.merge_loaded(v);
        for p in 0..self.panes.len() {
            for t in 0..self.panes[p].tabs.len() {
                let tab = &mut self.panes[p].tabs[t];
                let want = self.folder_views.get(&tab.path);
                let untouched = tab.spec == Default::default() && tab.mode == ViewMode::List;
                if untouched && (want.sort != tab.spec || want.mode != tab.mode) {
                    tab.spec = want.sort;
                    tab.mode = want.mode;
                    tab.thumb = want.thumb;
                    self.resort(p, t);
                }
            }
        }
        self.invalidate();
    }
}
