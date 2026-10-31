//! 清單與樹狀列的右鍵選單。預設是兩層（同 Windows 11）：先顯示程式自己的常用項目（`menu.rs` 自繪，立即出現、跟著主題），
//! 最後一項「顯示更多選項」才開啟完整的 Windows 選單（`ShellHost`，游標停住時已預先準備，也是立即出現）。
//! Shift+右鍵、設定 `context_menu.style = "full"`、「此電腦」直接顯示完整的 Windows 選單。
//! 樹狀列的項目只對那個節點動作（不是清單的選取），所以不用命令，而是 `MENU_TREE_*`。
//!
//! 第一層的命令與命令列共用（`cmd_menu_item`：快速鍵、不能用的原因）；程式自己的項目（在新分頁開啟、我的最愛、
//! 縮圖、批次重新命名）與完整選單相同（`entry_menu_extras`）。第三方擴充功能（7-Zip…）的項目由 ShellHost 在游標停住時
//! 預先載入（shellext.rs），項目相同才放進來（還沒準備好就不放，選單不會在顯示後才變長）；緩慢而被隱藏的列出名稱。

use windows::Win32::Foundation::POINT;
use zing_core::cmdbar::{self as core, Entry};
use zing_core::config::MenuStyle;

use crate::app::{App, MENU_FAV_ADD, MENU_FAV_DOWN, MENU_FAV_UP};
use crate::menu::{Action, MenuItem};
use crate::model::{MenuTarget, NodeKind};
use crate::shellext::ExtItem;
use crate::shellhost::ShellJob;

/// ShellHost 預先載入的第三方擴充功能項目與它們對應的項目、資料夾
pub struct MenuExts {
    pub items: Vec<String>,
    pub folder: String,
    pub menus: Vec<crate::shellext::ExtMenu>,
    /// 因為緩慢只在「顯示更多選項」中的擴充功能
    pub hidden: Vec<String>,
}

/// 第一層選單的「開啟方式…」（Windows 的 openas 動詞）
pub const MENU_OPEN_WITH: u32 = crate::shell::EXTRA_ID_BASE + 100;
/// 樹狀列第一層選單的項目，對象是節點的路徑
pub const MENU_TREE_OPEN: u32 = crate::shell::EXTRA_ID_BASE + 101;
pub const MENU_TREE_CUT: u32 = crate::shell::EXTRA_ID_BASE + 102;
pub const MENU_TREE_COPY: u32 = crate::shell::EXTRA_ID_BASE + 103;
pub const MENU_TREE_COPYPATH: u32 = crate::shell::EXTRA_ID_BASE + 104;
pub const MENU_TREE_PROPERTIES: u32 = crate::shell::EXTRA_ID_BASE + 105;
pub const MENU_TREE_DELETE: u32 = crate::shell::EXTRA_ID_BASE + 106;
pub const MENU_TREE_RENAME: u32 = crate::shell::EXTRA_ID_BASE + 107;

/// 「顯示更多選項」要開的完整選單，連同程式自己的項目（開啟第一層時算好，之後清單的選取改變也不影響）
pub struct PendingShellMenu {
    items: Vec<String>,
    folder: String,
    pt: POINT,
    extras: Vec<(u32, String)>,
    target: String,
}

const FILE_ITEMS: &[Entry] = &[
    Entry::Cmd("file.cut"),
    Entry::Cmd("file.copy"),
    Entry::Cmd("file.rename"),
    Entry::Cmd("file.delete"),
    Entry::Separator,
    Entry::Cmd("file.copypath"),
];

const BACKGROUND_ITEMS: &[Entry] = &[
    Entry::Sub(tr_key!("新增"), core::NEW_MENU),
    Entry::Sub(tr_key!("排序"), core::SORT_MENU),
    Entry::Sub(tr_key!("檢視"), core::VIEW_MENU),
    Entry::Cmd("nav.refresh"),
    Entry::Separator,
    Entry::Cmd("file.paste"),
    Entry::Separator,
    Entry::Cmd(core::TERMINAL),
];

/// 冒煙測試：內容對話框改成記下要顯示的路徑（對話框會搶前景，不顯示）
#[cfg(feature = "smoke")]
pub static TEST_PROPERTIES: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

impl App {
    /// 內容對話框（命令 file.properties、樹狀列的右鍵選單、Alt+雙擊）
    pub fn show_properties(&mut self, paths: Vec<String>) {
        #[cfg(feature = "smoke")]
        if self.isolated {
            if let Ok(mut t) = TEST_PROPERTIES.lock() {
                *t = Some(paths);
            }
            return;
        }
        self.shell_host.submit(ShellJob::Properties { paths });
    }

    /// 顯示清單的右鍵選單；`x`、`y` 是視窗內的座標（DIP），`pt` 是螢幕座標
    pub fn open_context_menu(&mut self, items: Vec<String>, folder: String, x: f32, y: f32, pt: POINT) {
        let (extras, target) = self.entry_menu_extras(&items, &folder);
        if self.full_menu_first() || folder.is_empty() {
            return self.show_context_menu(items, folder, pt, extras, target);
        }
        let menu = self.compact_menu_items(&items, &folder, &extras, &target);
        self.pending_shell_menu = Some(PendingShellMenu { items, folder, pt, extras, target });
        self.open_menu_at(menu, x, y);
    }

    /// Shift+右鍵或設定 `context_menu.style = "full"`：直接顯示完整的 Windows 選單
    fn full_menu_first(&self) -> bool {
        crate::input::Mods::current().shift || self.config.context_menu.style == MenuStyle::Full
    }

    fn compact_menu_items(&self, items: &[String], folder: &str, extras: &[(u32, String)], target: &str) -> Vec<MenuItem> {
        let extra = |(id, label): &(u32, String)| MenuItem::new(label.clone(), Action::Extra(*id, target.to_string()));
        let mut out = Vec::new();
        if items.is_empty() {
            out.extend(self.menu_entries(BACKGROUND_ITEMS));
            if !extras.is_empty() {
                out.push(MenuItem::separator());
                out.extend(extras.iter().map(extra));
            }
        } else {
            out.push(self.cmd_menu_item("nav.open"));
            let tab = self.tab();
            let single_file = items.len() == 1 && tab.targets().first().is_some_and(|&e| !tab.listing.entries[e].is_dir());
            if single_file {
                out.push(MenuItem::new(tr!("開啟方式…"), Action::Extra(MENU_OPEN_WITH, items[0].clone())));
            }
            out.push(MenuItem::separator());
            out.extend(self.menu_entries(FILE_ITEMS));
            if !extras.is_empty() {
                out.push(MenuItem::separator());
                out.extend(extras.iter().map(extra));
            }
        }
        self.finish_compact_menu(out, items, folder, self.cmd_menu_item("file.properties"))
    }

    /// 第一層選單共用的結尾：第三方擴充功能（預先準備的對象相同才放）、內容、「顯示更多選項」
    fn finish_compact_menu(&self, mut out: Vec<MenuItem>, items: &[String], folder: &str, properties: MenuItem) -> Vec<MenuItem> {
        let mut hidden = Vec::new();
        if let Some(m) = self.menu_exts.as_ref().filter(|m| m.items == items && m.folder == folder) {
            if !m.menus.is_empty() {
                out.push(MenuItem::separator());
            }
            for (k, menu) in m.menus.iter().enumerate() {
                out.extend(menu.items.iter().map(|it| ext_item(k, it)));
            }
            hidden = m.hidden.clone();
        }
        out.push(MenuItem::separator());
        out.push(properties);
        out.push(MenuItem::separator());
        out.push(MenuItem { key: Some("Shift+".to_string() + tr!("右鍵")), ..MenuItem::new(tr!("顯示更多選項"), Action::ShellMenu) });
        if !hidden.is_empty() {
            let names = hidden.join(tr!("、"));
            out.push(MenuItem { enabled: false, ..MenuItem::new(tr!("較慢的項目在「顯示更多選項」中：{names}", names), Action::None) });
        }
        out
    }

    /// 在樹狀列的節點按右鍵時選單的對象：資料夾、磁碟機、我的最愛（「我的最愛」「此電腦」標題列沒有）。
    /// 游標停住時預先準備的也是它
    pub fn tree_menu_target(&self, i: usize) -> Option<MenuTarget> {
        let n = self.tree.nodes.get(i)?;
        match n.kind {
            NodeKind::Folder | NodeKind::Drive | NodeKind::Favorite => {
                Some(MenuTarget { items: vec![n.path.clone()], folder: zing_core::path::parent(&n.path).unwrap_or_default() })
            }
            NodeKind::FavRoot | NodeKind::Pc => None,
        }
    }

    /// 樹狀列的右鍵選單；`x`、`y` 是視窗內的座標（DIP），`pt` 是螢幕座標
    pub fn open_tree_menu(&mut self, i: usize, x: f32, y: f32, pt: POINT) {
        let Some(n) = self.tree.nodes.get(i) else { return };
        let (kind, path) = (n.kind, n.path.clone());
        if kind == NodeKind::FavRoot {
            let cur = self.tab().path.clone();
            if !cur.is_empty() && !self.is_favorite(&cur) {
                self.open_menu_at(vec![MenuItem::new(tr!("將目前資料夾加入我的最愛"), Action::Extra(MENU_FAV_ADD, cur))], x, y);
            }
            return;
        }
        let Some(MenuTarget { items, folder }) = self.tree_menu_target(i) else { return };
        let mut extras = self.folder_extras(&path);
        if kind == NodeKind::Favorite {
            let k = self.favorites.iter().position(|f| f.eq_ignore_ascii_case(&path)).unwrap_or(0);
            if k > 0 {
                extras.push((MENU_FAV_UP, tr!("我的最愛中上移").into()));
            }
            if k + 1 < self.favorites.len() {
                extras.push((MENU_FAV_DOWN, tr!("我的最愛中下移").into()));
            }
        }
        if self.full_menu_first() {
            return self.show_context_menu(items, folder, pt, extras, path);
        }
        let item = |id: u32, label: &str| MenuItem::new(label, Action::Extra(id, path.clone()));
        let mut out = vec![item(MENU_TREE_OPEN, tr!("開啟"))];
        // 「在新分頁開啟」在 extras 的第一項，放在「開啟」下方
        let (new_tab, rest) = extras.split_first().map_or((None, &extras[..]), |(f, r)| (Some(f), r));
        out.extend(new_tab.map(|(id, label)| item(*id, label)));
        out.push(MenuItem::separator());
        out.push(item(MENU_TREE_CUT, tr!("剪下")));
        out.push(item(MENU_TREE_COPY, tr!("複製")));
        // 我的最愛是捷徑、磁碟機不能刪除或改名；刪掉開著的資料夾時分頁移到上層（`on_folder_gone`）
        if kind == NodeKind::Folder {
            out.push(item(MENU_TREE_RENAME, tr!("重新命名")));
            out.push(item(MENU_TREE_DELETE, tr!("刪除")));
        }
        out.push(item(MENU_TREE_COPYPATH, tr!("複製路徑")));
        if !rest.is_empty() {
            out.push(MenuItem::separator());
            out.extend(rest.iter().map(|(id, label)| item(*id, label)));
        }
        let menu = self.finish_compact_menu(out, &items, &folder, item(MENU_TREE_PROPERTIES, tr!("內容")));
        self.pending_shell_menu = Some(PendingShellMenu { items, folder, pt, extras, target: path });
        self.open_menu_at(menu, x, y);
    }

    /// 精簡選單中第三方擴充功能的項目：交給 ShellHost 以同一個物件執行
    pub fn invoke_menu_ext(&mut self, ext: usize, id: u32) {
        let Some(p) = self.pending_shell_menu.as_ref() else { return };
        let (items, folder, pt) = (p.items.clone(), p.folder.clone(), p.pt);
        self.shell_host.submit(crate::shellhost::ShellJob::InvokeExt { items, folder, ext, id, pt });
    }

    /// 「顯示更多選項」：在同一個位置開啟完整的 Windows 選單
    pub fn show_full_context_menu(&mut self) {
        let Some(p) = self.pending_shell_menu.take() else { return };
        self.show_context_menu(p.items, p.folder, p.pt, p.extras, p.target);
    }

    /// 第一層選單中程式自己的項目
    pub fn on_compact_extra(&mut self, id: u32, target: String) {
        match id {
            MENU_OPEN_WITH => crate::services::launch(&self.notifier, target, None, None, Some("openas"), crate::services::Show::Normal),
            MENU_TREE_OPEN => self.navigate(self.active_pane, target, true),
            MENU_TREE_CUT | MENU_TREE_COPY => {
                self.shell_host.submit(ShellJob::SetClipboard { paths: vec![target], cut: id == MENU_TREE_CUT });
            }
            MENU_TREE_COPYPATH => self.shell_host.submit(ShellJob::SetText { text: target, status: tr!("已複製路徑").into(), reply: None }),
            MENU_TREE_PROPERTIES => self.show_properties(vec![target]),
            MENU_TREE_RENAME => {
                if let Some(i) = self.tree.find(&target) {
                    crate::window::begin_tree_rename(self, i);
                }
            }
            MENU_TREE_DELETE => {
                let permanent = crate::input::Mods::current().shift;
                self.submit_op(crate::shell::FileOp::Delete { paths: vec![target], permanent });
            }
            _ => self.on_menu_extra(id, target),
        }
    }
}

fn ext_item(ext: usize, it: &ExtItem) -> MenuItem {
    if it.separator {
        return MenuItem::separator();
    }
    let action = if it.sub.is_empty() { Action::ShellExt(ext, it.id) } else { Action::None };
    MenuItem { enabled: it.enabled, sub: it.sub.iter().map(|s| ext_item(ext, s)).collect(), ..MenuItem::new(it.label.clone(), action) }
}
