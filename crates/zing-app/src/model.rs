//! UI 狀態模型：分頁、窗格、樹狀列。這裡只有純狀態操作，不呼叫任何服務。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use zing_core::folderview::{FolderView, ViewMode};
use zing_core::listing::Listing;
use zing_core::path;
use zing_core::sort::{SortSpec, filter_order};
use zing_core::text::Pattern;

use crate::services::{TabId, Watcher};

pub struct Tab {
    pub id: TabId,
    pub path: String,
    pub listing: Arc<Listing>,
    /// 全部項目的排序結果（listing 索引）
    pub sorted: Vec<u32>,
    /// 篩選後實際顯示的項目（listing 索引）
    pub view: Vec<u32>,
    /// 依 listing 索引記錄是否選取，篩選時選取狀態不會遺失。
    /// 只經下面的方法修改，才能同步更新 `sel`
    selected: Vec<bool>,
    /// 可見的選取項目數與總大小：選取、清單、篩選改變時更新（繪製時直接讀，不必每一幀掃過清單）
    sel: (usize, u64),
    /// 焦點（view 索引）
    pub focus: usize,
    pub anchor: usize,
    /// 捲動位置（DIP）
    pub scroll: f32,
    pub filter: String,
    pub pattern: Pattern,
    pub spec: SortSpec,
    /// 清單或縮圖檢視
    pub mode: ViewMode,
    /// 縮圖邊長（DIP）
    pub thumb: u16,
    /// 每次載入遞增，用來丟棄過時的背景結果
    pub generation: u64,
    /// 每次換資料夾遞增，用來丟棄過時的監看通知
    pub nav_generation: u64,
    pub loading: bool,
    /// 重新整理中：保留舊內容直到新結果完成，避免閃爍
    pub refreshing: bool,
    pub error: Option<String>,
    pub back: Vec<String>,
    pub forward: Vec<String>,
    pub cancel: Arc<AtomicBool>,
    /// 資料夾大小計算的取消旗標：只在換資料夾時更換（自動重新整理不取消）
    pub size_cancel: Arc<AtomicBool>,
    pub watcher: Option<Watcher>,
    /// 載入完成後要選取的項目（名稱、是否進入重新命名）
    pub pending_select: Option<(String, bool)>,
    pub refresh_due: bool,
}

impl Tab {
    pub fn new(id: TabId, path: String, view: FolderView) -> Self {
        Self {
            id,
            path,
            listing: Arc::new(Listing::default()),
            sorted: Vec::new(),
            view: Vec::new(),
            selected: Vec::new(),
            sel: (0, 0),
            focus: 0,
            anchor: 0,
            scroll: 0.0,
            filter: String::new(),
            pattern: Pattern::default(),
            spec: view.sort,
            mode: view.mode,
            thumb: view.thumb,
            generation: 0,
            nav_generation: 0,
            loading: false,
            refreshing: false,
            error: None,
            back: Vec::new(),
            forward: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            size_cancel: Arc::new(AtomicBool::new(false)),
            watcher: None,
            pending_select: None,
            refresh_due: false,
        }
    }

    pub fn title(&self) -> String {
        if self.path.is_empty() { tr!("此電腦").into() } else { path::file_name(&self.path).to_string() }
    }

    /// 換資料夾前清空內容
    pub fn reset_contents(&mut self) {
        self.listing = Arc::new(Listing::default());
        self.sorted.clear();
        self.view.clear();
        self.selected.clear();
        self.sel = (0, 0);
        self.focus = 0;
        self.anchor = 0;
        self.scroll = 0.0;
        self.filter.clear();
        self.pattern = Pattern::default();
        self.error = None;
        self.refreshing = false;
        self.pending_select = None;
        self.refresh_due = false;
    }

    pub fn rebuild_view(&mut self, show_hidden: bool) {
        let focused = self.focused_entry();
        self.view = filter_order(&self.listing, &self.sorted, &self.pattern, show_hidden);
        self.focus = focused.and_then(|e| self.view.iter().position(|&v| v as usize == e)).unwrap_or(0);
        self.anchor = self.anchor.min(self.view.len().saturating_sub(1));
        self.recount_selection();
    }

    pub fn set_filter(&mut self, filter: String, show_hidden: bool) {
        self.pattern = Pattern::new(&filter);
        self.filter = filter;
        self.rebuild_view(show_hidden);
        self.focus = 0;
        self.anchor = 0;
        self.scroll = 0.0;
    }

    /// 套用新的列舉結果。重新整理時依名稱還原選取與焦點。
    pub fn apply_listing(&mut self, listing: Arc<Listing>, sorted: Vec<u32>, show_hidden: bool) {
        if self.refreshing {
            let names: HashSet<Vec<u16>> =
                self.selected_indices_all().into_iter().map(|i| self.listing.name(i).to_vec()).collect();
            let focus_name = self.focused_entry().map(|i| self.listing.name(i).to_vec());
            self.selected = (0..listing.len()).map(|i| names.contains(listing.name(i))).collect();
            self.listing = listing;
            self.sorted = sorted;
            self.view = filter_order(&self.listing, &self.sorted, &self.pattern, show_hidden);
            self.focus = focus_name
                .and_then(|n| self.view.iter().position(|&v| self.listing.name(v as usize) == n.as_slice()))
                .unwrap_or(self.focus.min(self.view.len().saturating_sub(1)));
            self.anchor = self.focus;
            self.recount_selection();
        } else {
            // 初次載入的分段結果是前綴穩定的，選取陣列直接延伸即可
            self.selected.resize(listing.len(), false);
            self.listing = listing;
            self.sorted = sorted;
            self.rebuild_view(show_hidden);
        }
    }

    pub fn focused_entry(&self) -> Option<usize> {
        self.view.get(self.focus).map(|&v| v as usize)
    }

    /// 所有選取項目（listing 索引，不論是否被篩選隱藏）
    fn selected_indices_all(&self) -> Vec<usize> {
        self.selected.iter().enumerate().filter(|(_, s)| **s).map(|(i, _)| i).collect()
    }

    /// 目前可見的選取項目（依顯示順序）
    pub fn selected_entries(&self) -> Vec<usize> {
        self.view.iter().map(|&v| v as usize).filter(|&i| self.selected.get(i).copied().unwrap_or(false)).collect()
    }

    /// 選取項目；沒有選取時以焦點項目代替
    pub fn targets(&self) -> Vec<usize> {
        let sel = self.selected_entries();
        if sel.is_empty() { self.focused_entry().into_iter().collect() } else { sel }
    }

    pub fn full_path(&self, entry: usize) -> String {
        path::join(&self.path, &self.listing.name_string(entry))
    }

    pub fn target_paths(&self) -> Vec<String> {
        self.targets().into_iter().map(|i| self.full_path(i)).collect()
    }

    /// listing 第 `entry` 個項目是否選取
    pub fn is_selected(&self, entry: usize) -> bool {
        self.selected.get(entry).copied().unwrap_or(false)
    }

    /// 改變一個可見項目（`entry` 必須在 view 中）的選取狀態，同步更新選取數
    fn set_selected(&mut self, entry: usize, on: bool) {
        let Some(s) = self.selected.get_mut(entry) else { return };
        if *s == on {
            return;
        }
        *s = on;
        let size = self.listing.entries[entry].size;
        if on {
            self.sel.0 += 1;
            self.sel.1 += size;
        } else {
            self.sel.0 -= 1;
            self.sel.1 -= size;
        }
    }

    /// 掃過 view 計算選取數（只在清單或篩選改變時）
    fn scan_selection(&self) -> (usize, u64) {
        let mut n = 0;
        let mut bytes = 0;
        for &v in &self.view {
            let v = v as usize;
            if self.selected[v] {
                n += 1;
                bytes += self.listing.entries[v].size;
            }
        }
        (n, bytes)
    }

    fn recount_selection(&mut self) {
        self.sel = self.scan_selection();
    }

    /// 項目的大小改變後（資料夾大小算好）：有選取的才需要重算
    pub fn sizes_changed(&mut self, entries: &[usize]) {
        if entries.iter().any(|&e| self.is_selected(e)) {
            self.recount_selection();
        }
    }

    pub fn is_selected_view(&self, view_idx: usize) -> bool {
        self.view.get(view_idx).is_some_and(|&v| self.selected.get(v as usize).copied().unwrap_or(false))
    }

    pub fn clear_selection(&mut self) {
        self.selected.iter_mut().for_each(|s| *s = false);
        self.sel = (0, 0);
    }

    pub fn select_only(&mut self, view_idx: usize) {
        self.clear_selection();
        if let Some(&v) = self.view.get(view_idx) {
            self.set_selected(v as usize, true);
        }
        self.focus = view_idx;
        self.anchor = view_idx;
    }

    pub fn toggle(&mut self, view_idx: usize) {
        if let Some(&v) = self.view.get(view_idx) {
            let v = v as usize;
            self.set_selected(v, !self.selected[v]);
        }
        self.focus = view_idx;
        self.anchor = view_idx;
    }

    /// 從 anchor 選到 `view_idx`；`additive` 為真時保留原有選取
    pub fn select_range(&mut self, view_idx: usize, additive: bool) {
        if !additive {
            self.clear_selection();
        }
        let (a, b) = if self.anchor <= view_idx { (self.anchor, view_idx) } else { (view_idx, self.anchor) };
        for i in a..=b.min(self.view.len().saturating_sub(1)) {
            let v = self.view[i] as usize;
            self.set_selected(v, true);
        }
        self.focus = view_idx;
    }

    pub fn select_all(&mut self) {
        for &v in &self.view {
            self.selected[v as usize] = true;
        }
        self.recount_selection();
    }

    pub fn invert_selection(&mut self) {
        for &v in &self.view {
            self.selected[v as usize] ^= true;
        }
        self.recount_selection();
    }

    /// 可見的選取項目數與總大小（事件更新的值，不掃描）
    pub fn selection_count(&self) -> (usize, u64) {
        // debug build 核對：有修改選取、清單或篩選卻沒更新的地方，冒煙測試就會失敗
        debug_assert_eq!(self.sel, self.scan_selection(), "選取數沒有同步更新");
        self.sel
    }

    pub fn find_view(&self, name: &str) -> Option<usize> {
        let w: Vec<u16> = name.encode_utf16().collect();
        self.view.iter().position(|&v| zing_core::text::eq_ignore_case(self.listing.name(v as usize), &w))
    }
}

impl Drop for Tab {
    /// 關閉分頁或窗格時中止這個分頁的背景計算
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.size_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: usize,
}

impl Pane {
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
}

// ---------------------------------------------------------------- 樹狀列

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    FavRoot,
    Favorite,
    Pc,
    Drive,
    Folder,
}

/// 一個資料夾的子資料夾：名稱（已依隱藏設定篩選、依自然順序排序）與各自是否還有子資料夾
#[derive(Clone, Debug)]
pub struct Children {
    pub names: Vec<String>,
    /// 與 `names` 對應；None 表示不知道（逐資料夾列舉時）
    pub subdirs: Vec<Option<bool>>,
    pub when: Instant,
}

impl Children {
    pub fn new(names: Vec<String>, subdirs: Vec<Option<bool>>) -> Self {
        Children { names, subdirs, when: Instant::now() }
    }
}

/// 子資料夾的預先載入狀態
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    Idle,
    Pending,
    /// 不預先載入（網路磁碟、隨身碟、光碟機）：一律顯示箭頭，點了才載入
    Skipped,
    Done,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub path: String,
    pub name: String,
    pub depth: u16,
    pub kind: NodeKind,
    pub expanded: bool,
    pub loading: bool,
    /// 有沒有子資料夾；None 表示還不知道
    pub subdirs: Option<bool>,
    pub probe: Probe,
    /// 預先載入的子資料夾：點擊時直接展開，不必等列舉
    pub prefetched: Option<Children>,
}

impl Node {
    /// 是否顯示展開箭頭：確定有子資料夾，或無法預先得知（與檔案總管相同）。
    /// 本機磁碟在預先載入完成前不顯示，避免顯示後才消失。
    pub fn arrow(&self) -> bool {
        self.expanded || self.loading || self.subdirs.unwrap_or(self.probe == Probe::Skipped)
    }
}

/// `begin_expand` 的結果
#[derive(Debug, PartialEq, Eq)]
pub enum Expand {
    /// 不可展開（已展開、載入中、沒有子資料夾）
    No,
    /// 以預先載入的內容立即展開；`stale` 為 true 時內容可能已過時，呼叫端應在背景重新載入
    Done { path: String, stale: bool },
    /// 需要先載入
    Load(String),
}

/// `p` 是 `old` 本身或在它底下時，換成 `new` 開頭的路徑（資料夾改名）
pub fn replace_prefix(p: &str, old: &str, new: &str) -> Option<String> {
    if p.eq_ignore_ascii_case(old) {
        return Some(new.to_string());
    }
    let head = p.get(..old.len())?;
    let rest = p[old.len()..].strip_prefix('\\')?;
    head.eq_ignore_ascii_case(old).then(|| path::join(new, rest))
}

/// 依 `order` 排列我的最愛（不分大小寫）；不在 `order` 中的維持原本的相對順序，排在後面
pub fn order_favorites(favorites: &mut [String], order: &[String]) {
    let rank = |f: &String| order.iter().position(|o| o.eq_ignore_ascii_case(f)).unwrap_or(usize::MAX);
    favorites.sort_by_key(rank);
}

/// 預先載入的內容超過這個時間就在展開後重新確認
const PREFETCH_FRESH: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub scroll: f32,
    pub focus: usize,
    /// 正在展開到這個路徑（換資料夾時同步樹狀列）
    pub reveal: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reveal {
    /// 需要先載入這個節點的子資料夾
    Load(String),
    /// 已展開到目標；回傳目標節點（找不到時為 None）
    Done(Option<usize>),
}

impl Tree {
    pub fn build(favorites: &[String], drives: &Listing) -> Self {
        let mut nodes = vec![Node {
            path: String::new(),
            name: tr!("我的最愛").into(),
            depth: 0,
            kind: NodeKind::FavRoot,
            expanded: true,
            loading: false,
            subdirs: Some(!favorites.is_empty()),
            probe: Probe::Done,
            prefetched: None,
        }];
        for f in favorites {
            nodes.push(folder_node(f, path::file_name(f).to_string(), 1, NodeKind::Favorite));
        }
        nodes.push(Node {
            path: String::new(),
            name: tr!("此電腦").into(),
            depth: 0,
            kind: NodeKind::Pc,
            expanded: true,
            loading: false,
            subdirs: Some(true),
            probe: Probe::Done,
            prefetched: None,
        });
        for i in 0..drives.len() {
            let p = drives.name_string(i);
            let name = p.trim_end_matches('\\').to_string();
            nodes.push(folder_node(&p, name, 1, NodeKind::Drive));
        }
        Self { nodes, ..Default::default() }
    }

    /// 子樹的結束位置（不含）
    fn subtree_end(&self, i: usize) -> usize {
        let d = self.nodes[i].depth;
        self.nodes[i + 1..].iter().position(|n| n.depth <= d).map_or(self.nodes.len(), |p| i + 1 + p)
    }

    /// 依 `favorites` 的順序重新排列我的最愛的節點（連同展開的子樹），焦點跟著原本的節點
    pub fn reorder_favorites(&mut self, favorites: &[String]) {
        let Some(root) = self.nodes.iter().position(|n| n.kind == NodeKind::FavRoot) else { return };
        let (start, end) = (root + 1, self.subtree_end(root));
        let depth = self.nodes[root].depth + 1;
        let mut groups: Vec<Vec<(usize, Node)>> = Vec::new();
        for (k, n) in self.nodes.drain(start..end).enumerate() {
            match groups.last_mut() {
                Some(g) if n.depth > depth => g.push((start + k, n)),
                _ => groups.push(vec![(start + k, n)]),
            }
        }
        groups.sort_by_key(|g| favorites.iter().position(|f| f.eq_ignore_ascii_case(&g[0].1.path)).unwrap_or(usize::MAX));
        let old_focus = self.focus;
        let mut moved = Vec::with_capacity(end - start);
        for (at, (orig, n)) in (start..).zip(groups.into_iter().flatten()) {
            if orig == old_focus {
                self.focus = at;
            }
            moved.push(n);
        }
        self.nodes.splice(start..start, moved);
    }

    pub fn collapse(&mut self, i: usize) {
        let end = self.subtree_end(i);
        let depth = self.nodes[i].depth + 1;
        // 子資料夾清單留著，再次展開時立即顯示（放久了會在展開後重新確認）
        let (names, subdirs) =
            self.nodes.drain(i + 1..end).filter(|n| n.depth == depth).map(|n| (n.name, n.subdirs)).unzip();
        self.nodes[i].expanded = false;
        if !self.nodes[i].path.is_empty() {
            self.nodes[i].prefetched = Some(Children::new(names, subdirs));
        }
        if self.focus > i && self.focus < end {
            self.focus = i;
        } else if self.focus >= end {
            self.focus -= end - i - 1;
        }
    }

    /// 展開：有預先載入的內容時立即展開，否則標記為載入中並回傳要載入的路徑
    pub fn begin_expand(&mut self, i: usize) -> Expand {
        let n = &mut self.nodes[i];
        if n.expanded || n.loading || n.subdirs == Some(false) || n.path.is_empty() {
            return Expand::No;
        }
        let path = n.path.clone();
        if let Some(c) = n.prefetched.take() {
            let stale = c.when.elapsed() > PREFETCH_FRESH;
            self.expand_with(i, c);
            return Expand::Done { path, stale };
        }
        n.loading = true;
        Expand::Load(path)
    }

    fn expand_with(&mut self, i: usize, c: Children) {
        let depth = self.nodes[i].depth + 1;
        let parent = self.nodes[i].path.clone();
        let children: Vec<Node> = c
            .names
            .into_iter()
            .zip(c.subdirs)
            .map(|(name, sub)| {
                let mut n = folder_node(&path::join(&parent, &name), name, depth, NodeKind::Folder);
                n.subdirs = sub;
                n
            })
            .collect();
        let count = children.len();
        let n = &mut self.nodes[i];
        n.loading = false;
        n.expanded = count > 0;
        n.subdirs = Some(count > 0);
        n.probe = Probe::Done;
        n.prefetched = None;
        self.nodes.splice(i + 1..i + 1, children);
        if self.focus > i {
            self.focus += count;
        }
    }

    /// 載入完成：展開等待中的節點
    pub fn insert_children(&mut self, path_: &str, c: Children) {
        let Some(i) = self.nodes.iter().position(|n| n.loading && n.path.eq_ignore_ascii_case(path_)) else {
            return;
        };
        self.expand_with(i, c);
    }

    /// 可見範圍內需要預先載入的節點：標記為進行中並回傳路徑
    pub fn take_probes(&mut self, first: usize, count: usize) -> Vec<String> {
        let end = (first + count).min(self.nodes.len());
        let mut out = Vec::new();
        for n in &mut self.nodes[first.min(end)..end] {
            let kind_ok = matches!(n.kind, NodeKind::Drive | NodeKind::Folder | NodeKind::Favorite);
            if kind_ok && n.probe == Probe::Idle && !n.expanded && !n.loading && !n.path.is_empty() {
                n.probe = Probe::Pending;
                out.push(n.path.clone());
            }
        }
        out
    }

    /// 預先載入的結果；None 表示這個位置不預先載入
    pub fn set_probe(&mut self, path_: &str, c: Option<Children>) {
        for n in &mut self.nodes {
            if n.probe != Probe::Pending || !n.path.eq_ignore_ascii_case(path_) {
                continue;
            }
            match &c {
                _ if n.expanded || n.loading => n.probe = Probe::Done,
                Some(c) => {
                    n.subdirs = Some(!c.names.is_empty());
                    n.prefetched = Some(c.clone());
                    n.probe = Probe::Done;
                }
                None => n.probe = Probe::Skipped,
            }
        }
    }

    /// 資料夾的子資料夾改變了（檔案作業）：收合中的節點丟掉預先載入的內容，看得到時重新確認
    pub fn invalidate(&mut self, path_: &str) {
        for n in &mut self.nodes {
            if !n.expanded && n.path.eq_ignore_ascii_case(path_) && n.probe == Probe::Done {
                n.probe = Probe::Idle;
                n.prefetched = None;
            }
        }
    }

    /// 以重新載入的內容更新已展開節點的子資料夾：保留仍存在的子節點（含其展開的子樹），
    /// 加入新的、移除已不存在的。同一個路徑可能出現在我的最愛與此電腦之下，都會更新
    pub fn reconcile(&mut self, path_: &str, c: Children) {
        // 由後往前：更新後面的不影響前面節點的位置
        let found: Vec<usize> =
            (0..self.nodes.len()).filter(|&i| self.nodes[i].expanded && self.nodes[i].path.eq_ignore_ascii_case(path_)).collect();
        for &i in found.iter().rev() {
            self.reconcile_at(i, c.clone());
        }
    }

    fn reconcile_at(&mut self, i: usize, c: Children) {
        let end = self.subtree_end(i);
        let depth = self.nodes[i].depth + 1;
        let focus_path = (self.focus > i && self.focus < end).then(|| self.nodes[self.focus].path.clone());
        let old: Vec<Node> = self.nodes.drain(i + 1..end).collect();
        let mut groups: HashMap<String, Vec<Node>> = HashMap::new();
        let mut cur: Option<String> = None;
        for n in old {
            if n.depth == depth {
                cur = Some(n.name.to_lowercase());
            }
            if let Some(k) = &cur {
                groups.entry(k.clone()).or_default().push(n);
            }
        }
        let parent = self.nodes[i].path.clone();
        let mut fresh = Vec::new();
        for (name, sub) in c.names.into_iter().zip(c.subdirs) {
            match groups.remove(&name.to_lowercase()) {
                Some(mut g) => {
                    if !g[0].expanded {
                        if sub.is_some() {
                            g[0].subdirs = sub;
                        }
                        // 預先載入的內容可能已過時（例如在裡面新增了資料夾）：看得到時重新確認，箭頭維持到結果出來
                        if g[0].probe == Probe::Done {
                            g[0].probe = Probe::Idle;
                            g[0].prefetched = None;
                        }
                    }
                    fresh.extend(g);
                }
                None => {
                    let mut n = folder_node(&path::join(&parent, &name), name, depth, NodeKind::Folder);
                    n.subdirs = sub;
                    fresh.push(n);
                }
            }
        }
        let count = fresh.len();
        let removed = end - i - 1;
        self.nodes.splice(i + 1..i + 1, fresh);
        self.nodes[i].expanded = count > 0;
        self.nodes[i].subdirs = Some(count > 0);
        if let Some(fp) = focus_path {
            self.focus = self.nodes.iter().position(|n| n.path == fp).unwrap_or(i);
        } else if self.focus >= end {
            self.focus = self.focus + count - removed;
        }
    }

    /// 隱藏檔設定改變：預先載入的內容都要重新取得
    pub fn reset_probes(&mut self) {
        for n in &mut self.nodes {
            if matches!(n.kind, NodeKind::Drive | NodeKind::Folder | NodeKind::Favorite) {
                // 展開中的也重設：途中的預先載入是舊條件的結果（會被丟棄），收合後要重新載入
                n.probe = Probe::Idle;
                n.prefetched = None;
                if !n.expanded {
                    n.subdirs = None;
                }
            }
        }
    }

    /// 在「此電腦」之下尋找路徑對應的節點（不分大小寫）
    pub fn find(&self, p: &str) -> Option<usize> {
        let pc = self.nodes.iter().position(|n| n.kind == NodeKind::Pc)?;
        if p.is_empty() {
            return Some(pc);
        }
        let target = p.trim_end_matches('\\');
        self.nodes[pc + 1..]
            .iter()
            .position(|n| n.path.trim_end_matches('\\').eq_ignore_ascii_case(target))
            .map(|k| pc + 1 + k)
    }

    /// 目前資料夾的上層是隱藏資料夾（例如 AppData）時，子資料夾清單裡沒有它：與檔案總管相同，
    /// 仍把這一層加到樹狀列（放在上層的子節點最後），才能展開到目前資料夾。上層必須已載入。
    fn insert_hidden(&mut self, parent: &str, p: &str) -> Option<usize> {
        let pi = self.find(parent)?;
        let n = &self.nodes[pi];
        if n.loading || !(n.expanded || n.subdirs == Some(false)) {
            return None;
        }
        let at = self.subtree_end(pi);
        let node = folder_node(p, path::file_name(p).to_string(), n.depth + 1, NodeKind::Folder);
        self.nodes.insert(at, node);
        self.nodes[pi].expanded = true;
        self.nodes[pi].subdirs = Some(true);
        if self.focus >= at {
            self.focus += 1;
        }
        Some(at)
    }

    /// 逐層展開到 `reveal` 目標
    pub fn step_reveal(&mut self) -> Reveal {
        let Some(target) = self.reveal.clone() else { return Reveal::Done(None) };
        let mut chain = Vec::new();
        let mut cur = Some(target.clone());
        while let Some(c) = cur {
            if c.is_empty() {
                break;
            }
            cur = path::parent(&c);
            chain.push(c);
        }
        chain.reverse();
        for (k, p) in chain.iter().enumerate() {
            let found = self.find(p).or_else(|| if k > 0 { self.insert_hidden(&chain[k - 1], p) } else { None });
            let Some(i) = found else {
                self.reveal = None;
                return Reveal::Done(None);
            };
            if k + 1 == chain.len() {
                self.reveal = None;
                return Reveal::Done(Some(i));
            }
            let n = &self.nodes[i];
            if n.loading {
                return Reveal::Done(None);
            }
            if !n.expanded {
                match self.begin_expand(i) {
                    Expand::Load(p) => return Reveal::Load(p),
                    // 以預先載入的內容展開了，或沒有（可見的）子資料夾：下一層由 insert_hidden 補上
                    Expand::Done { .. } | Expand::No => {}
                }
            }
        }
        self.reveal = None;
        Reveal::Done(self.find(&target))
    }
}

fn folder_node(p: &str, name: String, depth: u16, kind: NodeKind) -> Node {
    Node {
        path: p.to_string(),
        name,
        depth,
        kind,
        expanded: false,
        loading: false,
        subdirs: None,
        probe: Probe::Idle,
        prefetched: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zing_core::listing::ATTR_DIRECTORY;

    fn tab_with(names: &[&str]) -> Tab {
        let mut l = Listing::default();
        for n in names {
            l.push_str(n, 0, 1, 0);
        }
        let mut t = Tab::new(1, "C:\\x".into(), FolderView::default());
        let sorted = zing_core::sort::sorted_order(&l, t.spec);
        t.apply_listing(Arc::new(l), sorted, true);
        t
    }

    #[test]
    fn selection_survives_refresh() {
        let mut t = tab_with(&["a", "b", "c"]);
        t.select_only(1);
        t.toggle(2);
        t.refreshing = true;
        let mut l = Listing::default();
        for n in ["0", "a", "b", "c"] {
            l.push_str(n, 0, 1, 0);
        }
        let sorted = zing_core::sort::sorted_order(&l, t.spec);
        t.apply_listing(Arc::new(l), sorted, true);
        let names: Vec<_> = t.selected_entries().into_iter().map(|i| t.listing.name_string(i)).collect();
        assert_eq!(names, ["b", "c"]);
        assert_eq!(t.listing.name_string(t.focused_entry().unwrap()), "c");
    }

    #[test]
    fn range_and_filter() {
        let mut t = tab_with(&["a1", "a2", "b1", "b2"]);
        t.select_only(0);
        t.select_range(2, false);
        assert_eq!(t.selection_count().0, 3);
        t.set_filter("b".into(), true);
        assert_eq!(t.view.len(), 2);
        // 被篩選隱藏的選取不算在可見選取內
        assert_eq!(t.selection_count().0, 1);
        assert_eq!(t.find_view("B2"), Some(1));
    }

    fn kids(names: &[&str]) -> Children {
        Children::new(names.iter().map(|s| s.to_string()).collect(), vec![None; names.len()])
    }

    fn drive_tree() -> (Tree, usize) {
        let mut drives = Listing::default();
        drives.push_str("C:\\", ATTR_DIRECTORY, 0, 0);
        let tree = Tree::build(&[], &drives);
        let c = tree.find("C:\\").unwrap();
        (tree, c)
    }

    #[test]
    fn tree_reorder_favorites() {
        let favs: Vec<String> = ["C:\\a", "C:\\b"].map(String::from).to_vec();
        let mut t = Tree::build(&favs, &Listing::default());
        // 展開第一個我的最愛（含一個子節點），焦點在第二個
        t.nodes.insert(2, folder_node("C:\\a\\x", "x".into(), 2, NodeKind::Folder));
        t.nodes[1].expanded = true;
        t.focus = 3;
        t.reorder_favorites(&["C:\\b".into(), "C:\\a".into()]);
        let paths: Vec<&str> = t.nodes[..5].iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["", "C:\\b", "C:\\a", "C:\\a\\x", ""]);
        assert_eq!(t.focus, 1);
    }

    #[test]
    fn rename_prefix() {
        assert_eq!(replace_prefix(r"E:\a\B", r"e:\a\b", r"E:\a\c").as_deref(), Some(r"E:\a\c"));
        assert_eq!(replace_prefix(r"E:\a\b\x\y", r"E:\a\b", r"E:\a\c").as_deref(), Some(r"E:\a\c\x\y"));
        assert_eq!(replace_prefix(r"E:\a\bb", r"E:\a\b", r"E:\a\c"), None);
        assert_eq!(replace_prefix(r"E:\a", r"E:\a\b", r"E:\a\c"), None);
    }

    #[test]
    fn favorites_order() {
        let mut f: Vec<String> = ["C:\\a", "D:\\b", "E:\\c", "F:\\d"].map(String::from).to_vec();
        order_favorites(&mut f, &["e:\\C".into(), "x:\\gone".into(), "C:\\a".into()]);
        assert_eq!(f, ["E:\\c", "C:\\a", "D:\\b", "F:\\d"]);
    }

    #[test]
    fn tree_expand_collapse() {
        let (mut tree, c) = drive_tree();
        assert_eq!(tree.begin_expand(c), Expand::Load("C:\\".into()));
        tree.insert_children("C:\\", kids(&["Users", "Windows"]));
        assert_eq!(tree.nodes.len(), 5);
        assert_eq!(tree.find("c:\\users"), Some(c + 1));
        tree.reveal = Some("C:\\Users\\me".into());
        match tree.step_reveal() {
            Reveal::Load(p) => assert_eq!(p, "C:\\Users"),
            Reveal::Done(_) => panic!("應先載入 C:\\Users"),
        }
        tree.insert_children("C:\\Users", kids(&["me"]));
        match tree.step_reveal() {
            Reveal::Done(Some(i)) => assert_eq!(tree.nodes[i].path, "C:\\Users\\me"),
            _ => panic!(),
        }
        tree.collapse(c);
        assert_eq!(tree.nodes.len(), 3);
        // 收合後再展開：用保留的子資料夾清單立即展開
        assert_eq!(tree.begin_expand(c), Expand::Done { path: "C:\\".into(), stale: false });
        assert_eq!(tree.nodes.len(), 5);
    }

    #[test]
    fn tree_probe_and_instant_expand() {
        let (mut tree, c) = drive_tree();
        // 預先載入前不顯示箭頭（本機磁碟），送出後不重複送
        assert!(!tree.nodes[c].arrow());
        assert_eq!(tree.take_probes(0, 50), vec!["C:\\".to_string()]);
        assert!(!tree.take_probes(0, 50).iter().any(|p| p == "C:\\a"));
        let mut k = kids(&["Users", "Windows", "空"]);
        k.subdirs = vec![Some(true), Some(true), Some(false)];
        tree.set_probe("c:\\", Some(k));
        assert!(tree.nodes[c].arrow());
        // 點擊時立即展開，子節點的箭頭由索引的資訊決定
        assert_eq!(tree.begin_expand(c), Expand::Done { path: "C:\\".into(), stale: false });
        assert_eq!(tree.nodes.len(), 6);
        let empty = tree.find("C:\\空").unwrap();
        assert!(!tree.nodes[empty].arrow());
        assert_eq!(tree.begin_expand(empty), Expand::No);
        assert!(tree.nodes[tree.find("C:\\Users").unwrap()].arrow());
        // 沒有子資料夾的節點：預先載入後箭頭消失
        let users = tree.find("C:\\Users").unwrap();
        tree.nodes[users].subdirs = None;
        tree.take_probes(0, 50);
        tree.set_probe("C:\\Users", Some(kids(&[])));
        assert!(!tree.nodes[users].arrow());
        // 不預先載入的位置（網路磁碟）一律顯示箭頭
        let win = tree.find("C:\\Windows").unwrap();
        tree.nodes[win].subdirs = None;
        assert!(!tree.nodes[win].arrow());
        assert_eq!(tree.nodes[win].probe, Probe::Pending); // 與 Users 同一批送出
        tree.set_probe("C:\\Windows", None);
        assert_eq!(tree.nodes[win].probe, Probe::Skipped);
        assert!(tree.nodes[win].arrow());
    }

    #[test]
    fn reset_probes_releases_pending_expanded_node() {
        let (mut tree, c) = drive_tree();
        tree.begin_expand(c);
        tree.insert_children("C:\\", kids(&["a"]));
        let a = tree.find("C:\\a").unwrap();
        assert_eq!(tree.take_probes(0, 50), vec!["C:\\a".to_string()]);
        // 預先載入途中展開，接著顯示隱藏檔案改變：舊結果被丟棄，節點不能停在 Pending
        tree.begin_expand(a);
        tree.insert_children("C:\\a", kids(&["x"]));
        tree.reset_probes();
        assert_eq!(tree.nodes[a].probe, Probe::Idle);
        // 展開中不預先載入，收合後才重新載入
        assert!(!tree.take_probes(0, 50).iter().any(|p| p == "C:\\a"));
        tree.collapse(a);
        assert!(tree.take_probes(0, 50).iter().any(|p| p == "C:\\a"));
    }

    #[test]
    fn tree_reveal_through_hidden_folder() {
        let (mut tree, c) = drive_tree();
        tree.begin_expand(c);
        // 不顯示隱藏檔：C:\Users\me 底下的 AppData 不在清單中
        tree.insert_children("C:\\", kids(&["Users"]));
        tree.reveal = Some("C:\\Users\\me\\AppData\\Local".into());
        assert_eq!(tree.step_reveal(), Reveal::Load("C:\\Users".into()));
        tree.insert_children("C:\\Users", kids(&["me"]));
        assert_eq!(tree.step_reveal(), Reveal::Load("C:\\Users\\me".into()));
        tree.insert_children("C:\\Users\\me", kids(&[]));
        assert_eq!(tree.step_reveal(), Reveal::Load("C:\\Users\\me\\AppData".into()));
        tree.insert_children("C:\\Users\\me\\AppData", kids(&["Local", "Roaming"]));
        match tree.step_reveal() {
            Reveal::Done(Some(i)) => assert_eq!(tree.nodes[i].path, "C:\\Users\\me\\AppData\\Local"),
            r => panic!("{r:?}"),
        }
        let me = tree.find("C:\\Users\\me").unwrap();
        assert!(tree.nodes[me].expanded && tree.nodes[me].arrow());
    }

    #[test]
    fn tree_reconcile_keeps_subtrees() {
        let (mut tree, c) = drive_tree();
        tree.begin_expand(c);
        tree.insert_children("C:\\", kids(&["a", "b", "c"]));
        let b = tree.find("C:\\b").unwrap();
        tree.begin_expand(b);
        tree.insert_children("C:\\b", kids(&["b1", "b2"]));
        tree.focus = tree.find("C:\\b\\b2").unwrap();
        let ci = tree.find("C:\\c").unwrap();
        tree.nodes[ci].probe = Probe::Done;
        tree.nodes[ci].prefetched = Some(kids(&["x"]));
        // a 被刪除、新增 a2；b 的展開子樹與焦點保留
        tree.reconcile("C:\\", kids(&["a2", "B", "c"]));
        let paths: Vec<&str> = tree.nodes[c + 1..].iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, vec!["C:\\a2", "C:\\b", "C:\\b\\b1", "C:\\b\\b2", "C:\\c"]);
        assert_eq!(tree.nodes[tree.focus].path, "C:\\b\\b2");
        // 保留的子節點重新預先載入
        let ci = tree.find("C:\\c").unwrap();
        assert_eq!((tree.nodes[ci].probe, tree.nodes[ci].prefetched.is_none()), (Probe::Idle, true));
        tree.reconcile("C:\\", kids(&[]));
        assert!(!tree.nodes[c].expanded && !tree.nodes[c].arrow());
        assert_eq!(tree.focus, c);
    }
}
