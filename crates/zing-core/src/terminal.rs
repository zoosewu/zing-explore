//! 內建終端機的畫面（`Screen`）：解析 ConPTY 輸出的終端機控制碼（VT），維護格子、游標、捲動回看與替代畫面；
//! 以及按鍵轉成送給終端機的位元組（`key_bytes`）。不碰 Win32，可以單元測試（zing-app 的 terminal.rs 負責 ConPTY 與繪製）。
//!
//! 只實作 ConPTY 會送出的控制碼（游標移動、清除、捲動區域、顏色、替代畫面、視窗標題、游標位置查詢）；
//! 其他的忽略。字寬：東亞全形字、表情符號佔兩格，組合字元不佔格（與 ConPTY 的判斷大致相同）。

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    /// 16／256 色的索引
    Indexed(u8),
    Rgb(u8, u8, u8),
}

pub const BOLD: u8 = 1;
pub const UNDERLINE: u8 = 2;
pub const INVERSE: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: u8,
    /// 全形字的第二格（不畫字）
    pub cont: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: Color::Default, bg: Color::Default, attrs: 0, cont: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    /// ESC ( 等：選字集，略過下一個位元組
    Charset,
    Csi,
    Osc,
    /// OSC 中遇到 ESC（等 `\` 結束）
    OscEsc,
}

pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    grid: Vec<Vec<Cell>>,
    scrollback: VecDeque<Vec<Cell>>,
    max_scrollback: usize,
    /// 替代畫面（vim、less…）時保存的主畫面
    main: Option<Vec<Vec<Cell>>>,
    /// 游標（列, 欄）
    pub cursor: (usize, usize),
    saved: (usize, usize),
    pub cursor_visible: bool,
    /// 上下方向鍵送 `ESC O A`（應用程式游標鍵模式）
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub title: String,
    pen: Cell,
    /// 寫到最後一欄之後：下一個字先換行
    wrap_pending: bool,
    top: usize,
    bottom: usize,
    state: State,
    params: String,
    osc: String,
    utf8: Vec<u8>,
    /// 要回給程式的資料（游標位置查詢的回答）
    responses: Vec<u8>,
    /// 畫面改變了（繪製後清除）
    pub dirty: bool,
}

/// 字寬：0（組合字元）、1、2（東亞全形、表情符號）
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    match u {
        0x0300..=0x036F | 0x200B..=0x200F | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F => 0,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = (cols.max(2), rows.max(1));
        Screen {
            cols,
            rows,
            grid: vec![vec![Cell::default(); cols]; rows],
            scrollback: VecDeque::new(),
            max_scrollback: 5000,
            main: None,
            cursor: (0, 0),
            saved: (0, 0),
            cursor_visible: true,
            app_cursor: false,
            bracketed_paste: false,
            title: String::new(),
            pen: Cell::default(),
            wrap_pending: false,
            top: 0,
            bottom: rows - 1,
            state: State::Ground,
            params: String::new(),
            osc: String::new(),
            utf8: Vec::new(),
            responses: Vec::new(),
            dirty: true,
        }
    }

    /// 包含捲動回看的總列數；第 0 列是最舊的
    pub fn total_lines(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// 第 k 列（含捲動回看）
    pub fn line(&self, k: usize) -> &[Cell] {
        if k < self.scrollback.len() { &self.scrollback[k] } else { &self.grid[k - self.scrollback.len()] }
    }

    pub fn in_alt_screen(&self) -> bool {
        self.main.is_some()
    }

    /// 一列的文字（去掉結尾空白，全形字的第二格不算）
    pub fn line_text(cells: &[Cell]) -> String {
        let s: String = cells.iter().filter(|c| !c.cont).map(|c| c.ch).collect();
        s.trim_end().to_string()
    }

    /// 目前畫面的文字（測試用）
    pub fn text(&self) -> String {
        self.grid.iter().map(|l| Self::line_text(l)).collect::<Vec<_>>().join("\n").trim_end().to_string()
    }

    /// 取走要回給程式的資料
    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.responses)
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        for l in self.grid.iter_mut().chain(self.main.iter_mut().flatten()) {
            l.resize(cols, Cell::default());
        }
        // 變矮：上面的列移進捲動回看，游標仍在畫面中
        if rows < self.rows {
            let shift = (self.cursor.0 + 1).saturating_sub(rows);
            for l in self.grid.drain(..shift) {
                if self.main.is_none() {
                    self.scrollback.push_back(l);
                }
            }
            self.grid.truncate(rows);
            self.cursor.0 -= shift;
        }
        while self.grid.len() < rows {
            self.grid.push(vec![Cell::default(); cols]);
        }
        if let Some(m) = &mut self.main {
            m.resize(rows, vec![Cell::default(); cols]);
        }
        while self.scrollback.len() > self.max_scrollback {
            self.scrollback.pop_front();
        }
        self.cols = cols;
        self.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.cursor = (self.cursor.0.min(rows - 1), self.cursor.1.min(cols - 1));
        self.wrap_pending = false;
        self.dirty = true;
    }

    /// 送進終端機的輸出
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
        self.dirty = true;
    }

    fn byte(&mut self, b: u8) {
        match self.state {
            State::Ground => self.ground(b),
            State::Escape => self.escape(b),
            State::Charset => self.state = State::Ground,
            State::Csi => match b {
                0x20..=0x3F => self.params.push(b as char),
                0x40..=0x7E => {
                    self.state = State::Ground;
                    let params = std::mem::take(&mut self.params);
                    self.csi(&params, b as char);
                }
                0x1B => self.state = State::Escape,
                // 控制字元在 CSI 中照樣執行
                _ => self.control(b),
            },
            State::Osc => match b {
                0x07 => self.end_osc(),
                0x1B => self.state = State::OscEsc,
                _ => {
                    if self.osc.len() < 4096 {
                        self.osc.push(b as char)
                    }
                }
            },
            State::OscEsc => {
                self.end_osc();
                if b != b'\\' {
                    self.escape(b);
                }
            }
        }
    }

    fn end_osc(&mut self) {
        self.state = State::Ground;
        let osc = std::mem::take(&mut self.osc);
        // OSC 是位元組，標題可能是 UTF-8
        let bytes: Vec<u8> = osc.chars().map(|c| c as u32 as u8).collect();
        let text = String::from_utf8_lossy(&bytes);
        if let Some(t) = text.strip_prefix("0;").or_else(|| text.strip_prefix("2;")) {
            self.title = t.to_string();
        }
    }

    fn ground(&mut self, b: u8) {
        if !self.utf8.is_empty() || b >= 0x80 {
            return self.utf8_byte(b);
        }
        match b {
            0x1B => {
                self.state = State::Escape;
            }
            0x20..=0x7E => self.print(b as char),
            _ => self.control(b),
        }
    }

    fn utf8_byte(&mut self, b: u8) {
        if self.utf8.is_empty() {
            if b & 0xC0 == 0x80 || b >= 0xF8 {
                return self.print('\u{FFFD}');
            }
        } else if b & 0xC0 != 0x80 {
            // 不完整的序列
            self.utf8.clear();
            self.print('\u{FFFD}');
            return self.ground(b);
        }
        self.utf8.push(b);
        let need = match self.utf8[0] {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            _ => 4,
        };
        if self.utf8.len() == need {
            let c = std::str::from_utf8(&self.utf8).ok().and_then(|s| s.chars().next()).unwrap_or('\u{FFFD}');
            self.utf8.clear();
            self.print(c);
        }
    }

    fn control(&mut self, b: u8) {
        match b {
            0x08 => {
                self.cursor.1 = self.cursor.1.saturating_sub(1);
                self.wrap_pending = false;
            }
            0x09 => {
                self.cursor.1 = ((self.cursor.1 / 8 + 1) * 8).min(self.cols - 1);
                self.wrap_pending = false;
            }
            0x0A..=0x0C => self.linefeed(),
            0x0D => {
                self.cursor.1 = 0;
                self.wrap_pending = false;
            }
            _ => {}
        }
    }

    fn escape(&mut self, b: u8) {
        self.state = State::Ground;
        match b {
            b'[' => {
                self.params.clear();
                self.state = State::Csi;
            }
            b']' => {
                self.osc.clear();
                self.state = State::Osc;
            }
            b'(' | b')' | b'*' | b'+' => self.state = State::Charset,
            b'7' => self.saved = self.cursor,
            b'8' => self.restore_cursor(),
            b'D' => self.linefeed(),
            b'E' => {
                self.cursor.1 = 0;
                self.linefeed();
            }
            b'M' => self.reverse_index(),
            b'c' => {
                let (c, r) = (self.cols, self.rows);
                let title = std::mem::take(&mut self.title);
                *self = Screen::new(c, r);
                self.title = title;
            }
            _ => {}
        }
    }

    fn restore_cursor(&mut self) {
        self.cursor = (self.saved.0.min(self.rows - 1), self.saved.1.min(self.cols - 1));
        self.wrap_pending = false;
    }

    fn blank(&self) -> Cell {
        Cell { bg: self.pen.bg, ..Cell::default() }
    }

    fn print(&mut self, c: char) {
        let w = char_width(c);
        if w == 0 {
            return;
        }
        if self.wrap_pending || (w == 2 && self.cursor.1 + 1 >= self.cols) {
            self.cursor.1 = 0;
            self.linefeed();
            self.wrap_pending = false;
        }
        let (r, col) = self.cursor;
        // 蓋掉全形字的一半時，另一半清掉
        self.clear_wide_at(r, col);
        if w == 2 {
            self.clear_wide_at(r, col + 1);
        }
        self.grid[r][col] = Cell { ch: c, cont: false, ..self.pen };
        if w == 2 {
            self.grid[r][col + 1] = Cell { ch: ' ', cont: true, ..self.pen };
        }
        let next = col + w;
        if next >= self.cols {
            self.cursor.1 = self.cols - 1;
            self.wrap_pending = true;
        } else {
            self.cursor.1 = next;
        }
    }

    fn clear_wide_at(&mut self, r: usize, col: usize) {
        if col >= self.cols {
            return;
        }
        let blank = self.blank();
        if self.grid[r][col].cont && col > 0 {
            self.grid[r][col - 1] = blank;
        }
        if col + 1 < self.cols && self.grid[r][col + 1].cont {
            self.grid[r][col + 1] = blank;
        }
    }

    fn linefeed(&mut self) {
        self.wrap_pending = false;
        if self.cursor.0 == self.bottom {
            self.scroll_up(1);
        } else if self.cursor.0 + 1 < self.rows {
            self.cursor.0 += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.cursor.0 == self.top {
            self.scroll_down(1);
        } else {
            self.cursor.0 = self.cursor.0.saturating_sub(1);
        }
    }

    /// 捲動區域往上捲 `n` 列；整個畫面捲動時（不在替代畫面）移出的列進入捲動回看
    fn scroll_up(&mut self, n: usize) {
        let blank = vec![self.blank(); self.cols];
        for _ in 0..n.min(self.bottom - self.top + 1) {
            let line = self.grid.remove(self.top);
            if self.top == 0 && self.main.is_none() {
                self.scrollback.push_back(line);
                if self.scrollback.len() > self.max_scrollback {
                    self.scrollback.pop_front();
                }
            }
            self.grid.insert(self.bottom, blank.clone());
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let blank = vec![self.blank(); self.cols];
        for _ in 0..n.min(self.bottom - self.top + 1) {
            self.grid.remove(self.bottom);
            self.grid.insert(self.top, blank.clone());
        }
    }

    fn erase(&mut self, r: usize, from: usize, to: usize) {
        let blank = self.blank();
        for c in from.min(self.cols)..to.min(self.cols) {
            self.grid[r][c] = blank;
        }
    }

    fn csi(&mut self, raw: &str, f: char) {
        let private = raw.starts_with('?');
        let body = raw.trim_start_matches(['?', '>', '<', '=']);
        // 中間字元（例如 `ESC [ 2 SP q` 游標樣式）不處理
        if body.bytes().any(|b| (0x20..=0x2F).contains(&b)) {
            return;
        }
        let ps: Vec<usize> = body.split([';', ':']).map(|p| p.parse().unwrap_or(0)).collect();
        let p = |i: usize, d: usize| ps.get(i).copied().filter(|&v| v != 0).unwrap_or(d);
        let (row, col) = self.cursor;
        self.wrap_pending = false;
        match f {
            'A' => self.cursor.0 = row.saturating_sub(p(0, 1)).max(if row >= self.top { self.top } else { 0 }),
            'B' => self.cursor.0 = (row + p(0, 1)).min(if row <= self.bottom { self.bottom } else { self.rows - 1 }),
            'C' => self.cursor.1 = (col + p(0, 1)).min(self.cols - 1),
            'D' => self.cursor.1 = col.saturating_sub(p(0, 1)),
            'E' => self.cursor = ((row + p(0, 1)).min(self.rows - 1), 0),
            'F' => self.cursor = (row.saturating_sub(p(0, 1)), 0),
            'G' | '`' => self.cursor.1 = (p(0, 1) - 1).min(self.cols - 1),
            'd' => self.cursor.0 = (p(0, 1) - 1).min(self.rows - 1),
            'H' | 'f' => self.cursor = ((p(0, 1) - 1).min(self.rows - 1), (p(1, 1) - 1).min(self.cols - 1)),
            'J' => match ps.first().copied().unwrap_or(0) {
                0 => {
                    self.erase(row, col, self.cols);
                    for r in row + 1..self.rows {
                        self.erase(r, 0, self.cols);
                    }
                }
                1 => {
                    for r in 0..row {
                        self.erase(r, 0, self.cols);
                    }
                    self.erase(row, 0, col + 1);
                }
                2 => {
                    for r in 0..self.rows {
                        self.erase(r, 0, self.cols);
                    }
                }
                3 => self.scrollback.clear(),
                _ => {}
            },
            'K' => match ps.first().copied().unwrap_or(0) {
                0 => self.erase(row, col, self.cols),
                1 => self.erase(row, 0, col + 1),
                2 => self.erase(row, 0, self.cols),
                _ => {}
            },
            '@' => {
                let n = p(0, 1).min(self.cols - col);
                let blank = self.blank();
                let line = &mut self.grid[row];
                for _ in 0..n {
                    line.pop();
                    line.insert(col, blank);
                }
            }
            'P' => {
                let n = p(0, 1).min(self.cols - col);
                let blank = self.blank();
                let line = &mut self.grid[row];
                for _ in 0..n {
                    line.remove(col);
                    line.push(blank);
                }
            }
            'X' => self.erase(row, col, col + p(0, 1)),
            'L' | 'M' if (self.top..=self.bottom).contains(&row) => {
                let n = p(0, 1).min(self.bottom - row + 1);
                let blank = vec![self.blank(); self.cols];
                for _ in 0..n {
                    if f == 'L' {
                        self.grid.remove(self.bottom);
                        self.grid.insert(row, blank.clone());
                    } else {
                        self.grid.remove(row);
                        self.grid.insert(self.bottom, blank.clone());
                    }
                }
                self.cursor.1 = 0;
            }
            'S' => self.scroll_up(p(0, 1)),
            'T' if !private => self.scroll_down(p(0, 1)),
            'm' => self.sgr(&ps),
            'r' if !private => {
                let (t, b) = (p(0, 1) - 1, p(1, self.rows) - 1);
                if t < b && b < self.rows {
                    self.top = t;
                    self.bottom = b;
                    self.cursor = (0, 0);
                }
            }
            's' if !private => self.saved = self.cursor,
            'u' if !private => self.restore_cursor(),
            'h' | 'l' if private => {
                let on = f == 'h';
                for &m in &ps {
                    match m {
                        1 => self.app_cursor = on,
                        25 => self.cursor_visible = on,
                        47 | 1047 | 1049 => self.alt_screen(on),
                        2004 => self.bracketed_paste = on,
                        _ => {}
                    }
                }
            }
            'n' if !private => match ps.first().copied().unwrap_or(0) {
                5 => self.responses.extend_from_slice(b"\x1b[0n"),
                6 => self.responses.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes()),
                _ => {}
            },
            'c' if !raw.starts_with('>') => self.responses.extend_from_slice(b"\x1b[?1;0c"),
            _ => {}
        }
    }

    fn alt_screen(&mut self, on: bool) {
        match (on, self.main.is_some()) {
            (true, false) => {
                self.saved = self.cursor;
                let blank = vec![vec![Cell::default(); self.cols]; self.rows];
                self.main = Some(std::mem::replace(&mut self.grid, blank));
            }
            (false, true) => {
                if let Some(m) = self.main.take() {
                    self.grid = m;
                }
                self.restore_cursor();
            }
            _ => {}
        }
        self.top = 0;
        self.bottom = self.rows - 1;
    }

    fn sgr(&mut self, ps: &[usize]) {
        let mut i = 0;
        if ps.is_empty() {
            self.pen = Cell::default();
        }
        while i < ps.len() {
            match ps[i] {
                0 => self.pen = Cell::default(),
                1 => self.pen.attrs |= BOLD,
                22 => self.pen.attrs &= !BOLD,
                4 => self.pen.attrs |= UNDERLINE,
                24 => self.pen.attrs &= !UNDERLINE,
                7 => self.pen.attrs |= INVERSE,
                27 => self.pen.attrs &= !INVERSE,
                n @ 30..=37 => self.pen.fg = Color::Indexed((n - 30) as u8),
                n @ 90..=97 => self.pen.fg = Color::Indexed((n - 90 + 8) as u8),
                39 => self.pen.fg = Color::Default,
                n @ 40..=47 => self.pen.bg = Color::Indexed((n - 40) as u8),
                n @ 100..=107 => self.pen.bg = Color::Indexed((n - 100 + 8) as u8),
                49 => self.pen.bg = Color::Default,
                n @ (38 | 48) => {
                    let color = match ps.get(i + 1) {
                        Some(5) => {
                            i += 2;
                            ps.get(i).map(|&v| Color::Indexed(v as u8))
                        }
                        Some(2) => {
                            i += 4;
                            match (ps.get(i - 2), ps.get(i - 1), ps.get(i)) {
                                (Some(&r), Some(&g), Some(&b)) => Some(Color::Rgb(r as u8, g as u8, b as u8)),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    if let Some(c) = color {
                        if n == 38 {
                            self.pen.fg = c;
                        } else {
                            self.pen.bg = c;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// 16 色的預設調色盤（Windows Terminal 的 Campbell）
pub const PALETTE16: [(u8, u8, u8); 16] = [
    (12, 12, 12),
    (197, 15, 31),
    (19, 161, 14),
    (193, 156, 0),
    (0, 55, 218),
    (136, 23, 152),
    (58, 150, 221),
    (204, 204, 204),
    (118, 118, 118),
    (231, 72, 86),
    (22, 198, 12),
    (249, 241, 165),
    (59, 120, 255),
    (180, 0, 158),
    (97, 214, 214),
    (242, 242, 242),
];

/// 索引色 → RGB（0–15 用調色盤、16–231 是 6×6×6 色塊、232–255 是灰階）
pub fn indexed_rgb(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => PALETTE16[i as usize],
        16..=231 => {
            let i = i - 16;
            let v = |x: u8| if x == 0 { 0 } else { 55 + x * 40 };
            (v(i / 36), v(i / 6 % 6), v(i % 6))
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            (g, g, g)
        }
    }
}

/// 不產生字元的按鍵（方向鍵、功能鍵…）送給終端機的位元組；`vk` 是 Windows 的虛擬鍵碼。
/// 一般字元由 WM_CHAR 送（Ctrl+字母也是，已是控制字元）
pub fn key_bytes(vk: u16, ctrl: bool, shift: bool, alt: bool, app_cursor: bool) -> Option<Vec<u8>> {
    let m = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let csi = |final_: char| -> Vec<u8> {
        if m > 1 { format!("\x1b[1;{m}{final_}").into_bytes() } else if app_cursor { format!("\x1bO{final_}").into_bytes() } else { format!("\x1b[{final_}").into_bytes() }
    };
    let tilde = |n: u8| -> Vec<u8> { if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() } };
    let ss3 = |final_: char| -> Vec<u8> { if m > 1 { format!("\x1b[1;{m}{final_}").into_bytes() } else { format!("\x1bO{final_}").into_bytes() } };
    Some(match vk {
        0x26 => csi('A'),
        0x28 => csi('B'),
        0x27 => csi('C'),
        0x25 => csi('D'),
        0x24 => csi('H'),
        0x23 => csi('F'),
        0x2D => tilde(2),
        0x2E => tilde(3),
        0x21 => tilde(5),
        0x22 => tilde(6),
        0x70 => ss3('P'),
        0x71 => ss3('Q'),
        0x72 => ss3('R'),
        0x73 => ss3('S'),
        0x74 => tilde(15),
        0x75 => tilde(17),
        0x76 => tilde(18),
        0x77 => tilde(19),
        0x78 => tilde(20),
        0x79 => tilde(21),
        0x7A => tilde(23),
        0x7B => tilde(24),
        // Enter、Backspace、Tab、Esc 也在這裡處理（WM_CHAR 的值會因修飾鍵不同）
        0x0D => b"\r".to_vec(),
        0x08 if ctrl => vec![0x08],
        0x08 => vec![0x7F],
        0x09 if shift => b"\x1b[Z".to_vec(),
        0x09 => b"\t".to_vec(),
        0x1B => vec![0x1B],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(s: &mut Screen, t: &str) {
        s.feed(t.as_bytes());
    }

    #[test]
    fn prints_wraps_and_scrolls_into_scrollback() {
        let mut s = Screen::new(5, 2);
        feed(&mut s, "abcdefg\r\nxy\r\nz");
        assert_eq!(s.text(), "xy\nz");
        assert_eq!(s.scrollback_len(), 2);
        assert_eq!(Screen::line_text(s.line(0)), "abcde");
        assert_eq!(Screen::line_text(s.line(1)), "fg");
        assert_eq!(s.cursor, (1, 1));
    }

    #[test]
    fn cursor_moves_and_erases() {
        let mut s = Screen::new(10, 3);
        feed(&mut s, "hello\x1b[2;3Hxy\x1b[1;2H\x1b[K");
        assert_eq!(s.text(), "h\n  xy");
        feed(&mut s, "\x1b[2J\x1b[H");
        assert_eq!(s.text(), "");
        assert_eq!(s.cursor, (0, 0));
        feed(&mut s, "abcdef\x1b[1;3H\x1b[2P");
        assert_eq!(s.text(), "abef");
        feed(&mut s, "\x1b[1;2H\x1b[2@");
        assert_eq!(s.text(), "a  bef");
    }

    #[test]
    fn colors_and_attributes() {
        let mut s = Screen::new(10, 1);
        feed(&mut s, "\x1b[1;31ma\x1b[38;5;200mb\x1b[38;2;1;2;3;48;5;4mc\x1b[0md");
        let l = s.line(0);
        assert_eq!((l[0].fg, l[0].attrs), (Color::Indexed(1), BOLD));
        assert_eq!(l[1].fg, Color::Indexed(200));
        assert_eq!((l[2].fg, l[2].bg), (Color::Rgb(1, 2, 3), Color::Indexed(4)));
        assert_eq!((l[3].fg, l[3].attrs), (Color::Default, 0));
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let mut s = Screen::new(5, 2);
        feed(&mut s, "a中文");
        assert_eq!(s.text(), "a中文".to_string());
        assert!(s.line(0)[2].cont);
        // 剩一格放不下全形字：換到下一列
        let mut s = Screen::new(4, 2);
        feed(&mut s, "abc中");
        assert_eq!(s.text(), "abc\n中");
        assert_eq!(s.cursor, (1, 2));
    }

    #[test]
    fn utf8_split_across_chunks() {
        let mut s = Screen::new(10, 1);
        let bytes = "中".as_bytes();
        s.feed(&bytes[..1]);
        s.feed(&bytes[1..]);
        assert_eq!(s.text(), "中");
        s.feed(&[0xE4, b'x']);
        assert_eq!(s.text(), "中\u{FFFD}x");
    }

    #[test]
    fn alternate_screen_keeps_the_main_one() {
        let mut s = Screen::new(10, 2);
        feed(&mut s, "shell$ \x1b[?1049h\x1b[Hvim");
        assert!(s.in_alt_screen());
        assert_eq!(s.text(), "vim");
        feed(&mut s, "\x1b[?1049l");
        assert_eq!(s.text(), "shell$");
        assert_eq!(s.cursor, (0, 7));
    }

    #[test]
    fn scroll_region_and_reverse_index() {
        let mut s = Screen::new(5, 4);
        feed(&mut s, "1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(s.text(), "1\n3\n\n4");
        assert_eq!(s.scrollback_len(), 0, "捲動區域內的捲動不進入捲動回看");
        feed(&mut s, "\x1b[2;1H\x1bM");
        assert_eq!(s.text(), "1\n\n3\n4");
    }

    #[test]
    fn title_and_cursor_report() {
        let mut s = Screen::new(10, 3);
        feed(&mut s, "\x1b]0;C:\\Windows\x07ab\x1b]2;標題\x1b\\");
        assert_eq!(s.title, "標題");
        feed(&mut s, "\x1b[6n");
        assert_eq!(s.take_responses(), b"\x1b[1;3R");
        assert!(s.take_responses().is_empty());
    }

    #[test]
    fn resize_keeps_the_cursor_line() {
        let mut s = Screen::new(5, 4);
        feed(&mut s, "1\r\n2\r\n3\r\n4");
        s.resize(3, 2);
        assert_eq!(s.text(), "3\n4");
        assert_eq!(s.scrollback_len(), 2);
        assert_eq!(s.cursor, (1, 1));
        s.resize(6, 3);
        assert_eq!(s.rows, 3);
        assert_eq!(s.line(s.total_lines() - 1).len(), 6);
    }

    #[test]
    fn keys() {
        assert_eq!(key_bytes(0x26, false, false, false, false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes(0x26, false, false, false, true).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes(0x25, true, false, false, false).unwrap(), b"\x1b[1;5D");
        assert_eq!(key_bytes(0x2E, false, false, false, false).unwrap(), b"\x1b[3~");
        assert_eq!(key_bytes(0x74, false, true, false, false).unwrap(), b"\x1b[15;2~");
        assert_eq!(key_bytes(0x08, false, false, false, false).unwrap(), [0x7F]);
        assert_eq!(key_bytes(0x41, false, false, false, false), None);
        assert_eq!(indexed_rgb(196), (255, 0, 0));
        assert_eq!(indexed_rgb(232), (8, 8, 8));
    }
}
