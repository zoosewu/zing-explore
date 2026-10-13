//! 滑鼠與鍵盤輸入。

use fx_core::keymap::KeyChord;
use fx_core::path;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_MENU,
    VK_NEXT, VK_PRIOR, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP,
};

use crate::app::{App, Focus, MENU_FAV_ADD, Modal, Press};
use crate::model::NodeKind;
use crate::view::{COLUMN_KEYS, Hit, clamp_col};

#[derive(Clone, Copy, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Mods {
    pub fn current() -> Self {
        let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
        Self { ctrl: down(VK_CONTROL.0), shift: down(VK_SHIFT.0), alt: down(VK_MENU.0) }
    }
}

const DRAG_THRESHOLD: f32 = 4.0;
const NAV_COMMANDS: [&str; 4] = ["nav.back", "nav.forward", "nav.up", "nav.refresh"];

impl App {
    fn fresh_hit(&mut self, x: f32, y: f32) -> Hit {
        self.layout = self.compute_layout();
        self.hit_test(&self.layout, x, y)
    }

    pub fn on_mouse_move(&mut self, x: f32, y: f32) {
        match self.press {
            Some(Press::Row { pane, x: x0, y: y0, .. }) => {
                if (x - x0).abs() > DRAG_THRESHOLD || (y - y0).abs() > DRAG_THRESHOLD {
                    let tab = self.panes[pane].tab();
                    let paths = tab.target_paths();
                    self.press = None;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    if !paths.is_empty() && !tab.path.is_empty() {
                        self.modal = Some(Modal::DragOut { paths });
                    }
                }
            }
            Some(Press::Scroll { pane, grab }) => {
                if let Some(pl) = self.layout.panes.get(pane) {
                    let list_h = pl.list.h();
                    let max = self.max_scroll(pane, list_h);
                    let track = pl.scrollbar.inset(2.0, 2.0);
                    let thumb_h = (track.h() * list_h / (list_h + max)).max(24.0);
                    let frac = ((y - grab - track.t) / (track.h() - thumb_h).max(1.0)).clamp(0.0, 1.0);
                    self.panes[pane].tab_mut().scroll = (frac * max).round();
                    self.invalidate();
                }
            }
            Some(Press::ColResize { col, start_x, start_w }) => {
                self.cols[col] = clamp_col(start_w - (x - start_x));
                self.invalidate();
            }
            Some(Press::TreeSplit { start_x, start_w }) => {
                let dx = x - start_x;
                let right = self.tree_side == fx_core::config::TreeSide::Right;
                self.tree_width = (if right { start_w - dx } else { start_w + dx }).clamp(120.0, 800.0);
                self.invalidate();
            }
            None => {
                let hit = self.hit_test(&self.layout, x, y);
                if hit != self.hover {
                    self.hover = hit;
                    self.invalidate();
                }
            }
        }
    }

    pub fn on_mouse_leave(&mut self) {
        if self.hover != Hit::None {
            self.hover = Hit::None;
            self.invalidate();
        }
    }

    pub fn on_lbutton_down(&mut self, x: f32, y: f32) {
        let mods = Mods::current();
        let hit = self.fresh_hit(x, y);
        match hit {
            Hit::Nav(i) => self.exec(NAV_COMMANDS[i]),
            Hit::Crumb(p) => {
                let pane = self.active_pane;
                self.navigate(pane, p, true);
            }
            Hit::Address => crate::window::begin_address_edit(self),
            Hit::FilterChip => self.set_filter(String::new()),
            Hit::Button(i) => {
                let cmd = self.config.buttons[i].cmd.clone();
                self.exec(&cmd);
            }
            Hit::TreeArrow(i) => {
                self.focus = Focus::Tree;
                self.tree.focus = i;
                self.toggle_tree_node(i);
            }
            Hit::TreeRow(i) => {
                self.focus = Focus::Tree;
                self.tree.focus = i;
                self.open_tree_node(i);
            }
            Hit::TreeEmpty => self.focus = Focus::Tree,
            Hit::TreeSplitter => {
                self.press = Some(Press::TreeSplit { start_x: x, start_w: self.tree_width });
            }
            Hit::Tab(p, t) => {
                self.active_pane = p;
                self.panes[p].active = t;
                self.focus = Focus::List;
                self.sync_tree();
            }
            Hit::TabClose(p, t) => self.close_tab(p, t),
            Hit::TabNew(p) => {
                self.active_pane = p;
                self.exec("tab.new");
            }
            Hit::TabStrip(p) => self.active_pane = p,
            Hit::Header(p, c) => {
                self.active_pane = p;
                self.set_sort(COLUMN_KEYS[c], None);
            }
            Hit::HeaderEdge(_, c) => {
                self.press = Some(Press::ColResize { col: c, start_x: x, start_w: self.cols[c] });
            }
            Hit::Row(p, i) => {
                self.activate_pane(p);
                let tab = self.panes[p].tab_mut();
                let mut deselect_on_up = false;
                if mods.shift {
                    tab.select_range(i, mods.ctrl);
                } else if mods.ctrl {
                    tab.toggle(i);
                } else if tab.is_selected_view(i) {
                    // 保留多選以便拖曳；放開滑鼠且沒有拖曳時才改成單選
                    tab.focus = i;
                    tab.anchor = i;
                    deselect_on_up = true;
                } else {
                    tab.select_only(i);
                }
                self.press = Some(Press::Row { pane: p, row: i, x, y, deselect_on_up });
            }
            Hit::ListEmpty(p) => {
                self.activate_pane(p);
                if !mods.ctrl {
                    self.panes[p].tab_mut().clear_selection();
                }
            }
            Hit::Scrollbar(p) => {
                self.activate_pane(p);
                let pl = &self.layout.panes[p];
                let list_h = pl.list.h();
                let max = self.max_scroll(p, list_h);
                let track = pl.scrollbar.inset(2.0, 2.0);
                let thumb_h = (track.h() * list_h / (list_h + max)).max(24.0);
                let tab = self.panes[p].tab_mut();
                let thumb_t = track.t + (track.h() - thumb_h) * (tab.scroll / max);
                let grab = if y >= thumb_t && y < thumb_t + thumb_h { y - thumb_t } else { thumb_h / 2.0 };
                self.press = Some(Press::Scroll { pane: p, grab });
                self.on_mouse_move(x, y);
            }
            Hit::None => {}
        }
        if self.press.is_some() {
            unsafe { SetCapture(self.hwnd) };
        }
        self.invalidate();
    }

    fn activate_pane(&mut self, p: usize) {
        if self.active_pane != p {
            self.active_pane = p;
            self.sync_tree();
        }
        self.focus = Focus::List;
    }

    pub fn on_lbutton_up(&mut self) {
        if let Some(Press::Row { pane, row, deselect_on_up: true, .. }) = self.press {
            self.panes[pane].tab_mut().select_only(row);
        }
        self.end_press();
    }

    pub fn end_press(&mut self) {
        if self.press.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
            self.invalidate();
        }
    }

    pub fn on_double_click(&mut self, x: f32, y: f32) {
        match self.fresh_hit(x, y) {
            Hit::Row(p, i) => {
                self.activate_pane(p);
                self.panes[p].tab_mut().focus = i;
                self.open_focused();
            }
            Hit::TreeRow(i) | Hit::TreeArrow(i) => self.toggle_tree_node(i),
            Hit::TabStrip(p) => {
                self.active_pane = p;
                self.exec("tab.new");
            }
            Hit::ListEmpty(_) => self.go_up(),
            _ => self.on_lbutton_down(x, y),
        }
    }

    pub fn on_rbutton_up(&mut self, x: f32, y: f32, screen: POINT) {
        match self.fresh_hit(x, y) {
            Hit::Row(p, i) => {
                self.activate_pane(p);
                let tab = self.panes[p].tab_mut();
                if !tab.is_selected_view(i) {
                    tab.select_only(i);
                }
                tab.focus = i;
                let (items, folder) = (tab.target_paths(), tab.path.clone());
                let (extras, target) = self.entry_menu_extras(&items, &folder);
                self.paint();
                self.modal = Some(Modal::ContextMenu { items, folder, pt: screen, extras, target });
            }
            Hit::ListEmpty(p) => {
                self.activate_pane(p);
                self.panes[p].tab_mut().clear_selection();
                let folder = self.panes[p].tab().path.clone();
                let (extras, target) = self.entry_menu_extras(&[], &folder);
                self.paint();
                self.modal = Some(Modal::ContextMenu { items: Vec::new(), folder, pt: screen, extras, target });
            }
            Hit::TreeRow(i) | Hit::TreeArrow(i) => {
                let n = &self.tree.nodes[i];
                match n.kind {
                    NodeKind::FavRoot => {
                        let cur = self.tab().path.clone();
                        if !cur.is_empty() && !self.is_favorite(&cur) {
                            let extras = vec![(MENU_FAV_ADD, "將目前資料夾加入我的最愛".to_string())];
                            self.modal =
                                Some(Modal::ContextMenu { items: Vec::new(), folder: String::new(), pt: screen, extras, target: cur });
                        }
                    }
                    NodeKind::Pc => {}
                    _ => {
                        let target = n.path.clone();
                        let folder = path::parent(&target).unwrap_or_default();
                        let extras = self.folder_extras(&target);
                        self.modal = Some(Modal::ContextMenu { items: vec![target.clone()], folder, pt: screen, extras, target });
                    }
                }
            }
            _ => {}
        }
        self.invalidate();
    }

    pub fn on_mbutton_up(&mut self, x: f32, y: f32) {
        match self.fresh_hit(x, y) {
            Hit::Tab(p, t) | Hit::TabClose(p, t) => self.close_tab(p, t),
            Hit::Row(p, i) => {
                let tab = self.panes[p].tab();
                let e = tab.view[i] as usize;
                if tab.listing.entries[e].is_dir() {
                    let target = tab.full_path(e);
                    self.active_pane = p;
                    self.open_in_new_tab(target);
                }
            }
            Hit::TreeRow(i) => {
                let target = self.tree.nodes[i].path.clone();
                if !target.is_empty() {
                    self.open_in_new_tab(target);
                }
            }
            _ => {}
        }
    }

    /// `delta`：WHEEL_DELTA（120）的倍數，正值往上
    pub fn on_wheel(&mut self, x: f32, y: f32, delta: i16) {
        let lines = 3.0 * delta as f32 / 120.0;
        let step = lines * self.row_h();
        if let Some(tr) = self.layout.tree.filter(|r| r.contains(x, y)) {
            let max = self.tree_max_scroll(tr.h());
            self.tree.scroll = (self.tree.scroll - step).clamp(0.0, max);
            self.invalidate();
            return;
        }
        let target = self.layout.panes.iter().position(|pl| pl.area.contains(x, y));
        if let Some(p) = target {
            let list_h = self.layout.panes[p].list.h();
            let max = self.max_scroll(p, list_h);
            let tab = self.panes[p].tab_mut();
            tab.scroll = (tab.scroll - step).clamp(0.0, max);
            self.invalidate();
        }
    }

    /// 回傳 true 表示已處理
    pub fn on_key(&mut self, vk: u16, mods: Mods) -> bool {
        self.suppress_char = false;
        const VK_BACK: u16 = 0x08;
        let plain = !mods.ctrl && !mods.alt && !mods.shift;

        // 篩選字串不為空時，Backspace 刪字、Esc 清除，優先於快速鍵
        if self.focus == Focus::List && plain && !self.tab().filter.is_empty() {
            if vk == VK_BACK {
                let mut f = self.tab().filter.clone();
                f.pop();
                self.set_filter(f);
                return true;
            }
            if vk == VK_ESCAPE.0 {
                self.set_filter(String::new());
                return true;
            }
        }

        let chord = KeyChord { ctrl: mods.ctrl, shift: mods.shift, alt: mods.alt, vk };
        if let Some(cmd) = self.config.command_for(chord).map(str::to_string) {
            self.exec(&cmd);
            self.suppress_char = true;
            return true;
        }

        match self.focus {
            Focus::List => self.list_key(vk, mods),
            Focus::Tree => self.tree_key(vk),
        }
    }

    fn list_key(&mut self, vk: u16, mods: Mods) -> bool {
        let p = self.active_pane;
        let page = self.layout.panes.get(p).map_or(10, |pl| (pl.list.h() / self.row_h()).floor().max(1.0) as usize);
        let tab = self.panes[p].tab_mut();
        let n = tab.view.len();
        if n == 0 && vk != VK_ESCAPE.0 {
            return false;
        }
        let last = n.saturating_sub(1);
        let target = match vk {
            v if v == VK_UP.0 => Some(tab.focus.saturating_sub(1)),
            v if v == VK_DOWN.0 => Some((tab.focus + 1).min(last)),
            v if v == VK_PRIOR.0 => Some(tab.focus.saturating_sub(page)),
            v if v == VK_NEXT.0 => Some((tab.focus + page).min(last)),
            v if v == VK_HOME.0 => Some(0),
            v if v == VK_END.0 => Some(last),
            v if v == VK_SPACE.0 && tab.filter.is_empty() => {
                let f = tab.focus;
                if mods.ctrl { tab.toggle(f) } else { tab.select_only(f) }
                self.suppress_char = true;
                self.invalidate();
                return true;
            }
            v if v == VK_ESCAPE.0 => {
                tab.clear_selection();
                self.invalidate();
                return true;
            }
            _ => None,
        };
        let Some(t) = target else { return false };
        if mods.shift {
            tab.select_range(t, mods.ctrl);
        } else if mods.ctrl {
            tab.focus = t;
        } else {
            tab.select_only(t);
        }
        self.ensure_visible(p);
        self.invalidate();
        true
    }

    fn tree_key(&mut self, vk: u16) -> bool {
        let n = self.tree.nodes.len();
        if n == 0 {
            return false;
        }
        let i = self.tree.focus.min(n - 1);
        match vk {
            v if v == VK_UP.0 => self.tree.focus = i.saturating_sub(1),
            v if v == VK_DOWN.0 => self.tree.focus = (i + 1).min(n - 1),
            v if v == VK_RIGHT.0 => {
                if self.tree.nodes[i].expanded {
                    self.tree.focus = (i + 1).min(n - 1);
                } else {
                    self.toggle_tree_node(i);
                }
            }
            v if v == VK_LEFT.0 => {
                if self.tree.nodes[i].expanded {
                    self.toggle_tree_node(i);
                } else {
                    let d = self.tree.nodes[i].depth;
                    if let Some(parent) = self.tree.nodes[..i].iter().rposition(|x| x.depth < d) {
                        self.tree.focus = parent;
                    }
                }
            }
            v if v == VK_SPACE.0 => {
                self.open_tree_node(i);
                self.suppress_char = true;
            }
            v if v == VK_ESCAPE.0 => self.focus = Focus::List,
            _ => return false,
        }
        let f = self.tree.focus;
        self.ensure_tree_visible(f);
        self.invalidate();
        true
    }

    pub fn on_char(&mut self, ch: u16) {
        if std::mem::take(&mut self.suppress_char) {
            return;
        }
        if self.focus != Focus::List || ch < 0x20 || ch == 0x7F || (0xD800..0xE000).contains(&ch) {
            return;
        }
        let c = char::from_u32(ch as u32).unwrap_or(' ');
        if c == ' ' && self.tab().filter.is_empty() {
            return;
        }
        let mut f = self.tab().filter.clone();
        f.push(c);
        self.set_filter(f);
    }

    /// 拖放進來時的目標資料夾（螢幕座標轉成 DIP 後由呼叫端傳入）
    pub fn drop_target_at(&mut self, x: f32, y: f32) -> Option<String> {
        let hit = self.fresh_hit(x, y);
        let (hover, folder) = match hit {
            Hit::Row(p, i) => {
                let tab = self.panes[p].tab();
                let e = tab.view[i] as usize;
                if tab.listing.entries[e].is_dir() {
                    (Some((p, Some(i))), Some(tab.full_path(e)))
                } else {
                    (Some((p, None)), Some(tab.path.clone()))
                }
            }
            Hit::ListEmpty(p) | Hit::Header(p, _) | Hit::Tab(p, _) | Hit::Scrollbar(p) => {
                (Some((p, None)), Some(self.panes[p].tab().path.clone()))
            }
            Hit::TreeRow(i) | Hit::TreeArrow(i) => (None, Some(self.tree.nodes[i].path.clone())),
            Hit::Crumb(p) => (None, Some(p)),
            _ => (None, None),
        };
        let folder = folder.filter(|f| !f.is_empty());
        let hover = if folder.is_some() { hover } else { None };
        if hover != self.drop_hover {
            self.drop_hover = hover;
            self.invalidate();
        }
        folder
    }
}
