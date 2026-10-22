//! 清單的右鍵選單。預設是兩層（同 Windows 11）：先顯示程式自己的常用項目（`menu.rs` 自繪，立即出現、跟著主題），
//! 最後一項「顯示更多選項」才開啟完整的 Windows 選單（`ShellHost`，游標停住時已預先準備，也是立即出現）。
//! Shift+右鍵、設定 `context_menu.style = "full"`、「此電腦」直接顯示完整的 Windows 選單。
//!
//! 第一層的命令與命令列共用（`cmd_menu_item`：快速鍵、不能用的原因）；程式自己的項目（在新分頁開啟、我的最愛、
//! 縮圖、批次重新命名）與完整選單相同（`entry_menu_extras`）。第三方擴充功能（7-Zip…）的項目由 ShellHost 在游標停住時
//! 預先載入（shellext.rs），項目相同才放進來（還沒準備好就不放，選單不會在顯示後才變長）；緩慢而被隱藏的列出名稱。

use windows::Win32::Foundation::POINT;
use zing_core::cmdbar::{self as core, Entry};
use zing_core::config::MenuStyle;

use crate::app::App;
use crate::menu::{Action, MenuItem};
use crate::shellext::ExtItem;

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

/// 「顯示更多選項」要開的完整選單
pub struct PendingShellMenu {
    items: Vec<String>,
    folder: String,
    pt: POINT,
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

impl App {
    /// 顯示清單的右鍵選單；`x`、`y` 是視窗內的座標（DIP），`pt` 是螢幕座標
    pub fn open_context_menu(&mut self, items: Vec<String>, folder: String, x: f32, y: f32, pt: POINT) {
        let shift = crate::input::Mods::current().shift;
        if shift || self.config.context_menu.style == MenuStyle::Full || folder.is_empty() {
            let (extras, target) = self.entry_menu_extras(&items, &folder);
            return self.show_context_menu(items, folder, pt, extras, target);
        }
        let menu = self.compact_menu_items(&items, &folder);
        self.pending_shell_menu = Some(PendingShellMenu { items, folder, pt });
        self.open_menu_at(menu, x, y);
    }

    fn compact_menu_items(&self, items: &[String], folder: &str) -> Vec<MenuItem> {
        let (extras, target) = self.entry_menu_extras(items, folder);
        let extra = |(id, label): &(u32, String)| MenuItem::new(label.clone(), Action::Extra(*id, target.clone()));
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
        out.push(self.cmd_menu_item("file.properties"));
        out.push(MenuItem::separator());
        out.push(MenuItem { key: Some("Shift+".to_string() + tr!("右鍵")), ..MenuItem::new(tr!("顯示更多選項"), Action::ShellMenu) });
        if !hidden.is_empty() {
            let names = hidden.join(tr!("、"));
            out.push(MenuItem { enabled: false, ..MenuItem::new(tr!("較慢的項目在「顯示更多選項」中：{names}", names), Action::None) });
        }
        out
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
        let (extras, target) = self.entry_menu_extras(&p.items, &p.folder);
        self.show_context_menu(p.items, p.folder, p.pt, extras, target);
    }

    /// 第一層選單中程式自己的項目
    pub fn on_compact_extra(&mut self, id: u32, target: String) {
        if id == MENU_OPEN_WITH {
            crate::services::launch(&self.notifier, target, None, None, Some("openas"), crate::services::Show::Normal);
        } else {
            self.on_menu_extra(id, target);
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
