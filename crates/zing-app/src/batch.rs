//! 批次重新命名面板：選取多個項目時按 F2（或命令 file.batchrename）。
//!
//! 「搜尋」「取代為」兩個 Win32 EDIT（支援輸入法），其餘由 D2D 繪製；規則與預覽在 `zing_core::batchrename`。
//! Tab 切換欄位，Enter 執行，Esc 取消，↑↓ 捲動預覽，Alt+R 規則運算式、Alt+C 區分大小寫、
//! Alt+E 範圍、Alt+U 大小寫轉換（也可點選項）。有衝突時不能執行。
//! 執行交給檔案作業佇列（FileOp::BatchRename）；新名稱與舊名稱互相重疊時經過暫時名稱分兩階段改名。

use windows::Win32::Foundation::HWND;
use zing_core::batchrename::{CaseMode, Planned, Rule, Scope, plan};
use zing_core::theme::Color;

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::services::TabId;
use crate::shell::FileOp;
use crate::view::Hit;
use crate::window;

const WIDTH: f32 = 780.0;
const PAD: f32 = 14.0;
/// 預覽一次顯示的列數
pub const ROWS: usize = 12;

/// 面板送給主視窗的動作（WM_APP_BATCH 的 wParam）
pub const CMD_RUN: usize = 1;
pub const CMD_CANCEL: usize = 2;
pub const CMD_TAB: usize = 3;
/// lParam：捲動的列數（有號）
pub const CMD_SCROLL: usize = 4;
/// lParam：選項索引（0 規則運算式、1 區分大小寫、2 範圍、3 大小寫）
pub const CMD_TOGGLE: usize = 5;

pub struct Batch {
    pub tab: TabId,
    pub dir: String,
    pub items: Vec<(String, bool)>,
    /// 資料夾中沒有要改名的其他項目
    others: Vec<String>,
    pub rule: Rule,
    /// 兩個輸入框：搜尋、取代為
    pub edits: [HWND; 2],
    pub field: usize,
    pub plan: Result<Vec<Planned>, String>,
    pub top: usize,
    /// 嘗試執行但有衝突時顯示提示
    pub blocked: bool,
}

impl Batch {
    fn replan(&mut self) {
        self.plan = plan(&self.items, &self.rule, &self.others);
        self.blocked = false;
    }

    /// (會改名的數量, 有問題的數量)
    pub fn counts(&self) -> (usize, usize) {
        match &self.plan {
            Ok(p) => (p.iter().filter(|x| x.changed()).count(), p.iter().filter(|x| x.problem.is_some()).count()),
            Err(_) => (0, 0),
        }
    }
}

pub struct BatchLayout {
    pub card: Rect,
    pub title: Rect,
    pub labels: [Rect; 2],
    pub inputs: [Rect; 2],
    pub chips: [Rect; 4],
    pub help: Rect,
    pub rows: Vec<Rect>,
    pub footer: Rect,
    pub run: Rect,
    pub cancel: Rect,
}

/// 實心底色上的文字：亮的底色用深色字
fn ink_on(bg: Color) -> Color {
    let l = 0.2126 * bg.r as f32 + 0.7152 * bg.g as f32 + 0.0722 * bg.b as f32;
    if l > 150.0 { Color::rgb(0x141414) } else { Color::rgb(0xffffff) }
}

fn scope_name(s: Scope) -> &'static str {
    match s {
        Scope::Stem => tr!("範圍：主檔名"),
        Scope::Ext => tr!("範圍：副檔名"),
        Scope::Full => tr!("範圍：整個名稱"),
    }
}

fn case_name(c: CaseMode) -> &'static str {
    match c {
        CaseMode::Keep => tr!("大小寫：不變"),
        CaseMode::Lower => tr!("大小寫：全小寫"),
        CaseMode::Upper => tr!("大小寫：全大寫"),
        CaseMode::Title => tr!("大小寫：字首大寫"),
    }
}

impl App {
    /// 開啟面板；選取（含焦點）少於兩個項目時回傳 false
    pub fn open_batch_rename(&mut self) -> bool {
        if self.batch.is_some() {
            return true;
        }
        let tab = self.tab();
        if tab.path.is_empty() {
            return false;
        }
        let picked: Vec<usize> = tab.view.iter().map(|&e| e as usize).filter(|&e| tab.selected[e]).collect();
        if picked.len() < 2 {
            return false;
        }
        let items: Vec<(String, bool)> =
            picked.iter().map(|&e| (tab.listing.name_string(e), tab.listing.entries[e].is_dir())).collect();
        let others: Vec<String> =
            (0..tab.listing.len()).filter(|&e| !tab.selected[e]).map(|e| tab.listing.name_string(e)).collect();
        let (tab_id, dir) = (tab.id, tab.path.clone());
        if let Some(e) = self.edit.take() {
            window::destroy_edit(e.hwnd);
        }
        let rule = Rule::default();
        let plan = plan(&items, &rule, &others);
        self.batch = Some(Batch {
            tab: tab_id,
            dir,
            items,
            others,
            rule,
            edits: [HWND::default(); 2],
            field: 0,
            plan,
            top: 0,
            blocked: false,
        });
        if let Some(lay) = self.batch_layout() {
            let e0 = window::make_batch_edit(self, lay.inputs[0].inset(8.0, 0.0));
            let e1 = window::make_batch_edit(self, lay.inputs[1].inset(8.0, 0.0));
            let b = self.batch.as_mut().unwrap();
            b.edits = [e0.unwrap_or_default(), e1.unwrap_or_default()];
            window::focus_edit(b.edits[0]);
        }
        self.invalidate();
        true
    }

    pub fn close_batch(&mut self) {
        if let Some(b) = self.batch.take() {
            for e in b.edits {
                window::destroy_edit(e);
            }
            window::focus_main(self.hwnd);
            self.invalidate();
        }
    }

    pub fn batch_layout(&self) -> Option<BatchLayout> {
        self.batch.as_ref()?;
        let (w, h) = self.gfx.size();
        let width = WIDTH.min(w - 32.0).max(320.0);
        let rh = self.row_h();
        let field_h = (rh + 8.0).max(28.0);
        let full = PAD + rh + 8.0 + 2.0 * (field_h + 6.0) + rh + 4.0 + rh + 6.0 + ROWS as f32 * rh + 8.0 + rh + 8.0 + PAD;
        let l = ((w - width) / 2.0).round();
        let t = ((h - full) / 2.0).max(6.0).round();
        let inner_l = l + PAD;
        let inner_r = l + width - PAD;
        let label_w = 72.0;
        let title = Rect::new(inner_l, t + PAD, inner_r, t + PAD + rh);
        let mut y = title.b + 8.0;
        let mut labels = [Rect::default(); 2];
        let mut inputs = [Rect::default(); 2];
        for k in 0..2 {
            labels[k] = Rect::new(inner_l, y, inner_l + label_w, y + field_h);
            inputs[k] = Rect::new(inner_l + label_w, y, inner_r, y + field_h);
            y += field_h + 6.0;
        }
        let mut chips = [Rect::default(); 4];
        let mut x = inner_l + label_w;
        let p = self.batch.as_ref().unwrap();
        let texts = [tr!("規則運算式"), tr!("區分大小寫"), scope_name(p.rule.scope), case_name(p.rule.case)];
        for (k, text) in texts.iter().enumerate() {
            let cw = self.gfx.measure(text, Font::Small) + 24.0;
            chips[k] = Rect::new(x, y, x + cw, y + rh);
            x += cw + 6.0;
        }
        y += rh + 4.0;
        let help = Rect::new(inner_l + label_w, y, inner_r, y + rh);
        y += rh + 6.0;
        let rows: Vec<Rect> = (0..ROWS).map(|i| Rect::new(inner_l, y + i as f32 * rh, inner_r, y + (i + 1) as f32 * rh)).collect();
        y += ROWS as f32 * rh + 8.0;
        let footer = Rect::new(inner_l, y, inner_r, y + rh);
        let cancel = Rect::new(inner_r - 72.0, y - 2.0, inner_r, y + rh + 2.0);
        let run = Rect::new(cancel.l - 8.0 - 96.0, cancel.t, cancel.l - 8.0, cancel.b);
        Some(BatchLayout {
            card: Rect::new(l, t, l + width, footer.b + PAD),
            title,
            labels,
            inputs,
            chips,
            help,
            rows,
            footer,
            run,
            cancel,
        })
    }

    /// 面板開啟時的點擊判定：面板外為 Hit::None（點擊會取消）
    pub fn batch_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let lay = self.batch_layout()?;
        if !lay.card.contains(x, y) {
            return Some(Hit::None);
        }
        if let Some(i) = lay.chips.iter().position(|r| r.contains(x, y)) {
            return Some(Hit::BatchChip(i));
        }
        if let Some(i) = lay.inputs.iter().position(|r| r.contains(x, y)) {
            return Some(Hit::BatchField(i));
        }
        if lay.run.contains(x, y) {
            return Some(Hit::BatchRun);
        }
        if lay.cancel.contains(x, y) {
            return Some(Hit::BatchCancel);
        }
        Some(Hit::Batch)
    }

    pub fn batch_click(&mut self, hit: Hit) {
        match hit {
            Hit::BatchChip(i) => self.batch_toggle(i),
            Hit::BatchField(i) => self.batch_focus(i),
            Hit::BatchRun => self.run_batch(),
            Hit::BatchCancel | Hit::None => self.close_batch(),
            _ => self.batch_focus(self.batch.as_ref().map_or(0, |b| b.field)),
        }
    }

    fn batch_focus(&mut self, field: usize) {
        if let Some(b) = self.batch.as_mut() {
            b.field = field.min(1);
            window::focus_edit(b.edits[b.field]);
            self.invalidate();
        }
    }

    pub fn batch_toggle(&mut self, i: usize) {
        let Some(b) = self.batch.as_mut() else { return };
        let r = &mut b.rule;
        match i {
            0 => r.regex = !r.regex,
            1 => r.case_sensitive = !r.case_sensitive,
            2 => {
                r.scope = match r.scope {
                    Scope::Stem => Scope::Ext,
                    Scope::Ext => Scope::Full,
                    Scope::Full => Scope::Stem,
                }
            }
            _ => {
                r.case = match r.case {
                    CaseMode::Keep => CaseMode::Lower,
                    CaseMode::Lower => CaseMode::Upper,
                    CaseMode::Upper => CaseMode::Title,
                    CaseMode::Title => CaseMode::Keep,
                }
            }
        }
        b.replan();
        let field = b.field;
        self.batch_focus(field);
    }

    /// 輸入框內容改變（EN_CHANGE）；不是面板的輸入框回傳 false
    pub fn batch_text_changed(&mut self, hwnd: HWND, text: String) -> bool {
        let Some(b) = self.batch.as_mut() else { return false };
        let Some(k) = b.edits.iter().position(|&e| e == hwnd) else { return false };
        if k == 0 {
            b.rule.find = text;
        } else {
            b.rule.replace = text;
        }
        b.replan();
        self.invalidate();
        true
    }

    /// 輸入框送來的按鍵動作（WM_APP_BATCH）
    pub fn batch_command(&mut self, cmd: usize, arg: isize) {
        match cmd {
            CMD_RUN => self.run_batch(),
            CMD_CANCEL => self.close_batch(),
            CMD_TAB => {
                let next = self.batch.as_ref().map_or(0, |b| 1 - b.field);
                self.batch_focus(next);
            }
            CMD_SCROLL => {
                if let Some(b) = self.batch.as_mut() {
                    let n = b.items.len();
                    b.top = (b.top as isize + arg).clamp(0, n.saturating_sub(ROWS) as isize) as usize;
                    self.invalidate();
                }
            }
            CMD_TOGGLE => self.batch_toggle(arg as usize),
            _ => {}
        }
    }

    /// 執行：有衝突或規則有誤時不執行（提示），沒有任何變更時直接關閉
    pub fn run_batch(&mut self) {
        let Some(b) = self.batch.as_mut() else { return };
        let (changed, problems) = b.counts();
        if b.plan.is_err() || problems > 0 {
            b.blocked = true;
            self.invalidate();
            return;
        }
        let pairs: Vec<(String, String)> = match &b.plan {
            Ok(p) => p.iter().filter(|x| x.changed()).map(|x| (x.old.clone(), x.new.clone())).collect(),
            Err(_) => Vec::new(),
        };
        let (tab, dir) = (b.tab, b.dir.clone());
        self.close_batch();
        if changed > 0 && self.find_tab(tab).is_some() {
            self.submit_op(FileOp::BatchRename { dir, pairs });
        }
    }

    pub fn paint_batch(&self) {
        let (Some(b), Some(lay)) = (self.batch.as_ref(), self.batch_layout()) else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius + 2.0;
        let (w, h) = g.size();
        // 底下的內容變暗，表示面板是模態的
        g.fill(Rect::new(0.0, 0.0, w, h), Color { r: 0, g: 0, b: 0, a: 70 });
        let card = lay.card;
        g.fill_round(card, radius, c.surface);
        g.stroke_round(card, radius, c.border, 1.0);
        let n = b.items.len();
        g.text(&tr!("批次重新命名 {n} 個項目", n), Font::Bold, lay.title, c.text);
        for (k, label) in [tr!("搜尋"), tr!("取代為")].iter().enumerate() {
            g.text(label, Font::Text, lay.labels[k], c.text_secondary);
            let r = lay.inputs[k];
            g.fill_round(r, self.theme.metrics.radius, c.background);
            g.stroke_round(r, self.theme.metrics.radius, if b.field == k { c.accent } else { c.border }, 1.0);
        }
        let on = [b.rule.regex, b.rule.case_sensitive, b.rule.scope != Scope::Stem, b.rule.case != CaseMode::Keep];
        let texts = [tr!("規則運算式"), tr!("區分大小寫"), scope_name(b.rule.scope), case_name(b.rule.case)];
        for k in 0..4 {
            let r = lay.chips[k];
            let fill = if on[k] { c.selection } else if self.hover == Hit::BatchChip(k) { c.hover } else { c.background };
            g.fill_round(r, r.h() / 2.0, fill);
            g.stroke_round(r, r.h() / 2.0, c.border, 1.0);
            let col = if on[k] { c.selection_text } else { c.text };
            g.text(texts[k], Font::Small, Rect::new(r.l + 12.0, r.t, r.r - 6.0, r.b), col);
        }
        let help = if b.rule.regex {
            tr!("{n} 編號、{n:3} 補零、{n:3:10} 從 10 開始；$1、${名稱} 引用群組；Alt+R/C/E/U 切換選項")
        } else {
            tr!("搜尋空白時以「取代為」當作新名稱；{n} 編號、{n:3} 補零、{n:3:10} 從 10 開始；Alt+R/C/E/U 切換選項")
        };
        g.text(help, Font::Small, lay.help, c.text_secondary);

        // 預覽：舊名稱 → 新名稱（有問題的顯示原因）
        match &b.plan {
            Err(e) => g.text(e, Font::Text, lay.rows[0], c.error),
            Ok(p) => {
                let mid = (lay.rows[0].l + lay.rows[0].r) / 2.0;
                for (k, r) in lay.rows.iter().enumerate() {
                    let Some(x) = p.get(b.top + k) else { break };
                    if k % 2 == 1 {
                        g.fill(*r, c.background.with_alpha(90));
                    }
                    let old_col = if x.changed() { c.text_secondary } else { c.text_secondary.with_alpha(140) };
                    g.text(&x.old, Font::Text, Rect::new(r.l + 6.0, r.t, mid - 20.0, r.b), old_col);
                    g.text("→", Font::Text, Rect::new(mid - 16.0, r.t, mid, r.b), c.text_secondary);
                    let (text, col) = match &x.problem {
                        Some(pr) => (tr!("{name}（{problem}）", name = x.new, problem = pr.describe()), c.error),
                        None if x.changed() => (x.new.clone(), c.text),
                        None => (tr!("（不變）").to_string(), c.text_secondary.with_alpha(140)),
                    };
                    g.text(&text, Font::Text, Rect::new(mid + 4.0, r.t, r.r - 6.0, r.b), col);
                }
            }
        }

        let (changed, problems) = b.counts();
        let (msg, col) = if b.plan.is_err() {
            (tr!("規則運算式有誤").to_string(), c.error)
        } else if problems > 0 {
            (tr!("{problems} 個名稱有問題，修正後才能執行", problems), if b.blocked { c.error } else { c.text_secondary })
        } else {
            (tr!("{changed} 個項目會改名　Enter 執行　Esc 取消　Tab 切換欄位", changed), c.text_secondary)
        };
        g.text(&msg, Font::Small, Rect::new(lay.footer.l, lay.footer.t, lay.run.l - 8.0, lay.footer.b), col);
        let can_run = b.plan.is_ok() && problems == 0 && changed > 0;
        let run_fill = if can_run { c.accent } else { c.border };
        g.fill_round(lay.run, self.theme.metrics.radius, run_fill);
        let run_text = if can_run { ink_on(c.accent) } else { c.text_secondary };
        g.text(tr!("重新命名"), Font::Text, lay.run.inset(14.0, 0.0), run_text);
        let cancel_fill = if self.hover == Hit::BatchCancel { c.hover } else { c.background };
        g.fill_round(lay.cancel, self.theme.metrics.radius, cancel_fill);
        g.stroke_round(lay.cancel, self.theme.metrics.radius, c.border, 1.0);
        g.text(tr!("取消"), Font::Text, lay.cancel.inset(20.0, 0.0), c.text);
    }
}
