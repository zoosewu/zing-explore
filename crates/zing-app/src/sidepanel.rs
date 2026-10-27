//! 右側面板（設定頁、主題編輯、自訂命令列、快速鍵）：停靠在窗格區域最右側，一次只開一個（`App::side`）。
//!
//! 宿主（view.rs、input.rs）只經過這裡：位置與寬度（`side_width`）、命中（`side_hit`）、點擊（`side_click`）、
//! 滾輪（`side_wheel`）、Esc（`side_escape`）、繪製（`paint_side`）、關閉（`close_side_panels`）。
//! 各面板只提供自己的列、列的內容與動作；捲動、列的位置、捲到看得見（`side_reveal`）、底部兩個按鈕是共用的。
//! 面板的命中仍是各自的 `Hit` 變體，`Hit::side_panel` 判斷屬於面板的命中。
//!
//! 鍵盤（`Focus::Side`，開啟面板或按 Tab 進入）：各面板只提供一行中可按的目標（`side_targets`：命中與位置，與滑鼠的
//! 命中相同）；↑↓ 換行、←→ 在行中移動、Enter／空白鍵等於點那個目標、焦點框、捲到看得見都在這裡。
//! 冒煙測試逐行核對目標與滑鼠的命中一致。

use crate::app::{App, Focus};
use crate::cmdbaredit::CmdBarEditor;
use crate::gfx::{Font, Rect};
use crate::keyspanel::KeysPanel;
use crate::prefspage::SettingsPage;
use crate::themepanel::ThemeEditor;
use crate::view::Hit;
use crate::window;

pub enum SidePanel {
    Settings(SettingsPage),
    Theme(ThemeEditor),
    BarEdit(CmdBarEditor),
    Keys(KeysPanel),
}

/// 開著的是哪一個面板（不借用 `App::side`）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideKind {
    Settings,
    Theme,
    BarEdit,
    Keys,
}

/// 鍵盤焦點所在的一行：清單上方的固定行（例如搜尋框）、清單的第 k 列、清單下方的固定行（底部按鈕）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Top(usize),
    Row(usize),
    Bottom(usize),
}

/// 鍵盤焦點：第幾行、行中第幾個目標；`visible` 在按了方向鍵後才為 true（用滑鼠時不畫焦點框）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SideCursor {
    pub line: Line,
    pub part: usize,
    pub visible: bool,
}

impl Default for SideCursor {
    fn default() -> Self {
        SideCursor { line: Line::Row(0), part: 0, visible: false }
    }
}

/// 面板中捲動清單的幾何：清單範圍、列高、列數、固定在上方不捲動的列數
pub struct ListGeometry {
    pub list: Rect,
    pub row_h: f32,
    pub rows: usize,
    pub pinned: usize,
}

/// 第 k 列的位置（前 `pinned` 列固定在清單上方，不隨捲動）
pub fn row_rect(list: Rect, k: usize, row_h: f32, scroll: f32, pinned: usize) -> Rect {
    let y = if k < pinned { list.t + k as f32 * row_h } else { list.t + k as f32 * row_h - scroll };
    Rect::new(list.l, y, list.r, y + row_h)
}

/// 底部兩個並排按鈕的位置（`inner` 是面板扣掉邊距的範圍，按鈕貼齊它的底部）
pub fn footer_buttons(inner: Rect, row_h: f32) -> [Rect; 2] {
    let bh = (row_h + 6.0).max(28.0);
    let row = Rect::new(inner.l, inner.b - bh, inner.r, inner.b);
    let half = (inner.w() - 6.0) / 2.0;
    [Rect::new(row.l, row.t, row.l + half, row.b), Rect::new(row.l + half + 6.0, row.t, row.r, row.b)]
}

impl Hit {
    /// 屬於右側面板的命中（點擊交給 `side_click`）
    pub fn side_panel(&self) -> bool {
        matches!(
            self,
            Hit::ThemeRow(_)
                | Hit::ThemeText(_)
                | Hit::ThemeCell(..)
                | Hit::ThemeReset(_)
                | Hit::ThemeStep(..)
                | Hit::ThemeSlider(_)
                | Hit::ThemeDelete(_)
                | Hit::ThemeButton(_)
                | Hit::ThemeLink(_)
                | Hit::ThemePanel
                | Hit::SettingsRow(_)
                | Hit::SettingsBack
                | Hit::SettingsButton(_)
                | Hit::SettingsPanel
                | Hit::BarEditRow(_)
                | Hit::BarEditPart(..)
                | Hit::BarEditButton(_)
                | Hit::BarEditPanel
                | Hit::BarEditSearch(_)
                | Hit::KeysRow(_)
                | Hit::KeysPart(..)
                | Hit::KeysButton(_)
                | Hit::KeysPanel
        )
    }

    /// 屬於清單第幾列的命中
    pub fn side_row(&self) -> Option<usize> {
        match *self {
            Hit::ThemeRow(k)
            | Hit::ThemeText(k)
            | Hit::ThemeCell(k, _)
            | Hit::ThemeReset(k)
            | Hit::ThemeStep(k, _)
            | Hit::ThemeSlider(k)
            | Hit::ThemeDelete(k)
            | Hit::SettingsRow(k)
            | Hit::BarEditRow(k)
            | Hit::BarEditPart(k, _)
            | Hit::KeysRow(k)
            | Hit::KeysPart(k, _) => Some(k),
            _ => None,
        }
    }
}

impl App {
    pub fn side_kind(&self) -> Option<SideKind> {
        self.side.as_ref().map(|s| match s {
            SidePanel::Settings(_) => SideKind::Settings,
            SidePanel::Theme(_) => SideKind::Theme,
            SidePanel::BarEdit(_) => SideKind::BarEdit,
            SidePanel::Keys(_) => SideKind::Keys,
        })
    }

    pub fn settings_page(&self) -> Option<&SettingsPage> {
        match &self.side {
            Some(SidePanel::Settings(p)) => Some(p),
            _ => None,
        }
    }

    pub fn settings_page_mut(&mut self) -> Option<&mut SettingsPage> {
        match &mut self.side {
            Some(SidePanel::Settings(p)) => Some(p),
            _ => None,
        }
    }

    pub fn theme_editor(&self) -> Option<&ThemeEditor> {
        match &self.side {
            Some(SidePanel::Theme(p)) => Some(p),
            _ => None,
        }
    }

    pub fn theme_editor_mut(&mut self) -> Option<&mut ThemeEditor> {
        match &mut self.side {
            Some(SidePanel::Theme(p)) => Some(p),
            _ => None,
        }
    }

    pub fn cmdbar_editor(&self) -> Option<&CmdBarEditor> {
        match &self.side {
            Some(SidePanel::BarEdit(p)) => Some(p),
            _ => None,
        }
    }

    pub fn cmdbar_editor_mut(&mut self) -> Option<&mut CmdBarEditor> {
        match &mut self.side {
            Some(SidePanel::BarEdit(p)) => Some(p),
            _ => None,
        }
    }

    pub fn keys_panel(&self) -> Option<&KeysPanel> {
        match &self.side {
            Some(SidePanel::Keys(p)) => Some(p),
            _ => None,
        }
    }

    pub fn keys_panel_mut(&mut self) -> Option<&mut KeysPanel> {
        match &mut self.side {
            Some(SidePanel::Keys(p)) => Some(p),
            _ => None,
        }
    }

    fn side_scroll_mut(&mut self) -> Option<&mut f32> {
        Some(match self.side.as_mut()? {
            SidePanel::Settings(p) => &mut p.scroll,
            SidePanel::Theme(p) => &mut p.scroll,
            SidePanel::BarEdit(p) => &mut p.scroll,
            SidePanel::Keys(p) => &mut p.scroll,
        })
    }

    pub fn side_scroll(&self) -> f32 {
        match &self.side {
            Some(SidePanel::Settings(p)) => p.scroll,
            Some(SidePanel::Theme(p)) => p.scroll,
            Some(SidePanel::BarEdit(p)) => p.scroll,
            Some(SidePanel::Keys(p)) => p.scroll,
            None => 0.0,
        }
    }

    /// 開啟一個面板（取代開著的那個）；鍵盤焦點移到面板
    pub fn open_side(&mut self, panel: SidePanel) {
        self.close_side_panels();
        self.side = Some(panel);
        self.focus = Focus::Side;
        self.side_cursor = SideCursor::default();
        self.layout = self.compute_layout();
        self.side_settle();
        self.invalidate();
    }

    /// 關閉右側的面板與它的輸入框；主題編輯面板保留目前的主題（不還原）
    pub fn close_side_panels(&mut self) {
        self.side = None;
        if self.focus == Focus::Side {
            self.focus = Focus::List;
        }
        if self.edit.as_ref().is_some_and(|e| e.kind.in_side_panel())
            && let Some(e) = self.edit.take()
        {
            window::destroy_edit(e.hwnd);
        }
        self.invalidate();
    }

    /// 面板的寬度（窗格區域寬度 `avail` 中）
    pub fn side_width(&self, avail: f32) -> Option<f32> {
        Some(match self.side_kind()? {
            SideKind::Theme => crate::themepanel::PANEL_W.min(avail * 0.5),
            SideKind::Settings => crate::prefspage::PANEL_W.min(avail * 0.6),
            SideKind::BarEdit => crate::cmdbaredit::PANEL_W.min(avail * 0.6),
            SideKind::Keys => crate::keyspanel::PANEL_W.min(avail * 0.6),
        })
    }

    /// 面板中捲動清單的幾何
    pub fn side_geometry(&self, panel: Rect) -> Option<ListGeometry> {
        let rh = self.row_h();
        Some(match self.side_kind()? {
            SideKind::Settings => ListGeometry {
                list: self.settings_layout(panel).list,
                row_h: rh,
                rows: self.settings_page()?.rows().len(),
                pinned: 0,
            },
            SideKind::Theme => {
                ListGeometry { list: self.theme_panel_layout(panel).list, row_h: rh, rows: self.theme_rows().len(), pinned: 0 }
            }
            SideKind::BarEdit => ListGeometry {
                list: self.cmdbar_edit_layout(panel).1,
                row_h: rh + 4.0,
                rows: self.cmdbar_edit_rows().len(),
                pinned: 0,
            },
            // 搜尋列固定在最上方
            SideKind::Keys => {
                ListGeometry { list: self.keys_layout(panel).1, row_h: rh + 6.0, rows: self.keys_rows().len(), pinned: 1 }
            }
        })
    }

    /// 第 k 列的位置（目前的捲動）
    pub fn side_row(&self, panel: Rect, k: usize) -> Option<Rect> {
        let g = self.side_geometry(panel)?;
        Some(row_rect(g.list, k, g.row_h, self.side_scroll(), g.pinned))
    }

    /// 捲到第 k 列看得見（不在範圍內時置中），回傳它的位置
    pub fn side_reveal(&mut self, k: usize) -> Option<Rect> {
        let panel = self.layout.side_panel?;
        let g = self.side_geometry(panel)?;
        let top = k as f32 * g.row_h;
        let fixed = g.pinned as f32 * g.row_h;
        let scroll = self.side_scroll_mut()?;
        if k >= g.pinned && (top < *scroll + fixed || top + g.row_h > *scroll + g.list.h()) {
            *scroll = (top - g.list.h() / 2.0).max(0.0);
        }
        self.invalidate();
        self.side_row(panel, k)
    }

    /// 清單上方、下方固定行的數目
    fn side_fixed_lines(&self) -> (usize, usize) {
        match self.side_kind() {
            Some(SideKind::Settings | SideKind::BarEdit) => (1, 1),
            Some(SideKind::Theme) => (0, 2),
            Some(SideKind::Keys) => (0, 1),
            None => (0, 0),
        }
    }

    /// 由上到下所有的行
    pub fn side_lines(&self) -> Vec<Line> {
        let (top, bottom) = self.side_fixed_lines();
        let rows = self.layout.side_panel.and_then(|p| self.side_geometry(p)).map_or(0, |g| g.rows);
        (0..top).map(Line::Top).chain((0..rows).map(Line::Row)).chain((0..bottom).map(Line::Bottom)).collect()
    }

    /// 一行中可用鍵盤按的目標（由左到右）：命中與位置，位置的中心點以滑鼠命中會得到同一個命中。
    /// 只用滑鼠的（滑桿、拖曳把手）不列入，鍵盤有對應的按鈕
    pub fn side_targets(&self, line: Line) -> Vec<(Hit, Rect)> {
        let Some(panel) = self.layout.side_panel else { return Vec::new() };
        let mut v = match self.side_kind() {
            Some(SideKind::Settings) => self.settings_targets(panel, line),
            Some(SideKind::Theme) => self.theme_targets(panel, line),
            Some(SideKind::BarEdit) => self.cmdbar_edit_targets(panel, line),
            Some(SideKind::Keys) => self.keys_targets(panel, line),
            None => Vec::new(),
        };
        v.sort_by(|a, b| a.1.l.total_cmp(&b.1.l));
        v
    }

    /// 把鍵盤焦點放在某一行（捲到看得見）
    pub fn side_focus_line(&mut self, line: Line, part: usize) {
        if let Line::Row(k) = line {
            self.side_reveal(k);
        }
        let n = self.side_targets(line).len();
        self.side_cursor.line = line;
        self.side_cursor.part = part.min(n.saturating_sub(1));
        self.invalidate();
    }

    /// 焦點所在的行沒有目標了（列改變、收起選項）：移到下面最近的、沒有就往上找
    pub fn side_settle(&mut self) {
        if self.side.is_none() {
            return;
        }
        let lines = self.side_lines();
        let cur = lines.iter().position(|l| *l == self.side_cursor.line).unwrap_or(0);
        let has = |l: &Line| !self.side_targets(*l).is_empty();
        if lines.get(cur).is_some_and(has) {
            let part = self.side_cursor.part;
            return self.side_focus_line(lines[cur], part);
        }
        let next = lines[cur.min(lines.len())..].iter().find(|l| has(l)).or_else(|| lines[..cur.min(lines.len())].iter().rev().find(|l| has(l)));
        if let Some(&l) = next {
            self.side_focus_line(l, 0);
        }
    }

    /// 點了面板中的 `hit`：鍵盤焦點跟著移到那裡（不畫焦點框）
    fn side_cursor_to(&mut self, hit: &Hit) {
        self.focus = Focus::Side;
        self.side_cursor.visible = false;
        let lines = match hit.side_row() {
            Some(k) => vec![Line::Row(k)],
            None => self.side_lines().into_iter().filter(|l| !matches!(l, Line::Row(_))).collect(),
        };
        for line in lines {
            if let Some(part) = self.side_targets(line).iter().position(|(h, _)| h == hit) {
                self.side_cursor.line = line;
                self.side_cursor.part = part;
                return;
            }
        }
    }

    /// 鍵盤焦點在面板時的按鍵；回傳是否處理了（其他按鍵照常交給快速鍵）
    pub fn side_key(&mut self, vk: u16, ctrl_or_alt: bool) -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_END, VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP};
        let nav = [VK_UP.0, VK_DOWN.0, VK_LEFT.0, VK_RIGHT.0, VK_HOME.0, VK_END.0];
        let activate = vk == VK_RETURN.0 || vk == VK_SPACE.0;
        if ctrl_or_alt || self.side.is_none() || !(nav.contains(&vk) || activate) {
            return false;
        }
        // 第一次按鍵只顯示焦點框（用滑鼠開啟或點過面板時焦點框是隱藏的）
        if !self.side_cursor.visible {
            self.side_cursor.visible = true;
            self.side_settle();
            return true;
        }
        let SideCursor { line, part, .. } = self.side_cursor;
        let targets = self.side_targets(line);
        if activate {
            let Some((hit, r)) = targets.get(part).cloned() else { return true };
            self.side_click(hit, (r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
            self.side_cursor.visible = true;
            self.side_settle();
            return true;
        }
        match vk {
            v if v == VK_LEFT.0 => self.side_focus_line(line, part.saturating_sub(1)),
            v if v == VK_RIGHT.0 => self.side_focus_line(line, part + 1),
            _ => {
                let lines = self.side_lines();
                let cur = lines.iter().position(|l| *l == line).unwrap_or(0);
                let has = |l: &Line| !self.side_targets(*l).is_empty();
                let found = match vk {
                    v if v == VK_UP.0 => lines[..cur].iter().rev().find(|l| has(l)),
                    v if v == VK_DOWN.0 => lines.get(cur + 1..).unwrap_or_default().iter().find(|l| has(l)),
                    v if v == VK_HOME.0 => lines.iter().find(|l| has(l)),
                    _ => lines.iter().rev().find(|l| has(l)),
                };
                let Some(&next) = found else { return true };
                // 換行時選水平位置最接近的目標（色票、圖示格可以上下移動）
                let x = targets.get(part).map_or(0.0, |(_, r)| (r.l + r.r) / 2.0);
                if let Line::Row(k) = next {
                    self.side_reveal(k);
                }
                let near = self
                    .side_targets(next)
                    .iter()
                    .enumerate()
                    .min_by(|a, b| ((a.1.1.l + a.1.1.r) / 2.0 - x).abs().total_cmp(&((b.1.1.l + b.1.1.r) / 2.0 - x).abs()))
                    .map_or(0, |(i, _)| i);
                self.side_focus_line(next, near);
            }
        }
        true
    }

    /// 焦點框（鍵盤焦點在面板、按過方向鍵後）
    fn paint_side_focus(&self) {
        if self.focus != Focus::Side || !self.side_cursor.visible {
            return;
        }
        let Some(r) = self.side_targets(self.side_cursor.line).get(self.side_cursor.part).map(|t| t.1) else { return };
        // 捲出清單的列不畫
        if let (Line::Row(_), Some(g)) = (self.side_cursor.line, self.layout.side_panel.and_then(|p| self.side_geometry(p)))
            && (r.t < g.list.t - 1.0 || r.b > g.list.b + 1.0)
        {
            return;
        }
        self.gfx.stroke_round(r.inset(-1.0, -1.0), self.theme.metrics.radius, self.theme.color.accent, 2.0);
    }

    pub fn side_hit(&self, panel: Rect, x: f32, y: f32) -> Hit {
        match self.side_kind() {
            Some(SideKind::Settings) => self.settings_hit(panel, x, y),
            Some(SideKind::Theme) => self.theme_hit(panel, x, y),
            Some(SideKind::BarEdit) => self.cmdbar_edit_hit(panel, x, y),
            Some(SideKind::Keys) => self.keys_hit(panel, x, y),
            None => Hit::None,
        }
    }

    pub fn side_click(&mut self, hit: Hit, x: f32, y: f32) {
        self.side_cursor_to(&hit);
        match self.side_kind() {
            Some(SideKind::Settings) => self.settings_click(hit),
            Some(SideKind::Theme) => self.theme_click(hit, x),
            Some(SideKind::BarEdit) => self.cmdbar_edit_click(hit, y),
            Some(SideKind::Keys) => self.keys_click(hit),
            None => {}
        }
    }

    /// 滾輪：主題面板的數字欄位上微調，其他捲動清單
    pub fn side_wheel(&mut self, panel: Rect, x: f32, y: f32, delta: i16) {
        let notches = (delta as i32 / 120).signum();
        if self.side_kind() == Some(SideKind::Theme) && self.theme_nudge_at(panel, x, y, notches) {
            return;
        }
        let Some(g) = self.side_geometry(panel) else { return };
        let max = (g.rows as f32 * g.row_h - g.list.h()).max(0.0);
        let step = 3.0 * self.row_h();
        if let Some(scroll) = self.side_scroll_mut() {
            *scroll = (*scroll - notches as f32 * step).clamp(0.0, max);
        }
        self.invalidate();
    }

    /// Esc：面板先收起自己展開的東西（設定頁另外回到分類清單）；主題面板沒有展開時不處理（有未儲存的修改，
    /// 以「取消」按鈕關閉），其他面板關閉。回傳是否處理了
    pub fn side_escape(&mut self) -> bool {
        match self.side_kind() {
            Some(SideKind::Settings) => {
                self.settings_escape();
                true
            }
            Some(SideKind::Theme) => self.theme_escape(),
            Some(SideKind::BarEdit | SideKind::Keys) => {
                self.close_side_panels();
                true
            }
            None => false,
        }
    }

    pub fn paint_side(&self, r: Rect) {
        match self.side_kind() {
            Some(SideKind::Settings) => self.paint_settings(r),
            Some(SideKind::Theme) => self.paint_theme_editor(r),
            Some(SideKind::BarEdit) => self.paint_cmdbar_editor(r),
            Some(SideKind::Keys) => self.paint_keys_panel(r),
            None => return,
        }
        self.paint_side_focus();
        self.gfx.vline(r.l - 1.0, r.t, r.b, self.theme.color.border);
    }

    /// 底部兩個按鈕：`primary` 是強調色的那個，`hot(k)` 是游標是否在第 k 個上
    pub fn paint_footer_buttons(&self, buttons: &[Rect; 2], labels: [&'static str; 2], primary: Option<usize>, hot: impl Fn(usize) -> bool) {
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius;
        for (k, b) in buttons.iter().enumerate() {
            let main = primary == Some(k);
            let fill = match (main, hot(k)) {
                (true, true) => c.accent.with_alpha(220),
                (true, false) => c.accent,
                (false, true) => c.hover,
                (false, false) => c.background,
            };
            g.fill_round(*b, radius, fill);
            g.stroke_round(*b, radius, if main { c.accent } else { c.border }, 1.0);
            let label = zing_core::i18n::tr(labels[k]);
            let w = g.measure(label, Font::Text);
            let col = if main { crate::themepanel::contrast(c.accent) } else { c.text };
            g.text(label, Font::Text, Rect::new(b.l + ((b.w() - w) / 2.0).max(4.0), b.t, b.r - 4.0, b.b), col);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_scroll_except_pinned() {
        let list = Rect::new(0.0, 100.0, 300.0, 400.0);
        assert_eq!(row_rect(list, 0, 30.0, 45.0, 1).t, 100.0);
        assert_eq!(row_rect(list, 2, 30.0, 45.0, 1).t, 115.0);
        assert_eq!(row_rect(list, 2, 30.0, 45.0, 0).t, 115.0);
        assert_eq!(row_rect(list, 0, 30.0, 45.0, 0).t, 55.0);
    }

    #[test]
    fn footer_buttons_fill_the_bottom() {
        let [a, b] = footer_buttons(Rect::new(10.0, 0.0, 216.0, 500.0), 24.0);
        assert_eq!((a.l, a.b, b.r, b.b), (10.0, 500.0, 216.0, 500.0));
        assert_eq!(a.t, 470.0);
        assert_eq!(b.l - a.r, 6.0);
    }
}
