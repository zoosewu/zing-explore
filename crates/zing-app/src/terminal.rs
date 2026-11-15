//! 內建終端機窗格（`terminal.toggle`，預設 Ctrl+`）：視窗底部、窗格下方，可拖曳調整高度。
//!
//! - 殼層以 Windows 的虛擬主控台（ConPTY）執行；輸出的控制碼由 `zing_core::terminal::Screen` 解析（UI 執行緒，
//!   很快），畫面跟著主題、用等寬字型自己畫。
//! - UI 執行緒不等待：啟動（CreatePseudoConsole、CreateProcess）與關閉（ClosePseudoConsole）在背景執行緒；
//!   讀取輸出一條執行緒（經 channel 交給 UI，佇列從空變成有資料時才送一個 `Msg::TerminalOutput`），寫入一條執行緒。
//! - 開啟時位於目前的資料夾；換資料夾不自動 cd（會打斷執行中的程式），`terminal.cd` 手動切換。
//! - 殼層結束（輸入 exit）時窗格自動關閉。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW, UpdateProcThreadAttribute,
};
use windows::core::{PCWSTR, PWSTR};
use zing_core::terminal::{BOLD, Cell, Color as TermColor, INVERSE, Screen, UNDERLINE, indexed_rgb, key_bytes};
use zing_core::text::wide_z;
use zing_core::theme::Color;

use crate::app::{App, Focus};
use crate::gfx::{Font, Rect};
use crate::input::Mods;
use crate::services::{Msg, Notifier};

/// 預設高度（DIP）
pub const DEFAULT_HEIGHT: f32 = 240.0;
const PAD_X: f32 = 6.0;
const PAD_Y: f32 = 4.0;

/// 殼層的種類（`cd` 的寫法不同）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    PowerShell,
    Cmd,
}

/// 啟動好的虛擬主控台：關閉時（Drop）結束殼層
struct Pty {
    hpc: HPCON,
    process: HANDLE,
    input: Sender<Vec<u8>>,
}

// SAFETY: HPCON 與行程 HANDLE 是核心物件的代號，可以在任何執行緒使用與關閉
unsafe impl Send for Pty {}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe {
            ClosePseudoConsole(self.hpc);
            let _ = CloseHandle(self.process);
        }
    }
}

/// 背景執行緒啟動好之後交給 UI
pub struct Started {
    pty: Pty,
    output: Receiver<Vec<u8>>,
    pending: Arc<AtomicBool>,
    kind: ShellKind,
}

/// 文字位置：（第幾列（含捲動回看）, 第幾欄）
pub type Pos = (usize, usize);

pub struct Terminal {
    pub screen: Screen,
    /// 往上捲回看了幾列（0 是最新）
    pub scroll: usize,
    pub height: f32,
    /// 選取的範圍（拖曳的起點, 終點）
    pub selection: Option<(Pos, Pos)>,
    pty: Option<Pty>,
    output: Option<Receiver<Vec<u8>>>,
    pending: Arc<AtomicBool>,
    pub kind: ShellKind,
    /// 第幾次啟動（丟棄上一個殼層晚到的訊息）
    generation: u64,
    /// WM_CHAR 送來的 UTF-16 高位代理字元，等低位
    high: Option<u16>,
    pub error: Option<String>,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // ClosePseudoConsole 會等主控台結束，不在 UI 執行緒等
        if let Some(pty) = self.pty.take() {
            let _ = std::thread::Builder::new().name("zing-pty-close".into()).spawn(move || drop(pty));
        }
    }
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 找 PATH 中的程式（背景執行緒）
fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
}

/// 設定 `terminal.shell` → 命令列與種類（背景執行緒，`auto` 要找 pwsh）
fn shell_command(setting: &str) -> (String, ShellKind) {
    zing_core::guard::io("尋找殼層");
    match setting {
        "cmd" => ("cmd.exe".into(), ShellKind::Cmd),
        "powershell" => ("powershell.exe -NoLogo".into(), ShellKind::PowerShell),
        "pwsh" => ("pwsh.exe -NoLogo".into(), ShellKind::PowerShell),
        _ if on_path("pwsh.exe") => ("pwsh.exe -NoLogo".into(), ShellKind::PowerShell),
        _ => ("powershell.exe -NoLogo".into(), ShellKind::PowerShell),
    }
}

/// 建立虛擬主控台並執行殼層；回傳它與讀取輸出的管道
unsafe fn open(cmd: &str, dir: &str, cols: i16, rows: i16) -> windows::core::Result<(Pty, HANDLE)> {
    unsafe {
        let (mut in_read, mut in_write, mut out_read, mut out_write) = (HANDLE::default(), HANDLE::default(), HANDLE::default(), HANDLE::default());
        CreatePipe(&mut in_read, &mut in_write, None, 0)?;
        CreatePipe(&mut out_read, &mut out_write, None, 0)?;
        let hpc = CreatePseudoConsole(COORD { X: cols, Y: rows }, in_read, out_write, 0);
        // 主控台已複製了自己那一端
        let _ = CloseHandle(in_read);
        let _ = CloseHandle(out_write);
        let hpc = match hpc {
            Ok(h) => h,
            Err(e) => {
                let _ = CloseHandle(in_write);
                let _ = CloseHandle(out_read);
                return Err(e);
            }
        };
        let mut size = 0usize;
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
        let mut buf = vec![0u8; size];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(buf.as_mut_ptr() as _);
        let started = (|| {
            InitializeProcThreadAttributeList(Some(list), 1, None, &mut size)?;
            UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize, Some(hpc.0 as *const _), size_of::<HPCON>(), None, None)?;
            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = list;
            let mut pi = PROCESS_INFORMATION::default();
            let mut cmd_w = wide_z(cmd);
            let dir_w = wide_z(dir);
            let dir_p = if dir.is_empty() { PCWSTR::null() } else { PCWSTR(dir_w.as_ptr()) };
            let r = CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(cmd_w.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                None,
                dir_p,
                &si.StartupInfo,
                &mut pi,
            );
            DeleteProcThreadAttributeList(list);
            r?;
            let _ = CloseHandle(pi.hThread);
            Ok(pi.hProcess)
        })();
        let process = match started {
            Ok(p) => p,
            Err(e) => {
                ClosePseudoConsole(hpc);
                let _ = CloseHandle(in_write);
                let _ = CloseHandle(out_read);
                return Err(e);
            }
        };
        // 寫入一條執行緒：管道滿了也不會卡住 UI
        let (tx, rx) = channel::<Vec<u8>>();
        let input = in_write.0 as usize;
        let _ = std::thread::Builder::new().name("zing-pty-in".into()).spawn(move || {
            let h = HANDLE(input as _);
            for bytes in rx {
                if WriteFile(h, Some(&bytes), None, None).is_err() {
                    break;
                }
            }
            let _ = CloseHandle(h);
        });
        Ok((Pty { hpc, process, input: tx }, out_read))
    }
}

/// 在背景啟動；好了送 `Msg::TerminalReady`，之後讀取的輸出以 `Msg::TerminalOutput` 通知
fn spawn(setting: String, dir: String, cols: i16, rows: i16, n: Notifier, generation: u64) {
    let _ = std::thread::Builder::new().name("zing-pty-open".into()).spawn(move || {
        let (cmd, kind) = shell_command(&setting);
        // 開在不存在的資料夾（壓縮檔裡、此電腦）時改用使用者資料夾
        let dir = if !dir.is_empty() && std::path::Path::new(&dir).is_dir() { dir } else { std::env::var("USERPROFILE").unwrap_or_default() };
        match unsafe { open(&cmd, &dir, cols, rows) } {
            Ok((pty, out)) => {
                let (tx, rx) = channel::<Vec<u8>>();
                let pending = Arc::new(AtomicBool::new(false));
                n.send(Msg::TerminalReady { generation, started: Box::new(Started { pty, output: rx, pending: pending.clone(), kind }) });
                let out = out.0 as usize;
                let mut buf = vec![0u8; 1 << 16];
                loop {
                    let mut read = 0u32;
                    let ok = unsafe { ReadFile(HANDLE(out as _), Some(&mut buf), Some(&mut read), None) }.is_ok();
                    if !ok || read == 0 {
                        break;
                    }
                    if tx.send(buf[..read as usize].to_vec()).is_err() {
                        break;
                    }
                    if !pending.swap(true, Ordering::AcqRel) {
                        n.send(Msg::TerminalOutput { generation });
                    }
                }
                unsafe {
                    let _ = CloseHandle(HANDLE(out as _));
                }
                n.send(Msg::TerminalExited { generation });
            }
            Err(e) => n.send(Msg::TerminalFailed { generation, error: e.message() }),
        }
    });
}

fn rgb(c: (u8, u8, u8)) -> Color {
    Color { r: c.0, g: c.1, b: c.2, a: 255 }
}

impl Terminal {
    fn send(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.scroll = 0;
        self.selection = None;
        if let Some(p) = &self.pty {
            let _ = p.input.send(bytes);
        }
    }

    /// 選取的文字（列與列之間換行，每列去掉結尾空白）
    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        if s == e {
            return None;
        }
        let mut out = Vec::new();
        for line in s.0..=e.0.min(self.screen.total_lines().saturating_sub(1)) {
            let cells = self.screen.line(line);
            let from = if line == s.0 { s.1 } else { 0 };
            let to = if line == e.0 { e.1 } else { cells.len() };
            out.push(Screen::line_text(&cells[from.min(cells.len())..to.min(cells.len())]));
        }
        Some(out.join("\r\n"))
    }

    fn in_selection(&self, line: usize, col: usize) -> bool {
        let Some((a, b)) = self.selection else { return false };
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        (line, col) >= s && (line, col) < e
    }
}

impl App {
    /// 窗格的高度（開著時）
    pub fn terminal_height(&self, avail: f32) -> Option<f32> {
        self.terminal.as_ref().map(|t| t.height.clamp(80.0, (avail * 0.75).max(80.0)))
    }

    /// `terminal.toggle`：關著就開（位於目前的資料夾）；開著但焦點不在它就移過去；焦點在它就關閉
    pub fn toggle_terminal(&mut self) {
        match (&self.terminal, self.focus == Focus::Terminal) {
            (Some(_), true) => self.close_terminal(),
            (Some(_), false) => self.focus = Focus::Terminal,
            (None, _) => self.open_terminal(),
        }
        self.layout = self.compute_layout();
        self.invalidate();
    }

    fn open_terminal(&mut self) {
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        let tab = self.tab();
        let dir = if zing_core::archive::is_inside(&tab.path) { String::new() } else { tab.path.clone() };
        self.terminal = Some(Terminal {
            screen: Screen::new(80, 24),
            scroll: 0,
            height: self.terminal_last_height,
            selection: None,
            pty: None,
            output: None,
            pending: Arc::new(AtomicBool::new(false)),
            kind: ShellKind::PowerShell,
            generation,
            high: None,
            error: None,
        });
        self.focus = Focus::Terminal;
        self.layout = self.compute_layout();
        let (cols, rows) = self.terminal_grid().unwrap_or((80, 24));
        if let Some(t) = &mut self.terminal {
            t.screen.resize(cols, rows);
        }
        spawn(self.config.terminal.shell.clone(), dir, cols as i16, rows as i16, self.notifier.clone(), generation);
    }

    pub fn close_terminal(&mut self) {
        if let Some(t) = self.terminal.take() {
            self.terminal_last_height = t.height;
        }
        if self.focus == Focus::Terminal {
            self.focus = Focus::List;
        }
        self.layout = self.compute_layout();
        self.invalidate();
    }

    pub fn on_terminal_ready(&mut self, generation: u64, started: Started) {
        let Some(t) = self.terminal.as_mut().filter(|t| t.generation == generation) else { return };
        let Started { pty, output, pending, kind } = started;
        // 開啟到啟動好之間窗格的大小可能變了
        let (cols, rows) = (t.screen.cols as i16, t.screen.rows as i16);
        unsafe {
            let _ = ResizePseudoConsole(pty.hpc, COORD { X: cols, Y: rows });
        }
        t.pty = Some(pty);
        t.output = Some(output);
        t.pending = pending;
        t.kind = kind;
    }

    pub fn on_terminal_output(&mut self, generation: u64) {
        let n = self.notifier.clone();
        let Some(t) = self.terminal.as_mut().filter(|t| t.generation == generation) else { return };
        let Some(rx) = &t.output else { return };
        t.pending.store(false, Ordering::Release);
        // 一次最多處理 256 KB，其他的下一個訊息再處理（大量輸出時 UI 照樣回應）
        let mut budget = 256 * 1024usize;
        let mut more = false;
        while let Ok(chunk) = rx.try_recv() {
            t.screen.feed(&chunk);
            budget = budget.saturating_sub(chunk.len());
            if budget == 0 {
                more = true;
                break;
            }
        }
        let reply = t.screen.take_responses();
        if !reply.is_empty()
            && let Some(p) = &t.pty
        {
            let _ = p.input.send(reply);
        }
        if more && !t.pending.swap(true, Ordering::AcqRel) {
            n.send(Msg::TerminalOutput { generation });
        }
        self.invalidate();
    }

    pub fn on_terminal_exited(&mut self, generation: u64) {
        if self.terminal.as_ref().is_some_and(|t| t.generation == generation) {
            self.close_terminal();
        }
    }

    pub fn on_terminal_failed(&mut self, generation: u64, error: String) {
        if let Some(t) = self.terminal.as_mut().filter(|t| t.generation == generation) {
            t.error = Some(error.clone());
            self.set_status(tr!("無法啟動終端機：{error}", error), true);
        }
    }

    /// `terminal.cd`：殼層切到目前的資料夾
    pub fn terminal_cd(&mut self) {
        let path = self.tab().path.clone();
        if path.is_empty() || zing_core::archive::is_inside(&path) {
            self.set_status(tr!("這個位置不是資料夾，終端機不能切換過去"), true);
            return;
        }
        if self.terminal.is_none() {
            return self.toggle_terminal();
        }
        let Some(t) = &mut self.terminal else { return };
        let line = match t.kind {
            ShellKind::Cmd => format!("cd /d \"{path}\"\r"),
            ShellKind::PowerShell => format!("Set-Location -LiteralPath '{}'\r", path.replace('\'', "''")),
        };
        t.send(line.into_bytes());
        self.focus = Focus::Terminal;
        self.invalidate();
    }

    /// 送文字給殼層（貼上、冒煙測試）；開著括號貼上模式時包起來，換行一律送 CR
    pub fn terminal_send_text(&mut self, text: &str) {
        let Some(t) = &mut self.terminal else { return };
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if t.screen.bracketed_paste { format!("\x1b[200~{text}\x1b[201~") } else { text };
        t.send(bytes.into_bytes());
    }

    /// `terminal.copy`：複製選取的文字（交給 ShellHost）
    pub fn terminal_copy(&mut self) -> bool {
        let Some(text) = self.terminal.as_ref().and_then(Terminal::selected_text) else { return false };
        self.shell_host.submit(crate::shellhost::ShellJob::SetText { text, status: tr!("已複製").into(), reply: None });
        if let Some(t) = &mut self.terminal {
            t.selection = None;
        }
        self.invalidate();
        true
    }

    /// `terminal.paste`：讀剪貼簿的文字（ShellHost）後以 `Msg::TerminalPaste` 送回
    pub fn terminal_paste(&mut self) {
        if self.terminal.is_some() {
            self.shell_host.submit(crate::shellhost::ShellJob::PasteText);
        }
    }

    /// 鍵盤焦點在終端機時的按鍵：回傳 true 表示處理了（不再產生 WM_CHAR）
    pub fn terminal_key(&mut self, vk: u16, mods: Mods) -> bool {
        let Some(t) = &mut self.terminal else { return false };
        // 有選取時 Ctrl+C 是複製（與 Windows Terminal 相同），否則送 ^C 給程式；Ctrl+Shift+C／V 一律是複製、貼上
        if mods.ctrl && vk == b'C' as u16 && (mods.shift || t.selection.is_some_and(|(a, b)| a != b)) {
            self.terminal_copy();
            return true;
        }
        if mods.ctrl && vk == b'V' as u16 {
            self.terminal_paste();
            return true;
        }
        // Shift+PageUp／PageDown：捲動回看
        if mods.shift && !mods.ctrl && (vk == 0x21 || vk == 0x22) {
            let page = t.screen.rows.max(2) - 1;
            t.scroll = if vk == 0x21 { (t.scroll + page).min(t.screen.scrollback_len()) } else { t.scroll.saturating_sub(page) };
            self.invalidate();
            return true;
        }
        match key_bytes(vk, mods.ctrl, mods.shift, mods.alt, t.screen.app_cursor) {
            Some(b) => {
                t.send(b);
                true
            }
            None => false,
        }
    }

    /// WM_CHAR（焦點在終端機）：字元與 Ctrl+字母的控制字元
    pub fn terminal_char(&mut self, ch: u16) {
        let Some(t) = &mut self.terminal else { return };
        if (0xD800..0xDC00).contains(&ch) {
            t.high = Some(ch);
            return;
        }
        let units: Vec<u16> = match t.high.take() {
            Some(h) if (0xDC00..0xE000).contains(&ch) => vec![h, ch],
            _ => vec![ch],
        };
        let s = String::from_utf16_lossy(&units);
        t.send(s.into_bytes());
    }

    /// 窗格的範圍能放幾欄、幾列
    pub fn terminal_grid(&self) -> Option<(usize, usize)> {
        self.terminal_grid_in(self.layout.terminal?)
    }

    fn terminal_grid_in(&self, r: Rect) -> Option<(usize, usize)> {
        let cw = self.gfx.measure("M", Font::Mono).max(1.0);
        let lh = self.terminal_line_h();
        Some((((r.w() - PAD_X * 2.0) / cw).floor().max(2.0) as usize, ((r.h() - PAD_Y * 2.0) / lh).floor().max(1.0) as usize))
    }

    fn terminal_line_h(&self) -> f32 {
        (self.row_h() - 2.0).max(12.0)
    }

    /// 繪製前（`r` 是這一幀的位置）：窗格大小改變就調整畫面與虛擬主控台
    pub fn sync_terminal_size(&mut self, r: Rect) {
        let Some((cols, rows)) = self.terminal_grid_in(r) else { return };
        let Some(t) = &mut self.terminal else { return };
        if (cols, rows) == (t.screen.cols, t.screen.rows) {
            return;
        }
        t.screen.resize(cols, rows);
        t.scroll = t.scroll.min(t.screen.scrollback_len());
        if let Some(p) = &t.pty {
            unsafe {
                let _ = ResizePseudoConsole(p.hpc, COORD { X: cols as i16, Y: rows as i16 });
            }
        }
    }

    /// 窗格中的點 → 文字位置（含捲動回看的列）
    pub fn terminal_pos(&self, x: f32, y: f32) -> Option<Pos> {
        let (r, t) = (self.layout.terminal?, self.terminal.as_ref()?);
        let cw = self.gfx.measure("M", Font::Mono).max(1.0);
        let row = ((y - r.t - PAD_Y) / self.terminal_line_h()).floor().clamp(0.0, (t.screen.rows - 1) as f32) as usize;
        let col = ((x - r.l - PAD_X) / cw).round().clamp(0.0, t.screen.cols as f32) as usize;
        let first = t.screen.total_lines() - t.screen.rows - t.scroll;
        Some((first + row, col))
    }

    /// 右鍵選單：複製、貼上、切換到目前的資料夾、關閉（項目都是命令）
    pub fn terminal_menu(&mut self, x: f32, y: f32) {
        self.focus = Focus::Terminal;
        let items = ["terminal.copy", "terminal.paste", "terminal.cd", "terminal.toggle"].iter().map(|c| self.cmd_menu_item(c)).collect();
        self.open_menu_at(items, x, y);
    }

    pub fn terminal_wheel(&mut self, delta: i16) {
        let Some(t) = &mut self.terminal else { return };
        let lines = 3 * (delta as i32 / 120).unsigned_abs() as usize;
        t.scroll = if delta > 0 { (t.scroll + lines).min(t.screen.scrollback_len()) } else { t.scroll.saturating_sub(lines) };
        self.invalidate();
    }

    // ------------------------------------------------------------ 繪製

    pub fn paint_terminal(&self, r: Rect) {
        let Some(t) = &self.terminal else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        g.fill(r, c.background);
        g.hline(r.l, r.r, r.t, c.border);
        let cw = g.measure("M", Font::Mono).max(1.0);
        let lh = self.terminal_line_h();
        let (x0, y0) = (r.l + PAD_X, r.t + PAD_Y);
        let fg_of = |cell: &Cell| -> Color {
            match cell.fg {
                TermColor::Default => c.text,
                // 粗體的前 8 色改用亮色（傳統終端機的做法）
                TermColor::Indexed(i) if i < 8 && cell.attrs & BOLD != 0 => rgb(indexed_rgb(i + 8)),
                TermColor::Indexed(i) => rgb(indexed_rgb(i)),
                TermColor::Rgb(r, gg, b) => rgb((r, gg, b)),
            }
        };
        let bg_of = |cell: &Cell| -> Option<Color> {
            match cell.bg {
                TermColor::Default => None,
                TermColor::Indexed(i) => Some(rgb(indexed_rgb(i))),
                TermColor::Rgb(r, gg, b) => Some(rgb((r, gg, b))),
            }
        };
        g.push_clip(r);
        let first = t.screen.total_lines() - t.screen.rows - t.scroll;
        for row in 0..t.screen.rows {
            let line_no = first + row;
            let cells = t.screen.line(line_no);
            let y = y0 + row as f32 * lh;
            let mut col = 0;
            while col < cells.len() {
                let cell = cells[col];
                let selected = t.in_selection(line_no, col);
                let (mut fg, mut bg) = (fg_of(&cell), bg_of(&cell));
                if cell.attrs & INVERSE != 0 {
                    (fg, bg) = (bg.unwrap_or(c.background), Some(fg));
                }
                if selected {
                    bg = Some(c.selection);
                    fg = c.selection_text;
                }
                // 同樣式、半形的一段一次畫；全形字一個一個放進兩格
                let wide = col + 1 < cells.len() && cells[col + 1].cont;
                let mut end = col + if wide { 2 } else { 1 };
                if !wide {
                    while end < cells.len() {
                        let n = cells[end];
                        let same = n.fg == cell.fg && n.bg == cell.bg && n.attrs == cell.attrs && !n.cont && t.in_selection(line_no, end) == selected;
                        if !same || (end + 1 < cells.len() && cells[end + 1].cont) {
                            break;
                        }
                        end += 1;
                    }
                }
                let cr = Rect::new(x0 + col as f32 * cw, y, x0 + end as f32 * cw, y + lh);
                if let Some(b) = bg {
                    g.fill(cr, b);
                }
                let text: String = cells[col..end].iter().filter(|c| !c.cont).map(|c| c.ch).collect();
                if !text.trim().is_empty() {
                    // 全形字依兩格寬置中，半形的一段給足空間（字型的字寬可能略大於量到的格寬）
                    let tr = if wide {
                        let w = g.measure(&text, Font::Mono);
                        Rect::new(cr.l + ((cr.w() - w) / 2.0).max(0.0), cr.t, cr.r + cw, cr.b)
                    } else {
                        Rect::new(cr.l, cr.t, cr.r + cw, cr.b)
                    };
                    g.text(&text, Font::Mono, tr, fg);
                }
                if cell.attrs & UNDERLINE != 0 {
                    g.hline(cr.l, cr.r, cr.b - 2.0, fg);
                }
                col = end;
            }
        }
        // 游標：焦點在終端機時是實心方塊，否則是外框
        if t.scroll == 0 && t.screen.cursor_visible {
            let (cy, cx) = t.screen.cursor;
            let w = if t.screen.line(first + cy).get(cx + 1).is_some_and(|n| n.cont) { 2.0 } else { 1.0 };
            let cur = Rect::new(x0 + cx as f32 * cw, y0 + cy as f32 * lh, x0 + (cx as f32 + w) * cw, y0 + (cy + 1) as f32 * lh);
            if self.focus == Focus::Terminal {
                g.fill(cur, c.text.with_alpha(160));
            } else {
                g.stroke_round(cur, 0.0, c.text_secondary, 1.0);
            }
        }
        if let Some(e) = &t.error {
            g.text(&tr!("無法啟動終端機：{error}", error = e.as_str()), Font::Text, Rect::new(x0, y0, r.r - PAD_X, y0 + lh * 2.0), c.error);
        }
        g.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_settings() {
        assert_eq!(shell_command("cmd"), ("cmd.exe".to_string(), ShellKind::Cmd));
        assert_eq!(shell_command("powershell").1, ShellKind::PowerShell);
    }
}
