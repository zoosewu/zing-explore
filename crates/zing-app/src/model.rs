//! UI 狀態模型：分頁、窗格、樹狀列。這裡只有純狀態操作，不呼叫任何服務。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
    /// 依 listing 索引記錄是否選取，篩選時選取狀態不會遺失
    pub selected: Vec<bool>,
    /// 焦點（view 索引）
    pub focus: usize,
    pub anchor: usize,
    /// 捲動位置（DIP）
    pub scroll: f32,
    pub filter: String,
    pub pattern: Pattern,
    pub spec: SortSpec,
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
    pub fn new(id: TabId, path: String, spec: SortSpec) -> Self {
        Self {
            id,
            path,
            listing: Arc::new(Listing::default()),
            sorted: Vec::new(),
            view: Vec::new(),
            selected: Vec::new(),
            focus: 0,
            anchor: 0,
            scroll: 0.0,
            filter: String::new(),
            pattern: Pattern::default(),
            spec,
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
        if self.path.is_empty() { "此電腦".into() } else { path::file_name(&self.path).to_string() }
    }

    /// 換資料夾前清空內容
    pub fn reset_contents(&mut self) {
        self.listing = Arc::new(Listing::default());
        self.sorted.clear();
        self.view.clear();
        self.selected.clear();
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

    pub fn is_selected_view(&self, view_idx: usize) -> bool {
        self.view.get(view_idx).is_some_and(|&v| self.selected.get(v as usize).copied().unwrap_or(false))
    }

    pub fn clear_selection(&mut self) {
        self.selected.iter_mut().for_each(|s| *s = false);
    }

    pub fn select_only(&mut self, view_idx: usize) {
        self.clear_selection();
        if let Some(&v) = self.view.get(view_idx) {
            self.selected[v as usize] = true;
        }
        self.focus = view_idx;
        self.anchor = view_idx;
    }

    pub fn toggle(&mut self, view_idx: usize) {
        if let Some(&v) = self.view.get(view_idx) {
            self.selected[v as usize] ^= true;
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
            self.selected[v] = true;
        }
        self.focus = view_idx;
    }

    pub fn select_all(&mut self) {
        for &v in &self.view {
            self.selected[v as usize] = true;
        }
    }

    pub fn invert_selection(&mut self) {
        for &v in &self.view {
            self.selected[v as usize] ^= true;
        }
    }

    pub fn selection_count(&self) -> (usize, u64) {
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

#[derive(Clone, Debug)]
pub struct Node {
    pub path: String,
    pub name: String,
    pub depth: u16,
    pub kind: NodeKind,
    pub expanded: bool,
    pub loading: bool,
    /// 已載入且沒有子資料夾
    pub leaf: bool,
}

#[derive(Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub scroll: f32,
    pub focus: usize,
    /// 正在展開到這個路徑（換資料夾時同步樹狀列）
    pub reveal: Option<String>,
}

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
            name: "我的最愛".into(),
            depth: 0,
            kind: NodeKind::FavRoot,
            expanded: true,
            loading: false,
            leaf: favorites.is_empty(),
        }];
        for f in favorites {
            nodes.push(folder_node(f, path::file_name(f).to_string(), 1, NodeKind::Favorite));
        }
        nodes.push(Node {
            path: String::new(),
            name: "此電腦".into(),
            depth: 0,
            kind: NodeKind::Pc,
            expanded: true,
            loading: false,
            leaf: false,
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

    pub fn collapse(&mut self, i: usize) {
        let end = self.subtree_end(i);
        self.nodes.drain(i + 1..end);
        self.nodes[i].expanded = false;
        if self.focus > i && self.focus < end {
            self.focus = i;
        } else if self.focus >= end {
            self.focus -= end - i - 1;
        }
    }

    /// 標記為載入中並回傳要載入的路徑；不可展開時回傳 None
    pub fn begin_expand(&mut self, i: usize) -> Option<String> {
        let n = &mut self.nodes[i];
        if n.expanded || n.loading || n.leaf || n.path.is_empty() {
            return None;
        }
        n.loading = true;
        Some(n.path.clone())
    }

    pub fn insert_children(&mut self, path_: &str, names: Vec<String>) {
        let Some(i) = self.nodes.iter().position(|n| n.loading && n.path.eq_ignore_ascii_case(path_)) else {
            return;
        };
        let depth = self.nodes[i].depth + 1;
        let parent = self.nodes[i].path.clone();
        let children: Vec<Node> =
            names.into_iter().map(|name| folder_node(&path::join(&parent, &name), name, depth, NodeKind::Folder)).collect();
        let count = children.len();
        let n = &mut self.nodes[i];
        n.loading = false;
        n.expanded = count > 0;
        n.leaf = count == 0;
        self.nodes.splice(i + 1..i + 1, children);
        if self.focus > i {
            self.focus += count;
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
            let Some(i) = self.find(p) else {
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
                return match self.begin_expand(i) {
                    Some(p) => Reveal::Load(p),
                    None => {
                        self.reveal = None;
                        Reveal::Done(None)
                    }
                };
            }
        }
        self.reveal = None;
        Reveal::Done(self.find(&target))
    }
}

fn folder_node(p: &str, name: String, depth: u16, kind: NodeKind) -> Node {
    Node { path: p.to_string(), name, depth, kind, expanded: false, loading: false, leaf: false }
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
        let mut t = Tab::new(1, "C:\\x".into(), SortSpec::default());
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

    #[test]
    fn tree_expand_collapse() {
        let mut drives = Listing::default();
        drives.push_str("C:\\", ATTR_DIRECTORY, 0, 0);
        let mut tree = Tree::build(&[], &drives);
        let c = tree.find("C:\\").unwrap();
        assert_eq!(tree.begin_expand(c).as_deref(), Some("C:\\"));
        tree.insert_children("C:\\", vec!["Users".into(), "Windows".into()]);
        assert_eq!(tree.nodes.len(), 5);
        assert_eq!(tree.find("c:\\users"), Some(c + 1));
        tree.reveal = Some("C:\\Users\\me".into());
        match tree.step_reveal() {
            Reveal::Load(p) => assert_eq!(p, "C:\\Users"),
            Reveal::Done(_) => panic!("應先載入 C:\\Users"),
        }
        tree.insert_children("C:\\Users", vec!["me".into()]);
        match tree.step_reveal() {
            Reveal::Done(Some(i)) => assert_eq!(tree.nodes[i].path, "C:\\Users\\me"),
            _ => panic!(),
        }
        tree.collapse(c);
        assert_eq!(tree.nodes.len(), 3);
    }
}
