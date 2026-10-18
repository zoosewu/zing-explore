//! 自訂命令列的面板（cmdbar.edit）：停靠在窗格右側（與設定頁、主題編輯面板同一個位置，一次只開一個）。
//!
//! - 上半部「目前的按鈕」：拖曳左側的 ⋮⋮ 或按 ↑↓ 調整順序、× 移除；點一個按鈕展開，可改名稱、從 Fluent 圖示中挑選，
//!   或「選擇圖片…」用自己的圖片檔。
//! - 下半部「加入命令」：依分組列出內建命令與 Lua 腳本的命令，點一下加到命令列；「自訂命令（進階）」輸入命令腳本。
//! - 標題下方的搜尋框依名稱、說明或命令篩選可加入的命令（輸入時即時更新）；有搜尋文字時只列出結果。
//! - 每次修改立即生效，並在背景寫入程式旁的 `toolbar.toml`（隔離模式不寫檔）。

use zing_core::cmdbar::{self as core, GLYPHS, Item};

use crate::app::{App, EditKind, Modal, Press};
use crate::gfx::{Font, Rect};
use crate::view::Hit;
use crate::window;

pub const PANEL_W: f32 = 400.0;
/// 底部按鈕：還原預設、完成
pub const BUTTONS: [&str; 2] = [tr_key!("還原預設"), tr_key!("完成")];
const GLYPH_COLS: usize = 10;

#[derive(Default)]
pub struct CmdBarEditor {
    pub scroll: f32,
    /// 展開編輯的按鈕（命令列的索引）
    pub expanded: Option<usize>,
    /// 搜尋文字（篩選可加入的命令）
    pub query: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Header(&'static str),
    /// 命令列的第 i 個按鈕
    Item(usize),
    /// 展開的按鈕：名稱
    Name(usize),
    /// 展開的按鈕：圖示的第幾列
    Icons(usize, usize),
    /// 展開的按鈕：選擇圖片、預設圖示
    IconActions(usize),
    AddSeparator,
    Group(&'static str),
    /// 可加入的命令：（命令, 名稱）
    Avail(String, String),
    Custom,
}

/// 列中可點的部分（`Hit::BarEditPart` 的第二個值）
pub const PART_UP: usize = 0;
pub const PART_DOWN: usize = 1;
pub const PART_REMOVE: usize = 2;
pub const PART_DRAG: usize = 3;
pub const PART_IMAGE: usize = 0;
pub const PART_DEFAULT_ICON: usize = 1;

impl App {
    /// 關閉右側的面板（設定頁、主題、命令列、快速鍵）
    pub fn close_side_panels(&mut self) {
        if self.settings_page.is_some() {
            self.close_settings();
        }
        if self.theme_editor.is_some() {
            self.close_theme_editor(false);
        }
        self.cmdbar_editor = None;
        if self.keys_panel.is_some() {
            self.close_keys_panel();
        }
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::BarLabel { .. } | EditKind::BarCustom | EditKind::BarSearch))
            && let Some(e) = self.edit.take()
        {
            window::destroy_edit(e.hwnd);
        }
        self.invalidate();
    }

    pub fn open_cmdbar_editor(&mut self) {
        self.close_side_panels();
        self.cmdbar_editor = Some(CmdBarEditor::default());
        self.invalidate();
    }

    pub fn cmdbar_edit_rows(&self) -> Vec<Row> {
        let Some(ed) = self.cmdbar_editor.as_ref() else { return Vec::new() };
        let q = ed.query.trim().to_lowercase();
        if !q.is_empty() {
            return self.bar_search_rows(&q);
        }
        let mut out = vec![Row::Header(tr!("目前的按鈕"))];
        for i in 0..self.config.cmdbar.len() {
            out.push(Row::Item(i));
            if ed.expanded == Some(i) && !self.config.cmdbar[i].is_separator() {
                out.push(Row::Name(i));
                out.extend((0..GLYPHS.len().div_ceil(GLYPH_COLS)).map(|l| Row::Icons(i, l)));
                out.push(Row::IconActions(i));
            }
        }
        out.push(Row::AddSeparator);
        out.push(Row::Header(tr!("加入命令")));
        for (group, cmds) in core::catalog() {
            out.push(Row::Group(zing_core::i18n::tr(group)));
            out.extend(cmds.into_iter().map(|c| Row::Avail(c.to_string(), core::label_for(c).to_string())));
        }
        if !self.scripts.commands.is_empty() {
            out.push(Row::Group(tr!("腳本")));
            out.extend(self.scripts.commands.iter().map(|(n, t)| Row::Avail(n.clone(), t.clone())));
        }
        out.push(Row::Custom);
        out
    }

    /// 搜尋結果：符合的命令（保留分組標題），最後是自訂命令
    fn bar_search_rows(&self, q: &str) -> Vec<Row> {
        let long = |cmd: &str| zing_core::palette::BUILTIN.iter().find(|(c, _)| *c == cmd).map_or("", |(_, t)| zing_core::i18n::tr(t));
        let hit = |cmd: &str, title: &str| [cmd, title, long(cmd)].iter().any(|s| s.to_lowercase().contains(q));
        let mut out = vec![Row::Header(tr!("搜尋結果"))];
        let mut groups: Vec<(&'static str, Vec<Row>)> = core::catalog()
            .into_iter()
            .map(|(g, cmds)| (zing_core::i18n::tr(g), cmds.into_iter().filter(|c| hit(c, core::label_for(c))).map(|c| Row::Avail(c.to_string(), core::label_for(c).to_string())).collect()))
            .collect();
        groups.push((tr!("腳本"), self.scripts.commands.iter().filter(|(n, t)| hit(n, t)).map(|(n, t)| Row::Avail(n.clone(), t.clone())).collect()));
        let before = out.len();
        for (g, rows) in groups.into_iter().filter(|(_, r)| !r.is_empty()) {
            out.push(Row::Group(g));
            out.extend(rows);
        }
        if out.len() == before {
            out.push(Row::Group(tr!("找不到符合的命令")));
        }
        out.push(Row::Custom);
        out
    }

    /// 標題下方的搜尋框
    pub fn bar_search_rect(&self, r: Rect) -> Rect {
        let rh = self.row_h();
        let inner = r.inset(12.0, 10.0);
        let t = inner.t + rh * 2.0 + 4.0;
        Rect::new(inner.l, t, inner.r, t + (rh + 6.0).max(26.0))
    }

    pub fn cmdbar_edit_layout(&self, r: Rect) -> (Rect, Rect, [Rect; 2]) {
        let rh = self.row_h();
        let inner = r.inset(12.0, 10.0);
        let title = Rect::new(inner.l, inner.t, inner.r, inner.t + rh * 2.0);
        let bh = (rh + 6.0).max(28.0);
        let row = Rect::new(inner.l, inner.b - bh, inner.r, inner.b);
        let half = (inner.w() - 6.0) / 2.0;
        let buttons = [Rect::new(row.l, row.t, row.l + half, row.b), Rect::new(row.l + half + 6.0, row.t, row.r, row.b)];
        let list = Rect::new(inner.l, self.bar_search_rect(r).b + 6.0, inner.r, row.t - 8.0);
        (title, list, buttons)
    }

    pub fn bar_row_rect(&self, list: Rect, k: usize, scroll: f32) -> Rect {
        let rh = self.row_h() + 4.0;
        let y = list.t + k as f32 * rh - scroll;
        Rect::new(list.l, y, list.r, y + rh)
    }

    /// 列中各部分的位置：↑ ↓ × ⋮⋮（按鈕列）、圖示格（圖示列）、兩個按鈕（圖示動作列）
    pub fn bar_parts(&self, row: &Row, r: Rect) -> Vec<Rect> {
        let s = r.h() - 4.0;
        match row {
            Row::Item(_) => {
                let x = |k: f32| Rect::new(r.r - (k + 1.0) * (s + 2.0), r.t + 2.0, r.r - k * (s + 2.0) - 2.0, r.b - 2.0);
                vec![x(2.0), x(1.0), x(0.0), Rect::new(r.l, r.t, r.l + 18.0, r.b)]
            }
            Row::Icons(_, line) => (0..GLYPH_COLS)
                .take_while(|c| line * GLYPH_COLS + c < GLYPHS.len())
                .map(|c| {
                    let l = r.l + 24.0 + c as f32 * (s + 4.0);
                    Rect::new(l, r.t + 2.0, l + s, r.b - 2.0)
                })
                .collect(),
            Row::IconActions(_) => {
                let half = (r.w() - 24.0 - 6.0) / 2.0;
                vec![Rect::new(r.l + 24.0, r.t + 2.0, r.l + 24.0 + half, r.b - 2.0), Rect::new(r.l + 30.0 + half, r.t + 2.0, r.r, r.b - 2.0)]
            }
            _ => Vec::new(),
        }
    }

    pub fn cmdbar_edit_hit(&self, panel: Rect, x: f32, y: f32) -> Hit {
        let Some(ed) = self.cmdbar_editor.as_ref() else { return Hit::None };
        let (_, list, buttons) = self.cmdbar_edit_layout(panel);
        if let Some(k) = buttons.iter().position(|b| b.contains(x, y)) {
            return Hit::BarEditButton(k);
        }
        let search = self.bar_search_rect(panel);
        if search.contains(x, y) {
            let clear = !ed.query.is_empty() && x >= search.r - search.h();
            return Hit::BarEditSearch(clear);
        }
        if !list.contains(x, y) {
            return Hit::BarEditPanel;
        }
        for (k, row) in self.cmdbar_edit_rows().iter().enumerate() {
            let r = self.bar_row_rect(list, k, ed.scroll);
            if !r.contains(x, y) {
                continue;
            }
            if matches!(row, Row::Header(_) | Row::Group(_)) {
                return Hit::BarEditPanel;
            }
            if let Some(p) = self.bar_parts(row, r).iter().position(|p| p.contains(x, y)) {
                return Hit::BarEditPart(k, p);
            }
            return if matches!(row, Row::Icons(..)) { Hit::BarEditPanel } else { Hit::BarEditRow(k) };
        }
        Hit::BarEditPanel
    }

    pub fn cmdbar_edit_click(&mut self, hit: Hit, _y: f32) {
        let rows = self.cmdbar_edit_rows();
        match hit {
            Hit::BarEditRow(k) => match rows.get(k).cloned() {
                Some(Row::Item(i)) => {
                    if let Some(ed) = self.cmdbar_editor.as_mut() {
                        ed.expanded = if ed.expanded == Some(i) { None } else { Some(i) };
                    }
                }
                Some(Row::Name(i)) => self.begin_bar_label_edit(k, i),
                Some(Row::AddSeparator) => {
                    self.config.cmdbar.push(Item::new(core::SEPARATOR));
                    self.cmdbar_changed();
                }
                Some(Row::Avail(cmd, _)) => {
                    if !self.config.cmdbar.iter().any(|i| i.cmd == cmd) {
                        self.cmdbar_add(&cmd);
                    }
                }
                Some(Row::Custom) => self.begin_bar_custom_edit(k),
                _ => {}
            },
            Hit::BarEditPart(k, p) => match rows.get(k).cloned() {
                Some(Row::Item(i)) => match p {
                    PART_UP => self.bar_edit_move(i, -1),
                    PART_DOWN => self.bar_edit_move(i, 1),
                    PART_REMOVE => {
                        if let Some(ed) = self.cmdbar_editor.as_mut() {
                            ed.expanded = None;
                        }
                        self.cmdbar_remove(i);
                    }
                    _ => self.press = Some(Press::BarEditDrag { item: i }),
                },
                Some(Row::Icons(i, line)) => {
                    if let (Some(it), Some((name, _))) = (self.config.cmdbar.get_mut(i), GLYPHS.get(line * GLYPH_COLS + p)) {
                        it.icon = format!("glyph:{name}");
                        self.cmdbar_changed();
                    }
                }
                Some(Row::IconActions(i)) => {
                    if p == PART_IMAGE {
                        self.modal = Some(Modal::PickBarIcon { item: i });
                    } else if let Some(it) = self.config.cmdbar.get_mut(i) {
                        it.icon.clear();
                        self.cmdbar_changed();
                    }
                }
                _ => {}
            },
            Hit::BarEditButton(0) => {
                self.config.cmdbar = core::defaults();
                if let Some(ed) = self.cmdbar_editor.as_mut() {
                    ed.expanded = None;
                }
                self.cmdbar_changed();
            }
            Hit::BarEditButton(_) => self.close_side_panels(),
            Hit::BarEditSearch(true) => self.bar_query(String::new()),
            Hit::BarEditSearch(false) => self.begin_bar_search(),
            _ => {}
        }
        self.invalidate();
    }

    fn begin_bar_search(&mut self) {
        let (Some(panel), Some(ed)) = (self.layout.cmdbar_panel, self.cmdbar_editor.as_ref()) else { return };
        let s = self.bar_search_rect(panel);
        let text = ed.query.clone();
        let len = text.encode_utf16().count();
        window::begin_edit(self, Rect::new(s.l + 26.0, s.t + 1.0, s.r - s.h(), s.b - 1.0), &text, len, EditKind::BarSearch);
    }

    /// 搜尋框內容改變（EN_CHANGE）
    pub fn bar_query(&mut self, text: String) {
        if let Some(ed) = self.cmdbar_editor.as_mut() {
            ed.query = text;
            ed.scroll = 0.0;
            ed.expanded = None;
            self.invalidate();
        }
    }

    fn bar_edit_move(&mut self, i: usize, d: isize) {
        let j = i as isize + d;
        if j < 0 || j as usize >= self.config.cmdbar.len() {
            return;
        }
        self.cmdbar_move(i, d);
        if let Some(ed) = self.cmdbar_editor.as_mut()
            && ed.expanded == Some(i)
        {
            ed.expanded = Some(j as usize);
        }
    }

    /// 拖曳中：移到游標所在的按鈕位置
    pub fn cmdbar_edit_drag(&mut self, y: f32) {
        let Some(Press::BarEditDrag { item }) = self.press else { return };
        let (Some(panel), Some(ed)) = (self.layout.cmdbar_panel, self.cmdbar_editor.as_ref()) else { return };
        let (_, list, _) = self.cmdbar_edit_layout(panel);
        let rows = self.cmdbar_edit_rows();
        let target = rows.iter().enumerate().find_map(|(k, r)| match r {
            Row::Item(i) if self.bar_row_rect(list, k, ed.scroll).contains(list.l + 1.0, y) => Some(*i),
            _ => None,
        });
        if let Some(t) = target.filter(|t| *t != item) {
            let it = self.config.cmdbar.remove(item);
            self.config.cmdbar.insert(t, it);
            self.press = Some(Press::BarEditDrag { item: t });
            if let Some(ed) = self.cmdbar_editor.as_mut() {
                ed.expanded = None;
            }
            self.cmdbar_changed();
        }
    }

    pub fn cmdbar_edit_wheel(&mut self, panel: Rect, delta: i16) {
        let (_, list, _) = self.cmdbar_edit_layout(panel);
        let max = (self.cmdbar_edit_rows().len() as f32 * (self.row_h() + 4.0) - list.h()).max(0.0);
        let step = 3.0 * self.row_h();
        if let Some(ed) = self.cmdbar_editor.as_mut() {
            ed.scroll = (ed.scroll - (delta as i32 / 120).signum() as f32 * step).clamp(0.0, max);
        }
        self.invalidate();
    }

    fn edit_rect(&self, k: usize) -> Option<Rect> {
        let panel = self.layout.cmdbar_panel?;
        let ed = self.cmdbar_editor.as_ref()?;
        let (_, list, _) = self.cmdbar_edit_layout(panel);
        let r = self.bar_row_rect(list, k, ed.scroll);
        Some(Rect::new(r.l + 90.0, r.t + 1.0, r.r, r.b - 1.0))
    }

    fn begin_bar_label_edit(&mut self, k: usize, i: usize) {
        let (Some(r), Some(it)) = (self.edit_rect(k), self.config.cmdbar.get(i)) else { return };
        let text = it.title();
        let len = text.encode_utf16().count();
        window::begin_edit(self, r, &text, len, EditKind::BarLabel { item: i });
    }

    fn begin_bar_custom_edit(&mut self, k: usize) {
        let Some(r) = self.edit_rect(k) else { return };
        let text = "run notepad.exe {sel}";
        window::begin_edit(self, r, text, text.encode_utf16().count(), EditKind::BarCustom);
    }

    /// 名稱輸入框確認：空白或與預設名稱相同表示用預設名稱
    pub fn commit_bar_label(&mut self, i: usize, text: &str) {
        let Some(it) = self.config.cmdbar.get_mut(i) else { return };
        let t = text.trim();
        it.label = if t.is_empty() || t == core::label_for(&it.cmd) { String::new() } else { t.to_string() };
        self.cmdbar_changed();
    }

    /// 自訂命令：命令腳本本身先當名稱，之後可以改
    pub fn commit_bar_custom(&mut self, text: &str) {
        let cmd = text.trim();
        if cmd.is_empty() {
            return;
        }
        let label = cmd.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        let bar = &mut self.config.cmdbar;
        let at = bar.iter().position(|i| i.cmd == core::MENU_MORE).unwrap_or(bar.len());
        bar.insert(at, Item { cmd: cmd.to_string(), label, icon: String::new() });
        if let Some(ed) = self.cmdbar_editor.as_mut() {
            ed.expanded = Some(at);
        }
        self.cmdbar_changed();
    }

    /// 選擇圖片的結果
    pub fn set_bar_image(&mut self, i: usize, path: String) {
        if let Some(it) = self.config.cmdbar.get_mut(i) {
            it.icon = path;
            self.cmdbar_changed();
        }
    }

    pub fn paint_cmdbar_editor(&self, r: Rect) {
        let Some(ed) = self.cmdbar_editor.as_ref() else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius;
        g.fill(r, c.surface);
        let (title, list, buttons) = self.cmdbar_edit_layout(r);
        let half = title.h() / 2.0;
        g.text(tr!("自訂命令列"), Font::Bold, Rect::new(title.l, title.t, title.r, title.t + half), c.text);
        g.text(tr!("拖曳 ⋮⋮ 調整順序；點一個按鈕可改名稱與圖示"), Font::Small, Rect::new(title.l, title.t + half, title.r, title.b), c.text_secondary);
        let s = self.bar_search_rect(r);
        g.fill_round(s, radius, c.background);
        g.stroke_round(s, radius, if matches!(self.hover, Hit::BarEditSearch(false)) { c.accent } else { c.border }, 1.0);
        g.text(crate::gfx::glyph::FILTER, Font::GlyphSmall, Rect::new(s.l + 6.0, s.t, s.l + 24.0, s.b), c.text_secondary);
        if !matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::BarSearch)) {
            let (text, col) = if ed.query.is_empty() { (tr!("搜尋要加入的命令"), c.text_secondary) } else { (ed.query.as_str(), c.text) };
            g.text(text, Font::Small, Rect::new(s.l + 26.0, s.t, s.r - s.h(), s.b), col);
        }
        if !ed.query.is_empty() {
            let col = if self.hover == Hit::BarEditSearch(true) { c.text } else { c.text_secondary };
            g.text("×", Font::Text, Rect::new(s.r - s.h() + 6.0, s.t, s.r, s.b), col);
        }
        g.push_clip(list);
        let dragging = match self.press {
            Some(Press::BarEditDrag { item }) => Some(item),
            _ => None,
        };
        for (k, row) in self.cmdbar_edit_rows().iter().enumerate() {
            let rr = self.bar_row_rect(list, k, ed.scroll);
            if rr.b < list.t || rr.t > list.b {
                continue;
            }
            let hover = self.hover == Hit::BarEditRow(k);
            let parts = self.bar_parts(row, rr);
            let part_hover = |p: usize| self.hover == Hit::BarEditPart(k, p);
            match row {
                Row::Header(h) => g.text(h, Font::Bold, Rect::new(rr.l, rr.t + 4.0, rr.r, rr.b), c.text_secondary),
                Row::Group(h) => g.text(h, Font::Small, Rect::new(rr.l + 4.0, rr.t + 4.0, rr.r, rr.b), c.text_secondary),
                Row::Item(i) => {
                    let it = &self.config.cmdbar[*i];
                    let any_hover = hover || (0..4).any(part_hover);
                    if dragging == Some(*i) {
                        g.fill_round(rr, radius, c.accent.with_alpha(50));
                    } else if any_hover || ed.expanded == Some(*i) {
                        g.fill_round(rr, radius, c.hover);
                    }
                    g.text("⋮⋮", Font::Small, parts[PART_DRAG], c.text_secondary);
                    if it.is_separator() {
                        let y = ((rr.t + rr.b) / 2.0).round();
                        g.hline(rr.l + 24.0, rr.l + 120.0, y, c.border);
                        g.text(tr!("分隔線"), Font::Small, Rect::new(rr.l + 128.0, rr.t, rr.r, rr.b), c.text_secondary);
                    } else {
                        self.paint_item_icon(it, Rect::new(rr.l + 22.0, rr.t, rr.l + 44.0, rr.b), c.text, 1.0);
                        let mut t = it.title();
                        if it.is_menu() && it.cmd != core::MENU_MORE {
                            t.push_str(" ▾");
                        }
                        g.text(&t, Font::Text, Rect::new(rr.l + 48.0, rr.t, parts[PART_UP].l - 4.0, rr.b), c.text);
                    }
                    for (p, s) in [(PART_UP, "↑"), (PART_DOWN, "↓"), (PART_REMOVE, "×")] {
                        let col = if part_hover(p) { if p == PART_REMOVE { c.error } else { c.accent } } else { c.text_secondary };
                        let w = g.measure(s, Font::Text);
                        let b = parts[p];
                        g.text(s, Font::Text, Rect::new(b.l + (b.w() - w) / 2.0, b.t, b.r, b.b), col);
                    }
                }
                Row::Name(i) => {
                    g.text(tr!("名稱"), Font::Small, Rect::new(rr.l + 24.0, rr.t, rr.l + 88.0, rr.b), c.text_secondary);
                    let col = if hover { c.accent } else { c.text };
                    g.text(&self.config.cmdbar[*i].title(), Font::Text, Rect::new(rr.l + 90.0, rr.t, rr.r, rr.b), col);
                }
                Row::Icons(i, line) => {
                    let cur = &self.config.cmdbar[*i].icon;
                    for (cidx, cell) in parts.iter().enumerate() {
                        let (name, gl) = GLYPHS[line * GLYPH_COLS + cidx];
                        let selected = cur.strip_prefix("glyph:") == Some(name);
                        if selected || part_hover(cidx) {
                            g.fill_round(*cell, 3.0, if selected { c.accent.with_alpha(60) } else { c.hover });
                        }
                        g.text(gl, Font::Glyph, Rect::new(cell.l + (cell.w() - 16.0) / 2.0, cell.t, cell.r, cell.b), c.text);
                    }
                }
                Row::IconActions(_) => {
                    for (p, label) in [(PART_IMAGE, tr!("選擇圖片…")), (PART_DEFAULT_ICON, tr!("預設圖示"))] {
                        let b = parts[p];
                        g.fill_round(b, radius, if part_hover(p) { c.hover } else { c.background });
                        g.stroke_round(b, radius, c.border, 1.0);
                        let w = g.measure(label, Font::Small);
                        g.text(label, Font::Small, Rect::new(b.l + ((b.w() - w) / 2.0).max(4.0), b.t, b.r, b.b), c.text);
                    }
                }
                Row::AddSeparator | Row::Custom => {
                    if hover {
                        g.fill_round(rr, radius, c.hover);
                    }
                    let label = if *row == Row::Custom { tr!("＋ 自訂命令（進階）…") } else { tr!("＋ 分隔線") };
                    g.text(label, Font::Text, Rect::new(rr.l + 8.0, rr.t, rr.r, rr.b), c.accent);
                }
                Row::Avail(cmd, title) => {
                    let added = self.config.cmdbar.iter().any(|i| i.cmd == *cmd);
                    if hover && !added {
                        g.fill_round(rr, radius, c.hover);
                    }
                    let it = Item::new(cmd);
                    self.paint_item_icon(&it, Rect::new(rr.l + 22.0, rr.t, rr.l + 44.0, rr.b), c.text_secondary, 1.0);
                    let col = if added { c.text_secondary } else { c.text };
                    g.text(title, Font::Text, Rect::new(rr.l + 48.0, rr.t, rr.r - 70.0, rr.b), col);
                    let (mark, mc) = if added { (tr!("已加入"), c.text_secondary) } else { ("＋", c.accent) };
                    let w = g.measure(mark, Font::Small);
                    g.text(mark, Font::Small, Rect::new(rr.r - 8.0 - w, rr.t, rr.r, rr.b), mc);
                }
            }
        }
        g.pop_clip();
        for (k, b) in buttons.iter().enumerate() {
            let primary = k == 1;
            let hot = self.hover == Hit::BarEditButton(k);
            g.fill_round(*b, radius, if primary { c.accent } else if hot { c.hover } else { c.background });
            g.stroke_round(*b, radius, if primary { c.accent } else { c.border }, 1.0);
            let label = zing_core::i18n::tr(BUTTONS[k]);
            let w = g.measure(label, Font::Text);
            let col = if primary { crate::themepanel::contrast(c.accent) } else { c.text };
            g.text(label, Font::Text, Rect::new(b.l + ((b.w() - w) / 2.0).max(4.0), b.t, b.r, b.b), col);
        }
    }
}
