//! 歡迎畫面：第一次啟動時（程式旁還沒有 session.toml）在視窗中央顯示的卡片，列出常用的設定。
//!
//! - 項目是 `zing_core::prefs::PREFS` 的一部分（`ITEMS`）。以下拉選單（menu.rs）選的選項（語言、主題…）立即套用並寫回
//!   config.toml（`App::set_pref`，與設定頁相同），背後的畫面立即看到效果。
//! - 開關（顯示隱藏檔案、預設的檔案總管、NTFS 索引服務…）按下按鈕才套用（`Welcome::pending`）：第一次啟動時
//!   `ITEMS` 標記的預設開啟；Esc 關閉不套用。NTFS 索引服務已安裝時開關是灰色的「已啟用」。
//! - 第一幀之後才顯示；開著時攔下滑鼠與鍵盤（↑↓／Tab 移動、Enter／空白鍵切換、Esc 關閉）。
//! - 「更多設定」套用後開啟設定頁；命令 `welcome.show`（「⋯」選單）可再次開啟。

use zing_core::prefs::{BUILTIN_THEMES, Kind, PREFS, choice_label};

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::menu::{Action, MenuItem};
use crate::sidepanel::footer_buttons;
use crate::view::Hit;

/// 卡片上的設定（`PREFS` 的 key；空字串的依種類找）與第一次啟動時開關是否預設開啟
const ITEMS: &[(Item, bool)] = &[
    (Item::Key("language"), false),
    (Item::Key("theme"), false),
    (Item::DefaultApp, true),
    (Item::Key("startup"), false),
    (Item::Key("show_hidden"), true),
    (Item::Key("update.check"), false),
    (Item::Action("index.install"), true),
];
pub const BUTTONS: [&str; 2] = [tr_key!("更多設定…"), tr_key!("開始使用")];
const CARD_W: f32 = 560.0;

enum Item {
    Key(&'static str),
    DefaultApp,
    Action(&'static str),
}

#[derive(Default)]
pub struct Welcome {
    /// 鍵盤焦點：0..項目數是項目，之後是兩個按鈕
    pub focus: usize,
    /// 每個項目的開關按下按鈕時要改成的值（None：沿用目前的值；選項一律 None）
    pub pending: Vec<Option<bool>>,
    /// NTFS 索引服務是否已安裝（背景查詢，None 是還不知道）
    pub index_installed: Option<bool>,
}

pub struct WelcomeLayout {
    pub card: Rect,
    pub title: Rect,
    pub subtitle: Rect,
    /// 每個項目：整列（含說明）、名稱那一行、控制項
    pub rows: Vec<(Rect, Rect, Rect)>,
    pub buttons: [Rect; 2],
    pub hints: bool,
}

/// 卡片上各項目在 `PREFS` 中的索引
pub fn items() -> Vec<usize> {
    ITEMS
        .iter()
        .filter_map(|(it, _)| item_index(it))
        .collect()
}

fn item_index(it: &Item) -> Option<usize> {
    PREFS.iter().position(|p| match it {
        Item::Key(k) => p.key == *k,
        Item::DefaultApp => p.kind == Kind::DefaultApp,
        Item::Action(cmd) => p.kind == Kind::Action(cmd),
    })
}

/// 說明的行數（沒有說明是 0）
fn hint_lines(i: usize) -> usize {
    if PREFS[i].hint.is_empty() { 0 } else { zing_core::i18n::tr(PREFS[i].hint).lines().count() }
}

impl App {
    pub fn open_welcome(&mut self) {
        self.close_menu();
        self.close_palette();
        self.close_side_panels();
        self.refresh_default_app(false);
        let first = self.first_run;
        let pending = ITEMS.iter().filter(|(it, _)| item_index(it).is_some()).map(|&(_, on)| (first && on).then_some(true)).collect();
        self.welcome = Some(Welcome { focus: 0, pending, index_installed: None });
        self.query_index_installed();
        self.invalidate();
    }

    pub fn on_index_installed(&mut self, installed: bool) {
        if let Some(w) = self.welcome.as_mut() {
            w.index_installed = Some(installed);
            self.invalidate();
        }
    }

    /// 第 k 個項目的開關目前的值與能否切換（選項是 None）
    fn welcome_current(&self, k: usize) -> Option<(bool, bool)> {
        let p = &PREFS[*items().get(k)?];
        match p.kind {
            Kind::Bool => Some((self.config.pref_value(p.key) == "true", true)),
            Kind::DefaultApp => Some((self.default_app == Some(crate::defaultapp::Status::On), self.default_app.is_some())),
            // 已安裝的服務在這裡不移除（設定頁的「移除 NTFS 索引服務」）；還不知道時也不能切換
            Kind::Action(_) => {
                let installed = self.welcome.as_ref()?.index_installed;
                Some((installed == Some(true), installed == Some(false)))
            }
            Kind::Choice(_) | Kind::Theme | Kind::Text | Kind::Tabs | Kind::SlowList => None,
        }
    }

    /// 第 k 個項目的開關畫出來的值（按下按鈕後的值）與能否切換
    pub fn welcome_switch(&self, k: usize) -> Option<(bool, bool)> {
        let (current, enabled) = self.welcome_current(k)?;
        let pending = self.welcome.as_ref()?.pending.get(k).copied().flatten();
        Some((pending.unwrap_or(current), enabled))
    }

    /// 按下按鈕：套用與目前不同的開關
    fn welcome_apply(&mut self) {
        let Some(pending) = self.welcome.as_ref().map(|w| w.pending.clone()) else { return };
        for (k, (&i, want)) in items().iter().zip(pending).enumerate() {
            let (Some(on), Some((current, _))) = (want, self.welcome_current(k)) else { continue };
            if on == current {
                continue;
            }
            match PREFS[i].kind {
                Kind::Bool => self.set_pref(i, on.to_string()),
                Kind::DefaultApp => self.set_default_app(on),
                // 隔離模式（冒煙測試）不跳出 UAC
                Kind::Action(cmd) if on && !self.isolated => self.exec(cmd),
                _ => {}
            }
        }
    }

    pub fn close_welcome(&mut self) {
        if self.welcome.take().is_some() {
            self.close_menu();
            self.invalidate();
        }
    }

    pub fn welcome_layout(&self) -> Option<WelcomeLayout> {
        self.welcome.as_ref()?;
        let (w, h) = self.gfx.size();
        let rh = self.row_h();
        let width = CARD_W.min(w - 32.0).max(320.0);
        let list = items();
        let line = (rh + 8.0).max(28.0);
        let button_h = (rh + 6.0).max(28.0);
        let fixed = 18.0 + rh + 4.0 + rh + 14.0 + 12.0 + button_h + 18.0;
        // 說明每一行多一列的高度；視窗太矮時不顯示說明
        let with_hints: Vec<f32> = list.iter().map(|&i| line + rh * hint_lines(i) as f32).collect();
        let hints = fixed + with_hints.iter().sum::<f32>() <= h - 16.0;
        let heights: Vec<f32> = if hints { with_hints } else { vec![line; list.len()] };
        let card_h = fixed + heights.iter().sum::<f32>();
        let l = ((w - width) / 2.0).round();
        let t = ((h - card_h) / 2.0).max(8.0).round();
        let card = Rect::new(l, t, l + width, t + card_h);
        let inner = card.inset(18.0, 0.0);
        let title = Rect::new(inner.l, t + 18.0, inner.r, t + 18.0 + rh);
        let subtitle = Rect::new(inner.l, title.b + 4.0, inner.r, title.b + 4.0 + rh);
        let value_l = inner.l + (inner.w() * 0.55).min(300.0);
        let mut y = subtitle.b + 14.0;
        let rows = heights
            .iter()
            .map(|&row_h| {
                let row = Rect::new(inner.l - 6.0, y, inner.r + 6.0, y + row_h);
                let first = Rect::new(inner.l, y, inner.r, y + line);
                let control = Rect::new(value_l, y, inner.r, y + line);
                y += row_h;
                (row, first, control)
            })
            .collect();
        let buttons = footer_buttons(Rect::new(inner.l, y + 12.0, inner.r, y + 12.0 + button_h), rh);
        Some(WelcomeLayout { card, title, subtitle, rows, buttons, hints })
    }

    /// 開著時所有位置都屬於歡迎畫面（底下的元件點不到）
    pub fn welcome_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let lay = self.welcome_layout()?;
        if let Some(k) = lay.buttons.iter().position(|b| b.contains(x, y)) {
            return Some(Hit::WelcomeButton(k));
        }
        if let Some(k) = lay.rows.iter().position(|(r, _, _)| r.contains(x, y)) {
            return Some(Hit::WelcomeRow(k));
        }
        Some(Hit::WelcomePanel)
    }

    pub fn welcome_click(&mut self, hit: Hit) {
        match hit {
            Hit::WelcomeRow(k) => {
                if let Some(w) = self.welcome.as_mut() {
                    w.focus = k;
                }
                self.welcome_activate(k);
            }
            Hit::WelcomeButton(b) => self.welcome_button(b),
            _ => {}
        }
        self.invalidate();
    }

    fn welcome_button(&mut self, b: usize) {
        self.welcome_apply();
        self.close_welcome();
        if b == 0 {
            self.open_settings();
        }
    }

    /// 第 k 個項目：選項開下拉選單、開關切換、命令執行
    fn welcome_activate(&mut self, k: usize) {
        let Some(&i) = items().get(k) else { return };
        let p = &PREFS[i];
        match p.kind {
            Kind::Choice(_) | Kind::Theme => {
                let current = self.config.pref_value(p.key);
                let list = match p.kind {
                    Kind::Choice(list) => list,
                    _ => BUILTIN_THEMES,
                };
                let menu: Vec<MenuItem> = list
                    .iter()
                    .map(|(v, _)| {
                        let mut it = MenuItem::new(choice_label(p, v), Action::Pref(i, v.to_string()));
                        it.checked = v.eq_ignore_ascii_case(&current);
                        it
                    })
                    .collect();
                if let Some((_, _, control)) = self.welcome_layout().and_then(|l| l.rows.get(k).copied()) {
                    self.open_menu(menu, control);
                }
            }
            Kind::Bool | Kind::DefaultApp | Kind::Action(_) => {
                if let Some((on, true)) = self.welcome_switch(k)
                    && let Some(slot) = self.welcome.as_mut().and_then(|w| w.pending.get_mut(k))
                {
                    *slot = Some(!on);
                }
            }
            Kind::Text | Kind::Tabs | Kind::SlowList => {}
        }
    }

    /// ↑↓、Tab／Shift+Tab 移動焦點，Enter／空白鍵啟動，Esc 關閉
    pub fn welcome_key(&mut self, vk: u16, shift: bool) -> bool {
        let n = items().len() + BUTTONS.len();
        let Some(w) = self.welcome.as_mut() else { return false };
        match vk {
            0x1B => self.close_welcome(),
            0x26 => w.focus = (w.focus + n - 1) % n,
            0x28 => w.focus = (w.focus + 1) % n,
            0x09 => w.focus = if shift { (w.focus + n - 1) % n } else { (w.focus + 1) % n },
            0x0D | 0x20 => {
                let f = w.focus;
                match f.checked_sub(items().len()) {
                    Some(b) => self.welcome_button(b),
                    None => self.welcome_activate(f),
                }
            }
            _ => {}
        }
        self.invalidate();
        true
    }

    pub fn paint_welcome(&self) {
        let (Some(w), Some(lay)) = (self.welcome.as_ref(), self.welcome_layout()) else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius + 2.0;
        let (ww, wh) = g.size();
        g.fill(Rect::new(0.0, 0.0, ww, wh), zing_core::theme::Color { r: 0, g: 0, b: 0, a: 128 });
        let card = lay.card;
        let shadow = zing_core::theme::Color { r: 0, g: 0, b: 0, a: 48 };
        g.fill_round(Rect::new(card.l - 2.0, card.t, card.r + 2.0, card.b + 4.0), radius + 2.0, shadow);
        g.fill_round(card, radius, c.surface);
        g.stroke_round(card, radius, c.border, 1.0);
        g.text(tr!("歡迎使用 zing-explore"), Font::Bold, lay.title, c.text);
        g.text(tr!("先調整幾個常用的設定，按「開始使用」套用；之後都能在設定頁（Ctrl+,）修改"), Font::Small, lay.subtitle, c.text_secondary);
        let rh = self.row_h();
        for (k, (&i, (row, first, control))) in items().iter().zip(&lay.rows).enumerate() {
            let p = &PREFS[i];
            if self.hover == Hit::WelcomeRow(k) {
                g.fill_round(*row, self.theme.metrics.radius, c.hover);
            }
            if w.focus == k {
                g.stroke_round(row.inset(1.0, 1.0), self.theme.metrics.radius, c.accent, 1.5);
            }
            g.text(zing_core::i18n::tr(p.label), Font::Text, Rect::new(first.l, first.t, control.l - 8.0, first.b), c.text);
            if lay.hints && !p.hint.is_empty() {
                for (n, text) in zing_core::i18n::tr(p.hint).lines().enumerate() {
                    let t = first.b - 4.0 + rh * n as f32;
                    g.text(text, Font::Small, Rect::new(first.l, t, row.r - 12.0, t + rh), c.text_secondary);
                }
            }
            match p.kind {
                Kind::Choice(_) | Kind::Theme => {
                    let v = self.config.pref_value(p.key);
                    g.text(&format!("{} ▾", choice_label(p, &v)), Font::Text, *control, c.accent);
                }
                Kind::Bool | Kind::DefaultApp | Kind::Action(_) => {
                    let (on, enabled) = self.welcome_switch(k).unwrap_or_default();
                    let right = self.paint_switch(*first, control.l, on, enabled);
                    let installed = matches!(p.kind, Kind::Action(_)) && w.index_installed == Some(true);
                    let label = if installed { tr!("已啟用") } else if on { tr!("開") } else { tr!("關") };
                    g.text(label, Font::Text, Rect::new(right + 8.0, first.t, control.r, first.b), c.text_secondary);
                }
                Kind::Text | Kind::Tabs | Kind::SlowList => {}
            }
        }
        let n = items().len();
        self.paint_footer_buttons(&lay.buttons, BUTTONS, Some(1), |b| self.hover == Hit::WelcomeButton(b) || w.focus == n + b);
    }
}
