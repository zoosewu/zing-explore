//! 滑鼠與鍵盤輸入。

use zing_core::keymap::KeyChord;
use zing_core::path;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_LCONTROL, VK_RCONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_MENU,
    VK_NEXT, VK_PRIOR, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP,
};

use crate::app::{App, Focus, MENU_FAV_ADD, MENU_FAV_DOWN, MENU_FAV_UP, Modal, Press};
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
        // 左右兩邊分別也看：輸入法啟用時，右 Ctrl 不一定會反映在 VK_CONTROL
        let ctrl = down(VK_CONTROL.0) || down(VK_LCONTROL.0) || down(VK_RCONTROL.0);
        Self { ctrl, shift: down(VK_SHIFT.0), alt: down(VK_MENU.0) }
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
            Some(Press::ThemeSlider { .. }) => self.theme_slide(x),
            Some(Press::PreviewSplit { start_x, start_w }) => {
                // 預覽窗格在右側：往左拖變寬
                self.preview.width = (start_w - (x - start_x)).clamp(200.0, 900.0);
                self.invalidate();
            }
            Some(Press::TreeSplit { start_x, start_w }) => {
                let dx = x - start_x;
                let right = self.tree_side == zing_core::config::TreeSide::Right;
                self.tree_width = (if right { start_w - dx } else { start_w + dx }).clamp(120.0, 800.0);
                self.invalidate();
            }
            Some(Press::BarEditDrag { .. }) => self.cmdbar_edit_drag(y),
            None => {
                let hit = self.hit_test(&self.layout, x, y);
                if hit != self.hover {
                    if self.menu.is_some() {
                        self.menu_hover(&hit);
                    }
                    self.update_tip(&hit);
                    self.hover = hit;
                    self.schedule_menu_prepare();
                    self.invalidate();
                }
            }
        }
    }

    /// 游標移到清單的另一列（或空白處）：停住 150 ms 後預先準備右鍵選單
    fn schedule_menu_prepare(&self) {
        if self.perf.is_some() || !matches!(self.hover, Hit::Row(..) | Hit::ListEmpty(_)) {
            return;
        }
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), crate::app::TIMER_MENU_PREPARE, 150, None);
        }
    }

    /// 按下右鍵時會用的項目：游標所在列已選取就是所有選取項目，否則只有那一列（與 `on_rbutton_up` 相同）
    pub fn menu_target_at(&self, hit: &Hit) -> Option<(Vec<String>, String)> {
        match *hit {
            Hit::Row(p, i) => {
                let tab = self.panes.get(p)?.tab();
                let items = if tab.is_selected_view(i) { tab.target_paths() } else { vec![tab.full_path(*tab.view.get(i)? as usize)] };
                Some((items, tab.path.clone()))
            }
            Hit::ListEmpty(p) => Some((Vec::new(), self.panes.get(p)?.tab().path.clone())).filter(|(_, f)| !f.is_empty()),
            _ => None,
        }
    }

    pub fn on_menu_prepare_timer(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), crate::app::TIMER_MENU_PREPARE);
        }
        let Some(target) = self.menu_target_at(&self.hover) else { return };
        if self.menu_prepared.as_ref() == Some(&target) {
            return;
        }
        let (items, folder) = target.clone();
        self.menu_prepared = Some(target);
        let (slow, persist) = (crate::shellext::SlowPolicy::new(&self.config.context_menu), !self.isolated);
        self.shell_host.submit(crate::shellhost::ShellJob::Prepare { items, folder, slow, persist });
    }

    pub fn on_mouse_leave(&mut self) {
        self.hide_tip();
        if self.hover != Hit::None {
            self.hover = Hit::None;
            self.invalidate();
        }
    }

    pub fn on_lbutton_down(&mut self, x: f32, y: f32) {
        let mods = Mods::current();
        self.hide_tip();
        self.leave_cmdbar();
        let hit = self.fresh_hit(x, y);
        // 下拉選單開著：點項目執行，點選單外關閉（不傳給底下的元件）
        if self.menu.is_some() {
            self.menu_click(hit);
            return;
        }
        // Lua 的 ask()：點輸入框外視為取消
        if self.ask.is_some() {
            if self.ask_layout().is_some_and(|(card, _, _)| !card.contains(x, y)) {
                self.finish_ask(false, None);
            }
            return;
        }
        // 批次重新命名面板：點面板外取消
        if self.batch.is_some() {
            self.batch_click(hit);
            return;
        }
        // 命令面板開啟時：點項目執行，點面板外關閉（不傳給底下的元件）
        if self.palette.is_some() {
            match hit {
                Hit::PaletteRow(i) => self.run_palette(Some(i)),
                Hit::Palette => {}
                _ => self.close_palette(),
            }
            return;
        }
        if hit.side_panel() {
            self.side_click(hit, x, y);
            return;
        }
        match hit {
            Hit::Nav(i) => self.exec(NAV_COMMANDS[i]),
            Hit::Crumb(p) => {
                let pane = self.active_pane;
                self.navigate(pane, p, true);
            }
            Hit::Address => crate::window::begin_address_edit(self),
            Hit::FilterChip => self.set_filter(String::new()),
            Hit::CmdButton(k) => {
                if let Some(&(i, r, _)) = self.layout.cmdbar.as_ref().and_then(|b| b.buttons.get(k)) {
                    self.cmdbar_click(i, r);
                }
            }
            Hit::CmdBar | Hit::MenuItem(..) | Hit::Menu => {}
            Hit::SettingsGear => {
                if self.settings_page().is_some() {
                    self.close_side_panels();
                } else {
                    self.open_settings();
                }
            }
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
            Hit::Preview => {}
            // 批次重新命名面板開啟時在函式開頭處理
            Hit::BatchChip(_) | Hit::BatchField(_) | Hit::BatchRun | Hit::BatchCancel | Hit::Batch => {}
            Hit::PreviewSplitter => {
                self.press = Some(Press::PreviewSplit { start_x: x, start_w: self.preview.width });
            }
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
            Hit::JobPause(id) => self.job_toggle_pause(id),
            Hit::JobCancel(id) => self.job_cancel(id),
            Hit::JobsClear => self.jobs_clear(),
            Hit::JobsClose | Hit::JobsIndicator => self.jobs_toggle(),
            Hit::None | Hit::PaletteRow(_) | Hit::Palette | Hit::JobsPanel => {}
            // 右側面板的命中在 match 之前已交給 side_click
            _ => {}
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
        if self.batch.is_some() {
            return;
        }
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
        if self.batch.is_some() {
            return;
        }
        self.hide_tip();
        let hit = self.fresh_hit(x, y);
        if self.menu.is_some() {
            self.menu_right_click(hit, x, y);
            return;
        }
        match hit {
            Hit::CmdButton(k) => {
                let i = self.layout.cmdbar.as_ref().and_then(|b| b.buttons.get(k)).map(|b| b.0);
                self.cmdbar_right_click(i, x, y);
            }
            Hit::CmdBar => self.cmdbar_right_click(None, x, y),
            Hit::Row(p, i) => {
                self.activate_pane(p);
                let tab = self.panes[p].tab_mut();
                if !tab.is_selected_view(i) {
                    tab.select_only(i);
                }
                tab.focus = i;
                let (items, folder) = (tab.target_paths(), tab.path.clone());
                self.paint();
                self.open_context_menu(items, folder, x, y, screen);
            }
            Hit::ListEmpty(p) => {
                self.activate_pane(p);
                self.panes[p].tab_mut().clear_selection();
                let folder = self.panes[p].tab().path.clone();
                self.paint();
                self.open_context_menu(Vec::new(), folder, x, y, screen);
            }
            Hit::TreeRow(i) | Hit::TreeArrow(i) => {
                let n = &self.tree.nodes[i];
                match n.kind {
                    NodeKind::FavRoot => {
                        let cur = self.tab().path.clone();
                        if !cur.is_empty() && !self.is_favorite(&cur) {
                            let extras = vec![(MENU_FAV_ADD, tr!("將目前資料夾加入我的最愛").to_string())];
                            self.show_context_menu(Vec::new(), String::new(), screen, extras, cur);
                        }
                    }
                    NodeKind::Pc => {}
                    kind => {
                        let target = n.path.clone();
                        let folder = path::parent(&target).unwrap_or_default();
                        let mut extras = self.folder_extras(&target);
                        if kind == NodeKind::Favorite {
                            let k = self.favorites.iter().position(|f| f.eq_ignore_ascii_case(&target)).unwrap_or(0);
                            if k > 0 {
                                extras.push((MENU_FAV_UP, tr!("我的最愛中上移").into()));
                            }
                            if k + 1 < self.favorites.len() {
                                extras.push((MENU_FAV_DOWN, tr!("我的最愛中下移").into()));
                            }
                        }
                        self.show_context_menu(vec![target.clone()], folder, screen, extras, target);
                    }
                }
            }
            _ => {}
        }
        self.invalidate();
    }

    pub fn on_mbutton_up(&mut self, x: f32, y: f32) {
        if self.batch.is_some() {
            return;
        }
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
    pub fn on_wheel(&mut self, x: f32, y: f32, delta: i16, ctrl: bool) {
        self.hide_tip();
        if self.menu.is_some() {
            self.close_menu();
            return;
        }
        if self.batch.is_some() {
            self.batch_command(crate::batch::CMD_SCROLL, -3 * delta as isize / 120);
            return;
        }
        if self.palette_contains(x, y) {
            self.palette_move(-3 * delta as isize / 120);
            return;
        }
        if let Some(sp) = self.layout.side_panel.filter(|r| r.contains(x, y)) {
            self.side_wheel(sp, x, y, delta);
            return;
        }
        if ctrl && let Some(p) = self.layout.panes.iter().position(|pl| pl.list.contains(x, y)) {
            let notches = (delta as i32 / 120).signum();
            self.zoom_thumbs(p, notches * crate::views::THUMB_STEP);
            return;
        }
        let lines = 3.0 * delta as f32 / 120.0;
        let step = lines * self.row_h();
        if let Some(pr) = self.layout.preview.filter(|r| r.contains(x, y)) {
            let (_, content, _) = self.preview_rects(pr);
            let max = self.preview_max_scroll(content.h());
            self.preview.scroll = (self.preview.scroll - lines * self.caption_line_h()).clamp(0.0, max);
            self.invalidate();
            return;
        }
        if let Some(tr) = self.layout.tree.filter(|r| r.contains(x, y)) {
            let max = self.tree_max_scroll(tr.h());
            self.tree.scroll = (self.tree.scroll - step).clamp(0.0, max);
            self.invalidate();
            return;
        }
        let target = self.layout.panes.iter().position(|pl| pl.area.contains(x, y));
        if let Some(p) = target {
            let list = self.layout.panes[p].list;
            let max = self.max_scroll(p, list.h());
            // 縮圖：每格滾輪捲動約 3/4 格高
            let step = self.grid(p, list).map_or(step, |g| delta as f32 / 120.0 * g.cell_h * 0.75);
            let tab = self.panes[p].tab_mut();
            tab.scroll = (tab.scroll - step).clamp(0.0, max);
            self.invalidate();
        }
    }

    /// 回傳 true 表示已處理
    pub fn on_key(&mut self, vk: u16, mods: Mods) -> bool {
        self.suppress_char = false;
        self.hide_tip();
        if self.menu.is_some() {
            self.suppress_char = true;
            return self.menu_key(vk);
        }
        // 鍵盤操作命令列中（F10）
        if self.bar_focus.is_some() && self.cmdbar_key(vk) {
            self.suppress_char = true;
            return true;
        }
        // 快速鍵編輯面板正在錄製或依按鍵搜尋：按鍵交給面板
        if self.keys_capturing() {
            self.suppress_char = true;
            self.keys_key(vk, mods);
            return true;
        }
        // 批次重新命名面板開啟、但焦點不在它的輸入框（例如點了面板後）時
        if self.batch.is_some() {
            match vk {
                0x1B => self.close_batch(),
                0x0D => self.run_batch(),
                0x09 => self.batch_command(crate::batch::CMD_TAB, 0),
                _ => {}
            }
            self.suppress_char = true;
            return true;
        }
        const VK_BACK: u16 = 0x08;
        let plain = !mods.ctrl && !mods.alt && !mods.shift;

        // 右側面板：Esc 收起展開的選項、回上一層或關閉面板（篩選與選取之前）
        if plain && vk == VK_ESCAPE.0 && self.side_escape() {
            return true;
        }

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
        // F10 移到命令列（與 Windows 的選單列相同；使用者把 F10 指定給命令時以命令為準）
        if plain && vk == 0x79 && self.config.command_bar.visible && self.config.command_for(chord).is_none() {
            self.cmdbar_focus_toggle();
            self.suppress_char = true;
            return true;
        }
        if let Some(cmd) = self.config.command_for(chord).map(str::to_string) {
            self.exec(&cmd);
            self.suppress_char = true;
            return true;
        }

        match self.focus {
            Focus::List => self.list_key(vk, mods),
            Focus::Tree => self.tree_key(vk, mods),
        }
    }

    fn list_key(&mut self, vk: u16, mods: Mods) -> bool {
        let p = self.active_pane;
        let list = self.layout.panes.get(p).map(|pl| pl.list);
        let grid = list.and_then(|l| self.grid(p, l));
        // 縮圖：上下一列是 cols 項，一頁是可見列數 × cols
        let cols = grid.map_or(1, |g| g.cols);
        let line_h = grid.map_or(self.row_h(), |g| g.cell_h);
        let page = list.map_or(10, |l| (l.h() / line_h).floor().max(1.0) as usize) * cols;
        let tab = self.panes[p].tab_mut();
        let n = tab.view.len();
        if n == 0 && vk != VK_ESCAPE.0 {
            return false;
        }
        let last = n.saturating_sub(1);
        let target = match vk {
            v if v == VK_UP.0 => Some(tab.focus.saturating_sub(cols)),
            v if v == VK_DOWN.0 => Some((tab.focus + cols).min(last)),
            v if v == VK_LEFT.0 && grid.is_some() => Some(tab.focus.saturating_sub(1)),
            v if v == VK_RIGHT.0 && grid.is_some() => Some((tab.focus + 1).min(last)),
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

    fn tree_key(&mut self, vk: u16, mods: Mods) -> bool {
        let n = self.tree.nodes.len();
        if n == 0 {
            return false;
        }
        let i = self.tree.focus.min(n - 1);
        // Ctrl+↑／↓：調整我的最愛的順序
        if mods.ctrl && (vk == VK_UP.0 || vk == VK_DOWN.0) {
            if self.tree.nodes[i].kind != NodeKind::Favorite {
                return false;
            }
            let p = self.tree.nodes[i].path.clone();
            if let Err(e) = self.move_favorite(&p, if vk == VK_UP.0 { -1 } else { 1 }) {
                self.set_status(e, true);
            }
            return true;
        }
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
