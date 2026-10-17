//! 設定頁（命令 settings.open，預設 Ctrl+,）：停靠在窗格右側（與主題編輯面板同一個位置，兩者只開一個），
//! 列出 `zing_core::prefs::PREFS`。
//!
//! - 選項（語言、主題、位置）：點一下展開選項，再點選一個；開關：點一下切換；文字：點一下輸入，Enter 確定。
//! - 每次修改立即套用，並在背景只改寫 config.toml 中的那一行（保留註解，見 `zing_core::prefs::write_pref`）；
//!   隔離模式（冒煙測試）不寫檔。
//! - 其他設定（工具列按鈕、縮圖產生器、快速鍵…）以列出的命令開啟對應的編輯器。

use zing_core::prefs::{BUILTIN_THEMES, Kind, PREFS, choice_label, toml_literal};

use crate::app::{App, EditKind};
use crate::gfx::{Font, Rect};
use crate::services::Msg;
use crate::settings;
use crate::view::Hit;
use crate::window;

/// 面板寬度
pub const PANEL_W: f32 = 420.0;
/// 底部按鈕：以文字編輯器開啟設定檔、關閉
pub const BUTTONS: [&str; 2] = [tr_key!("編輯設定檔"), tr_key!("關閉")];

#[derive(Default)]
pub struct SettingsPage {
    pub scroll: f32,
    /// 展開選項的欄位
    pub expanded: Option<usize>,
    /// themes 資料夾中的使用者主題（開啟時在背景讀取）
    pub user_themes: Vec<String>,
}

/// 面板中的一列
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    Header(&'static str),
    Field(usize),
    /// 欄位的說明（小字）
    Hint(usize),
    /// 展開的欄位的第 k 個選項
    Option(usize, usize),
}

pub struct PanelLayout {
    pub title: Rect,
    pub list: Rect,
    pub note: Rect,
    pub buttons: [Rect; 2],
}

impl SettingsPage {
    /// 欄位的選項：（設定檔中的值, 顯示名稱）
    pub fn options(&self, i: usize) -> Vec<(String, String)> {
        let p = &PREFS[i];
        match p.kind {
            Kind::Choice(list) => list.iter().map(|(v, _)| (v.to_string(), choice_label(p, v))).collect(),
            Kind::Theme => BUILTIN_THEMES
                .iter()
                .map(|(v, _)| (v.to_string(), choice_label(p, v)))
                .chain(self.user_themes.iter().map(|t| (t.clone(), t.clone())))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        let mut group = "";
        for (i, p) in PREFS.iter().enumerate() {
            if p.group != group {
                group = p.group;
                out.push(Row::Header(zing_core::i18n::tr(group)));
            }
            out.push(Row::Field(i));
            if !p.hint.is_empty() {
                out.push(Row::Hint(i));
            }
            if self.expanded == Some(i) {
                out.extend((0..self.options(i).len()).map(|k| Row::Option(i, k)));
            }
        }
        out
    }
}

impl App {
    pub fn open_settings(&mut self) {
        if self.theme_editor.is_some() {
            self.close_theme_editor(false);
        }
        if self.settings_page.is_none() {
            self.settings_page = Some(SettingsPage::default());
            let n = self.notifier.clone();
            self.pool.spawn(move || n.send(Msg::UserThemes(settings::user_themes())));
        }
        self.invalidate();
    }

    pub fn close_settings(&mut self) {
        self.settings_page = None;
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::Pref { .. }))
            && let Some(e) = self.edit.take()
        {
            window::destroy_edit(e.hwnd);
        }
        self.invalidate();
    }

    pub fn on_user_themes(&mut self, themes: Vec<String>) {
        if let Some(page) = self.settings_page.as_mut() {
            page.user_themes = themes;
            self.invalidate();
        }
    }

    pub fn settings_layout(&self, r: Rect) -> PanelLayout {
        let rh = self.row_h();
        let inner = r.inset(12.0, 10.0);
        let title = Rect::new(inner.l, inner.t, inner.r, inner.t + rh);
        let bh = (rh + 6.0).max(26.0);
        let row = Rect::new(inner.l, inner.b - bh, inner.r, inner.b);
        let half = (inner.w() - 6.0) / 2.0;
        let buttons = [Rect::new(row.l, row.t, row.l + half, row.b), Rect::new(row.l + half + 6.0, row.t, row.r, row.b)];
        let note = Rect::new(inner.l, row.t - 6.0 - rh, inner.r, row.t - 6.0);
        let list = Rect::new(inner.l, title.b + 6.0, inner.r, note.t - 4.0);
        PanelLayout { title, list, note, buttons }
    }

    /// 第 k 列的位置與其中數值的範圍
    pub fn settings_row_rects(&self, list: Rect, k: usize, scroll: f32) -> (Rect, Rect) {
        let rh = self.row_h();
        let y = list.t + k as f32 * rh - scroll;
        let row = Rect::new(list.l, y, list.r, y + rh);
        let value_l = list.l + (list.w() * 0.5).min(200.0);
        (row, Rect::new(value_l, y, list.r, y + rh))
    }

    pub fn settings_hit(&self, panel: Rect, x: f32, y: f32) -> Hit {
        let Some(page) = self.settings_page.as_ref() else { return Hit::None };
        let pl = self.settings_layout(panel);
        if let Some(k) = pl.buttons.iter().position(|r| r.contains(x, y)) {
            return Hit::SettingsButton(k);
        }
        if pl.list.contains(x, y) {
            for (k, row) in page.rows().iter().enumerate() {
                if matches!(row, Row::Field(_) | Row::Option(..)) && self.settings_row_rects(pl.list, k, page.scroll).0.contains(x, y) {
                    return Hit::SettingsRow(k);
                }
            }
        }
        Hit::SettingsPanel
    }

    pub fn settings_click(&mut self, hit: Hit) {
        match hit {
            Hit::SettingsRow(k) => {
                let Some(page) = self.settings_page.as_ref() else { return };
                match page.rows().get(k).copied() {
                    Some(Row::Field(i)) => match PREFS[i].kind {
                        Kind::Choice(_) | Kind::Theme => {
                            if let Some(page) = self.settings_page.as_mut() {
                                page.expanded = if page.expanded == Some(i) { None } else { Some(i) };
                            }
                            self.invalidate();
                        }
                        Kind::Bool => {
                            let on = self.config.pref_value(PREFS[i].key) == "true";
                            self.set_pref(i, (!on).to_string());
                        }
                        Kind::Text => self.begin_pref_edit(i),
                        Kind::Action(cmd) => self.exec(cmd),
                    },
                    Some(Row::Option(i, o)) => {
                        let value = page.options(i).get(o).map(|(v, _)| v.clone());
                        if let Some(page) = self.settings_page.as_mut() {
                            page.expanded = None;
                        }
                        if let Some(v) = value {
                            self.set_pref(i, v);
                        }
                    }
                    _ => {}
                }
            }
            Hit::SettingsButton(0) => self.exec("config.edit"),
            Hit::SettingsButton(1) => self.close_settings(),
            _ => {}
        }
    }

    pub fn settings_wheel(&mut self, panel: Rect, delta: i16) {
        let pl = self.settings_layout(panel);
        let rh = self.row_h();
        let step = 3.0 * self.theme.metrics.row_height;
        if let Some(page) = self.settings_page.as_mut() {
            let max = (page.rows().len() as f32 * rh - pl.list.h()).max(0.0);
            page.scroll = (page.scroll - (delta as i32 / 120).signum() as f32 * step).clamp(0.0, max);
        }
        self.invalidate();
    }

    /// Esc：先收起展開的選項，再關閉設定頁
    pub fn settings_escape(&mut self) {
        match self.settings_page.as_mut() {
            Some(page) if page.expanded.is_some() => {
                page.expanded = None;
                self.invalidate();
            }
            Some(_) => self.close_settings(),
            None => {}
        }
    }

    fn begin_pref_edit(&mut self, i: usize) {
        let Some(panel) = self.layout.settings_panel else { return };
        let Some(page) = self.settings_page.as_ref() else { return };
        let pl = self.settings_layout(panel);
        let Some(k) = page.rows().iter().position(|r| *r == Row::Field(i)) else { return };
        let (_, value) = self.settings_row_rects(pl.list, k, page.scroll);
        let text = self.config.pref_value(PREFS[i].key);
        let len = text.encode_utf16().count();
        window::begin_edit(self, value, &text, len, EditKind::Pref { field: i });
    }

    /// 修改一個設定：立即套用，並在背景寫入 config.toml
    pub fn set_pref(&mut self, i: usize, value: String) {
        let p = &PREFS[i];
        if self.config.pref_value(p.key) == value {
            self.invalidate();
            return;
        }
        if let Err(e) = self.config.set_pref(p.key, &value) {
            self.set_status(e, true);
            return;
        }
        match p.key {
            "language" => self.set_language(self.config.language),
            "theme" => self.load_theme_by_name(value.clone()),
            "show_hidden" => self.set_show_hidden(self.config.show_hidden),
            "layout.tree" => {
                self.tree_side = self.config.layout.tree;
                // 選了左側或右側就顯示樹狀列
                if self.tree_side != zing_core::config::TreeSide::Hidden {
                    self.tree_visible = true;
                }
            }
            _ => {}
        }
        self.invalidate();
        if self.isolated {
            return;
        }
        let (key, literal) = (p.key, toml_literal(p, &value));
        let n = self.notifier.clone();
        self.pool.spawn(move || match settings::write_pref(key, &literal) {
            Ok(()) => n.status(tr!("已儲存到 config.toml"), false),
            Err(e) => n.status(e, true),
        });
    }

    pub fn paint_settings(&self, r: Rect) {
        let Some(page) = self.settings_page.as_ref() else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius;
        g.fill(r, c.surface);
        let pl = self.settings_layout(r);
        g.text(tr!("設定"), Font::Bold, pl.title, c.text);
        g.push_clip(pl.list);
        for (k, row) in page.rows().iter().enumerate() {
            let (rr, value) = self.settings_row_rects(pl.list, k, page.scroll);
            if rr.b < pl.list.t || rr.t > pl.list.b {
                continue;
            }
            let hover = self.hover == Hit::SettingsRow(k);
            match *row {
                Row::Header(h) => g.text(h, Font::Bold, Rect::new(rr.l, rr.t + 2.0, rr.r, rr.b), c.text_secondary),
                Row::Hint(i) => {
                    g.text(zing_core::i18n::tr(PREFS[i].hint), Font::Small, Rect::new(rr.l + 8.0, rr.t, rr.r - 6.0, rr.b), c.text_secondary)
                }
                Row::Field(i) => {
                    let p = &PREFS[i];
                    if hover {
                        g.fill_round(rr, radius, c.hover);
                    }
                    let action = matches!(p.kind, Kind::Action(_));
                    let label_r = if action { rr.r - 24.0 } else { value.l - 6.0 };
                    g.text(zing_core::i18n::tr(p.label), Font::Text, Rect::new(rr.l + 8.0, rr.t, label_r, rr.b), c.text);
                    let v = self.config.pref_value(p.key);
                    let vr = Rect::new(value.l, rr.t, rr.r - 6.0, rr.b);
                    match p.kind {
                        Kind::Choice(_) | Kind::Theme => {
                            let arrow = if page.expanded == Some(i) { "▴" } else { "▾" };
                            g.text(&format!("{} {arrow}", choice_label(p, &v)), Font::Text, vr, c.accent);
                        }
                        Kind::Bool => {
                            // 開關：軌道與圓鈕
                            let on = v == "true";
                            let h = (rr.h() - 10.0).clamp(10.0, 18.0);
                            let track = Rect::new(vr.l, rr.t + (rr.h() - h) / 2.0, vr.l + h * 1.8, rr.t + (rr.h() + h) / 2.0);
                            g.fill_round(track, h / 2.0, if on { c.accent } else { c.background });
                            g.stroke_round(track, h / 2.0, if on { c.accent } else { c.border }, 1.0);
                            let d = h - 6.0;
                            let x = if on { track.r - 3.0 - d } else { track.l + 3.0 };
                            g.fill_round(Rect::new(x, track.t + 3.0, x + d, track.b - 3.0), d / 2.0, if on { c.background } else { c.text_secondary });
                            let label = if on { tr!("開") } else { tr!("關") };
                            g.text(label, Font::Text, Rect::new(track.r + 8.0, rr.t, vr.r, rr.b), c.text_secondary);
                        }
                        Kind::Text if v.is_empty() => g.text(tr!("（未設定）"), Font::Text, vr, c.text_secondary),
                        Kind::Text => g.text(&v, Font::Text, vr, c.text_secondary),
                        Kind::Action(_) => g.text("›", Font::Text, Rect::new(rr.r - 18.0, rr.t, rr.r, rr.b), c.text_secondary),
                    }
                }
                Row::Option(i, o) => {
                    let options = page.options(i);
                    let Some((v, label)) = options.get(o) else { continue };
                    let current = self.config.pref_value(PREFS[i].key).eq_ignore_ascii_case(v);
                    let ir = Rect::new(value.l - 12.0, rr.t, rr.r, rr.b);
                    g.fill_round(ir, radius, if hover { c.hover } else { c.background });
                    if current {
                        g.text("✓", Font::Text, Rect::new(ir.l + 6.0, rr.t, ir.l + 22.0, rr.b), c.accent);
                    }
                    g.text(label, Font::Text, Rect::new(ir.l + 24.0, rr.t, ir.r - 6.0, rr.b), if current { c.accent } else { c.text });
                }
            }
        }
        g.pop_clip();
        g.text(tr!("變更立即生效，並儲存到程式旁的 config.toml"), Font::Small, pl.note, c.text_secondary);
        for (k, b) in pl.buttons.iter().enumerate() {
            let fill = if self.hover == Hit::SettingsButton(k) { c.hover } else { c.background };
            g.fill_round(*b, radius, fill);
            g.stroke_round(*b, radius, c.border, 1.0);
            let label = zing_core::i18n::tr(BUTTONS[k]);
            let w = g.measure(label, Font::Text);
            let l = b.l + ((b.w() - w) / 2.0).max(4.0);
            g.text(label, Font::Text, Rect::new(l, b.t, b.r - 4.0, b.b), c.text);
        }
    }
}
