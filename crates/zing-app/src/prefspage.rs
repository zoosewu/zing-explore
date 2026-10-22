//! 設定頁（命令 settings.open，預設 Ctrl+,）：停靠在窗格右側（與主題編輯面板同一個位置，兩者只開一個），
//! 列出 `zing_core::prefs::PREFS`。
//!
//! - 先選分類（`prefs::groups()`，每個分類下方列出它的項目），再選要設定的項目；標題列的「‹」或 Esc 回到分類清單。
//! - 選項（語言、主題、位置）：點一下展開選項，再點選一個；開關：點一下切換；文字：點一下輸入，Enter 確定。
//! - 緩慢的右鍵選單擴充功能（`Kind::SlowList`）：表格逐一切換是否照樣放在第一層；「隱藏緩慢的擴充功能項目」
//!   關閉時表格以灰階顯示、不能切換。表格的內容在進入分類時於背景讀取（`shellext::slow_list`）。
//! - 每次修改立即套用，並在背景只改寫 config.toml 中的那一行（保留註解，見 `zing_core::prefs::write_pref`）；
//!   隔離模式（冒煙測試）不寫檔。
//! - 其他設定（工具列按鈕、縮圖產生器、快速鍵…）以列出的命令開啟對應的編輯器。

use zing_core::prefs::{BUILTIN_THEMES, Kind, PREFS, choice_label, groups, toml_literal};

use crate::app::{App, EditKind};
use crate::gfx::{Font, Rect};
use crate::services::Msg;
use crate::settings;
use crate::shellext::SlowExt;
use crate::sidepanel::{footer_buttons, row_rect};
use crate::view::Hit;
use crate::window;

/// 面板寬度
pub const PANEL_W: f32 = 420.0;
/// 底部按鈕：以文字編輯器開啟設定檔、關閉
pub const BUTTONS: [&str; 2] = [tr_key!("編輯設定檔"), tr_key!("關閉")];

#[derive(Default)]
pub struct SettingsPage {
    pub scroll: f32,
    /// 目前的分類（`prefs::groups()` 的索引）；None 是分類清單
    pub category: Option<usize>,
    /// 展開選項的欄位
    pub expanded: Option<usize>,
    /// themes 資料夾中的使用者主題（開啟時在背景讀取）
    pub user_themes: Vec<String>,
    /// 緩慢的右鍵選單擴充功能（進入有表格的分類時在背景讀取）
    pub slow: Vec<SlowExt>,
}

/// 面板中的一列
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    /// 分類清單的一個分類（`prefs::groups()` 的索引）與它下方的項目一覽（小字）
    Category(usize),
    CategoryHint(usize),
    Field(usize),
    /// 欄位的說明（小字）
    Hint(usize),
    /// 展開的欄位的第 k 個選項
    Option(usize, usize),
    /// 緩慢的擴充功能表格（`Kind::SlowList` 的欄位）：欄名、第 k 個擴充功能、沒有項目
    SlowHead(usize),
    Slow(usize, usize),
    SlowEmpty(usize),
}

pub struct PanelLayout {
    pub title: Rect,
    pub list: Rect,
    pub note: Rect,
    pub buttons: [Rect; 2],
}

/// 分類下方的項目一覽：各項目的名稱（去掉「…」）
fn category_summary(group: &str) -> String {
    let names: Vec<&str> =
        PREFS.iter().filter(|p| p.group == group).map(|p| zing_core::i18n::tr(p.label).trim_end_matches('…')).collect();
    names.join(tr!("、"))
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
        let groups = groups();
        let Some(group) = self.category.and_then(|g| groups.get(g)) else {
            return (0..groups.len()).flat_map(|g| [Row::Category(g), Row::CategoryHint(g)]).collect();
        };
        let mut out = Vec::new();
        for (i, p) in PREFS.iter().enumerate().filter(|(_, p)| p.group == *group) {
            out.push(Row::Field(i));
            if !p.hint.is_empty() {
                out.push(Row::Hint(i));
            }
            if self.expanded == Some(i) {
                out.extend((0..self.options(i).len()).map(|k| Row::Option(i, k)));
            }
            if p.kind == Kind::SlowList {
                if self.slow.is_empty() {
                    out.push(Row::SlowEmpty(i));
                } else {
                    out.push(Row::SlowHead(i));
                    out.extend((0..self.slow.len()).map(|k| Row::Slow(i, k)));
                }
            }
        }
        out
    }
}

impl App {
    pub fn open_settings(&mut self) {
        if self.settings_page().is_none() {
            self.open_side(crate::sidepanel::SidePanel::Settings(SettingsPage::default()));
            let n = self.notifier.clone();
            self.pool.spawn(move || n.send(Msg::UserThemes(settings::user_themes())));
        }
        self.invalidate();
    }

    fn end_pref_edit(&mut self) {
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::Pref { .. }))
            && let Some(e) = self.edit.take()
        {
            window::destroy_edit(e.hwnd);
        }
    }

    pub fn on_user_themes(&mut self, themes: Vec<String>) {
        if let Some(page) = self.settings_page_mut() {
            page.user_themes = themes;
            self.invalidate();
        }
    }

    pub fn on_slow_menu_exts(&mut self, list: Vec<SlowExt>) {
        if let Some(page) = self.settings_page_mut() {
            page.slow = list;
            self.invalidate();
        }
    }

    /// 進入分類（None：回到分類清單）
    pub fn settings_category(&mut self, category: Option<usize>) {
        self.end_pref_edit();
        let Some(page) = self.settings_page_mut() else { return };
        page.category = category;
        page.scroll = 0.0;
        page.expanded = None;
        let groups = groups();
        let group = category.and_then(|g| groups.get(g).copied());
        if PREFS.iter().any(|p| Some(p.group) == group && p.kind == Kind::SlowList) {
            let (n, persist) = (self.notifier.clone(), !self.isolated);
            self.pool.spawn(move || n.send(Msg::SlowMenuExts(crate::shellext::slow_list(persist))));
        }
        self.invalidate();
    }

    pub fn settings_layout(&self, r: Rect) -> PanelLayout {
        let rh = self.row_h();
        let inner = r.inset(12.0, 10.0);
        let title = Rect::new(inner.l, inner.t, inner.r, inner.t + rh);
        let buttons = footer_buttons(inner, rh);
        let note = Rect::new(inner.l, buttons[0].t - 6.0 - rh, inner.r, buttons[0].t - 6.0);
        let list = Rect::new(inner.l, title.b + 6.0, inner.r, note.t - 4.0);
        PanelLayout { title, list, note, buttons }
    }

    /// 第 k 列的位置與其中數值的範圍
    pub fn settings_row_rects(&self, list: Rect, k: usize, scroll: f32) -> (Rect, Rect) {
        let row = row_rect(list, k, self.row_h(), scroll, 0);
        let value_l = list.l + (list.w() * 0.5).min(200.0);
        (row, Rect::new(value_l, row.t, list.r, row.b))
    }

    /// 緩慢的擴充功能表格的欄：名稱、類型、耗時、開關的左緣
    fn slow_columns(list: Rect) -> [f32; 4] {
        [list.l + 8.0, list.l + list.w() * 0.42, list.l + list.w() * 0.64, list.l + list.w() * 0.78]
    }

    /// 緩慢的擴充功能表格可以切換（「隱藏緩慢的擴充功能項目」開啟時）
    pub fn slow_table_enabled(&self) -> bool {
        self.config.context_menu.hide_slow
    }

    pub fn settings_hit(&self, panel: Rect, x: f32, y: f32) -> Hit {
        let Some(page) = self.settings_page() else { return Hit::None };
        let pl = self.settings_layout(panel);
        if let Some(k) = pl.buttons.iter().position(|r| r.contains(x, y)) {
            return Hit::SettingsButton(k);
        }
        if page.category.is_some() && pl.title.contains(x, y) {
            return Hit::SettingsBack;
        }
        if pl.list.contains(x, y) {
            let rows = page.rows();
            let k = ((y - pl.list.t + page.scroll) / self.row_h()).floor();
            if k >= 0.0 {
                let k = k as usize;
                match rows.get(k) {
                    Some(Row::Category(_) | Row::Field(_) | Row::Option(..)) => return Hit::SettingsRow(k),
                    // 分類的兩列是同一個按鈕
                    Some(Row::CategoryHint(_)) => return Hit::SettingsRow(k - 1),
                    Some(Row::Slow(..)) if self.slow_table_enabled() => return Hit::SettingsRow(k),
                    _ => {}
                }
            }
        }
        Hit::SettingsPanel
    }

    pub fn settings_click(&mut self, hit: Hit) {
        match hit {
            Hit::SettingsRow(k) => {
                let Some(page) = self.settings_page() else { return };
                match page.rows().get(k).copied() {
                    Some(Row::Category(g)) => self.settings_category(Some(g)),
                    Some(Row::Field(i)) => match PREFS[i].kind {
                        Kind::Choice(_) | Kind::Theme => {
                            if let Some(page) = self.settings_page_mut() {
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
                        Kind::SlowList => {}
                    },
                    Some(Row::Option(i, o)) => {
                        let value = page.options(i).get(o).map(|(v, _)| v.clone());
                        if let Some(page) = self.settings_page_mut() {
                            page.expanded = None;
                        }
                        if let Some(v) = value {
                            self.set_pref(i, v);
                        }
                    }
                    Some(Row::Slow(i, e)) if self.slow_table_enabled() => {
                        let Some(clsid) = page.slow.get(e).map(|e| e.clsid.clone()) else { return };
                        let mut allow = self.config.context_menu.allow_slow.clone();
                        match allow.iter().position(|a| *a == clsid) {
                            Some(k) => {
                                allow.remove(k);
                            }
                            None => allow.push(clsid),
                        }
                        self.set_pref(i, allow.join(","));
                    }
                    _ => {}
                }
            }
            Hit::SettingsBack => self.settings_category(None),
            Hit::SettingsButton(0) => self.exec("config.edit"),
            Hit::SettingsButton(1) => self.close_side_panels(),
            _ => {}
        }
    }

    /// Esc：先收起展開的選項，再回到分類清單，最後關閉設定頁
    pub fn settings_escape(&mut self) {
        match self.settings_page_mut() {
            Some(page) if page.expanded.is_some() => {
                page.expanded = None;
                self.invalidate();
            }
            Some(page) if page.category.is_some() => self.settings_category(None),
            Some(_) => self.close_side_panels(),
            None => {}
        }
    }

    fn begin_pref_edit(&mut self, i: usize) {
        let Some(panel) = self.layout.side_panel else { return };
        let Some(k) = self.settings_page().and_then(|p| p.rows().iter().position(|r| *r == Row::Field(i))) else { return };
        if self.side_reveal(k).is_none() {
            return;
        }
        let (_, value) = self.settings_row_rects(self.settings_layout(panel).list, k, self.side_scroll());
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
        let old = self.config.clone();
        if let Err(e) = self.config.set_pref(p.key, &value) {
            self.set_status(e, true);
            return;
        }
        self.apply_config(&old);
        if p.key == "theme" {
            self.load_theme_by_name(value.clone());
        }
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

    /// 開關：軌道與圓鈕（`enabled` 為 false 時灰階）；回傳軌道的右緣
    fn paint_switch(&self, rr: Rect, l: f32, on: bool, enabled: bool) -> f32 {
        let c = &self.theme.color;
        let g = &self.gfx;
        let h = (rr.h() - 10.0).clamp(10.0, 18.0);
        let track = Rect::new(l, rr.t + (rr.h() - h) / 2.0, l + h * 1.8, rr.t + (rr.h() + h) / 2.0);
        let (fill, edge, knob) = match (on, enabled) {
            (true, true) => (c.accent, c.accent, c.background),
            (true, false) => (c.text_secondary, c.text_secondary, c.background),
            (false, true) => (c.background, c.border, c.text_secondary),
            (false, false) => (c.background, c.border, c.border),
        };
        g.fill_round(track, h / 2.0, fill);
        g.stroke_round(track, h / 2.0, edge, 1.0);
        let d = h - 6.0;
        let x = if on { track.r - 3.0 - d } else { track.l + 3.0 };
        g.fill_round(Rect::new(x, track.t + 3.0, x + d, track.b - 3.0), d / 2.0, knob);
        track.r
    }

    pub fn paint_settings(&self, r: Rect) {
        let Some(page) = self.settings_page() else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius;
        let rh = self.row_h();
        g.fill(r, c.surface);
        let pl = self.settings_layout(r);
        let groups = groups();
        match page.category.and_then(|k| groups.get(k)) {
            Some(group) => {
                if self.hover == Hit::SettingsBack {
                    g.fill_round(pl.title, radius, c.hover);
                }
                let title = format!("‹  {}", zing_core::i18n::tr(group));
                g.text(&title, Font::Bold, Rect::new(pl.title.l + 4.0, pl.title.t, pl.title.r, pl.title.b), c.text);
            }
            None => g.text(tr!("設定"), Font::Bold, pl.title, c.text),
        }
        let slow_on = self.slow_table_enabled();
        let cols = Self::slow_columns(pl.list);
        g.push_clip(pl.list);
        for (k, row) in page.rows().iter().enumerate() {
            let (rr, value) = self.settings_row_rects(pl.list, k, page.scroll);
            if rr.b < pl.list.t || rr.t > pl.list.b {
                continue;
            }
            let hover = self.hover == Hit::SettingsRow(k);
            match *row {
                Row::Category(gi) => {
                    let block = Rect::new(rr.l, rr.t, rr.r, rr.b + rh);
                    if hover {
                        g.fill_round(block, radius, c.hover);
                    }
                    g.text(zing_core::i18n::tr(groups[gi]), Font::Text, Rect::new(rr.l + 8.0, rr.t + 2.0, rr.r - 24.0, rr.b + 2.0), c.text);
                    g.text("›", Font::Text, Rect::new(rr.r - 18.0, rr.t + rh / 2.0, rr.r, rr.b + rh / 2.0), c.text_secondary);
                }
                Row::CategoryHint(gi) => {
                    let summary = category_summary(groups[gi]);
                    g.text(&summary, Font::Small, Rect::new(rr.l + 8.0, rr.t, rr.r - 24.0, rr.b), c.text_secondary)
                }
                Row::Hint(i) => {
                    let hint = if PREFS[i].kind == Kind::SlowList && !slow_on {
                        tr!("「隱藏緩慢的擴充功能項目」關閉時，全部照樣放在第一層")
                    } else {
                        zing_core::i18n::tr(PREFS[i].hint)
                    };
                    g.text(hint, Font::Small, Rect::new(rr.l + 8.0, rr.t, rr.r - 6.0, rr.b), c.text_secondary)
                }
                Row::Field(i) => {
                    let p = &PREFS[i];
                    if p.kind == Kind::SlowList {
                        let color = if slow_on { c.text } else { c.text_secondary };
                        g.text(zing_core::i18n::tr(p.label), Font::Bold, Rect::new(rr.l + 8.0, rr.t + 2.0, rr.r, rr.b), color);
                        continue;
                    }
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
                            let on = v == "true";
                            let right = self.paint_switch(rr, vr.l, on, true);
                            let label = if on { tr!("開") } else { tr!("關") };
                            g.text(label, Font::Text, Rect::new(right + 8.0, rr.t, vr.r, rr.b), c.text_secondary);
                        }
                        Kind::Text if v.is_empty() => g.text(tr!("（未設定）"), Font::Text, vr, c.text_secondary),
                        Kind::Text => g.text(&v, Font::Text, vr, c.text_secondary),
                        Kind::Action(_) => g.text("›", Font::Text, Rect::new(rr.r - 18.0, rr.t, rr.r, rr.b), c.text_secondary),
                        Kind::SlowList => {}
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
                Row::SlowHead(_) => {
                    let heads = [tr!("擴充功能"), tr!("類型"), tr!("耗時"), tr!("放在第一層")];
                    for (n, h) in heads.iter().enumerate() {
                        let right = cols.get(n + 1).map_or(rr.r, |x| x - 4.0);
                        g.text(h, Font::Small, Rect::new(cols[n], rr.t, right, rr.b), c.text_secondary);
                    }
                    g.fill(Rect::new(rr.l, rr.b - 1.0, rr.r, rr.b), c.border);
                }
                Row::Slow(_, e) => {
                    let Some(ext) = page.slow.get(e) else { continue };
                    if hover && slow_on {
                        g.fill_round(rr, radius, c.hover);
                    }
                    let text = if slow_on { c.text } else { c.text_secondary };
                    let kinds: Vec<&str> = ext.kinds.iter().map(|k| zing_core::i18n::tr(k)).collect();
                    g.text(&ext.name, Font::Text, Rect::new(cols[0], rr.t, cols[1] - 4.0, rr.b), text);
                    g.text(&kinds.join(tr!("、")), Font::Small, Rect::new(cols[1], rr.t, cols[2] - 4.0, rr.b), c.text_secondary);
                    g.text(&format!("{} ms", ext.ms), Font::Small, Rect::new(cols[2], rr.t, cols[3] - 4.0, rr.b), c.text_secondary);
                    let on = !slow_on || self.config.context_menu.allow_slow.contains(&ext.clsid);
                    self.paint_switch(rr, cols[3], on, slow_on);
                }
                Row::SlowEmpty(_) => {
                    let msg = tr!("目前沒有偵測到緩慢的擴充功能");
                    g.text(msg, Font::Small, Rect::new(rr.l + 8.0, rr.t, rr.r - 6.0, rr.b), c.text_secondary);
                }
            }
        }
        g.pop_clip();
        g.text(tr!("變更立即生效，並儲存到程式旁的 config.toml"), Font::Small, pl.note, c.text_secondary);
        self.paint_footer_buttons(&pl.buttons, BUTTONS, None, |k| self.hover == Hit::SettingsButton(k));
    }
}
