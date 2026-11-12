//! 設定（命令 settings.open，預設 Ctrl+,）：視窗中央的對話框（背景變暗，對話框外點不到底下的元件、點了就關閉），
//! 列出 `zing_core::prefs::PREFS`。沿用右側面板的機制（`SidePanel::Settings`：捲動、鍵盤、焦點框），只是位置在中央
//! （`Layout::side_dialog`）。
//!
//! - 上方是分頁（`prefs::groups()`：一般、外觀、命令列、系統），分頁中依副標題（`Pref::section`）分組。
//! - 自訂主題、自訂命令列、快速鍵等編輯器從這裡開啟時記住分頁（`App::side_return`），完成或 Esc 後回到這裡。
//!   移除 NTFS 索引服務先確認（`App::confirm`）。
//! - 選項（語言、主題、位置）：點一下開啟下拉選單；開關：點一下切換；文字：點一下輸入，Enter 確定。
//! - 緩慢的右鍵選單擴充功能（`Kind::SlowList`）：表格逐一切換是否照樣放在第一層；「隱藏緩慢的擴充功能項目」
//!   關閉時表格以灰階顯示、不能切換。表格的內容在進入分類時於背景讀取（`shellext::slow_list`）。
//! - 每次修改立即套用，並在背景只改寫 config.toml 中的那一行（保留註解，見 `zing_core::prefs::write_pref`）；
//!   隔離模式（冒煙測試）不寫檔。
//! - 指定的分頁（`Kind::Tabs`）：點一下改成目前開著的分頁。設為預設的檔案總管（`Kind::DefaultApp`）：開關顯示登錄的狀態
//!   （開啟設定頁時在背景讀取），切換在背景寫入登錄（`defaultapp.rs`）。
//! - 其他設定（工具列按鈕、縮圖產生器、快速鍵…）以列出的命令開啟對應的編輯器。

use zing_core::prefs::{BUILTIN_THEMES, Kind, PREFS, choice_label, groups, toml_literal};

use crate::app::{App, EditKind};
use crate::gfx::{Font, Rect};
use crate::menu::{Action, MenuItem};
use crate::services::Msg;
use crate::settings;
use crate::shellext::SlowExt;
use crate::sidepanel::{Line, footer_buttons, row_rect};
use crate::view::Hit;
use crate::window;

/// 「指定的分頁」的按鈕
const USE_CURRENT_TABS: &str = tr_key!("使用目前的分頁");
/// 對話框的大小（視窗太小時縮小）
const DIALOG_W: f32 = 680.0;
const DIALOG_H: f32 = 620.0;

#[derive(Default)]
pub struct SettingsPage {
    pub scroll: f32,
    /// 目前的分頁（`prefs::groups()` 的索引）
    pub category: usize,
    /// themes 資料夾中的使用者主題（開啟時在背景讀取）
    pub user_themes: Vec<String>,
    /// 緩慢的右鍵選單擴充功能（進入有表格的分類時在背景讀取）
    pub slow: Vec<SlowExt>,
}

/// 面板中的一列
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    /// 副標題（`Pref::section`）
    Section(&'static str),
    Field(usize),
    /// 欄位的說明（小字）的第 n 行
    Hint(usize, usize),
    /// 欄位目前的狀態（NTFS 索引服務：各磁碟的索引；緊接在欄位下方）
    Status(usize),
    /// 緩慢的擴充功能表格（`Kind::SlowList` 的欄位）：欄名、第 k 個擴充功能、沒有項目
    SlowHead(usize),
    Slow(usize, usize),
    SlowEmpty(usize),
}

pub struct PanelLayout {
    pub title: Rect,
    /// 各分頁的標籤
    pub tabs: Vec<Rect>,
    pub list: Rect,
    /// 「關閉」（右下角）
    pub close: Rect,
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
        let Some(group) = groups.get(self.category) else { return Vec::new() };
        let mut out = Vec::new();
        let mut section = "";
        for (i, p) in PREFS.iter().enumerate().filter(|(_, p)| p.group == *group) {
            if p.section != section {
                section = p.section;
                if !section.is_empty() {
                    out.push(Row::Section(section));
                }
            }
            out.push(Row::Field(i));
            if p.kind == Kind::IndexService {
                out.push(Row::Status(i));
            }
            if !p.hint.is_empty() {
                let lines = zing_core::i18n::tr(p.hint).lines().count().max(1);
                out.extend((0..lines).map(|n| Row::Hint(i, n)));
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
        self.open_settings_at(0);
    }

    /// 開啟設定對話框的第 `tab` 個分頁（已開著就切換分頁）
    pub fn open_settings_at(&mut self, tab: usize) {
        if self.settings_page().is_none() {
            self.open_side(crate::sidepanel::SidePanel::Settings(SettingsPage::default()));
            let n = self.notifier.clone();
            self.pool.spawn(move || n.send(Msg::UserThemes(settings::user_themes())));
            self.refresh_default_app(false);
        }
        self.settings_tab(tab);
        self.invalidate();
    }

    /// 開啟設定、切到這種項目所在的分頁並捲到它（命令 index.status、update.check：結果顯示在那一列）
    pub fn open_settings_on(&mut self, kind: Kind) {
        let Some(i) = PREFS.iter().position(|p| p.kind == kind) else { return };
        let tab = groups().iter().position(|g| *g == PREFS[i].group).unwrap_or(0);
        self.open_settings_at(tab);
        self.layout = self.compute_layout();
        if let Some(k) = self.settings_page().and_then(|p| p.rows().iter().position(|r| *r == Row::Field(i))) {
            self.side_reveal(k);
        }
    }

    /// 開著的是設定對話框（對話框外點不到底下的元件、按鍵不往下傳）
    pub fn settings_dialog(&self) -> bool {
        self.settings_page().is_some()
    }

    /// 對話框的位置：視窗中央
    pub fn settings_dialog_rect(&self, w: f32, h: f32) -> Rect {
        let (dw, dh) = (DIALOG_W.min(w - 48.0).max(320.0), DIALOG_H.min(h - 48.0).max(240.0));
        let (l, t) = (((w - dw) / 2.0).round(), ((h - dh) / 2.0).max(8.0).round());
        Rect::new(l, t, l + dw, t + dh)
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

    /// 切換到第 `tab` 個分頁；鍵盤焦點留在分頁列（←→ 繼續切換）
    pub fn settings_tab(&mut self, tab: usize) {
        self.end_pref_edit();
        let groups = groups();
        let tab = tab.min(groups.len().saturating_sub(1));
        let Some(page) = self.settings_page_mut() else { return };
        page.category = tab;
        page.scroll = 0.0;
        self.side_cursor.line = Line::Top(0);
        self.side_cursor.part = tab;
        let group = groups.get(tab).copied();
        if PREFS.iter().any(|p| Some(p.group) == group && p.kind == Kind::IndexService) {
            self.refresh_index_info();
        }
        if PREFS.iter().any(|p| Some(p.group) == group && p.kind == Kind::SlowList) {
            let (n, persist) = (self.notifier.clone(), !self.isolated);
            self.pool.spawn(move || n.send(Msg::SlowMenuExts(crate::shellext::slow_list(persist))));
        }
        self.invalidate();
    }

    pub fn settings_layout(&self, r: Rect) -> PanelLayout {
        let rh = self.row_h();
        let inner = r.inset(18.0, 14.0);
        let title = Rect::new(inner.l, inner.t, inner.r, inner.t + rh);
        let tab_h = (rh + 8.0).max(28.0);
        let mut x = inner.l;
        let tabs = groups()
            .iter()
            .map(|g| {
                let w = self.gfx.measure(zing_core::i18n::tr(g), Font::Text) + 28.0;
                let t = Rect::new(x, title.b + 8.0, x + w, title.b + 8.0 + tab_h);
                x += w + 4.0;
                t
            })
            .collect::<Vec<_>>();
        let tabs_b = title.b + 8.0 + tab_h;
        let close = footer_buttons(inner, rh)[1];
        let list = Rect::new(inner.l, tabs_b + 10.0, inner.r, close.t - 10.0);
        PanelLayout { title, tabs, list, close }
    }

    /// 第 k 列的位置與其中數值的範圍
    pub fn settings_row_rects(&self, list: Rect, k: usize, scroll: f32) -> (Rect, Rect) {
        let row = row_rect(list, k, self.row_h(), scroll, 0);
        let value_l = list.l + (list.w() * 0.5).min(200.0);
        (row, Rect::new(value_l, row.t, list.r, row.b))
    }

    /// 「指定的分頁」那一列的按鈕（按了改成目前開著的分頁）：數值區的右端
    pub fn tabs_button(&self, row: Rect) -> Rect {
        self.row_button(row, USE_CURRENT_TABS)
    }

    /// 列右端的按鈕（`label` 是原文）
    fn row_button(&self, row: Rect, label: &'static str) -> Rect {
        let w = self.gfx.measure(zing_core::i18n::tr(label), Font::Text) + 24.0;
        Rect::new(row.r - 4.0 - w, row.t + 2.0, row.r - 4.0, row.b - 2.0)
    }

    /// 只有右端的按鈕可以按的欄位（指定的分頁、檢查更新）：按鈕的位置與文字
    pub fn field_button(&self, i: usize, row: Rect) -> Option<(Rect, &'static str)> {
        let label = match PREFS[i].kind {
            Kind::Tabs => USE_CURRENT_TABS,
            Kind::UpdateCheck => self.update_button_label(),
            _ => return None,
        };
        Some((self.row_button(row, label), label))
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
        if pl.close.contains(x, y) {
            return Hit::SettingsButton(0);
        }
        if let Some(g) = pl.tabs.iter().position(|r| r.contains(x, y)) {
            return Hit::SettingsTab(g);
        }
        if pl.list.contains(x, y) {
            let rows = page.rows();
            let k = ((y - pl.list.t + page.scroll) / self.row_h()).floor();
            if k >= 0.0 {
                let k = k as usize;
                match rows.get(k) {
                    // 指定的分頁、檢查更新：只有按鈕可以按
                    Some(Row::Field(i)) if self.field_button(*i, pl.list).is_some() => {
                        let (rr, _) = self.settings_row_rects(pl.list, k, page.scroll);
                        if self.field_button(*i, rr).is_some_and(|(b, _)| b.contains(x, y)) {
                            return Hit::SettingsRow(k);
                        }
                    }
                    Some(Row::Field(_)) => return Hit::SettingsRow(k),
                    Some(Row::Slow(..)) if self.slow_table_enabled() => return Hit::SettingsRow(k),
                    _ => {}
                }
            }
        }
        Hit::SettingsPanel
    }

    /// 鍵盤焦點的目標（`side_targets`）：上方是分頁列，下方是「關閉」
    pub fn settings_targets(&self, panel: Rect, line: Line) -> Vec<(Hit, Rect)> {
        let Some(page) = self.settings_page() else { return Vec::new() };
        let pl = self.settings_layout(panel);
        match line {
            Line::Top(_) => pl.tabs.iter().enumerate().map(|(g, r)| (Hit::SettingsTab(g), *r)).collect(),
            Line::Bottom(_) => vec![(Hit::SettingsButton(0), pl.close)],
            Line::Row(k) => {
                let (rr, _) = self.settings_row_rects(pl.list, k, page.scroll);
                match page.rows().get(k) {
                    Some(Row::Field(i)) if self.field_button(*i, rr).is_some() => {
                        self.field_button(*i, rr).map(|(b, _)| (Hit::SettingsRow(k), b)).into_iter().collect()
                    }
                    Some(Row::Field(_)) => vec![(Hit::SettingsRow(k), rr)],
                    Some(Row::Slow(..)) if self.slow_table_enabled() => vec![(Hit::SettingsRow(k), rr)],
                    _ => Vec::new(),
                }
            }
        }
    }

    pub fn settings_click(&mut self, hit: Hit) {
        match hit {
            Hit::SettingsRow(k) => {
                let Some(page) = self.settings_page() else { return };
                match page.rows().get(k).copied() {
                    Some(Row::Field(i)) => match PREFS[i].kind {
                        Kind::Choice(_) | Kind::Theme => {
                            let options = page.options(i);
                            self.open_choice_menu(k, i, options);
                        }
                        Kind::Bool => {
                            let on = self.config.pref_value(PREFS[i].key) == "true";
                            self.set_pref(i, (!on).to_string());
                        }
                        Kind::Text => self.begin_pref_edit(i),
                        Kind::Action(cmd) => {
                            // 開出編輯器（主題、命令列、快速鍵）時記住分頁，完成後回到這裡
                            let tab = page.category;
                            self.exec(cmd);
                            if self.side.is_some() && !self.settings_dialog() {
                                self.side_return = Some(tab);
                            }
                        }
                        Kind::Tabs => {
                            let tabs: Vec<String> = self
                                .panes
                                .iter()
                                .flat_map(|p| p.tabs.iter().map(|t| zing_core::startup::tab_value(&t.path)))
                                .collect();
                            self.set_pref(i, tabs.join("|"));
                        }
                        Kind::DefaultApp => self.toggle_default_app(),
                        Kind::IndexService => self.toggle_index_service(),
                        Kind::UpdateCheck => self.update_button_click(),
                        Kind::SlowList => {}
                    },
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
            Hit::SettingsTab(g) => self.settings_tab(g),
            Hit::SettingsButton(_) => self.close_side_panels(),
            _ => {}
        }
    }

    /// 第 k 列（`PREFS[i]`）的選項：在數值處下方開啟下拉選單，目前的值打勾
    fn open_choice_menu(&mut self, k: usize, i: usize, options: Vec<(String, String)>) {
        let Some(panel) = self.layout.side_panel else { return };
        if self.side_reveal(k).is_none() {
            return;
        }
        let (_, value) = self.settings_row_rects(self.settings_layout(panel).list, k, self.side_scroll());
        let current = self.config.pref_value(PREFS[i].key);
        let items = options
            .into_iter()
            .map(|(v, label)| MenuItem { checked: v.eq_ignore_ascii_case(&current), ..MenuItem::new(label, Action::Pref(i, v)) })
            .collect();
        self.open_menu(items, value);
    }

    /// Esc：關閉對話框
    pub fn settings_escape(&mut self) {
        self.close_side_panels();
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
        self.broadcast(crate::hub::Shared::Pref { key: p.key.to_string(), value: value.clone() });
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
    pub fn paint_switch(&self, rr: Rect, l: f32, on: bool, enabled: bool) -> f32 {
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
        let pl = self.settings_layout(r);
        g.text(tr!("設定"), Font::Bold, pl.title, c.text);
        // 分頁：目前的分頁下方畫強調色的線
        for (gi, (t, name)) in pl.tabs.iter().zip(groups()).enumerate() {
            let active = gi == page.category;
            if self.hover == Hit::SettingsTab(gi) && !active {
                g.fill_round(*t, radius, c.hover);
            }
            let label = zing_core::i18n::tr(name);
            let w = g.measure(label, Font::Text);
            g.text(label, Font::Text, Rect::new(t.l + (t.w() - w) / 2.0, t.t, t.r, t.b), if active { c.text } else { c.text_secondary });
            if active {
                g.fill(Rect::new(t.l + 8.0, t.b - 2.0, t.r - 8.0, t.b), c.accent);
            }
        }
        if let Some(last) = pl.tabs.last() {
            g.fill(Rect::new(pl.tabs[0].l, last.b, pl.list.r, last.b + 1.0), c.border);
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
                Row::Section(name) => {
                    g.text(zing_core::i18n::tr(name), Font::Bold, Rect::new(rr.l, rr.t + 4.0, rr.r, rr.b), c.text_secondary)
                }
                Row::Status(_) => {
                    let (text, error) = match (&self.index_note, self.index_installed) {
                        (Some((t, e)), _) => (t.clone(), *e),
                        (None, None) => (tr!("查詢中…").to_string(), false),
                        (None, Some(_)) => (String::new(), false),
                    };
                    g.text(&text, Font::Small, Rect::new(rr.l + 8.0, rr.t, rr.r - 6.0, rr.b), if error { c.error } else { c.text })
                }
                Row::Hint(i, n) => {
                    let hint = match (&PREFS[i].kind, &self.default_app) {
                        (Kind::SlowList, _) if !slow_on => tr!("「隱藏緩慢的擴充功能項目」關閉時，全部照樣放在第一層").to_string(),
                        (Kind::DefaultApp, Some(crate::defaultapp::Status::Other(exe))) => {
                            tr!("目前是另一個位置的 zing：{exe}；開啟後改成這一個", exe)
                        }
                        _ => zing_core::i18n::tr(PREFS[i].hint).to_string(),
                    };
                    let line = hint.lines().nth(n).unwrap_or("");
                    g.text(line, Font::Small, Rect::new(rr.l + 8.0, rr.t, rr.r - 6.0, rr.b), c.text_secondary)
                }
                Row::Field(i) => {
                    let p = &PREFS[i];
                    if p.kind == Kind::SlowList {
                        let color = if slow_on { c.text } else { c.text_secondary };
                        g.text(zing_core::i18n::tr(p.label), Font::Bold, Rect::new(rr.l + 8.0, rr.t + 2.0, rr.r, rr.b), color);
                        continue;
                    }
                    if hover && self.field_button(i, rr).is_none() {
                        g.fill_round(rr, radius, c.hover);
                    }
                    let action = matches!(p.kind, Kind::Action(_));
                    let label_r = if action { rr.r - 24.0 } else { value.l - 6.0 };
                    g.text(zing_core::i18n::tr(p.label), Font::Text, Rect::new(rr.l + 8.0, rr.t, label_r, rr.b), c.text);
                    let v = self.config.pref_value(p.key);
                    let vr = Rect::new(value.l, rr.t, rr.r - 6.0, rr.b);
                    match p.kind {
                        Kind::Choice(_) | Kind::Theme => {
                            g.text(&format!("{} ▾", choice_label(p, &v)), Font::Text, vr, c.text);
                        }
                        Kind::Bool => {
                            let on = v == "true";
                            let right = self.paint_switch(rr, vr.l, on, true);
                            let label = if on { tr!("開") } else { tr!("關") };
                            g.text(label, Font::Text, Rect::new(right + 8.0, rr.t, vr.r, rr.b), c.text_secondary);
                        }
                        Kind::Text if v.is_empty() => g.text(tr!("（未設定）"), Font::Text, vr, c.text_secondary),
                        Kind::Text => g.text(&v, Font::Text, vr, c.text_secondary),
                        Kind::Tabs => {
                            let button = self.tabs_button(rr);
                            let text = if v.is_empty() {
                                tr!("（未設定）").to_string()
                            } else {
                                let names: Vec<String> = v
                                    .split('|')
                                    .map(zing_core::startup::tab_path)
                                    .map(|p| if p.is_empty() { tr!("此電腦").to_string() } else { p })
                                    .collect();
                                names.join(tr!("、"))
                            };
                            g.text(&text, Font::Text, Rect::new(vr.l, rr.t, button.l - 8.0, rr.b), c.text_secondary);
                            self.paint_button(button, USE_CURRENT_TABS, false, hover);
                        }
                        Kind::IndexService => {
                            let on = self.index_installed == Some(true);
                            let right = self.paint_switch(rr, vr.l, on, self.index_installed.is_some());
                            let label = if on { tr!("開") } else { tr!("關") };
                            g.text(label, Font::Text, Rect::new(right + 8.0, rr.t, vr.r, rr.b), c.text_secondary);
                        }
                        Kind::UpdateCheck => {
                            if let Some((button, label)) = self.field_button(i, rr) {
                                let (text, error) = self.update_row_text();
                                g.text(&text, Font::Text, Rect::new(vr.l, rr.t, button.l - 8.0, rr.b), if error { c.error } else { c.text_secondary });
                                self.paint_button(button, label, false, hover);
                            }
                        }
                        Kind::DefaultApp => {
                            let on = self.default_app == Some(crate::defaultapp::Status::On);
                            let right = self.paint_switch(rr, vr.l, on, self.default_app.is_some());
                            let label = if on { tr!("開") } else { tr!("關") };
                            g.text(label, Font::Text, Rect::new(right + 8.0, rr.t, vr.r, rr.b), c.text_secondary);
                        }
                        Kind::Action(_) => g.text("›", Font::Text, Rect::new(rr.r - 18.0, rr.t, rr.r, rr.b), c.text_secondary),
                        Kind::SlowList => {}
                    }
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
        self.paint_button(pl.close, tr_key!("關閉"), false, self.hover == Hit::SettingsButton(0));
    }
}
