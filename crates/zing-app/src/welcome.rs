//! 歡迎畫面：第一次啟動時（程式旁還沒有 session.toml）在視窗中央顯示的卡片，列出常用的設定。
//!
//! - 項目是 `zing_core::prefs::PREFS` 的一部分（`ITEMS`），改了立即套用並寫回 config.toml（`App::set_pref`，與設定頁相同），
//!   背後的畫面立即看到效果；選項以下拉選單（menu.rs）選擇。
//! - 第一幀之後才顯示；開著時攔下滑鼠與鍵盤（↑↓／Tab 移動、Enter／空白鍵切換、Esc 關閉）。
//! - 「更多設定」開啟設定頁；命令 `welcome.show`（「⋯」選單）可再次開啟。

use zing_core::prefs::{BUILTIN_THEMES, Kind, PREFS, choice_label};

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::menu::{Action, MenuItem};
use crate::sidepanel::footer_buttons;
use crate::view::Hit;

/// 卡片上的設定（`PREFS` 的 key；空字串的依種類找）
const ITEMS: &[Item] = &[
    Item::Key("language"),
    Item::Key("theme"),
    Item::DefaultApp,
    Item::Key("startup"),
    Item::Key("show_hidden"),
    Item::Key("update.check"),
    Item::Action("index.install"),
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
        .filter_map(|it| {
            PREFS.iter().position(|p| match it {
                Item::Key(k) => p.key == *k,
                Item::DefaultApp => p.kind == Kind::DefaultApp,
                Item::Action(cmd) => p.kind == Kind::Action(cmd),
            })
        })
        .collect()
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
        self.welcome = Some(Welcome::default());
        self.invalidate();
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
            Kind::Bool => {
                let on = self.config.pref_value(p.key) == "true";
                self.set_pref(i, (!on).to_string());
            }
            Kind::DefaultApp => self.toggle_default_app(),
            Kind::Action(cmd) => self.exec(cmd),
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
        g.text(tr!("先調整幾個常用的設定；之後都能在設定頁（Ctrl+,）修改"), Font::Small, lay.subtitle, c.text_secondary);
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
                Kind::Bool | Kind::DefaultApp => {
                    let (on, enabled) = match p.kind {
                        Kind::Bool => (self.config.pref_value(p.key) == "true", true),
                        _ => (self.default_app == Some(crate::defaultapp::Status::On), self.default_app.is_some()),
                    };
                    let right = self.paint_switch(*first, control.l, on, enabled);
                    let label = if on { tr!("開") } else { tr!("關") };
                    g.text(label, Font::Text, Rect::new(right + 8.0, first.t, control.r, first.b), c.text_secondary);
                }
                Kind::Action(_) => {
                    let label = tr!("啟用…");
                    let bw = g.measure(label, Font::Text) + 24.0;
                    let b = Rect::new(control.l, first.t + 3.0, control.l + bw, first.b - 3.0);
                    g.stroke_round(b, self.theme.metrics.radius, c.border, 1.0);
                    g.text(label, Font::Text, Rect::new(b.l + 12.0, b.t, b.r, b.b), c.text);
                }
                Kind::Text | Kind::Tabs | Kind::SlowList => {}
            }
        }
        let n = items().len();
        self.paint_footer_buttons(&lay.buttons, BUTTONS, Some(1), |b| self.hover == Hit::WelcomeButton(b) || w.focus == n + b);
    }
}
