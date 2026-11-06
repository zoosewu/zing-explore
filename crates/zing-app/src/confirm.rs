//! 確認對話框：還原預設、移除服務、完成編輯器等要再確認一次的動作（`App::confirm`）。
//!
//! - 視窗中央的小卡片，背景變暗；開著時在最上層（下拉選單之下），底下的元件點不到。
//! - 按鈕：確認（強調色）在左、取消在右（Windows 的慣例）。Enter 確認、Esc 或點卡片外取消、←→／Tab 切換按鈕。

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::sidepanel::footer_buttons;
use crate::view::Hit;

const CARD_W: f32 = 420.0;

pub struct Confirm {
    message: String,
    /// 確認按鈕的文字（原文，顯示時翻譯）
    ok: &'static str,
    /// 鍵盤焦點：0 確認、1 取消
    pub focus: usize,
    action: Box<dyn FnOnce(&mut App)>,
}

pub struct ConfirmLayout {
    pub card: Rect,
    pub text: Rect,
    pub buttons: [Rect; 2],
}

impl App {
    /// 先問 `message`，按下確認（按鈕文字 `ok`，原文）才執行 `action`
    pub fn confirm(&mut self, message: impl Into<String>, ok: &'static str, action: impl FnOnce(&mut App) + 'static) {
        self.close_menu();
        self.hide_tip();
        self.confirm = Some(Confirm { message: message.into(), ok, focus: 0, action: Box::new(action) });
        self.invalidate();
    }

    pub fn confirm_layout(&self) -> Option<ConfirmLayout> {
        let c = self.confirm.as_ref()?;
        let (w, h) = self.gfx.size();
        let rh = self.row_h();
        // 寬度依最長的一行（訊息以換行字元分行，不自動換行）
        let longest = c.message.lines().map(|l| self.gfx.measure(l, Font::Text)).fold(0.0, f32::max);
        let width = (longest + 36.0).max(CARD_W).min(w - 32.0);
        let text_h = c.message.lines().count().max(1) as f32 * (rh + 2.0);
        let button_h = (rh + 6.0).max(28.0);
        let card_h = 20.0 + text_h + 18.0 + button_h + 18.0;
        let l = ((w - width) / 2.0).round();
        let t = ((h - card_h) / 2.0).max(8.0).round();
        let card = Rect::new(l, t, l + width, t + card_h);
        let inner = card.inset(18.0, 0.0);
        let text = Rect::new(inner.l, t + 20.0, inner.r, t + 20.0 + text_h);
        let buttons = footer_buttons(Rect::new(inner.l, text.b + 18.0, inner.r, text.b + 18.0 + button_h), rh);
        Some(ConfirmLayout { card, text, buttons })
    }

    /// 開著時所有位置都屬於確認對話框
    pub fn confirm_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let lay = self.confirm_layout()?;
        if let Some(k) = lay.buttons.iter().position(|b| b.contains(x, y)) {
            return Some(Hit::ConfirmButton(k));
        }
        Some(if lay.card.contains(x, y) { Hit::ConfirmPanel } else { Hit::ConfirmOutside })
    }

    pub fn confirm_click(&mut self, hit: Hit) {
        match hit {
            Hit::ConfirmButton(k) => self.finish_confirm(k == 0),
            Hit::ConfirmOutside => self.finish_confirm(false),
            _ => {}
        }
    }

    /// Enter（或空白鍵）按下焦點所在的按鈕，Esc 取消，←→／Tab 切換按鈕；其他按鍵不做事
    pub fn confirm_key(&mut self, vk: u16) {
        let Some(c) = self.confirm.as_mut() else { return };
        match vk {
            0x1B => self.finish_confirm(false),
            0x0D | 0x20 => {
                let ok = c.focus == 0;
                self.finish_confirm(ok);
            }
            0x09 | 0x25 | 0x27 => {
                c.focus = 1 - c.focus;
                self.invalidate();
            }
            _ => {}
        }
    }

    fn finish_confirm(&mut self, ok: bool) {
        let Some(c) = self.confirm.take() else { return };
        self.invalidate();
        if ok {
            (c.action)(self);
        }
    }

    pub fn paint_confirm(&self) {
        let (Some(c), Some(lay)) = (self.confirm.as_ref(), self.confirm_layout()) else { return };
        let col = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius + 2.0;
        let (ww, wh) = g.size();
        g.fill(Rect::new(0.0, 0.0, ww, wh), zing_core::theme::Color { r: 0, g: 0, b: 0, a: 96 });
        let card = lay.card;
        g.fill_round(Rect::new(card.l - 2.0, card.t, card.r + 2.0, card.b + 4.0), radius + 2.0, zing_core::theme::Color { r: 0, g: 0, b: 0, a: 48 });
        g.fill_round(card, radius, col.surface);
        g.stroke_round(card, radius, col.border, 1.0);
        let line_h = self.row_h() + 2.0;
        for (n, line) in c.message.lines().enumerate() {
            let t = lay.text.t + n as f32 * line_h;
            g.text(line, Font::Text, Rect::new(lay.text.l, t, lay.text.r, t + line_h), col.text);
        }
        self.paint_footer_buttons(&lay.buttons, [c.ok, tr_key!("取消")], Some(0), |k| self.hover == Hit::ConfirmButton(k));
        let focused = lay.buttons[c.focus];
        g.stroke_round(focused.inset(-2.0, -2.0), self.theme.metrics.radius, col.accent, 1.5);
    }
}
