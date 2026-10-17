//! 主題編輯面板（命令 theme.edit）：停靠在窗格右側，每次修改立即套用到整個畫面（就是預覽）。
//!
//! 給不寫程式的人用：不必知道色碼或設定檔的寫法。
//! - 基礎：風格（緊湊／Fluent／折衷）與深淺色，換了之後保留已改過的欄位（`Theme::rebased`）。
//! - 顏色：點一下展開色票，點色票套用；「更多顏色…」開系統的選色對話框；點色碼可以直接輸入。
//! - 數字：拖曳滑桿、按 −／＋，或在上面捲動滑鼠滾輪。
//! - 選項：點一下展開選單；開關：點一下切換；字型：從系統安裝的字型中挑選。
//! - 檔案顏色：依類別（圖片、影片…）新增，也能自訂萬用字元樣式。
//! - 改過的欄位右側有「↺」，點一下還原成基礎風格的值。
//! - 「儲存」：使用者主題直接覆蓋；內建主題詢問名稱存成新主題。「取消」還原開啟面板前的主題。
//!
//! 檔案讀寫（儲存、匯入、匯出、以文字編輯器開啟）在 themeedit.rs。

use zing_core::theme::{
    Color, FIELDS, FILE_PRESETS, FieldKind, FileRule, MODE_CHOICES, STYLE_CHOICES, SWATCHES, Theme, choice_label, rule_label,
};

use crate::app::{App, EditKind, Modal, Press};
use crate::gfx::{Font, Rect};
use crate::services::Msg;
use crate::view::Hit;
use crate::window;

/// 面板寬度
pub const PANEL_W: f32 = 400.0;
/// 底部按鈕：儲存、取消
pub const BUTTONS: [&str; 2] = [tr_key!("儲存"), tr_key!("取消")];
/// 按鈕下方的進階功能：另存新主題、匯出、以文字編輯器開啟
pub const LINKS: [&str; 3] = [tr_key!("另存新主題…"), tr_key!("匯出…"), tr_key!("文字編輯器")];
/// 色票每列的格數
const SWATCH_COLS: usize = 9;

pub struct ThemeEditor {
    /// 開啟面板（或上次儲存）時的主題與名稱：取消時還原
    pub original: Theme,
    pub original_name: String,
    pub scroll: f32,
    /// 展開中的選單或色票
    pub expanded: Option<Target>,
    /// 系統字型（展開字型選單時才在背景讀取）：（英文名稱, 顯示名稱）
    pub fonts: Option<Vec<(String, String)>>,
}

/// 可以展開的項目
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// FIELDS 中的欄位：顏色展開色票，選項與字型展開選單
    Field(usize),
    Style,
    Mode,
    /// 檔案顏色規則的顏色
    Rule(usize),
    /// 新增檔案顏色
    AddRule,
}

/// 面板中的一列
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    Header(&'static str),
    Style,
    Mode,
    Field(usize),
    /// 色票的第幾列
    Swatches(Target, usize),
    /// 展開的選單的第幾個選項
    Option(Target, usize),
    Rule(usize),
    AddRule,
}

pub struct PanelLayout {
    pub title: Rect,
    pub note: Rect,
    pub list: Rect,
    pub buttons: [Rect; 2],
    pub links: [Rect; 3],
}

/// 一列中各個可點的位置
pub struct RowRects {
    pub row: Rect,
    /// 數值區（名稱右側）
    pub value: Rect,
    /// 色塊（顏色、檔案顏色）
    pub swatch: Rect,
    /// 色碼、樣式文字
    pub text: Rect,
    /// 數字：−、滑桿、＋
    pub minus: Rect,
    pub track: Rect,
    pub plus: Rect,
    /// 最右側的「↺」（還原）或「×」（刪除）
    pub tail: Rect,
}

fn is_font(i: usize) -> bool {
    FIELDS[i].path == ["font", "family"]
}

impl ThemeEditor {
    pub fn new(original: Theme, original_name: String) -> Self {
        ThemeEditor { original, original_name, scroll: 0.0, expanded: None, fonts: None }
    }

    /// 選單的選項：（值, 顯示名稱）
    pub fn options(&self, theme: &Theme, t: Target) -> Vec<(String, String)> {
        let tr = zing_core::i18n::tr;
        match t {
            Target::Style => STYLE_CHOICES.iter().map(|(s, l)| (s.name().to_string(), tr(l).to_string())).collect(),
            Target::Mode => MODE_CHOICES.iter().map(|(m, l)| (mode_code(*m).to_string(), tr(l).to_string())).collect(),
            Target::AddRule => FILE_PRESETS
                .iter()
                .map(|(l, p, _)| (p.to_string(), tr(l).to_string()))
                .chain(std::iter::once((String::new(), tr!("自訂樣式…").to_string())))
                .collect(),
            Target::Field(i) if is_font(i) => match &self.fonts {
                Some(f) => {
                    let cur = theme.font.family.clone();
                    let mut v = f.clone();
                    if !v.iter().any(|(n, _)| *n == cur) {
                        v.insert(0, (cur.clone(), cur));
                    }
                    v
                }
                None => Vec::new(),
            },
            Target::Field(i) => match FIELDS[i].kind {
                FieldKind::Choice(opts) => opts.iter().map(|o| (o.to_string(), choice_label(o).to_string())).collect(),
                _ => Vec::new(),
            },
            Target::Rule(_) => Vec::new(),
        }
    }

    pub fn rows(&self, theme: &Theme) -> Vec<Row> {
        let tr = zing_core::i18n::tr;
        let mut out = vec![Row::Header(tr!("基礎")), Row::Style];
        let expand = |out: &mut Vec<Row>, t: Target| {
            if self.expanded != Some(t) {
                return;
            }
            let color = match t {
                Target::Rule(_) => true,
                Target::Field(i) => FIELDS[i].kind == FieldKind::Color,
                _ => false,
            };
            if color {
                out.extend((0..(SWATCHES.len() + 2).div_ceil(SWATCH_COLS)).map(|l| Row::Swatches(t, l)));
            } else {
                out.extend((0..self.options(theme, t).len().max(1)).map(|k| Row::Option(t, k)));
            }
        };
        expand(&mut out, Target::Style);
        out.push(Row::Mode);
        expand(&mut out, Target::Mode);
        let mut group = "";
        for (i, f) in FIELDS.iter().enumerate() {
            if f.group != group {
                group = f.group;
                out.push(Row::Header(tr(group)));
            }
            out.push(Row::Field(i));
            expand(&mut out, Target::Field(i));
        }
        out.push(Row::Header(tr!("檔案顏色")));
        for r in 0..theme.color.file_rule.len() {
            out.push(Row::Rule(r));
            expand(&mut out, Target::Rule(r));
        }
        out.push(Row::AddRule);
        expand(&mut out, Target::AddRule);
        out
    }
}

/// 在 `bg` 上看得清楚的文字顏色（黑或白）
pub fn contrast(bg: Color) -> Color {
    let luma = 0.299 * bg.r as f32 + 0.587 * bg.g as f32 + 0.114 * bg.b as f32;
    if luma > 150.0 { Color::rgb(0x141414) } else { Color::rgb(0xffffff) }
}

fn mode_code(m: zing_core::theme::Mode) -> &'static str {
    match m {
        zing_core::theme::Mode::Dark => "dark",
        zing_core::theme::Mode::Light => "light",
    }
}

impl App {
    pub fn open_theme_editor(&mut self) {
        if self.theme_editor.is_none() {
            self.close_side_panels();
            self.theme_editor = Some(ThemeEditor::new(self.theme.clone(), self.theme_name.clone()));
        }
        self.invalidate();
    }

    /// 關閉面板；`revert` 為 true 時還原開啟前的主題
    pub fn close_theme_editor(&mut self, revert: bool) {
        if let Some(ed) = self.theme_editor.take()
            && revert
            && self.theme != ed.original
        {
            self.theme_name = ed.original_name;
            self.apply_theme(ed.original);
        }
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::ThemeField { .. } | EditKind::ThemeSaveAs | EditKind::ThemeRule { .. }))
            && let Some(e) = self.edit.take()
        {
            window::destroy_edit(e.hwnd);
        }
        self.invalidate();
    }

    pub fn theme_rows(&self) -> Vec<Row> {
        self.theme_editor.as_ref().map_or_else(Vec::new, |ed| ed.rows(&self.theme))
    }

    /// 內建主題顯示名稱（緊湊深色…），使用者主題顯示名稱本身
    pub fn theme_display_name(&self) -> String {
        zing_core::prefs::BUILTIN_THEMES
            .iter()
            .find(|(v, _)| v.eq_ignore_ascii_case(&self.theme_name))
            .map_or_else(|| self.theme_name.clone(), |(_, l)| zing_core::i18n::tr(l).to_string())
    }

    pub fn theme_panel_layout(&self, r: Rect) -> PanelLayout {
        let rh = self.row_h();
        let inner = r.inset(12.0, 10.0);
        let title = Rect::new(inner.l, inner.t, inner.r, inner.t + rh);
        let note = Rect::new(inner.l, title.b, inner.r, title.b + rh);
        let link_h = rh;
        let links_row = Rect::new(inner.l, inner.b - link_h, inner.r, inner.b);
        let bh = (rh + 6.0).max(28.0);
        let row = Rect::new(inner.l, links_row.t - 4.0 - bh, inner.r, links_row.t - 4.0);
        let half = (inner.w() - 6.0) / 2.0;
        let buttons = [Rect::new(row.l, row.t, row.l + half, row.b), Rect::new(row.l + half + 6.0, row.t, row.r, row.b)];
        let third = inner.w() / 3.0;
        let links = [0, 1, 2].map(|k| Rect::new(inner.l + third * k as f32, links_row.t, inner.l + third * (k + 1) as f32, links_row.b));
        let list = Rect::new(inner.l, note.b + 4.0, inner.r, row.t - 8.0);
        PanelLayout { title, note, list, buttons, links }
    }

    pub fn theme_row_rects(&self, list: Rect, k: usize, scroll: f32) -> RowRects {
        let rh = self.row_h();
        let y = list.t + k as f32 * rh - scroll;
        let row = Rect::new(list.l, y, list.r, y + rh);
        let tail = Rect::new(list.r - rh, y, list.r, y + rh);
        let value_l = list.l + (list.w() * 0.42).min(150.0);
        let value = Rect::new(value_l, y, tail.l - 4.0, y + rh);
        let sw_h = (rh - 8.0).max(10.0);
        let swatch = Rect::new(value_l, y + (rh - sw_h) / 2.0, value_l + sw_h * 1.6, y + (rh + sw_h) / 2.0);
        let text = Rect::new(swatch.r + 8.0, y, value.r, y + rh);
        // 數字：− 滑桿 ＋ 數值（數值寬 40）
        let minus = Rect::new(value_l, y + 2.0, value_l + rh - 4.0, y + rh - 2.0);
        let plus_r = value.r - 40.0;
        let plus = Rect::new(plus_r - (rh - 4.0), y + 2.0, plus_r, y + rh - 2.0);
        let track = Rect::new(minus.r + 8.0, y, plus.l - 8.0, y + rh);
        RowRects { row, value, swatch, text, minus, track, plus, tail }
    }

    /// 色票列中第 c 格的位置
    fn swatch_cell(&self, row: Rect, c: usize) -> Rect {
        let size = (row.h() - 6.0).max(12.0);
        let l = row.l + 8.0 + c as f32 * (size + 6.0);
        Rect::new(l, row.t + 3.0, l + size, row.t + 3.0 + size)
    }

    pub fn theme_hit(&self, panel: Rect, x: f32, y: f32) -> Hit {
        let Some(ed) = self.theme_editor.as_ref() else { return Hit::None };
        let pl = self.theme_panel_layout(panel);
        if let Some(k) = pl.buttons.iter().position(|r| r.contains(x, y)) {
            return Hit::ThemeButton(k);
        }
        if let Some(k) = pl.links.iter().position(|r| r.contains(x, y)) {
            return Hit::ThemeLink(k);
        }
        if !pl.list.contains(x, y) {
            return Hit::ThemePanel;
        }
        for (k, row) in ed.rows(&self.theme).iter().enumerate() {
            let rr = self.theme_row_rects(pl.list, k, ed.scroll);
            if !rr.row.contains(x, y) {
                continue;
            }
            return match *row {
                Row::Header(_) => Hit::ThemePanel,
                Row::Swatches(_, line) => {
                    let first = line * SWATCH_COLS;
                    (0..SWATCH_COLS)
                        .find(|c| {
                            let cell = self.swatch_cell(rr.row, *c);
                            // 「更多顏色…」佔最後兩格
                            let cell = if first + c == SWATCHES.len() { Rect::new(cell.l, cell.t, rr.row.r, cell.b) } else { cell };
                            cell.contains(x, y)
                        })
                        .filter(|c| first + c <= SWATCHES.len())
                        .map_or(Hit::ThemePanel, |c| Hit::ThemeCell(k, first + c))
                }
                Row::Field(i) => {
                    let f = &FIELDS[i];
                    if !self.theme.is_default(f) && rr.tail.contains(x, y) {
                        return Hit::ThemeReset(k);
                    }
                    match f.kind {
                        FieldKind::Color if rr.text.contains(x, y) => Hit::ThemeText(k),
                        FieldKind::Number { .. } if rr.minus.contains(x, y) => Hit::ThemeStep(k, false),
                        FieldKind::Number { .. } if rr.plus.contains(x, y) => Hit::ThemeStep(k, true),
                        FieldKind::Number { .. } if rr.track.contains(x, y) => Hit::ThemeSlider(k),
                        _ => Hit::ThemeRow(k),
                    }
                }
                Row::Rule(_) if rr.tail.contains(x, y) => Hit::ThemeDelete(k),
                Row::Rule(_) if rr.text.contains(x, y) => Hit::ThemeText(k),
                _ => Hit::ThemeRow(k),
            };
        }
        Hit::ThemePanel
    }

    fn toggle_expand(&mut self, t: Target) {
        let need_fonts = matches!(t, Target::Field(i) if is_font(i));
        if let Some(ed) = self.theme_editor.as_mut() {
            ed.expanded = if ed.expanded == Some(t) { None } else { Some(t) };
            if need_fonts && ed.fonts.is_none() && ed.expanded == Some(t) {
                ed.fonts = Some(Vec::new());
                let n = self.notifier.clone();
                self.pool.spawn(move || n.send(Msg::Fonts(crate::gfx::system_font_families())));
            }
        }
        self.invalidate();
    }

    pub fn on_fonts(&mut self, fonts: Vec<(String, String)>) {
        if let Some(ed) = self.theme_editor.as_mut() {
            ed.fonts = Some(fonts);
            self.invalidate();
        }
    }

    fn collapse(&mut self) {
        if let Some(ed) = self.theme_editor.as_mut() {
            ed.expanded = None;
        }
    }

    pub fn theme_click(&mut self, hit: Hit, x: f32) {
        let rows = self.theme_rows();
        let row = |k: usize| rows.get(k).copied();
        match hit {
            Hit::ThemeRow(k) => match row(k) {
                Some(Row::Style) => self.toggle_expand(Target::Style),
                Some(Row::Mode) => self.toggle_expand(Target::Mode),
                Some(Row::Field(i)) => match FIELDS[i].kind {
                    FieldKind::Bool => {
                        if let Some(t) = self.theme.cycle_field(&FIELDS[i]) {
                            self.apply_theme(t);
                        }
                    }
                    FieldKind::Number { .. } => self.begin_theme_field_edit(k, i),
                    _ => self.toggle_expand(Target::Field(i)),
                },
                Some(Row::Rule(r)) => self.toggle_expand(Target::Rule(r)),
                Some(Row::AddRule) => self.toggle_expand(Target::AddRule),
                Some(Row::Option(t, o)) => self.pick_option(t, o),
                _ => {}
            },
            Hit::ThemeText(k) => match row(k) {
                Some(Row::Field(i)) => self.begin_theme_field_edit(k, i),
                Some(Row::Rule(r)) => self.begin_rule_edit(k, r),
                _ => {}
            },
            Hit::ThemeCell(k, c) => {
                let Some(Row::Swatches(t, _)) = row(k) else { return };
                if c == SWATCHES.len() {
                    // 更多顏色：系統的選色對話框
                    let initial = self.target_color(t).map(|c| c.to_hex()).unwrap_or_default();
                    self.modal = Some(Modal::PickColor { target: t, initial });
                } else {
                    self.set_target_color(t, Color::rgb(SWATCHES[c]));
                }
            }
            Hit::ThemeReset(k) => {
                if let Some(Row::Field(i)) = row(k)
                    && let Some(t) = self.theme.reset_field(&FIELDS[i])
                {
                    self.apply_theme(t);
                }
            }
            Hit::ThemeStep(k, plus) => self.nudge_row(k, if plus { 1 } else { -1 }),
            Hit::ThemeSlider(k) => {
                if let Some(Row::Field(i)) = row(k) {
                    self.press = Some(Press::ThemeSlider { field: i, row: k });
                    self.theme_slide(x);
                }
            }
            Hit::ThemeDelete(k) => {
                if let Some(Row::Rule(r)) = row(k) {
                    let mut t = self.theme.clone();
                    if r < t.color.file_rule.len() {
                        t.color.file_rule.remove(r);
                    }
                    self.collapse();
                    self.apply_theme(t);
                }
            }
            Hit::ThemeButton(0) => self.save_theme(),
            Hit::ThemeButton(1) => self.close_theme_editor(true),
            Hit::ThemeLink(0) => self.begin_theme_save_as(),
            Hit::ThemeLink(1) => self.export_theme(),
            Hit::ThemeLink(2) => self.open_theme_file(),
            _ => {}
        }
    }

    fn pick_option(&mut self, t: Target, o: usize) {
        let Some(ed) = self.theme_editor.as_ref() else { return };
        let Some((value, _)) = ed.options(&self.theme, t).get(o).cloned() else { return };
        self.collapse();
        match t {
            Target::Style => {
                if let Some((s, _)) = STYLE_CHOICES.iter().find(|(s, _)| s.name() == value) {
                    let next = self.theme.rebased(*s, self.theme.mode);
                    self.apply_theme(next);
                }
            }
            Target::Mode => {
                if let Some((m, _)) = MODE_CHOICES.iter().find(|(m, _)| mode_code(*m) == value) {
                    let next = self.theme.rebased(self.theme.style, *m);
                    self.apply_theme(next);
                }
            }
            Target::Field(i) => self.commit_theme_field(i, &value),
            Target::AddRule => {
                let mut th = self.theme.clone();
                let color = FILE_PRESETS.iter().find(|(_, p, _)| *p == value).map_or(0xf2c230, |p| p.2);
                let custom = value.is_empty();
                th.color.file_rule.push(FileRule { pattern: if custom { "*.txt".into() } else { value }, text: Color::rgb(color) });
                let r = th.color.file_rule.len() - 1;
                self.apply_theme(th);
                if custom {
                    // 自訂：直接輸入樣式
                    let rows = self.theme_rows();
                    if let Some(k) = rows.iter().position(|x| *x == Row::Rule(r)) {
                        self.begin_rule_edit(k, r);
                    }
                }
            }
            Target::Rule(_) => {}
        }
        self.invalidate();
    }

    fn target_color(&self, t: Target) -> Option<Color> {
        match t {
            Target::Field(i) => Color::parse(&self.theme.field_value(&FIELDS[i])).ok(),
            Target::Rule(r) => self.theme.color.file_rule.get(r).map(|r| r.text),
            _ => None,
        }
    }

    /// 色票或選色對話框選了顏色
    pub fn set_target_color(&mut self, t: Target, c: Color) {
        match t {
            Target::Field(i) => self.commit_theme_field(i, &c.to_hex()),
            Target::Rule(r) => {
                let mut th = self.theme.clone();
                if let Some(rule) = th.color.file_rule.get_mut(r) {
                    rule.text = c;
                    self.apply_theme(th);
                }
            }
            _ => {}
        }
    }

    /// 選色對話框的結果
    pub fn theme_pick_color(&mut self, t: Target, hex: String) {
        match Color::parse(&hex) {
            Ok(c) => self.set_target_color(t, c),
            Err(e) => self.set_status(e, true),
        }
    }

    fn nudge_row(&mut self, k: usize, steps: i32) {
        if let Some(Row::Field(i)) = self.theme_rows().get(k).copied()
            && let Some(t) = self.theme.nudge_field(&FIELDS[i], steps)
        {
            self.apply_theme(t);
        }
    }

    /// 拖曳滑桿（Press::ThemeSlider）
    pub fn theme_slide(&mut self, x: f32) {
        let Some(Press::ThemeSlider { field, row }) = self.press else { return };
        let (Some(panel), Some(ed)) = (self.layout.theme_panel, self.theme_editor.as_ref()) else { return };
        let FieldKind::Number { min, max, step } = FIELDS[field].kind else { return };
        let pl = self.theme_panel_layout(panel);
        let tr = self.theme_row_rects(pl.list, row, ed.scroll).track;
        let frac = ((x - tr.l) / tr.w().max(1.0)).clamp(0.0, 1.0);
        let v = ((min + frac * (max - min)) / step).round() * step;
        if let Ok(t) = self.theme.with_field(&FIELDS[field], &format!("{}", v.clamp(min, max)))
            && t != self.theme
        {
            self.apply_theme(t);
        }
    }

    /// 滾輪：數字欄位上微調，其他地方捲動清單
    pub fn theme_wheel(&mut self, panel: Rect, x: f32, y: f32, delta: i16) {
        let notches = (delta as i32 / 120).signum();
        let k = match self.theme_hit(panel, x, y) {
            Hit::ThemeRow(k) | Hit::ThemeStep(k, _) | Hit::ThemeSlider(k) => Some(k),
            _ => None,
        };
        if let Some(k) = k
            && let Some(Row::Field(i)) = self.theme_rows().get(k).copied()
            && matches!(FIELDS[i].kind, FieldKind::Number { .. })
        {
            self.nudge_row(k, notches);
            return;
        }
        let pl = self.theme_panel_layout(panel);
        let max = (self.theme_rows().len() as f32 * self.row_h() - pl.list.h()).max(0.0);
        let step = 3.0 * self.theme.metrics.row_height;
        if let Some(ed) = self.theme_editor.as_mut() {
            ed.scroll = (ed.scroll - notches as f32 * step).clamp(0.0, max);
        }
        self.invalidate();
    }

    /// Esc：收起展開的選單；沒有展開時回傳 false
    pub fn theme_escape(&mut self) -> bool {
        match self.theme_editor.as_mut() {
            Some(ed) if ed.expanded.is_some() => {
                ed.expanded = None;
                self.invalidate();
                true
            }
            _ => false,
        }
    }

    fn begin_theme_field_edit(&mut self, k: usize, i: usize) {
        let Some(panel) = self.layout.theme_panel else { return };
        let Some(ed) = self.theme_editor.as_ref() else { return };
        let pl = self.theme_panel_layout(panel);
        let rr = self.theme_row_rects(pl.list, k, ed.scroll);
        let r = match FIELDS[i].kind {
            FieldKind::Color => rr.text,
            FieldKind::Number { .. } => Rect::new(rr.plus.r + 2.0, rr.row.t, rr.value.r, rr.row.b),
            _ => rr.value,
        };
        let text = self.theme.field_value(&FIELDS[i]);
        let len = text.encode_utf16().count();
        window::begin_edit(self, r, &text, len, EditKind::ThemeField { field: i });
    }

    fn begin_rule_edit(&mut self, k: usize, r: usize) {
        let Some(panel) = self.layout.theme_panel else { return };
        let Some(ed) = self.theme_editor.as_ref() else { return };
        let Some(rule) = self.theme.color.file_rule.get(r) else { return };
        let pl = self.theme_panel_layout(panel);
        let rr = self.theme_row_rects(pl.list, k, ed.scroll);
        let text = rule.pattern.clone();
        let len = text.encode_utf16().count();
        window::begin_edit(self, Rect::new(rr.text.l, rr.row.t, rr.tail.l - 4.0, rr.row.b), &text, len, EditKind::ThemeRule { rule: r });
    }

    pub fn begin_theme_save_as(&mut self) {
        let Some(panel) = self.layout.theme_panel else { return };
        let pl = self.theme_panel_layout(panel);
        let b = pl.buttons[0];
        let r = Rect::new(pl.list.l, b.t - 8.0 - self.row_h(), pl.list.r, b.t - 8.0);
        let suggest = if self.is_user_theme() { tr!("{name} 複本", name = self.theme_name) } else { tr!("我的主題").to_string() };
        let len = suggest.encode_utf16().count();
        window::begin_edit(self, r, &suggest, len, EditKind::ThemeSaveAs);
    }

    /// 欄位輸入框確認
    pub fn commit_theme_field(&mut self, i: usize, text: &str) {
        match self.theme.with_field(&FIELDS[i], text) {
            Ok(t) => self.apply_theme(t),
            Err(e) => self.set_status(e, true),
        }
    }

    /// 檔案顏色的樣式輸入框確認（空白表示刪除）
    pub fn commit_rule_pattern(&mut self, r: usize, text: &str) {
        let mut th = self.theme.clone();
        let p = text.trim();
        if r >= th.color.file_rule.len() {
            return;
        }
        if p.is_empty() {
            th.color.file_rule.remove(r);
        } else {
            th.color.file_rule[r].pattern = p.to_string();
        }
        self.apply_theme(th);
    }

    /// 儲存：使用者主題直接覆蓋，內建主題詢問名稱另存
    pub fn save_theme(&mut self) {
        if self.is_user_theme() { self.save_theme_overwrite() } else { self.begin_theme_save_as() }
    }

    pub fn paint_theme_editor(&self, r: Rect) {
        let Some(ed) = self.theme_editor.as_ref() else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius;
        let tr = zing_core::i18n::tr;
        g.fill(r, c.surface);
        let pl = self.theme_panel_layout(r);
        let dirty = self.theme != ed.original;
        let title = tr!("主題：{name}", name = self.theme_display_name());
        g.text(&title, Font::Bold, pl.title, c.text);
        if dirty {
            let w = g.measure(&title, Font::Bold);
            g.text(tr!("● 尚未儲存"), Font::Small, Rect::new(pl.title.l + w + 10.0, pl.title.t, pl.title.r, pl.title.b), c.accent);
        }
        g.text(tr!("變更會立即顯示在畫面上，按「儲存」保留"), Font::Small, pl.note, c.text_secondary);
        g.push_clip(pl.list);
        for (k, row) in ed.rows(&self.theme).iter().enumerate() {
            let rr = self.theme_row_rects(pl.list, k, ed.scroll);
            if rr.row.b < pl.list.t || rr.row.t > pl.list.b {
                continue;
            }
            let hovered = |h: &Hit| matches!(h, Hit::ThemeRow(x) | Hit::ThemeText(x) | Hit::ThemeReset(x) | Hit::ThemeStep(x, _) | Hit::ThemeSlider(x) | Hit::ThemeDelete(x) if *x == k);
            let hover = hovered(&self.hover);
            let label_rect = Rect::new(rr.row.l + 8.0, rr.row.t, rr.value.l - 6.0, rr.row.b);
            let arrow = |t: Target| if ed.expanded == Some(t) { "▴" } else { "▾" };
            match *row {
                Row::Header(h) => g.text(h, Font::Bold, Rect::new(rr.row.l, rr.row.t + 2.0, rr.row.r, rr.row.b), c.text_secondary),
                Row::Style | Row::Mode => {
                    if hover {
                        g.fill_round(rr.row, radius, c.hover);
                    }
                    let (label, value, t) = if *row == Row::Style {
                        let v = STYLE_CHOICES.iter().find(|(s, _)| *s == self.theme.style).map_or("", |x| tr(x.1));
                        (tr!("風格"), v, Target::Style)
                    } else {
                        let v = MODE_CHOICES.iter().find(|(m, _)| *m == self.theme.mode).map_or("", |x| tr(x.1));
                        (tr!("深淺色"), v, Target::Mode)
                    };
                    g.text(label, Font::Text, label_rect, c.text);
                    g.text(&format!("{value} {}", arrow(t)), Font::Text, rr.value, c.accent);
                }
                Row::Field(i) => {
                    let f = &FIELDS[i];
                    if hover {
                        g.fill_round(rr.row, radius, c.hover);
                    }
                    g.text(tr(f.label), Font::Text, label_rect, c.text);
                    let v = self.theme.field_value(f);
                    match f.kind {
                        FieldKind::Color => {
                            if let Ok(col) = Color::parse(&v) {
                                g.fill_round(rr.swatch, 3.0, col);
                                g.stroke_round(rr.swatch, 3.0, c.border, 1.0);
                            }
                            let hex_col = if self.hover == Hit::ThemeText(k) { c.accent } else { c.text_secondary };
                            g.text(&v, Font::Small, rr.text, hex_col);
                        }
                        FieldKind::Number { min, max, .. } => {
                            let x: f32 = v.parse().unwrap_or(min);
                            for (b, s) in [(rr.minus, "−"), (rr.plus, "+")] {
                                g.stroke_round(b, b.h() / 2.0, c.border, 1.0);
                                let w = g.measure(s, Font::Text);
                                g.text(s, Font::Text, Rect::new(b.l + (b.w() - w) / 2.0, b.t, b.r, b.b), c.text);
                            }
                            let cy = (rr.track.t + rr.track.b) / 2.0;
                            let line = Rect::new(rr.track.l, cy - 2.0, rr.track.r, cy + 2.0);
                            g.fill_round(line, 2.0, c.border);
                            let frac = ((x - min) / (max - min)).clamp(0.0, 1.0);
                            let kx = rr.track.l + frac * rr.track.w();
                            g.fill_round(Rect::new(line.l, line.t, kx, line.b), 2.0, c.accent);
                            g.fill_round(Rect::new(kx - 6.0, cy - 6.0, kx + 6.0, cy + 6.0), 6.0, c.accent);
                            g.text(&v, Font::Text, Rect::new(rr.plus.r + 6.0, rr.row.t, rr.value.r, rr.row.b), c.text_secondary);
                        }
                        FieldKind::Bool => {
                            let on = v == "true";
                            let h = (rr.row.h() - 10.0).clamp(10.0, 18.0);
                            let track = Rect::new(rr.value.l, rr.row.t + (rr.row.h() - h) / 2.0, rr.value.l + h * 1.8, rr.row.t + (rr.row.h() + h) / 2.0);
                            g.fill_round(track, h / 2.0, if on { c.accent } else { c.background });
                            g.stroke_round(track, h / 2.0, if on { c.accent } else { c.border }, 1.0);
                            let d = h - 6.0;
                            let kx = if on { track.r - 3.0 - d } else { track.l + 3.0 };
                            g.fill_round(Rect::new(kx, track.t + 3.0, kx + d, track.b - 3.0), d / 2.0, if on { c.background } else { c.text_secondary });
                        }
                        _ => {
                            let shown = if is_font(i) {
                                ed.fonts.as_ref().and_then(|fs| fs.iter().find(|(n, _)| *n == v)).map_or(v.clone(), |(_, l)| l.clone())
                            } else {
                                choice_label(&v).to_string()
                            };
                            g.text(&format!("{shown} {}", arrow(Target::Field(i))), Font::Text, rr.value, c.accent);
                        }
                    }
                    if !self.theme.is_default(f) {
                        let col = if self.hover == Hit::ThemeReset(k) { c.accent } else { c.text_secondary };
                        g.text("↺", Font::Text, Rect::new(rr.tail.l + 4.0, rr.row.t, rr.tail.r, rr.row.b), col);
                    }
                }
                Row::Swatches(t, line) => {
                    let current = self.target_color(t);
                    for col in 0..SWATCH_COLS {
                        let idx = line * SWATCH_COLS + col;
                        let cell = self.swatch_cell(rr.row, col);
                        let hot = self.hover == Hit::ThemeCell(k, idx);
                        if idx < SWATCHES.len() {
                            let sw = Color::rgb(SWATCHES[idx]);
                            g.fill_round(cell, 3.0, sw);
                            let selected = current == Some(sw);
                            g.stroke_round(cell, 3.0, if selected || hot { c.accent } else { c.border }, if selected { 2.0 } else { 1.0 });
                        } else if idx == SWATCHES.len() {
                            let r = Rect::new(cell.l, rr.row.t, rr.row.r, rr.row.b);
                            g.text(tr!("更多顏色…"), Font::Small, r, if hot { c.accent } else { c.text_secondary });
                        }
                    }
                }
                Row::Option(t, o) => {
                    let options = ed.options(&self.theme, t);
                    let ir = Rect::new(rr.value.l - 12.0, rr.row.t, rr.row.r, rr.row.b);
                    g.fill_round(ir, radius, if hover { c.hover } else { c.background });
                    let Some((v, label)) = options.get(o) else {
                        g.text(tr!("載入中…"), Font::Text, Rect::new(ir.l + 24.0, rr.row.t, ir.r, rr.row.b), c.text_secondary);
                        continue;
                    };
                    let current = match t {
                        Target::Style => self.theme.style.name() == v,
                        Target::Mode => mode_code(self.theme.mode) == v,
                        Target::Field(i) => self.theme.field_value(&FIELDS[i]) == *v,
                        _ => false,
                    };
                    if current {
                        g.text("✓", Font::Text, Rect::new(ir.l + 6.0, rr.row.t, ir.l + 22.0, rr.row.b), c.accent);
                    }
                    g.text(label, Font::Text, Rect::new(ir.l + 24.0, rr.row.t, ir.r - 6.0, rr.row.b), if current { c.accent } else { c.text });
                }
                Row::Rule(r) => {
                    let Some(rule) = self.theme.color.file_rule.get(r) else { continue };
                    if hover {
                        g.fill_round(rr.row, radius, c.hover);
                    }
                    let name = rule_label(&rule.pattern).unwrap_or(tr!("自訂"));
                    g.text(name, Font::Text, label_rect, rule.text);
                    g.fill_round(rr.swatch, 3.0, rule.text);
                    g.stroke_round(rr.swatch, 3.0, c.border, 1.0);
                    let pc = if self.hover == Hit::ThemeText(k) { c.accent } else { c.text_secondary };
                    g.text(&rule.pattern, Font::Small, rr.text, pc);
                    let dc = if self.hover == Hit::ThemeDelete(k) { c.error } else { c.text_secondary };
                    g.text("×", Font::Text, Rect::new(rr.tail.l + 4.0, rr.row.t, rr.tail.r, rr.row.b), dc);
                }
                Row::AddRule => {
                    if hover {
                        g.fill_round(rr.row, radius, c.hover);
                    }
                    g.text(&format!("＋ {}", tr!("新增檔案顏色")), Font::Text, Rect::new(rr.row.l + 8.0, rr.row.t, rr.row.r, rr.row.b), c.accent);
                }
            }
        }
        g.pop_clip();
        for (k, b) in pl.buttons.iter().enumerate() {
            let primary = k == 0;
            let hot = self.hover == Hit::ThemeButton(k);
            let fill = if primary { c.accent } else if hot { c.hover } else { c.background };
            g.fill_round(*b, radius, if primary && hot { c.accent.with_alpha(220) } else { fill });
            g.stroke_round(*b, radius, if primary { c.accent } else { c.border }, 1.0);
            let label = tr(BUTTONS[k]);
            let w = g.measure(label, Font::Text);
            let l = b.l + ((b.w() - w) / 2.0).max(4.0);
            g.text(label, Font::Text, Rect::new(l, b.t, b.r - 4.0, b.b), if primary { contrast(c.accent) } else { c.text });
        }
        for (k, b) in pl.links.iter().enumerate() {
            let label = tr(LINKS[k]);
            let w = g.measure(label, Font::Small);
            let l = b.l + ((b.w() - w) / 2.0).max(2.0);
            let col = if self.hover == Hit::ThemeLink(k) { c.accent } else { c.text_secondary };
            g.text(label, Font::Small, Rect::new(l, b.t, b.r, b.b), col);
        }
    }
}
