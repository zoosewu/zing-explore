//! 資料夾大小：背景計算、顯示、排序與快取。
//!
//! - 有 NTFS 索引服務時大小直接由服務提供（`index.rs`）；這裡是沒有索引時的逐資料夾掃描。
//! - `view.foldersizes`（Ctrl+Shift+S）計算選取的資料夾，沒有選取資料夾時計算目前清單中所有資料夾；
//!   設定 `[folder_sizes] auto = true`（預設）時，進入本機固定磁碟上的資料夾就自動計算。
//!   計算執行緒以背景模式執行（磁碟與 CPU 優先權最低）。
//! - 在專用的執行緒池計算（不佔用排序等工作的執行緒），換資料夾時取消（自動重新整理不取消）；
//!   計算途中約每 150 ms 回報累計值。
//! - 結果以「上層資料夾 → 名稱」存放，寫進清單項目的 size，依大小排序時資料夾也一起排。
//! - 快取在 %LOCALAPPDATA%\zing-explore\folder-sizes.tsv（可重建的系統資料，見 CLAUDE.md）。
//!   上次執行留下的值、或被本程式的檔案作業改動過的資料夾顯示為「≈」（可能已過時）。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zing_core::dirsize::{self, CacheLine, DirSize};
use zing_core::path;

use crate::app::App;
use crate::model::Tab;
use crate::services::{Msg, Pool, is_fixed_drive};

/// 快取最多保留的筆數（較舊的捨棄）
const CACHE_MAX: usize = 50_000;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeState {
    Computing,
    /// 這次執行算出的
    Fresh,
    /// 上次執行留下的，或之後有檔案作業改動過：可能已過時
    Stale,
}

#[derive(Clone, Copy, Debug)]
pub struct SizeInfo {
    pub size: DirSize,
    pub state: SizeState,
    /// 計算完成的時間（Unix 秒）
    pub when: u64,
}

#[derive(Default)]
pub struct FolderSizes {
    /// 上層資料夾（小寫）→ 名稱（小寫）→ 大小
    map: HashMap<String, HashMap<String, SizeInfo>>,
    pool: Option<Pool>,
    /// 計算中的資料夾數
    pub pending: usize,
    /// 這一批完成的數量（全部完成時顯示在狀態列）
    batch_done: usize,
    /// 結果有變、需要寫進清單項目的上層資料夾（小寫）
    dirty: HashSet<String>,
    /// 索引服務查詢用的執行緒
    pub query_pool: Option<Pool>,
    /// 查詢中的資料夾（小寫），避免輪詢重複送出
    pub inflight: HashSet<String>,
    /// 由索引提供大小的資料夾（小寫）→ 變動標記；0 表示索引建立中，之後再試
    pub index_touched: HashMap<String, u64>,
    /// 已提示過可以啟用索引服務
    pub hinted: bool,
}

pub fn key(s: &str) -> String {
    s.to_lowercase()
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl FolderSizes {
    pub fn get(&self, parent_key: &str, name: &str) -> Option<&SizeInfo> {
        self.map.get(parent_key)?.get(&key(name))
    }

    pub fn has_parent(&self, parent_key: &str) -> bool {
        self.map.contains_key(parent_key)
    }

    pub fn set_info(&mut self, parent: &str, name: &str, info: SizeInfo) {
        self.map.entry(key(parent)).or_default().insert(key(name), info);
    }

    fn remove(&mut self, parent: &str, name: &str) {
        if let Some(m) = self.map.get_mut(&key(parent)) {
            m.remove(&key(name));
        }
    }

    pub fn mark_dirty(&mut self, parent_key: String) {
        self.dirty.insert(parent_key);
    }

    /// `dir` 與它所有上層資料夾的大小可能已改變
    pub fn mark_stale(&mut self, dir: &str) {
        let mut p = dir.to_string();
        while let Some(parent) = path::parent(&p) {
            if let Some(info) = self.map.get_mut(&key(&parent)).and_then(|m| m.get_mut(&key(path::file_name(&p))))
                && info.state == SizeState::Fresh
            {
                info.state = SizeState::Stale;
            }
            if parent == p {
                break;
            }
            p = parent;
        }
    }

    /// 寫進快取檔的內容（不含計算中的）
    fn snapshot(&self) -> Vec<CacheLine> {
        let mut v: Vec<CacheLine> = self
            .map
            .iter()
            .flat_map(|(parent, m)| {
                m.iter().filter(|(_, i)| i.state != SizeState::Computing).map(move |(name, i)| CacheLine {
                    path: path::join(parent, name),
                    size: i.size,
                    when: i.when,
                })
            })
            .collect();
        v.sort_unstable_by_key(|c| std::cmp::Reverse(c.when));
        v.truncate(CACHE_MAX);
        v
    }
}

/// 快取檔：%LOCALAPPDATA%\zing-explore\folder-sizes.tsv
fn cache_file() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("zing-explore").join("folder-sizes.tsv"))
}

impl App {
    /// 計算資料夾大小。`only_missing`：已有這次算出的值就略過（自動計算時）
    pub fn compute_folder_sizes(&mut self, p: usize, only_missing: bool, auto: bool) {
        let tab = self.panes[p].tab();
        let parent = tab.path.clone();
        let parent_key = key(&parent);
        // 選取的資料夾；沒有選取資料夾時為清單中所有資料夾
        let dirs = |idx: Vec<usize>| -> Vec<String> {
            idx.into_iter().filter(|&i| tab.listing.entries[i].is_dir()).map(|i| tab.listing.name_string(i)).collect()
        };
        let mut names = if auto { Vec::new() } else { dirs(tab.selected_entries()) };
        if names.is_empty() {
            names = dirs(tab.view.iter().map(|&v| v as usize).collect());
        }
        names.retain(|n| match self.sizes.get(&parent_key, n).map(|i| i.state) {
            Some(SizeState::Computing) => false,
            Some(SizeState::Fresh) => !only_missing,
            _ => true,
        });
        if names.is_empty() {
            return;
        }
        let cancel = tab.size_cancel.clone();
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(2, 4);
        if self.sizes.pool.is_none() {
            self.sizes.pool = Some(Pool::background(threads, "zing-sizes"));
        }
        if self.sizes.pending == 0 {
            self.sizes.batch_done = 0;
        }
        for name in names {
            let prev = self.sizes.get(&parent_key, &name).map(|i| i.size).unwrap_or_default();
            self.sizes.set_info(&parent, &name, SizeInfo { size: prev, state: SizeState::Computing, when: 0 });
            self.sizes.pending += 1;
            let (n, cancel, parent) = (self.notifier.clone(), cancel.clone(), parent.clone());
            self.sizes.pool.as_ref().unwrap().spawn(move || {
                let full = path::join(&parent, &name);
                let skip = auto && !is_fixed_drive(&full);
                let result = if skip || cancel.load(Ordering::Relaxed) {
                    None
                } else {
                    dirsize::dir_size(&full, &cancel, PROGRESS_INTERVAL, |s| {
                        n.send(Msg::FolderSize { parent: parent.clone(), name: name.clone(), size: *s, done: None });
                    })
                };
                n.send(Msg::FolderSize { parent, name, size: result.unwrap_or_default(), done: Some(result.is_some()) });
            });
        }
        self.invalidate();
    }

    /// `done`：None 為途中回報，Some(true) 完成，Some(false) 取消或略過
    pub fn on_folder_size(&mut self, parent: String, name: String, size: DirSize, done: Option<bool>) {
        let parent_key = key(&parent);
        match done {
            None => {
                if let Some(i) = self.sizes.map.get_mut(&parent_key).and_then(|m| m.get_mut(&key(&name)))
                    && i.state == SizeState::Computing
                {
                    i.size = size;
                }
            }
            Some(ok) => {
                self.sizes.pending = self.sizes.pending.saturating_sub(1);
                if ok {
                    self.sizes.set_info(&parent, &name, SizeInfo { size, state: SizeState::Fresh, when: now_secs() });
                    self.sizes.batch_done += 1;
                } else {
                    self.sizes.remove(&parent, &name);
                }
                self.sizes.dirty.insert(parent_key);
                if self.sizes.pending == 0 {
                    let n = self.sizes.batch_done;
                    if n > 0 {
                        self.set_status(format!("已計算 {n} 個資料夾的大小"), false);
                    }
                    self.save_size_cache();
                }
            }
        }
        self.invalidate();
    }

    /// 把結果寫進對應分頁的清單項目（依大小排序時重新排序）；處理完一批通知後呼叫
    pub fn apply_size_updates(&mut self) {
        if self.sizes.dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.sizes.dirty);
        for p in 0..self.panes.len() {
            for t in 0..self.panes[p].tabs.len() {
                if dirty.contains(&key(&self.panes[p].tabs[t].path)) && self.patch_tab_sizes(p, t) {
                    self.resort(p, t);
                }
            }
        }
    }

    /// 把已知的資料夾大小寫進分頁的清單；有改變時回傳 true。
    /// 先找出要改的項目，真的有改變才複製清單（清單可能與背景排序共用，複製 10 萬筆要數毫秒）。
    pub fn patch_tab_sizes(&mut self, p: usize, t: usize) -> bool {
        let tab: &mut Tab = &mut self.panes[p].tabs[t];
        let Some(m) = self.sizes.map.get(&key(&tab.path)) else { return false };
        let listing = &tab.listing;
        let changes: Vec<(usize, u64)> = (0..listing.len())
            .filter(|&i| listing.entries[i].is_dir())
            .filter_map(|i| {
                let bytes = match m.get(&key(&listing.name_string(i))) {
                    Some(info) if info.state != SizeState::Computing => info.size.bytes,
                    _ => 0,
                };
                (listing.entries[i].size != bytes).then_some((i, bytes))
            })
            .collect();
        if changes.is_empty() {
            return false;
        }
        let listing = Arc::make_mut(&mut tab.listing);
        for (i, bytes) in changes {
            listing.entries[i].size = bytes;
        }
        true
    }

    /// 大小欄的文字；計算中回傳 true（以強調色顯示）
    pub fn folder_size_text(&self, parent_key: &str, name: &str) -> Option<(String, bool)> {
        let info = self.sizes.get(parent_key, name)?;
        let s = zing_core::format::size_short(info.size.bytes);
        Some(match info.state {
            SizeState::Computing => (format!("{s}…"), true),
            SizeState::Stale => (format!("≈ {s}"), false),
            SizeState::Fresh if info.size.denied > 0 => (format!("≥ {s}"), false),
            SizeState::Fresh => (s, false),
        })
    }

    /// 第一幀之後在背景讀取快取
    pub fn load_size_cache(&self) {
        if self.isolated {
            return;
        }
        let Some(file) = cache_file() else { return };
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            let lines: Vec<CacheLine> =
                std::fs::read_to_string(file).map(|s| s.lines().filter_map(dirsize::parse_line).collect()).unwrap_or_default();
            if !lines.is_empty() {
                n.send(Msg::FolderSizesLoaded(lines));
            }
        });
    }

    pub fn on_size_cache_loaded(&mut self, lines: Vec<CacheLine>) {
        for c in lines {
            let (Some(parent), name) = (path::parent(&c.path), path::file_name(&c.path).to_string()) else { continue };
            // 這次已經算過（或正在算）的不覆蓋
            if self.sizes.get(&key(&parent), &name).is_none() {
                self.sizes.set_info(&parent, &name, SizeInfo { size: c.size, state: SizeState::Stale, when: c.when });
                self.sizes.dirty.insert(key(&parent));
            }
        }
        self.apply_size_updates();
        self.invalidate();
    }

    fn save_size_cache(&self) {
        if self.isolated {
            return;
        }
        let Some(file) = cache_file() else { return };
        let lines = self.sizes.snapshot();
        self.pool.spawn(move || {
            zing_core::guard::io("儲存資料夾大小快取");
            let mut text = String::with_capacity(lines.len() * 64);
            for l in &lines {
                text.push_str(&dirsize::to_line(l));
                text.push('\n');
            }
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = file.with_extension("tsv.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, file);
            }
        });
    }

    /// 進入資料夾（列舉完成）時，若設定為自動就計算
    pub fn auto_folder_sizes(&mut self, p: usize, t: usize) {
        if self.config.folder_sizes.auto && t == self.panes[p].active && !self.panes[p].tabs[t].path.is_empty() {
            self.compute_folder_sizes(p, true, true);
        }
    }
}

/// 換資料夾時取消舊的計算
pub fn new_cancel(old: &Arc<AtomicBool>) -> Arc<AtomicBool> {
    old.store(true, Ordering::Relaxed);
    Arc::new(AtomicBool::new(false))
}
