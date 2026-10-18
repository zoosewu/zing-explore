//! 游標停留提示：圖示按鈕的名稱、快速鍵，以及不能用的原因。
//!
//! 第一行是名稱，快速鍵以次要顏色靠右；按鈕不能用時，第二行以小字說明原因。
//! 停留 500 ms 才出現；已經有提示時移到另一個按鈕立即切換（不必再等）；移開、按下滑鼠、捲動就消失。
//! 畫在元件下方（放不下就上方），跟著主題的顏色與字型。

use zing_core::cmdbar::label_for;

use crate::app::{App, TIMER_TIP};
use crate::gfx::{Font, Rect};
use crate::view::Hit;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TipText {
    pub title: String,
    pub key: Option<String>,
    /// 不能用的原因
    pub detail: Option<String>,
}

impl TipText {
    /// 測試用：一行的文字
    #[cfg_attr(not(feature = "smoke"), allow(dead_code))]
    pub fn joined(&self) -> String {
        [Some(self.title.clone()), self.key.clone(), self.detail.clone()].into_iter().flatten().collect::<Vec<_>>().join(" ")
    }
}

pub struct Tip {
    pub hit: Hit,
    pub text: TipText,
    /// 指向的元件
    pub anchor: Rect,
    pub shown: bool,
}

const NAV: [&str; 4] = ["nav.back", "nav.forward", "nav.up", "nav.refresh"];
/// 名稱與快速鍵之間的間距
const KEY_GAP: f32 = 20.0;

impl App {
    fn tip_text(&self, title: &str, cmd: &str, check_disabled: bool) -> TipText {
        TipText {
            title: title.to_string(),
            key: self.key_text(cmd),
            detail: if check_disabled { self.cmd_disabled(cmd).map(String::from) } else { None },
        }
    }

    /// 這個位置的提示與元件範圍
    fn tip_for(&self, hit: &Hit) -> Option<(TipText, Rect)> {
        let lay = &self.layout;
        match *hit {
            Hit::Nav(i) => Some((self.tip_text(label_for(NAV[i]), NAV[i], true), lay.nav[i])),
            Hit::SettingsGear => Some((self.tip_text(tr!("設定"), "settings.open", false), lay.settings_btn)),
            Hit::CmdButton(k) => {
                let bl = lay.cmdbar.as_ref()?;
                let &(i, r, _) = bl.buttons.get(k)?;
                let it = self.config.cmdbar.get(i)?;
                Some((self.tip_text(&it.title(), &it.cmd, !it.is_menu()), r))
            }
            Hit::TabNew(p) => Some((self.tip_text(tr!("新增分頁"), "tab.new", false), lay.panes.get(p)?.tab_new)),
            Hit::TabClose(p, t) => Some((self.tip_text(tr!("關閉分頁"), "tab.close", false), *lay.panes.get(p)?.tab_close.get(t)?)),
            Hit::FilterChip => Some((TipText { title: tr!("清除篩選").into(), key: Some("Esc".into()), detail: None }, lay.filter_chip?)),
            _ => None,
        }
    }

    /// 不等待、立即顯示（鍵盤移到命令列的按鈕時）
    pub fn show_tip_now(&mut self, hit: &Hit) {
        match self.tip_for(hit) {
            Some((text, anchor)) => {
                self.tip = Some(Tip { hit: hit.clone(), text, anchor, shown: true });
                self.invalidate();
            }
            None => self.hide_tip(),
        }
    }

    /// 游標移到新的位置（hover 改變時呼叫）
    pub fn update_tip(&mut self, hit: &Hit) {
        if self.menu.is_some() {
            return self.hide_tip();
        }
        match self.tip_for(hit) {
            Some((text, anchor)) => {
                let already = self.tip.as_ref().is_some_and(|t| t.shown);
                self.tip = Some(Tip { hit: hit.clone(), text, anchor, shown: already });
                if already {
                    self.invalidate();
                } else {
                    unsafe {
                        windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), TIMER_TIP, 500, None);
                    }
                }
            }
            None => self.hide_tip(),
        }
    }

    pub fn on_tip_timer(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), TIMER_TIP);
        }
        let Some(hit) = self.tip.as_ref().map(|t| t.hit.clone()).filter(|h| *h == self.hover) else { return };
        // 狀態可能改變了（例如選取後按鈕變成可用）：重新取得文字
        if let Some((text, anchor)) = self.tip_for(&hit)
            && let Some(t) = self.tip.as_mut()
        {
            t.text = text;
            t.anchor = anchor;
            t.shown = true;
        }
        self.invalidate();
    }

    pub fn hide_tip(&mut self) {
        if let Some(t) = self.tip.take() {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), TIMER_TIP);
            }
            if t.shown {
                self.invalidate();
            }
        }
    }

    pub fn paint_tip(&self) {
        let Some(t) = self.tip.as_ref().filter(|t| t.shown) else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let tt = &t.text;
        let title_w = g.measure(&tt.title, Font::Text);
        let key_w = tt.key.as_ref().map_or(0.0, |k| g.measure(k, Font::Small) + KEY_GAP);
        let detail_w = tt.detail.as_ref().map_or(0.0, |d| g.measure(d, Font::Small));
        let w = (title_w + key_w).max(detail_w) + 20.0;
        let line = (self.row_h() + 2.0).max(22.0);
        let small = (line - 4.0).max(18.0);
        let h = line + if tt.detail.is_some() { small - 2.0 } else { 0.0 };
        let (ww, wh) = g.size();
        let l = ((t.anchor.l + t.anchor.r - w) / 2.0).clamp(4.0, (ww - w - 4.0).max(4.0));
        let top = if t.anchor.b + 6.0 + h > wh - 4.0 { t.anchor.t - 6.0 - h } else { t.anchor.b + 6.0 };
        let r = Rect::new(l, top, l + w, top + h);
        let shadow = zing_core::theme::Color { r: 0, g: 0, b: 0, a: 48 };
        g.fill_round(Rect::new(r.l, r.t + 1.0, r.r + 1.0, r.b + 3.0), 5.0, shadow);
        g.fill_round(r, 4.0, c.surface);
        g.stroke_round(r, 4.0, c.border, 1.0);
        let first = Rect::new(r.l + 10.0, r.t, r.r - 10.0, r.t + line);
        g.text(&tt.title, Font::Text, first, c.text);
        if let Some(k) = &tt.key {
            g.text(k, Font::SmallRight, first, c.text_secondary);
        }
        if let Some(d) = &tt.detail {
            g.text(d, Font::Small, Rect::new(first.l, first.b - 4.0, first.r, first.b - 4.0 + small), c.text_secondary);
        }
    }
}
