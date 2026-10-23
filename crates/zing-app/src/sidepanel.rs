//! 右側面板（設定頁、主題編輯、自訂命令列、快速鍵）：停靠在窗格區域最右側，一次只開一個（`App::side`）。
//!
//! 宿主（view.rs、input.rs）只經過這裡：位置與寬度（`side_width`）、命中（`side_hit`）、點擊（`side_click`）、
//! 滾輪（`side_wheel`）、Esc（`side_escape`）、繪製（`paint_side`）、關閉（`close_side_panels`）。
//! 各面板只提供自己的列、列的內容與動作；捲動、列的位置、捲到看得見（`side_reveal`）、底部兩個按鈕是共用的。
//! 面板的命中仍是各自的 `Hit` 變體，`Hit::side_panel` 判斷屬於面板的命中。

use crate::app::App;
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

    /// 開啟一個面板（取代開著的那個）
    pub fn open_side(&mut self, panel: SidePanel) {
        self.close_side_panels();
        self.side = Some(panel);
        self.invalidate();
    }

    /// 關閉右側的面板與它的輸入框；主題編輯面板保留目前的主題（不還原）
    pub fn close_side_panels(&mut self) {
        self.side = None;
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
