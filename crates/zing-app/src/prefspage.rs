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
//! - 最後一頁「關於」（zoo-release 的 docs/UI.md）：名稱、版本與「檢查更新」、一句話介紹、更新的狀態與它的按鈕
//!   （update.rs 的 `about_status`）、自動檢查更新、連結（版本說明・原始碼・回報問題・授權）、作者的其他專案
//!   （about.rs，進入這一頁時讀取，讀不到就不顯示）。一列中有多個按鈕或連結時以 `Hit::SettingsPart` 區分。

use zing_core::prefs::{BUILTIN_THEMES, Kind, PREFS, choice_label, groups, toml_literal};

use crate::app::{App, EditKind};
use crate::gfx::{Font, Rect};
use crate::menu::{Action, MenuItem};
use crate::services::Msg;
use crate::settings;
use crate::shellext::SlowExt;
use crate::sidepanel::{Line, footer_buttons, row_rect};
use crate::update::UpdateAction;
use crate::view::Hit;
use crate::window;

/// 「指定的分頁」的按鈕
const USE_CURRENT_TABS: &str = tr_key!("使用目前的分頁");
/// 對話框的大小（視窗太小時縮小）
pub const DIALOG_W: f32 = 680.0;
pub const DIALOG_H: f32 = 620.0;

#[derive(Default)]
pub struct SettingsPage {
    pub scroll: f32,
    /// 目前的分頁（`prefs::groups()` 的索引）
    pub category: usize,
    /// themes 資料夾中的使用者主題（開啟時在背景讀取）
    pub user_themes: Vec<String>,
    /// 緩慢的右鍵選單擴充功能（進入有表格的分類時在背景讀取）
    pub slow: Vec<SlowExt>,
    /// 作者的其他專案（進入「關於」時讀取，about.rs）
    pub projects: Vec<crate::about::Project>,
}

/// 其他專案每個佔幾列（封面與名稱、介紹）
pub const PROJECT_LINES: usize = 3;
/// 其他專案一列放幾個（zoo-release 的 docs/ui-layout.svg）
pub const PROJECT_COLUMNS: usize = 2;

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
    /// 「關於」（`Kind::About` 的欄位）：名稱與版本（「檢查更新」）、一句話介紹、更新的狀態（文字與按鈕）
    AboutTitle(usize),
    AboutTagline(usize),
    UpdateStatus(usize),
    /// 連結：版本說明、原始碼、回報問題、授權（`Kind::AboutLinks`）
    Links(usize),
    /// 作者的其他專案（`Kind::OtherProjects`）：第 g 組（`PROJECT_COLUMNS` 個專案並排）的第 n 列（0–2）
    Project(usize, usize, usize),
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
            // 沒有其他專案（讀不到、還沒讀到）時整個區塊不顯示，連副標題
            if p.kind == Kind::OtherProjects && self.projects.is_empty() {
                continue;
            }
            if p.section != section {
                section = p.section;
                if !section.is_empty() {
                    out.push(Row::Section(section));
                }
            }
            match p.kind {
                Kind::About => {
                    out.extend([Row::AboutTitle(i), Row::AboutTagline(i), Row::UpdateStatus(i)]);
                    continue;
                }
                Kind::AboutLinks => {
                    out.push(Row::Links(i));
                    continue;
                }
                Kind::OtherProjects => {
                    let groups = self.projects.len().div_ceil(PROJECT_COLUMNS);
                    out.extend((0..groups).flat_map(|g| (0..PROJECT_LINES).map(move |n| Row::Project(i, g, n))));
                    continue;
                }
                _ => {}
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
        if PREFS.iter().any(|p| Some(p.group) == group && p.kind == Kind::OtherProjects) {
            self.load_other_projects();
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

    /// 只有右端的按鈕可以按的欄位（指定的分頁）：按鈕的位置與文字
    pub fn field_button(&self, i: usize, row: Rect) -> Option<(Rect, &'static str)> {
        let label = match PREFS[i].kind {
            Kind::Tabs => USE_CURRENT_TABS,
            _ => return None,
        };
        Some((self.row_button(row, label), label))
    }

    /// 列右端由右往左排的按鈕（`labels` 是原文，依顯示的順序）
    fn right_buttons(&self, row: Rect, labels: &[&'static str]) -> Vec<Rect> {
        let mut right = row.r - 4.0;
        let mut out: Vec<Rect> = labels
            .iter()
            .rev()
            .map(|l| {
                let w = self.gfx.measure(zing_core::i18n::tr(l), Font::Text) + 24.0;
                let r = Rect::new(right - w, row.t + 2.0, right, row.b - 2.0);
                right -= w + 6.0;
                r
            })
            .collect();
        out.reverse();
        out
    }

    /// 「關於」的連結：（文字, 網址）
    fn about_links() -> [(String, String); 4] {
        let repo = format!("https://github.com/{}", crate::update::REPO);
        [
            (tr!("版本說明").into(), format!("{repo}/releases")),
            (tr!("原始碼").into(), repo.clone()),
            (tr!("回報問題").into(), format!("{repo}/issues")),
            (tr!("授權 {license}", license = crate::update::LICENSE), format!("{repo}/blob/master/LICENSE")),
        ]
    }

    /// 連結之間的分隔
    const LINK_SEP: &'static str = "  ·  ";

    /// 一列中可以按的部分（按鈕、連結）的位置；依 `Hit::SettingsPart` 的順序
    fn settings_parts(&self, row: Row, rr: Rect) -> Vec<Rect> {
        match row {
            Row::AboutTitle(_) => vec![self.row_button(rr, UpdateAction::Check.label())],
            // 其他專案：這一組的卡片
            Row::Project(_, g, 0) => {
                let n = self.settings_page().map_or(0, |p| p.projects.len().saturating_sub(g * PROJECT_COLUMNS).min(PROJECT_COLUMNS));
                (0..n).map(|j| self.project_card(rr, j).0).collect()
            }
            Row::UpdateStatus(_) => {
                let labels: Vec<&'static str> = self.about_status().actions.iter().map(|a| a.label()).collect();
                self.right_buttons(rr, &labels)
            }
            Row::Links(_) => {
                let sep = self.gfx.measure(Self::LINK_SEP, Font::Text);
                let mut x = rr.l + 8.0;
                Self::about_links()
                    .iter()
                    .map(|(t, _)| {
                        let w = self.gfx.measure(t, Font::Text);
                        let r = Rect::new(x, rr.t, x + w, rr.b);
                        x += w + sep;
                        r
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// 其他專案第 j 欄的卡片（第一列起佔 `PROJECT_LINES` 列）與封面的位置
    fn project_card(&self, first: Rect, j: usize) -> (Rect, Rect) {
        const GAP: f32 = 8.0;
        let w = (first.w() - GAP * (PROJECT_COLUMNS - 1) as f32) / PROJECT_COLUMNS as f32;
        let l = first.l + (w + GAP) * j as f32;
        let card = Rect::new(l, first.t, l + w, first.t + self.row_h() * PROJECT_LINES as f32);
        let h = card.h() - 12.0;
        (card, Rect::new(card.l + 8.0, card.t + 6.0, card.l + 8.0 + (h * 1.6).round(), card.b - 6.0))
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
                    // 其他專案：卡片的任何一列都算第一列（鍵盤目標也在第一列）
                    Some(&Row::Project(i, g, n)) => {
                        let (rr, _) = self.settings_row_rects(pl.list, k - n, page.scroll);
                        if let Some(j) = self.settings_parts(Row::Project(i, g, 0), rr).iter().position(|r| r.l <= x && x < r.r) {
                            return Hit::SettingsPart(k - n, j);
                        }
                    }
                    Some(&row @ (Row::AboutTitle(_) | Row::UpdateStatus(_) | Row::Links(_))) => {
                        let (rr, _) = self.settings_row_rects(pl.list, k, page.scroll);
                        if let Some(n) = self.settings_parts(row, rr).iter().position(|r| r.contains(x, y)) {
                            return Hit::SettingsPart(k, n);
                        }
                    }
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
                    // 「檢查更新」停用時沒有目標
                    Some(Row::AboutTitle(_)) if !self.can_check_update() => Vec::new(),
                    // 其他專案：每張卡片是一個目標（在第一列）
                    Some(&row @ (Row::AboutTitle(_) | Row::UpdateStatus(_) | Row::Links(_) | Row::Project(_, _, 0))) => {
                        self.settings_parts(row, rr).into_iter().enumerate().map(|(n, r)| (Hit::SettingsPart(k, n), r)).collect()
                    }
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
                        Kind::SlowList | Kind::About | Kind::AboutLinks | Kind::OtherProjects => {}
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
            Hit::SettingsPart(k, n) => {
                let Some(page) = self.settings_page() else { return };
                match page.rows().get(k) {
                    Some(Row::AboutTitle(_)) => self.check_update_now(),
                    Some(Row::UpdateStatus(_)) => {
                        if let Some(&a) = self.about_status().actions.get(n) {
                            self.update_action(a);
                        }
                    }
                    Some(Row::Links(_)) => {
                        if let Some((_, url)) = Self::about_links().into_iter().nth(n) {
                            crate::services::launch(&self.notifier, url, None, None, None, crate::services::Show::Normal);
                        }
                    }
                    Some(Row::Project(_, g, _)) => {
                        if let Some(url) = page.projects.get(g * PROJECT_COLUMNS + n).map(|p| p.url.clone()) {
                            crate::services::launch(&self.notifier, url, None, None, None, crate::services::Show::Normal);
                        }
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

    /// 按鈕；停用時（`enabled` 為 false）文字為次要色、沒有游標停住的底色
    fn paint_button_state(&self, b: Rect, label: &'static str, main: bool, hot: bool, enabled: bool) {
        if enabled {
            return self.paint_button(b, label, main, hot);
        }
        let c = &self.theme.color;
        let radius = self.theme.metrics.radius;
        self.gfx.fill_round(b, radius, c.background);
        self.gfx.stroke_round(b, radius, c.border, 1.0);
        let text = zing_core::i18n::tr(label);
        let w = self.gfx.measure(text, Font::Text);
        self.gfx.text(text, Font::Text, Rect::new(b.l + (b.w() - w) / 2.0, b.t, b.r, b.b), c.text_secondary);
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
        let rows = page.rows();
        for (k, row) in rows.iter().enumerate() {
            let (rr, value) = self.settings_row_rects(pl.list, k, page.scroll);
            // 其他專案的卡片從第一列畫到第三列
            let extra = if matches!(row, Row::Project(_, _, 0)) { self.row_h() * (PROJECT_LINES - 1) as f32 } else { 0.0 };
            if rr.b + extra < pl.list.t || rr.t > pl.list.b {
                continue;
            }
            let hover = self.hover == Hit::SettingsRow(k);
            match *row {
                Row::Section(name) => {
                    // 作者的其他專案與上面的「關於」以分隔線隔開
                    if matches!(rows.get(k + 1), Some(Row::Project(..))) {
                        g.fill(Rect::new(rr.l, rr.t, rr.r, rr.t + 1.0), c.border);
                    }
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
                        Kind::DefaultApp => {
                            let on = self.default_app == Some(crate::defaultapp::Status::On);
                            let right = self.paint_switch(rr, vr.l, on, self.default_app.is_some());
                            let label = if on { tr!("開") } else { tr!("關") };
                            g.text(label, Font::Text, Rect::new(right + 8.0, rr.t, vr.r, rr.b), c.text_secondary);
                        }
                        Kind::Action(_) => g.text("›", Font::Text, Rect::new(rr.r - 18.0, rr.t, rr.r, rr.b), c.text_secondary),
                        Kind::SlowList | Kind::About | Kind::AboutLinks | Kind::OtherProjects => {}
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
                Row::AboutTitle(_) => {
                    let name = "zing-explore";
                    let w = g.measure(name, Font::Bold);
                    g.text(name, Font::Bold, Rect::new(rr.l + 8.0, rr.t, rr.r, rr.b), c.text);
                    let version = format!("v{}", crate::update::current());
                    g.text(&version, Font::Text, Rect::new(rr.l + 8.0 + w + 10.0, rr.t, rr.r, rr.b), c.text_secondary);
                    let b = self.settings_parts(*row, rr)[0];
                    self.paint_button_state(b, UpdateAction::Check.label(), false, self.hover == Hit::SettingsPart(k, 0), self.can_check_update());
                }
                Row::AboutTagline(_) => {
                    g.text(tr!("輕量、快速、可自訂的 Windows 11 檔案總管"), Font::Text, Rect::new(rr.l + 8.0, rr.t, rr.r, rr.b), c.text_secondary)
                }
                Row::UpdateStatus(_) => {
                    let st = self.about_status();
                    let buttons = self.settings_parts(*row, rr);
                    let right = buttons.first().map_or(rr.r - 6.0, |b| b.l - 8.0);
                    g.text(&st.text, Font::Text, Rect::new(rr.l + 8.0, rr.t, right, rr.b), if st.error { c.error } else { c.text });
                    let after = rr.l + 8.0 + g.measure(&st.text, Font::Text);
                    if let Some(d) = &st.detail {
                        g.text(d, Font::Text, Rect::new(after, rr.t, right, rr.b), c.text_secondary);
                    }
                    // 下載中：與橫幅相同的進度條
                    if let Some(pct) = st.progress {
                        let l = after + 16.0;
                        let p = Rect::new(l, rr.t + rr.h() / 2.0 - 3.0, (l + 180.0).min(right - 60.0), rr.t + rr.h() / 2.0 + 3.0);
                        g.fill_round(p, 3.0, c.border);
                        g.fill_round(Rect::new(p.l, p.t, p.l + p.w() * pct.min(100) as f32 / 100.0, p.b), 3.0, c.accent);
                        g.text(&format!("{pct}%"), Font::Small, Rect::new(p.r + 8.0, rr.t, right, rr.b), c.text_secondary);
                    }
                    for (n, (b, a)) in buttons.iter().zip(&st.actions).enumerate() {
                        self.paint_button(*b, a.label(), a.primary(), self.hover == Hit::SettingsPart(k, n));
                    }
                }
                Row::Links(_) => {
                    let parts = self.settings_parts(*row, rr);
                    for (n, (r, (text, _))) in parts.iter().zip(Self::about_links()).enumerate() {
                        // 連結用一般的文字色（強調色只表示狀態）；游標停住時加底線
                        let hot = self.hover == Hit::SettingsPart(k, n);
                        g.text(&text, Font::Text, *r, if hot { c.text } else { c.text_secondary });
                        if hot {
                            g.hline(r.l, r.r, (r.t + r.h() / 2.0 + self.theme.font.size * 0.75).round(), c.text);
                        }
                        if n + 1 < parts.len() {
                            g.text(Self::LINK_SEP, Font::Text, Rect::new(r.r, rr.t, rr.r, rr.b), c.text_secondary);
                        }
                    }
                }
                Row::Project(_, gi, 0) => {
                    let rh = self.row_h();
                    for j in 0..PROJECT_COLUMNS {
                        let Some(proj) = page.projects.get(gi * PROJECT_COLUMNS + j) else { break };
                        let (card, cover) = self.project_card(rr, j);
                        if self.hover == Hit::SettingsPart(k, j) {
                            g.fill_round(card, radius, c.hover);
                        }
                        g.fill_round(cover, radius, c.surface);
                        if let Some(px) = &proj.cover {
                            g.keyed_image(&format!("about-cover:{}", proj.id), px, cover, 1.0);
                        }
                        g.stroke_round(cover, radius, c.border, 1.0);
                        let x = cover.r + 14.0;
                        g.text(&proj.name, Font::Bold, Rect::new(x, card.t + 4.0, card.r - 8.0, card.t + 4.0 + rh), c.text);
                        g.text(&proj.tagline, Font::Text, Rect::new(x, card.t + 4.0 + rh, card.r - 8.0, card.b - 4.0), c.text_secondary);
                    }
                }
                Row::Project(..) => {}
            }
        }
        g.pop_clip();
        self.paint_button(pl.close, tr_key!("關閉"), false, self.hover == Hit::SettingsButton(0));
    }
}
