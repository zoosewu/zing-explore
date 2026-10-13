//! 命令面板（Ctrl+Shift+P）。輸入框沿用 Win32 EDIT（支援輸入法），清單由 D2D 繪製。
//!
//! 清單：內建命令（zing_core::palette::BUILTIN）＋工具列按鈕＋設定檔中綁定的其他命令。
//! 以 `>` 開頭的輸入直接當成命令腳本執行。

use zing_core::config::PalettePosition;
use zing_core::palette::{BUILTIN, Entry, filter, touch_recent};
use zing_core::theme::Color;

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::view::Hit;
use crate::window;

/// 一次顯示的列數
pub const MAX_ROWS: usize = 12;
const WIDTH: f32 = 640.0;
const PAD: f32 = 8.0;
const INPUT_H: f32 = 34.0;
const RECENT_MAX: usize = 20;

pub struct Palette {
    pub entries: Vec<Entry>,
    pub query: String,
    /// 依排名排序的 `entries` 索引
    pub matches: Vec<usize>,
    /// 選取的是 `matches` 中第幾項
    pub sel: usize,
    /// 第一個可見的 `matches` 索引
    pub top: usize,
}

impl Palette {
    /// `>` 開頭：直接執行的命令腳本
    fn raw(&self) -> Option<&str> {
        self.query.trim_start().strip_prefix('>').map(str::trim)
    }

    fn visible_rows(&self) -> usize {
        if self.raw().is_some() { 1 } else { self.matches.len().clamp(1, MAX_ROWS) }
    }
}

pub struct PaletteLayout {
    pub card: Rect,
    pub input: Rect,
    /// 可見的列（第 i 列對應 `matches[top + i]`）
    pub rows: Vec<Rect>,
}

impl App {
    fn palette_entries(&self) -> Vec<Entry> {
        let key = |cmd: &str| self.config.key_for(cmd).map(|k| k.to_string());
        let mut v: Vec<Entry> =
            BUILTIN.iter().map(|&(cmd, title)| Entry { title: title.into(), cmd: cmd.into(), key: key(cmd) }).collect();
        for b in &self.config.buttons {
            if !v.iter().any(|e| e.cmd == b.cmd) {
                v.push(Entry { title: format!("按鈕：{}", b.label), cmd: b.cmd.clone(), key: key(&b.cmd) });
            }
        }
        for (chord, cmd) in &self.config.keys {
            if cmd != "palette.open" && !v.iter().any(|e| &e.cmd == cmd) {
                v.push(Entry { title: cmd.clone(), cmd: cmd.clone(), key: Some(chord.to_string()) });
            }
        }
        v
    }

    pub fn open_palette(&mut self) {
        if self.palette.is_some() {
            return;
        }
        let entries = self.palette_entries();
        let matches = filter(&entries, "", &self.palette_recent);
        self.palette = Some(Palette { entries, query: String::new(), matches, sel: 0, top: 0 });
        if let Some(lay) = self.palette_layout() {
            window::begin_palette_edit(self, lay.input.inset(10.0, 0.0));
        }
        self.invalidate();
    }

    pub fn palette_layout(&self) -> Option<PaletteLayout> {
        let p = self.palette.as_ref()?;
        let (w, h) = self.gfx.size();
        let width = WIDTH.min(w - 32.0).max(200.0);
        let l = ((w - width) / 2.0).round();
        let rh = self.row_h();
        let t = match self.config.palette.position {
            PalettePosition::Top => 6.0,
            // 以清單填滿時的高度置中，篩選結果變少時輸入框不會跳動
            PalettePosition::Center => {
                let full = PAD + INPUT_H + 6.0 + MAX_ROWS as f32 * rh + PAD;
                ((h - full) / 2.0).max(6.0).round()
            }
        };
        let input = Rect::new(l + PAD, t + PAD, l + width - PAD, t + PAD + INPUT_H);
        let y0 = input.b + 6.0;
        let rows: Vec<Rect> =
            (0..p.visible_rows()).map(|i| Rect::new(input.l, y0 + i as f32 * rh, input.r, y0 + (i + 1) as f32 * rh)).collect();
        let bottom = rows.last().map_or(input.b, |r| r.b) + PAD;
        Some(PaletteLayout { card: Rect::new(l, t, l + width, bottom), input, rows })
    }

    /// 面板開啟時的點擊判定：面板外為 Hit::None（點擊會關閉面板）
    pub fn palette_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let p = self.palette.as_ref()?;
        let lay = self.palette_layout()?;
        if !lay.card.contains(x, y) {
            return Some(Hit::None);
        }
        let row = lay.rows.iter().position(|r| r.contains(x, y)).map(|i| p.top + i).filter(|&i| i < p.matches.len());
        Some(match row {
            Some(i) if p.raw().is_none() => Hit::PaletteRow(i),
            _ => Hit::Palette,
        })
    }

    /// 面板內的點擊不應讓輸入框失去焦點
    pub fn palette_contains(&self, x: f32, y: f32) -> bool {
        self.palette_layout().is_some_and(|lay| lay.card.contains(x, y))
    }

    /// 輸入框內容改變（EN_CHANGE）
    pub fn palette_query(&mut self, text: String) {
        let recent = &self.palette_recent;
        let Some(p) = self.palette.as_mut() else { return };
        p.query = text;
        p.matches = if p.raw().is_some() { Vec::new() } else { filter(&p.entries, &p.query, recent) };
        p.sel = 0;
        p.top = 0;
        self.invalidate();
    }

    pub fn palette_move(&mut self, delta: isize) {
        let Some(p) = self.palette.as_mut() else { return };
        let n = p.matches.len();
        if n == 0 {
            return;
        }
        p.sel = (p.sel as isize + delta).clamp(0, n as isize - 1) as usize;
        if p.sel < p.top {
            p.top = p.sel;
        } else if p.sel >= p.top + MAX_ROWS {
            p.top = p.sel + 1 - MAX_ROWS;
        }
        self.invalidate();
    }

    /// 關閉面板並執行選取的命令（`index` 為 `matches` 索引；None 表示目前選取項）
    pub fn run_palette(&mut self, index: Option<usize>) {
        let Some(p) = self.palette.take() else { return };
        self.close_palette_edit();
        let cmd = match p.raw() {
            Some(raw) => raw.to_string(),
            None => match p.matches.get(index.unwrap_or(p.sel)) {
                Some(&k) => {
                    let cmd = p.entries[k].cmd.clone();
                    touch_recent(&mut self.palette_recent, &cmd, RECENT_MAX);
                    cmd
                }
                None => return,
            },
        };
        if !cmd.is_empty() {
            self.exec(&cmd);
        }
    }

    pub fn close_palette(&mut self) {
        if self.palette.take().is_some() {
            self.close_palette_edit();
        }
    }

    fn close_palette_edit(&mut self) {
        if let Some(e) = self.edit.take() {
            window::destroy_edit(e.hwnd);
        }
        // 先把焦點還給主視窗，命令（例如重新命名）才可以再開自己的編輯框
        window::focus_main(self.hwnd);
        self.invalidate();
    }

    pub fn paint_palette(&self) {
        let (Some(p), Some(lay)) = (self.palette.as_ref(), self.palette_layout()) else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius + 2.0;
        let card = lay.card;
        // 陰影
        let shadow = Color { r: 0, g: 0, b: 0, a: 48 };
        g.fill_round(Rect::new(card.l - 2.0, card.t, card.r + 2.0, card.b + 4.0), radius + 2.0, shadow);
        g.fill_round(card, radius, c.surface);
        g.stroke_round(card, radius, c.border, 1.0);
        g.fill_round(lay.input, self.theme.metrics.radius, c.background);
        g.stroke_round(lay.input, self.theme.metrics.radius, c.accent, 1.0);

        let row0 = lay.rows[0];
        if let Some(raw) = p.raw() {
            g.fill_round(row0, self.theme.metrics.radius, c.selection);
            let text = if raw.is_empty() { "輸入命令腳本後按 Enter 執行".to_string() } else { format!("執行命令：{raw}") };
            g.text(&text, Font::Text, Rect::new(row0.l + 10.0, row0.t, row0.r - 10.0, row0.b), c.text);
            return;
        }
        if p.matches.is_empty() {
            g.text("沒有符合的命令", Font::Text, Rect::new(row0.l + 10.0, row0.t, row0.r - 10.0, row0.b), c.text_secondary);
            return;
        }
        for (i, row) in lay.rows.iter().enumerate() {
            let mi = p.top + i;
            let Some(&k) = p.matches.get(mi) else { break };
            let e = &p.entries[k];
            if mi == p.sel {
                g.fill_round(*row, self.theme.metrics.radius, c.selection);
            } else if self.hover == Hit::PaletteRow(mi) {
                g.fill_round(*row, self.theme.metrics.radius, c.hover);
            }
            let mut right = row.r - 10.0;
            if let Some(key) = &e.key {
                g.text(key, Font::SmallRight, Rect::new(row.l, row.t, right, row.b), c.text_secondary);
                right -= g.measure(key, Font::Small) + 16.0;
            }
            let title_r = Rect::new(row.l + 10.0, row.t, right, row.b);
            g.text(&e.title, Font::Text, title_r, c.text);
            // 標題不同於命令本身時，在後面以次要顏色列出命令
            if e.title != e.cmd {
                let x = title_r.l + g.measure(&e.title, Font::Text) + 12.0;
                if x + 40.0 < right {
                    g.text(&e.cmd, Font::Small, Rect::new(x, row.t, right, row.b), c.text_secondary);
                }
            }
        }
        // 超過一頁時，右側顯示位置
        if p.matches.len() > MAX_ROWS {
            let track = Rect::new(card.r - 5.0, lay.rows[0].t, card.r - 2.0, lay.rows[lay.rows.len() - 1].b);
            let h = track.h() * MAX_ROWS as f32 / p.matches.len() as f32;
            let y = track.t + track.h() * p.top as f32 / p.matches.len() as f32;
            g.fill_round(Rect::new(track.l, y, track.r, y + h), 1.5, c.text_secondary.with_alpha(110));
        }
    }
}
