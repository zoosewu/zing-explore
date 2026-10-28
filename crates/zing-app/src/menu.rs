//! 程式自己的下拉選單（命令列的「新增▾」「排序▾」「檢視▾」「⋯」、命令列按鈕的右鍵選單）。
//!
//! 以 D2D 畫在視窗內，跟著主題的顏色與字型；快速鍵靠右顯示，用滑鼠操作也能學到快速鍵。
//! 子選單在游標停到該項時展開；鍵盤：↑↓ 移動、→ 進入子選單、← 返回、Enter 執行、Esc 關閉。
//! 在可加到命令列的項目上按右鍵，換成「加到命令列」選單。

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::view::Hit;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Exec(String),
    /// 命令列第 i 個按鈕：移除、左右移動
    BarRemove(usize),
    BarMove(usize, isize),
    /// 把命令加到命令列（「⋯」前面）
    BarAdd(String),
    /// 右鍵選單中程式自己的項目（`entry_menu_extras` 的 id, 對象）
    Extra(u32, String),
    /// 右鍵選單的「顯示更多選項」：開啟完整的 Windows 選單
    ShellMenu,
    /// 右鍵選單中第三方擴充功能的項目（第幾個擴充功能, 命令 id）
    ShellExt(usize, u32),
    /// 狀態列「有新版本」的選單（update.rs）
    Update(crate::update::Choice),
    /// 設定 `PREFS[i]` 為這個值（歡迎畫面的選項，welcome.rs）
    Pref(usize, String),
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub key: Option<String>,
    pub action: Action,
    pub enabled: bool,
    pub checked: bool,
    pub icon: Option<&'static str>,
    pub sub: Vec<MenuItem>,
    pub separator: bool,
    /// 可以加到命令列的命令（右鍵時用）
    pub cmd: Option<String>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, action: Action) -> Self {
        MenuItem { label: label.into(), key: None, action, enabled: true, checked: false, icon: None, sub: Vec::new(), separator: false, cmd: None }
    }

    pub fn separator() -> Self {
        MenuItem { separator: true, ..MenuItem::new("", Action::None) }
    }
}

pub struct Level {
    pub items: Vec<MenuItem>,
    pub rect: Rect,
    pub hover: Option<usize>,
}

pub struct Menu {
    pub levels: Vec<Level>,
}

const SEP_H: f32 = 9.0;
const PAD: f32 = 4.0;

impl App {
    fn menu_row_h(&self) -> f32 {
        (self.row_h() + 4.0).max(26.0)
    }

    fn item_h(&self, it: &MenuItem) -> f32 {
        if it.separator { SEP_H } else { self.menu_row_h() }
    }

    /// 選單的大小：最寬的名稱＋快速鍵
    fn level_size(&self, items: &[MenuItem]) -> (f32, f32) {
        let g = &self.gfx;
        let mut label_w: f32 = 0.0;
        let mut key_w: f32 = 0.0;
        for it in items.iter().filter(|i| !i.separator) {
            label_w = label_w.max(g.measure(&it.label, Font::Text));
            if let Some(k) = &it.key {
                key_w = key_w.max(g.measure(k, Font::Small));
            }
        }
        let w = 32.0 + label_w + if key_w > 0.0 { key_w + 32.0 } else { 0.0 } + 28.0;
        let h = items.iter().map(|i| self.item_h(i)).sum::<f32>() + PAD * 2.0;
        (w.max(160.0), h)
    }

    /// 在 `anchor` 下方（放不下就上方）開啟選單
    pub fn open_menu(&mut self, items: Vec<MenuItem>, anchor: Rect) {
        let rect = self.place_menu(&items, anchor.l, anchor.b + 2.0, Some(anchor.t - 2.0));
        self.menu = Some(Menu { levels: vec![Level { items, rect, hover: None }] });
        self.hide_tip();
        self.invalidate();
    }

    /// 在 (x, y) 開啟選單（右鍵選單）
    pub fn open_menu_at(&mut self, items: Vec<MenuItem>, x: f32, y: f32) {
        let rect = self.place_menu(&items, x, y, None);
        self.menu = Some(Menu { levels: vec![Level { items, rect, hover: None }] });
        self.hide_tip();
        self.invalidate();
    }

    fn place_menu(&self, items: &[MenuItem], x: f32, y: f32, above: Option<f32>) -> Rect {
        let (w, h) = self.level_size(items);
        let (ww, wh) = self.gfx.size();
        let l = x.min(ww - w - 4.0).max(4.0);
        let t = if y + h > wh - 4.0 { above.map_or(wh - h - 4.0, |a| a - h).max(4.0) } else { y };
        Rect::new(l, t, l + w, t + h)
    }

    pub fn close_menu(&mut self) {
        if self.menu.take().is_some() {
            self.invalidate();
        }
    }

    /// 第 i 項的位置
    fn menu_item_rect(&self, level: &Level, i: usize) -> Rect {
        let mut y = level.rect.t + PAD;
        for it in &level.items[..i] {
            y += self.item_h(it);
        }
        Rect::new(level.rect.l + PAD, y, level.rect.r - PAD, y + self.item_h(&level.items[i]))
    }

    pub fn menu_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let m = self.menu.as_ref()?;
        for (li, level) in m.levels.iter().enumerate().rev() {
            if !level.rect.contains(x, y) {
                continue;
            }
            for i in 0..level.items.len() {
                if !level.items[i].separator && self.menu_item_rect(level, i).contains(x, y) {
                    return Some(Hit::MenuItem(li, i));
                }
            }
            return Some(Hit::Menu);
        }
        None
    }

    /// 游標移動：更新反白、展開子選單
    pub fn menu_hover(&mut self, hit: &Hit) {
        let Hit::MenuItem(li, i) = *hit else { return };
        let sub = {
            let Some(m) = self.menu.as_mut() else { return };
            m.levels.truncate(li + 1);
            m.levels[li].hover = Some(i);
            let it = &m.levels[li].items[i];
            (!it.sub.is_empty() && it.enabled).then(|| it.sub.clone())
        };
        if let Some(items) = sub {
            self.open_submenu(li, i, items);
        }
        self.invalidate();
    }

    fn open_submenu(&mut self, li: usize, i: usize, items: Vec<MenuItem>) {
        let Some(m) = self.menu.as_ref() else { return };
        let r = self.menu_item_rect(&m.levels[li], i);
        let (w, h) = self.level_size(&items);
        let (ww, wh) = self.gfx.size();
        let parent = m.levels[li].rect;
        let l = if parent.r + w > ww - 4.0 { (parent.l - w).max(4.0) } else { parent.r - 2.0 };
        let t = (r.t - PAD).min(wh - h - 4.0).max(4.0);
        let rect = Rect::new(l, t, l + w, t + h);
        if let Some(m) = self.menu.as_mut() {
            m.levels.push(Level { items, rect, hover: None });
        }
    }

    /// 點選單：執行項目；點選單外關閉
    pub fn menu_click(&mut self, hit: Hit) {
        match hit {
            Hit::MenuItem(li, i) => {
                let Some(it) = self.menu.as_ref().and_then(|m| m.levels.get(li)).and_then(|l| l.items.get(i)).cloned() else { return };
                if !it.enabled {
                    return;
                }
                if !it.sub.is_empty() {
                    self.menu_hover(&Hit::MenuItem(li, i));
                    return;
                }
                self.close_menu();
                self.menu_action(it.action);
            }
            Hit::Menu => {}
            _ => self.close_menu(),
        }
    }

    /// 在選單項目上按右鍵：可以加到（或移出）命令列的命令換成對應的選單
    pub fn menu_right_click(&mut self, hit: Hit, x: f32, y: f32) {
        let Hit::MenuItem(li, i) = hit else {
            self.close_menu();
            return;
        };
        let Some(cmd) = self.menu.as_ref().and_then(|m| m.levels.get(li)).and_then(|l| l.items.get(i)).and_then(|it| it.cmd.clone()) else { return };
        let on_bar = self.config.cmdbar.iter().position(|b| b.cmd == cmd);
        let item = match on_bar {
            Some(k) => MenuItem::new(tr!("從命令列移除"), Action::BarRemove(k)),
            None => MenuItem::new(tr!("加到命令列"), Action::BarAdd(cmd)),
        };
        self.open_menu_at(vec![item], x, y);
    }

    pub fn menu_action(&mut self, a: Action) {
        match a {
            Action::None => {}
            Action::Exec(cmd) => {
                self.leave_cmdbar();
                self.exec(&cmd)
            }
            Action::BarRemove(i) => self.cmdbar_remove(i),
            Action::BarMove(i, d) => self.cmdbar_move(i, d),
            Action::BarAdd(cmd) => self.cmdbar_add(&cmd),
            Action::Extra(id, target) => self.on_compact_extra(id, target),
            Action::ShellMenu => self.show_full_context_menu(),
            Action::ShellExt(ext, id) => self.invoke_menu_ext(ext, id),
            Action::Update(c) => self.update_choice(c),
            Action::Pref(i, value) => self.set_pref(i, value),
        }
    }

    /// 選單開著時的按鍵；回傳 true 表示已處理（所有按鍵都不傳給底下）
    /// 選取第一個可選的項目（以鍵盤開啟的選單）
    pub fn menu_select_first(&mut self) {
        if let Some(level) = self.menu.as_mut().and_then(|m| m.levels.last_mut()) {
            level.hover = level.items.iter().position(|it| !it.separator && it.enabled);
            self.invalidate();
        }
    }

    pub fn menu_key(&mut self, vk: u16) -> bool {
        // 從命令列（F10）開啟的選單：最上層的 ←→ 換到相鄰的按鈕，是選單就開啟
        if self.bar_focus.is_some()
            && let Some(m) = self.menu.as_ref()
            && m.levels.len() == 1
        {
            let level = &m.levels[0];
            let opens_sub = level.hover.is_some_and(|i| !level.items[i].sub.is_empty());
            let d = match vk {
                0x25 => -1,
                0x27 if !opens_sub => 1,
                _ => 0,
            };
            if d != 0 {
                self.close_menu();
                self.bar_focus_move(d);
                if self.bar_focus.and_then(|i| self.config.cmdbar.get(i)).is_some_and(|it| it.is_menu()) {
                    self.bar_activate();
                }
                return true;
            }
        }
        let Some(m) = self.menu.as_mut() else { return false };
        let li = m.levels.len() - 1;
        let level = &mut m.levels[li];
        let n = level.items.len();
        let selectable = |it: &MenuItem| !it.separator && it.enabled;
        let step = |from: Option<usize>, d: isize| -> Option<usize> {
            let mut i = from.unwrap_or(if d > 0 { n - 1 } else { 0 }) as isize;
            for _ in 0..n {
                i = (i + d).rem_euclid(n as isize);
                if selectable(&level.items[i as usize]) {
                    return Some(i as usize);
                }
            }
            None
        };
        match vk {
            0x1B => {
                if li == 0 {
                    self.close_menu();
                } else {
                    m.levels.pop();
                }
            }
            0x26 => level.hover = step(level.hover, -1),
            0x28 => level.hover = step(level.hover, 1),
            0x25 if li > 0 => {
                m.levels.pop();
            }
            0x27 | 0x0D => {
                if let Some(i) = level.hover {
                    let it = level.items[i].clone();
                    if !it.sub.is_empty() {
                        self.open_submenu(li, i, it.sub);
                        if let Some(m) = self.menu.as_mut()
                            && let Some(last) = m.levels.last_mut()
                        {
                            last.hover = last.items.iter().position(selectable);
                        }
                    } else if vk == 0x0D && it.enabled {
                        self.close_menu();
                        self.menu_action(it.action);
                    }
                }
            }
            _ => {}
        }
        self.invalidate();
        true
    }

    pub fn paint_menu(&self) {
        let Some(m) = self.menu.as_ref() else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius.max(4.0);
        for level in &m.levels {
            let r = level.rect;
            let shadow = zing_core::theme::Color { r: 0, g: 0, b: 0, a: 56 };
            g.fill_round(Rect::new(r.l - 1.0, r.t + 1.0, r.r + 2.0, r.b + 4.0), radius + 2.0, shadow);
            g.fill_round(r, radius, c.surface);
            g.stroke_round(r, radius, c.border, 1.0);
            for (i, it) in level.items.iter().enumerate() {
                let ir = self.menu_item_rect(level, i);
                if it.separator {
                    let y = ((ir.t + ir.b) / 2.0).round();
                    g.hline(ir.l + 6.0, ir.r - 6.0, y, c.border);
                    continue;
                }
                if level.hover == Some(i) && it.enabled {
                    g.fill_round(ir, 4.0, c.hover);
                }
                let col = if it.enabled { c.text } else { c.text_secondary.with_alpha(120) };
                let icon_r = Rect::new(ir.l + 4.0, ir.t, ir.l + 28.0, ir.b);
                if it.checked {
                    g.text("✓", Font::Text, Rect::new(icon_r.l + 4.0, ir.t, icon_r.r, ir.b), c.accent);
                } else if let Some(gl) = it.icon {
                    g.text(gl, Font::GlyphSmall, Rect::new(icon_r.l + 2.0, ir.t, icon_r.r, ir.b), col);
                }
                g.text(&it.label, Font::Text, Rect::new(ir.l + 32.0, ir.t, ir.r - 24.0, ir.b), col);
                if let Some(k) = &it.key {
                    let w = g.measure(k, Font::Small);
                    g.text(k, Font::Small, Rect::new(ir.r - 24.0 - w, ir.t, ir.r - 8.0, ir.b), c.text_secondary);
                }
                if !it.sub.is_empty() {
                    g.text(crate::gfx::glyph::CHEVRON_RIGHT, Font::GlyphSmall, Rect::new(ir.r - 20.0, ir.t, ir.r, ir.b), col);
                }
            }
        }
    }
}
