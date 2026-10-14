//! 每個資料夾記住自己的檢視設定（`zing_core::folderview`）：進入資料夾時套用，變更排序時記下。
//!
//! 檔案在 %LOCALAPPDATA%\zing-explore\folders.tsv（可重建的系統資料，見 CLAUDE.md），第一幀之後在背景讀取，
//! 變更時在背景寫入。隔離模式（效能、冒煙測試）不讀寫檔案，記憶體內照常運作。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use zing_core::folderview::FolderViews;

use crate::app::App;
use crate::services::Msg;

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
                let want = self.folder_views.get(&tab.path).sort;
                if tab.spec == Default::default() && want != tab.spec {
                    tab.spec = want;
                    self.resort(p, t);
                }
            }
        }
        self.invalidate();
    }
}
