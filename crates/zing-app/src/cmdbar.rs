//! 命令列：工具列下方的按鈕列（Windows 11 檔案總管的樣式）。內容在 `zing_core::cmdbar`。
//!
//! - 不能用的按鈕變灰色而不隱藏（位置固定）；游標停留時顯示名稱、快速鍵與不能用的原因（tooltip.rs）。
//! - 剪貼簿等常用操作只顯示圖示，選單與自訂按鈕顯示圖示＋文字；設定 `command_bar.labels = false` 時全部只顯示圖示。
//! - 視窗變窄：先收起文字，再把右邊的按鈕依序移進「⋯」。
//! - 右鍵：從命令列移除、左移／右移、自訂命令列…；「⋯」中的項目按右鍵可加到命令列。
//! - 圖片圖示在背景讀取（`shell_image`），讀好之前不畫。
//! - 鍵盤（與 Windows 的選單列相同）：F10 移到命令列，←→ 移動（略過不能用的）、Enter／空白鍵／↓ 執行或開啟選單，
//!   選單開著時 ←→ 換到相鄰的按鈕，Esc 回到命令列，再按 Esc、F10 或 Tab 離開。焦點所在的按鈕立即顯示提示。

use zing_core::cmdbar::{self as core, Entry, Icon, Item};

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::menu::{Action, MenuItem};
use crate::services::Msg;
use crate::view::Hit;

/// 命令列上的一個按鈕：`item` 是 `config.cmdbar` 的索引
#[derive(Clone, Debug, Default)]
pub struct BarLayout {
    pub bar: Rect,
    pub buttons: Vec<(usize, Rect, bool)>,
    /// 放不下、移進「⋯」的按鈕
    pub overflow: Vec<usize>,
}

/// 只顯示圖示的命令（Windows 11 檔案總管的剪貼簿按鈕）
fn icon_only(cmd: &str) -> bool {
    matches!(cmd, "file.cut" | "file.copy" | "file.paste" | "file.rename" | "file.delete")
}

impl App {
    pub fn cmdbar_height(&self) -> f32 {
        if self.config.command_bar.visible { (self.theme.metrics.toolbar_height - 2.0).max(28.0) } else { 0.0 }
    }

    /// 按鈕寬度：`label` 為 false 時只有圖示
    fn button_w(&self, it: &Item, label: bool, h: f32) -> f32 {
        if it.is_separator() {
            return 9.0;
        }
        let arrow = if it.is_menu() && it.cmd != core::MENU_MORE { 14.0 } else { 0.0 };
        if !label {
            return h + arrow;
        }
        h + self.gfx.measure(&it.title(), Font::Text) + 8.0 + arrow
    }

    fn wants_label(&self, it: &Item) -> bool {
        self.config.command_bar.labels && !icon_only(&it.cmd) && it.cmd != core::MENU_MORE
    }

    pub fn layout_cmdbar(&self, bar: Rect) -> BarLayout {
        let items = &self.config.cmdbar;
        let h = bar.h() - 6.0;
        let avail = bar.w() - 12.0;
        let widths = |labels: bool| -> Vec<f32> {
            items.iter().map(|it| self.button_w(it, labels && self.wants_label(it), h)).collect()
        };
        let mut labels = true;
        let mut w = widths(true);
        if w.iter().sum::<f32>() + 2.0 * items.len() as f32 > avail {
            labels = false;
            w = widths(false);
        }
        // 還是放不下：「⋯」以外的按鈕從右邊開始移進「⋯」
        let more = items.iter().position(|i| i.cmd == core::MENU_MORE);
        let mut shown: Vec<usize> = (0..items.len()).collect();
        let mut overflow = Vec::new();
        let more_w = if more.is_none() { h } else { 0.0 };
        while shown.iter().map(|&i| w[i] + 2.0).sum::<f32>() + if overflow.is_empty() { 0.0 } else { more_w } > avail {
            let Some(pos) = shown.iter().rposition(|&i| Some(i) != more) else { break };
            overflow.insert(0, shown.remove(pos));
        }
        // 移除後兩端或連續的分隔線不顯示
        let mut out = Vec::new();
        let mut x = bar.l + 6.0;
        let mut last_sep = true;
        for (k, &i) in shown.iter().enumerate() {
            let sep = items[i].is_separator();
            let next_sep = shown.get(k + 1).is_none_or(|&j| items[j].is_separator());
            if sep && (last_sep || next_sep) {
                continue;
            }
            last_sep = sep;
            out.push((i, Rect::new(x, bar.t + 3.0, x + w[i], bar.b - 3.0), labels && self.wants_label(&items[i])));
            x += w[i] + 2.0;
        }
        overflow.retain(|&i| !items[i].is_separator());
        BarLayout { bar, buttons: out, overflow }
    }

    /// 命令現在不能用的原因（None 表示可以用）
    pub fn cmd_disabled(&self, cmd: &str) -> Option<&'static str> {
        let tab = self.tab();
        let in_pc = tab.path.is_empty();
        // 與檔案總管相同：沒有選取（只有焦點）時剪下、複製等按鈕是灰色的
        let has_sel = tab.selection_count().0 > 0;
        match cmd {
            "file.cut" | "file.copy" | "file.delete" | "file.delete --permanent" | "file.rename" | "file.batchrename" | "thumb.set" | "thumb.clear"
                if !has_sel =>
            {
                Some(tr!("請先選取項目"))
            }
            "file.paste" if in_pc => Some(tr!("「此電腦」不能貼上")),
            "file.paste" if !clipboard_has_files() => Some(tr!("剪貼簿上沒有檔案")),
            "file.newfolder" | "file.newfile" if in_pc => Some(tr!("「此電腦」不能建立新項目")),
            "copy --to={dest}" | "copy --move --to={dest}" | "pane.close" | "pane.focusnext" if self.panes.len() < 2 => Some(tr!("只有一個窗格")),
            "nav.back" if tab.back.is_empty() => Some(tr!("沒有上一頁")),
            "nav.forward" if tab.forward.is_empty() => Some(tr!("沒有下一頁")),
            "nav.up" if in_pc => Some(tr!("已在最上層")),
            _ => None,
        }
    }

    /// 選單中有勾選狀態的命令
    fn cmd_checked(&self, cmd: &str) -> bool {
        let tab = self.tab();
        use zing_core::folderview::ViewMode;
        let thumbs = |px: u16| tab.mode == ViewMode::Thumbnails && tab.thumb == px;
        match cmd {
            "view.sort --by=name" => tab.spec.key == zing_core::sort::SortKey::Name,
            "view.sort --by=modified" => tab.spec.key == zing_core::sort::SortKey::Modified,
            "view.sort --by=type" => tab.spec.key == zing_core::sort::SortKey::Type,
            "view.sort --by=size" => tab.spec.key == zing_core::sort::SortKey::Size,
            "view.sort --asc" => tab.spec.ascending,
            "view.sort --desc" => !tab.spec.ascending,
            "view.mode list" => tab.mode == ViewMode::List,
            "view.mode small" => thumbs(64),
            "view.mode medium" => thumbs(128),
            "view.mode large" => thumbs(192),
            "view.preview" => self.preview.visible,
            "tree.toggle" => self.tree_visible && self.tree_side != zing_core::config::TreeSide::Hidden,
            "view.togglehidden" => self.show_hidden,
            _ => false,
        }
    }

    /// 命令的快速鍵說明
    pub fn key_text(&self, cmd: &str) -> Option<String> {
        self.config.key_for(cmd).map(|k| k.to_string())
    }

    fn menu_entries(&self, list: &[Entry]) -> Vec<MenuItem> {
        list.iter()
            .map(|e| match *e {
                Entry::Separator => MenuItem::separator(),
                Entry::Sub(title, sub) => MenuItem { sub: self.menu_entries(sub), ..MenuItem::new(zing_core::i18n::tr(title), Action::None) },
                Entry::Cmd(cmd) => self.cmd_menu_item(cmd),
            })
            .collect()
    }

    fn cmd_menu_item(&self, cmd: &str) -> MenuItem {
        MenuItem {
            key: self.key_text(cmd),
            enabled: self.cmd_disabled(cmd).is_none(),
            checked: self.cmd_checked(cmd),
            icon: core::default_icon(cmd),
            cmd: Some(cmd.to_string()),
            ..MenuItem::new(core::label_for(cmd), Action::Exec(cmd.to_string()))
        }
    }

    /// 點命令列的第 `i` 個項目
    pub fn cmdbar_click(&mut self, i: usize, r: Rect) {
        let Some(it) = self.config.cmdbar.get(i).cloned() else { return };
        if it.is_menu() {
            let mut items = Vec::new();
            if it.cmd == core::MENU_MORE {
                // 放不下的按鈕排在最前面
                let overflow = self.layout.cmdbar.as_ref().map(|b| b.overflow.clone()).unwrap_or_default();
                for k in overflow {
                    let Some(o) = self.config.cmdbar.get(k) else { continue };
                    if o.is_menu() {
                        items.push(MenuItem { sub: self.menu_entries(core::menu(&o.cmd)), ..MenuItem::new(o.title(), Action::None) });
                    } else {
                        items.push(MenuItem { label: o.title(), ..self.cmd_menu_item(&o.cmd) });
                    }
                }
                if !items.is_empty() {
                    items.push(MenuItem::separator());
                }
                // 腳本定義的命令
                let scripts: Vec<MenuItem> = self.scripts.commands.iter().map(|(name, title)| MenuItem {
                    key: self.key_text(name),
                    cmd: Some(name.clone()),
                    icon: core::glyph("script"),
                    ..MenuItem::new(title.clone(), Action::Exec(name.clone()))
                }).collect();
                let mut more = self.menu_entries(core::MORE_MENU);
                if !scripts.is_empty() {
                    let at = more.iter().position(|m| m.separator).unwrap_or(more.len());
                    more.insert(at, MenuItem { sub: scripts, ..MenuItem::new(tr!("腳本"), Action::None) });
                }
                items.extend(more);
            } else {
                items = self.menu_entries(core::menu(&it.cmd));
            }
            self.open_menu(items, r);
            return;
        }
        if self.cmd_disabled(&it.cmd).is_none() {
            self.exec(&it.cmd);
        }
    }

    /// 在命令列按右鍵
    pub fn cmdbar_right_click(&mut self, i: Option<usize>, x: f32, y: f32) {
        let n = self.config.cmdbar.len();
        let mut items = Vec::new();
        if let Some(i) = i {
            items.push(MenuItem::new(tr!("從命令列移除"), Action::BarRemove(i)));
            items.push(MenuItem { enabled: i > 0, ..MenuItem::new(tr!("左移"), Action::BarMove(i, -1)) });
            items.push(MenuItem { enabled: i + 1 < n, ..MenuItem::new(tr!("右移"), Action::BarMove(i, 1)) });
            items.push(MenuItem::separator());
        }
        items.push(MenuItem { icon: core::glyph("settings"), ..MenuItem::new(tr!("自訂命令列…"), Action::Exec("cmdbar.edit".into())) });
        items.push(MenuItem::new(tr!("隱藏命令列"), Action::Exec("cmdbar.hide".into())));
        self.open_menu_at(items, x, y);
    }

    pub fn cmdbar_remove(&mut self, i: usize) {
        if i < self.config.cmdbar.len() {
            self.config.cmdbar.remove(i);
            self.cmdbar_changed();
        }
    }

    pub fn cmdbar_move(&mut self, i: usize, d: isize) {
        let j = i as isize + d;
        if i < self.config.cmdbar.len() && j >= 0 && (j as usize) < self.config.cmdbar.len() {
            self.config.cmdbar.swap(i, j as usize);
            self.cmdbar_changed();
        }
    }

    /// 加在「⋯」前面（沒有「⋯」就加在最後）
    pub fn cmdbar_add(&mut self, cmd: &str) {
        let bar = &mut self.config.cmdbar;
        let at = bar.iter().position(|i| i.cmd == core::MENU_MORE).unwrap_or(bar.len());
        bar.insert(at, Item::new(cmd));
        self.cmdbar_changed();
        self.set_status(tr!("已加到命令列：{name}", name = core::label_for(cmd)), false);
    }

    /// 命令列的按鈕改變：重畫，並在背景寫入 toolbar.toml（隔離模式不寫檔）
    pub fn cmdbar_changed(&mut self) {
        self.invalidate();
        if self.isolated {
            return;
        }
        let text = core::to_toml(&self.config.cmdbar);
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            zing_core::guard::io("儲存命令列");
            if let Err(e) = std::fs::write(crate::settings::toolbar_path(), text) {
                n.status(tr!("無法儲存 toolbar.toml：{e}", e), true);
            }
        });
    }

    /// 圖片圖示：已讀取就回傳，否則在背景讀取（繪製時呼叫）
    pub fn cmdbar_image(&self, path: &str) -> Option<std::rc::Rc<crate::icons::IconPixels>> {
        let mut cache = self.cmdbar_images.borrow_mut();
        match cache.get(path) {
            Some(v) => v.clone(),
            None => {
                cache.insert(path.to_string(), None);
                let (n, p) = (self.notifier.clone(), path.to_string());
                self.pool.spawn(move || {
                    zing_core::guard::io("讀取命令列的圖示");
                    let pixels = crate::thumbs::shell_image(&p, 64);
                    n.send(Msg::CmdBarImage { path: p, pixels });
                });
                None
            }
        }
    }

    pub fn on_cmdbar_image(&mut self, path: String, pixels: Option<crate::icons::IconPixels>) {
        self.cmdbar_images.borrow_mut().insert(path, pixels.map(std::rc::Rc::new));
        self.invalidate();
    }

    /// 畫一個圖示（Fluent 字元或圖片）在 `r` 中
    pub fn paint_item_icon(&self, it: &Item, r: Rect, col: zing_core::theme::Color, opacity: f32) {
        match it.icon() {
            Icon::Glyph(gl) => self.gfx.text(gl, Font::Glyph, r, col),
            Icon::Image(p) => {
                if let Some(px) = self.cmdbar_image(p) {
                    let s = 16.0;
                    let ir = Rect::new(r.l, (r.t + r.b - s) / 2.0, r.l + s, (r.t + r.b + s) / 2.0);
                    self.gfx.keyed_image(p, &px, ir, opacity);
                }
            }
            Icon::None => {}
        }
    }

    /// 鍵盤可以停留的按鈕（命令列索引）：不含分隔線與不能用的
    fn bar_focusable(&self) -> Vec<usize> {
        let Some(bl) = self.layout.cmdbar.as_ref() else { return Vec::new() };
        bl.buttons
            .iter()
            .map(|&(i, _, _)| i)
            .filter(|&i| self.config.cmdbar.get(i).is_some_and(|it| !it.is_separator() && (it.is_menu() || self.cmd_disabled(&it.cmd).is_none())))
            .collect()
    }

    /// F10：進入或離開命令列
    pub fn cmdbar_focus_toggle(&mut self) {
        if self.bar_focus.take().is_none() {
            self.bar_focus = self.bar_focusable().first().copied();
            self.show_bar_tip();
        } else {
            self.hide_tip();
        }
        self.invalidate();
    }

    pub fn leave_cmdbar(&mut self) {
        if self.bar_focus.take().is_some() {
            self.hide_tip();
            self.invalidate();
        }
    }

    /// 移到前（-1）、後（1）一個按鈕，到頭時繞回
    pub fn bar_focus_move(&mut self, d: isize) {
        let list = self.bar_focusable();
        if list.is_empty() {
            return self.leave_cmdbar();
        }
        let n = list.len() as isize;
        let next = match self.bar_focus.and_then(|i| list.iter().position(|&j| j == i)) {
            Some(p) => list[(p as isize + d).rem_euclid(n) as usize],
            None => list[0],
        };
        self.bar_focus = Some(next);
        self.show_bar_tip();
        self.invalidate();
    }

    /// 焦點所在的按鈕：選單就開啟（並選取第一項），命令就執行並離開命令列
    pub fn bar_activate(&mut self) {
        let Some(i) = self.bar_focus else { return };
        let Some(r) = self.layout.cmdbar.as_ref().and_then(|b| b.buttons.iter().find(|(j, _, _)| *j == i)).map(|b| b.1) else { return };
        let menu = self.config.cmdbar.get(i).is_some_and(|it| it.is_menu());
        if !menu {
            self.leave_cmdbar();
        }
        self.cmdbar_click(i, r);
        if menu {
            self.menu_select_first();
        }
    }

    /// 焦點在命令列時的按鍵；不是命令列的按鍵就離開命令列、回傳 false（照常處理）
    pub fn cmdbar_key(&mut self, vk: u16) -> bool {
        match vk {
            0x25 => self.bar_focus_move(-1),
            0x27 => self.bar_focus_move(1),
            0x24 | 0x23 => {
                let list = self.bar_focusable();
                self.bar_focus = if vk == 0x24 { list.first() } else { list.last() }.copied();
                self.show_bar_tip();
                self.invalidate();
            }
            0x0D | 0x20 | 0x28 => self.bar_activate(),
            0x1B | 0x79 | 0x09 => self.leave_cmdbar(),
            _ => {
                self.leave_cmdbar();
                return false;
            }
        }
        true
    }

    fn show_bar_tip(&mut self) {
        let k = self.bar_focus.and_then(|i| self.layout.cmdbar.as_ref()?.buttons.iter().position(|(j, _, _)| *j == i));
        match k {
            Some(k) => self.show_tip_now(&Hit::CmdButton(k)),
            None => self.hide_tip(),
        }
    }

    pub fn paint_cmdbar(&self, lay: &crate::view::Layout) {
        let Some(bl) = lay.cmdbar.as_ref() else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        g.fill(bl.bar, c.surface);
        g.hline(0.0, bl.bar.r, bl.bar.b - 1.0, c.border);
        for (k, &(i, r, label)) in bl.buttons.iter().enumerate() {
            // 版面可能是按鈕改變前算的
            let Some(it) = self.config.cmdbar.get(i) else { continue };
            if it.is_separator() {
                let x = ((r.l + r.r) / 2.0).round();
                g.vline(x, r.t + 4.0, r.b - 4.0, c.border);
                continue;
            }
            let disabled = !it.is_menu() && self.cmd_disabled(&it.cmd).is_some();
            let radius = self.theme.metrics.radius.max(4.0);
            let focused = self.bar_focus == Some(i);
            if (self.hover == Hit::CmdButton(k) || focused) && !disabled {
                g.fill_round(r, radius, c.hover);
            }
            if focused {
                g.stroke_round(r, radius, c.accent, 2.0);
            }
            let col = if disabled { c.text_secondary.with_alpha(110) } else { c.text };
            let icon_r = Rect::new(r.l + (r.h() - 16.0) / 2.0, r.t, r.l + (r.h() + 16.0) / 2.0 + 4.0, r.b);
            self.paint_item_icon(it, icon_r, col, if disabled { 0.45 } else { 1.0 });
            let mut x = r.l + r.h();
            if label {
                let t = it.title();
                g.text(&t, Font::Text, Rect::new(x, r.t, r.r, r.b), col);
                x += g.measure(&t, Font::Text) + 4.0;
            }
            if it.is_menu() && it.cmd != core::MENU_MORE {
                g.text(crate::gfx::glyph::CHEVRON_DOWN_SMALL, Font::GlyphSmall, Rect::new(x, r.t, r.r, r.b), c.text_secondary);
            }
        }
    }
}

/// 剪貼簿上有沒有檔案（不開啟剪貼簿，不會等待）
fn clipboard_has_files() -> bool {
    unsafe { windows::Win32::System::DataExchange::IsClipboardFormatAvailable(15).is_ok() }
}
