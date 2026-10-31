//! 滑鼠與鍵盤輸入。

use zing_core::keymap::{KeyChord, Scope};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_LCONTROL, VK_RCONTROL, VK_ESCAPE, VK_MENU,
    VK_SHIFT, };

use crate::app::{App, Focus, Modal, Press};
use crate::model::MenuTarget;
use crate::navkeys::PopupKey;
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

/// 清單中的位置：（窗格, 可見的第幾列；空白處為 None）
fn list_spot(hit: &Hit) -> Option<(usize, Option<usize>)> {
    match *hit {
        Hit::Row(p, i) => Some((p, Some(i))),
        Hit::ListEmpty(p) => Some((p, None)),
        _ => None,
    }
}

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

    /// 游標移到清單的另一列（或空白處）、樹狀列的另一個節點：停住 150 ms 後預先準備右鍵選單
    fn schedule_menu_prepare(&self) {
        if self.perf.is_some() || !matches!(self.hover, Hit::Row(..) | Hit::ListEmpty(_) | Hit::TreeRow(_) | Hit::TreeArrow(_)) {
            return;
        }
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), crate::app::TIMER_MENU_PREPARE, 150, None);
        }
    }

    /// 在 `hit` 按右鍵時選單的對象（`Tab::menu_target`、`App::tree_menu_target`）；不是清單或樹狀列就是 None
    pub fn menu_target_at(&self, hit: &Hit) -> Option<MenuTarget> {
        if let Hit::TreeRow(i) | Hit::TreeArrow(i) = *hit {
            return self.tree_menu_target(i);
        }
        let (p, at) = list_spot(hit)?;
        Some(self.panes.get(p)?.tab().menu_target(at))
    }

    pub fn on_menu_prepare_timer(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), crate::app::TIMER_MENU_PREPARE);
        }
        if let Some(target) = self.menu_target_at(&self.hover) {
            self.prepare_menu(target);
        }
    }

    /// 清單或樹狀列有焦點時按了鍵（移動焦點、選取、篩選…）：停住 150 ms 後預先準備鍵盤開啟的右鍵選單
    fn schedule_key_menu_prepare(&self) {
        if self.perf.is_some() || !matches!(self.focus, Focus::List | Focus::Tree) || self.menu.is_some() {
            return;
        }
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), crate::app::TIMER_KEY_MENU_PREPARE, 150, None);
        }
    }

    pub fn on_key_menu_prepare_timer(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), crate::app::TIMER_KEY_MENU_PREPARE);
        }
        // 計時期間焦點可能移走、或選單已經開了
        if self.menu.is_some() {
            return;
        }
        let target = match self.focus {
            Focus::List => Some(self.tab().key_menu_target()),
            Focus::Tree => self.tree_menu_target(self.tree.focus),
            _ => None,
        };
        if let Some(target) = target {
            self.prepare_menu(target);
        }
    }

    /// 送出預先準備（與上次相同就不送）。滑鼠停住、按鍵停住共用；ShellHost 只保留最後一份
    fn prepare_menu(&mut self, target: MenuTarget) {
        // 「此電腦」的空白處用完整的 Windows 選單，不必準備
        if target.items.is_empty() && target.folder.is_empty() {
            return;
        }
        if self.menu_prepared.as_ref() == Some(&target) {
            return;
        }
        let MenuTarget { items, folder } = target.clone();
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
        // 歡迎畫面開著：只有它能點
        if self.welcome.is_some() {
            self.welcome_click(hit);
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
            Hit::UpdateIndicator => {
                if let Some(r) = self.layout.update_indicator {
                    self.open_update_menu(r);
                }
            }
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
        if self.welcome.is_some() {
            return self.on_lbutton_down(x, y);
        }
        if self.batch.is_some() {
            return;
        }
        let hit = self.fresh_hit(x, y);
        // Alt+雙擊：內容（與檔案總管相同），清單與樹狀列都一樣；對象與右鍵選單相同
        if Mods::current().alt {
            let paths = match hit {
                Hit::Row(..) | Hit::ListEmpty(_) => list_spot(&hit).map(|(p, at)| {
                    self.activate_pane(p);
                    let t = self.panes[p].tab().menu_target(at);
                    if t.items.is_empty() { vec![t.folder] } else { t.items }
                }),
                Hit::TreeRow(i) | Hit::TreeArrow(i) => self.tree_menu_target(i).map(|t| t.items),
                _ => None,
            };
            if let Some(paths) = paths.filter(|p| p.iter().all(|s| !s.is_empty())) {
                self.show_properties(paths);
                return;
            }
        }
        match hit {
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
        if self.batch.is_some() || (self.welcome.is_some() && self.menu.is_none()) {
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
            Hit::Row(..) | Hit::ListEmpty(_) => {
                let Some((p, at)) = list_spot(&hit) else { return };
                self.activate_pane(p);
                // 對象與游標停住時預先準備的相同（同一個函式），選取改變之前先算
                let tab = self.panes[p].tab_mut();
                let target = tab.menu_target(at);
                tab.select_for_menu(at);
                self.paint();
                self.open_context_menu(target.items, target.folder, x, y, screen);
            }
            Hit::TreeRow(i) | Hit::TreeArrow(i) => self.open_tree_menu(i, x, y, screen),
            _ => {}
        }
        self.invalidate();
    }

    pub fn on_mbutton_up(&mut self, x: f32, y: f32) {
        if self.batch.is_some() || self.welcome.is_some() {
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
        if self.welcome.is_some() {
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
        // 滾輪的快速鍵（Ctrl+WheelUp 放大縮圖…）：游標所在的清單（先成為作用中的窗格）、樹狀列或全域
        // Ctrl 以滾輪訊息帶的旗標為準（MK_CONTROL），Shift、Alt 讀鍵盤狀態
        let mods = Mods { ctrl, ..Mods::current() };
        let vk = if delta > 0 { zing_core::keymap::WHEEL_UP } else { zing_core::keymap::WHEEL_DOWN };
        let chord = KeyChord { ctrl: mods.ctrl, shift: mods.shift, alt: mods.alt, vk, when: Scope::Global };
        let over_list = self.layout.panes.iter().position(|pl| pl.list.contains(x, y));
        let scope = match over_list {
            Some(_) => Scope::List,
            None if self.layout.tree.is_some_and(|r| r.contains(x, y)) => Scope::Tree,
            None => Scope::Global,
        };
        if let Some(cmd) = self.config.command_in(scope, chord).map(str::to_string) {
            if let Some(p) = over_list {
                self.active_pane = p;
            }
            self.exec(&cmd);
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
        self.schedule_key_menu_prepare();
        let chord = KeyChord { ctrl: mods.ctrl, shift: mods.shift, alt: mods.alt, vk, when: Scope::Global };
        // 「開始篩選」之後按了字元以外的鍵：取消
        if self.filter_armed && !chord.is_printable() {
            self.filter_armed = false;
        }
        // 快速鍵編輯面板正在錄製或依按鍵搜尋：按鍵交給面板（任何按鍵都要錄得到）
        if self.keys_capturing() {
            self.suppress_char = true;
            self.keys_key(vk, mods);
            return true;
        }
        // 彈出清單：下拉選單、歡迎畫面、命令列（F10）。按鍵先找 popup 情境的命令（ui.* 交給 App::ui）；
        // 命令列沒有處理的（例如 Tab）照樣往下找全域的快速鍵
        if self.menu.is_some() || self.welcome.is_some() || self.bar_focus.is_some() {
            match self.popup_key(chord, Scope::Popup) {
                PopupKey::Cmd(cmd) => {
                    if self.popup_command(&cmd) {
                        self.suppress_char = true;
                        return true;
                    }
                    if self.menu.is_some() || self.welcome.is_some() {
                        self.suppress_char = true;
                        return true;
                    }
                }
                PopupKey::Unbound => {
                    self.suppress_char = true;
                    return true;
                }
                PopupKey::Raw => {}
            }
            if self.menu.is_some() {
                self.suppress_char = true;
                return self.menu_key(vk);
            }
            if self.welcome.is_some() {
                self.suppress_char = true;
                return self.welcome_key(vk, mods.shift);
            }
            if self.cmdbar_key(vk) {
                self.suppress_char = true;
                return true;
            }
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
        let plain = !mods.ctrl && !mods.alt && !mods.shift;

        // 鍵盤焦點在右側面板：按鍵先找 popup 情境的命令（快速鍵之前），其他照原樣交給面板；
        // 面板沒有處理的（例如 Tab 回到清單）照樣往下找全域的快速鍵
        if self.focus == Focus::Side {
            match self.popup_key(chord, Scope::Popup) {
                PopupKey::Cmd(cmd) => {
                    if self.popup_command(&cmd) {
                        self.suppress_char = true;
                        return true;
                    }
                }
                PopupKey::Unbound => {
                    self.suppress_char = true;
                    return true;
                }
                PopupKey::Raw => {
                    if self.side_key(vk, mods.ctrl || mods.alt) {
                        self.suppress_char = true;
                        return true;
                    }
                }
            }
        } else if plain && vk == VK_ESCAPE.0 && self.side_escape() {
            // 焦點不在右側面板時，Esc 照樣先收起面板的選項、回上一層或關閉面板（篩選與選取之前）
            self.side_settle();
            return true;
        }

        let scope = self.key_scope();
        // 篩選中：沒有 Ctrl、Alt 的字元鍵是篩選文字（WM_CHAR）；filter 情境自己綁定的按鍵除外
        if scope == Scope::Filter && chord.is_printable() && self.config.command_for(chord.in_scope(Scope::Filter)).is_none() {
            return false;
        }
        if let Some(cmd) = self.config.command_in(scope, chord).map(str::to_string) {
            self.exec(&cmd);
            self.suppress_char = true;
            return true;
        }
        false
    }

    pub fn on_char(&mut self, ch: u16) {
        if std::mem::take(&mut self.suppress_char) {
            return;
        }
        if self.focus != Focus::List || ch < 0x20 || ch == 0x7F || (0xD800..0xE000).contains(&ch) {
            return;
        }
        self.list_typed(char::from_u32(ch as u32).unwrap_or(' '));
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
