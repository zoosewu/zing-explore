//! 快速鍵編輯面板（keys.edit）：停靠在窗格右側（與設定頁等同一個位置，一次只開一個）。
//!
//! - 命令依分組列出，右側以小方塊顯示目前的按鍵；游標停在按鍵上出現 ×，點一下移除。
//! - 「＋」後按下新的組合鍵就加上；按鍵已被其他命令使用時，在下一列詢問「取代」或「取消」，不會默默搶走。
//!   錄製中 Esc 取消（所以 Esc 本身不能指定）。
//! - 與預設（預設設定＋config.toml）不同的命令有 ↺ 還原；底部「全部還原」。
//! - 上方的搜尋框依名稱篩選；「依按鍵搜尋」後按一個組合鍵，找出用這個按鍵的命令。
//! - 每次變更立即生效，並在背景把與預設不同的按鍵寫進程式旁的 `keys.toml`（`zing_core::keyedit`）；隔離模式不寫檔。

use zing_core::keyedit;
use zing_core::keymap::KeyChord;
use zing_core::palette::BUILTIN;

use crate::app::{App, EditKind};
use crate::gfx::{Font, Rect};
use crate::input::Mods;
use crate::view::Hit;
use crate::window;

pub const PANEL_W: f32 = 440.0;
/// 底部按鈕：全部還原、完成
pub const BUTTONS: [&str; 2] = [tr_key!("全部還原"), tr_key!("完成")];

/// 搜尋列的部分
pub const PART_QUERY: usize = 0;
pub const PART_BY_KEY: usize = 1;
pub const PART_CLEAR: usize = 2;
/// 命令列：第 0..n 個是按鍵方塊，接著是 ＋ 與 ↺
pub const PART_ADD: usize = 1000;
pub const PART_RESET: usize = 1001;
/// 衝突列：取代、取消
pub const PART_REPLACE: usize = 0;
pub const PART_CANCEL: usize = 1;

#[derive(Default)]
pub struct KeysPanel {
    pub scroll: f32,
    pub query: String,
    /// 正在等使用者按下組合鍵的命令
    pub recording: Option<String>,
    /// 「依按鍵搜尋」：等使用者按下組合鍵
    pub awaiting_filter: bool,
    pub key_filter: Option<KeyChord>,
    /// 按下的按鍵已被其他命令使用：（命令, 按鍵, 其他命令）
    pub conflict: Option<(String, KeyChord, String)>,
    /// 最後一次操作的結果；第二個值為 true 表示警告
    pub message: Option<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Search,
    Header(&'static str),
    /// （命令, 名稱）
    Cmd(String, String),
    Conflict,
}

impl App {
    /// 所有命令：（命令, 名稱）—內建、命令列的自訂按鈕、腳本、設定檔中綁定的其他命令
    fn key_commands(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = vec![("palette.open".into(), tr!("命令面板").into())];
        v.extend(BUILTIN.iter().map(|&(cmd, title)| (cmd.to_string(), zing_core::i18n::tr(title).to_string())));
        for it in self.config.cmdbar.iter().filter(|i| !i.is_separator() && !i.is_menu()) {
            if !v.iter().any(|(c, _)| *c == it.cmd) {
                v.push((it.cmd.clone(), it.title()));
            }
        }
        for b in &self.config.buttons {
            if !v.iter().any(|(c, _)| *c == b.cmd) {
                v.push((b.cmd.clone(), zing_core::i18n::tr_str(&b.label).into_owned()));
            }
        }
        for (name, title) in &self.scripts.commands {
            if !v.iter().any(|(c, _)| c == name) {
                v.push((name.clone(), title.clone()));
            }
        }
        for (_, cmd) in self.config.keys.iter().chain(&self.config.config_keys) {
            if !v.iter().any(|(c, _)| c == cmd) {
                v.push((cmd.clone(), cmd.clone()));
            }
        }
        v
    }

    fn key_group(&self, cmd: &str) -> &'static str {
        if self.scripts.commands.iter().any(|(n, _)| n == cmd) {
            return tr_key!("腳本");
        }
        zing_core::cmdbar::group_of(cmd)
    }

    pub fn open_keys_panel(&mut self) {
        self.close_side_panels();
        self.keys_panel = Some(KeysPanel::default());
        self.invalidate();
    }

    pub fn close_keys_panel(&mut self) {
        self.keys_panel = None;
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::KeysSearch))
            && let Some(e) = self.edit.take()
        {
            window::destroy_edit(e.hwnd);
        }
        self.invalidate();
    }

    pub fn keys_rows(&self) -> Vec<Row> {
        let Some(kp) = self.keys_panel.as_ref() else { return Vec::new() };
        let q = kp.query.trim().to_lowercase();
        let mut groups: Vec<(&'static str, Vec<(String, String)>)> = Vec::new();
        for (cmd, title) in self.key_commands() {
            if !q.is_empty() && !title.to_lowercase().contains(&q) && !cmd.to_lowercase().contains(&q) {
                continue;
            }
            if let Some(k) = kp.key_filter
                && self.config.command_for(k) != Some(cmd.as_str())
            {
                continue;
            }
            let g = self.key_group(&cmd);
            match groups.iter_mut().find(|(n, _)| *n == g) {
                Some((_, v)) => v.push((cmd, title)),
                None => groups.push((g, vec![(cmd, title)])),
            }
        }
        let mut out = vec![Row::Search];
        for (g, cmds) in groups {
            out.push(Row::Header(zing_core::i18n::tr(g)));
            for (cmd, title) in cmds {
                let conflict = kp.conflict.as_ref().is_some_and(|(c, _, _)| *c == cmd);
                out.push(Row::Cmd(cmd, title));
                if conflict {
                    out.push(Row::Conflict);
                }
            }
        }
        out
    }

    pub fn keys_layout(&self, r: Rect) -> (Rect, Rect, Rect, [Rect; 2]) {
        let rh = self.row_h();
        let inner = r.inset(12.0, 10.0);
        let title = Rect::new(inner.l, inner.t, inner.r, inner.t + rh * 2.0);
        let bh = (rh + 6.0).max(28.0);
        let row = Rect::new(inner.l, inner.b - bh, inner.r, inner.b);
        let half = (inner.w() - 6.0) / 2.0;
        let buttons = [Rect::new(row.l, row.t, row.l + half, row.b), Rect::new(row.l + half + 6.0, row.t, row.r, row.b)];
        let note = Rect::new(inner.l, row.t - 6.0 - rh, inner.r, row.t - 6.0);
        let list = Rect::new(inner.l, title.b + 4.0, inner.r, note.t - 4.0);
        (title, list, note, buttons)
    }

    pub fn keys_row_rect(&self, list: Rect, k: usize, scroll: f32) -> Rect {
        let rh = self.row_h() + 6.0;
        // 搜尋列固定在最上方
        let y = if k == 0 { list.t } else { list.t + k as f32 * rh - scroll };
        Rect::new(list.l, y, list.r, y + rh)
    }

    /// 按鍵方塊（由右往左排）與 ＋、↺ 的位置
    pub fn key_parts(&self, cmd: &str, r: Rect) -> Vec<(usize, Rect)> {
        let g = &self.gfx;
        let h = r.h() - 8.0;
        let mut x = r.r;
        let mut out = Vec::new();
        let reset = Rect::new(x - h, r.t + 4.0, x, r.b - 4.0);
        x = reset.l - 2.0;
        let add = Rect::new(x - h, r.t + 4.0, x, r.b - 4.0);
        x = add.l - 6.0;
        out.push((PART_RESET, reset));
        out.push((PART_ADD, add));
        let chords = keyedit::chords_for(&self.config.keys, cmd);
        for (i, c) in chords.iter().enumerate().rev() {
            let w = g.measure(&c.to_string(), Font::Small) + 26.0;
            out.push((i, Rect::new(x - w, r.t + 4.0, x, r.b - 4.0)));
            x -= w + 4.0;
        }
        out
    }

    pub fn search_parts(&self, r: Rect) -> [Rect; 3] {
        let by_w = self.gfx.measure(tr!("依按鍵搜尋"), Font::Small) + 34.0;
        let by = Rect::new(r.r - by_w, r.t + 3.0, r.r, r.b - 3.0);
        let q = Rect::new(r.l, r.t + 3.0, by.l - 6.0, r.b - 3.0);
        let clear = Rect::new(q.r - q.h(), q.t, q.r, q.b);
        [q, by, clear]
    }

    pub fn conflict_parts(&self, r: Rect) -> [Rect; 2] {
        let w = 64.0;
        let cancel = Rect::new(r.r - w, r.t + 3.0, r.r, r.b - 3.0);
        let replace = Rect::new(cancel.l - 6.0 - w, r.t + 3.0, cancel.l - 6.0, r.b - 3.0);
        [replace, cancel]
    }

    pub fn keys_hit(&self, panel: Rect, x: f32, y: f32) -> Hit {
        let Some(kp) = self.keys_panel.as_ref() else { return Hit::None };
        let (_, list, _, buttons) = self.keys_layout(panel);
        if let Some(k) = buttons.iter().position(|b| b.contains(x, y)) {
            return Hit::KeysButton(k);
        }
        if !list.contains(x, y) {
            return Hit::KeysPanel;
        }
        let rows = self.keys_rows();
        // 搜尋列在最上層
        let search = self.keys_row_rect(list, 0, kp.scroll);
        if search.contains(x, y) {
            let parts = self.search_parts(search);
            let has_filter = !kp.query.is_empty() || kp.key_filter.is_some();
            if has_filter && parts[PART_CLEAR].contains(x, y) {
                return Hit::KeysPart(0, PART_CLEAR);
            }
            return parts.iter().position(|p| p.contains(x, y)).map_or(Hit::KeysPanel, |p| Hit::KeysPart(0, p));
        }
        for (k, row) in rows.iter().enumerate().skip(1) {
            let r = self.keys_row_rect(list, k, kp.scroll);
            if !r.contains(x, y) || r.t < search.b {
                continue;
            }
            return match row {
                Row::Cmd(cmd, _) => self.key_parts(cmd, r).iter().find(|(_, p)| p.contains(x, y)).map_or(Hit::KeysRow(k), |(i, _)| Hit::KeysPart(k, *i)),
                Row::Conflict => self.conflict_parts(r).iter().position(|p| p.contains(x, y)).map_or(Hit::KeysPanel, |p| Hit::KeysPart(k, p)),
                _ => Hit::KeysPanel,
            };
        }
        Hit::KeysPanel
    }

    pub fn keys_click(&mut self, hit: Hit) {
        let rows = self.keys_rows();
        let Some(kp) = self.keys_panel.as_mut() else { return };
        match hit {
            Hit::KeysPart(0, PART_QUERY) => {
                kp.awaiting_filter = false;
                self.begin_keys_search();
            }
            Hit::KeysPart(0, PART_BY_KEY) => {
                kp.awaiting_filter = !kp.awaiting_filter;
                kp.recording = None;
                kp.query.clear();
                kp.message = kp.awaiting_filter.then(|| (tr!("按下要找的組合鍵（Esc 取消）").to_string(), false));
            }
            Hit::KeysPart(0, PART_CLEAR) => {
                kp.query.clear();
                kp.key_filter = None;
                kp.awaiting_filter = false;
            }
            Hit::KeysPart(k, p) => match rows.get(k) {
                Some(Row::Cmd(cmd, title)) => match p {
                    PART_ADD => {
                        kp.recording = Some(cmd.clone());
                        kp.conflict = None;
                        kp.awaiting_filter = false;
                        kp.message = Some((tr!("「{title}」：按下新的組合鍵（Esc 取消）", title).to_string(), false));
                    }
                    PART_RESET => {
                        let (cmd, title) = (cmd.clone(), title.clone());
                        self.keys_reset_cmd(&cmd, &title);
                    }
                    i => {
                        let chord = keyedit::chords_for(&self.config.keys, cmd).get(i).copied();
                        if let Some(c) = chord {
                            keyedit::remove(&mut self.config.keys, cmd, c);
                            let msg = tr!("已移除「{title}」的 {chord}", title, chord = c.to_string());
                            self.keys_changed(msg, false);
                        }
                    }
                },
                Some(Row::Conflict) => {
                    let Some((cmd, chord, _)) = kp.conflict.take() else { return };
                    if p == PART_REPLACE {
                        let other = keyedit::add(&mut self.config.keys, &cmd, chord);
                        let msg = tr!("{chord} 已從「{other}」移除，改為「{title}」", chord = chord.to_string(), other = self.cmd_title(&other.unwrap_or_default()), title = self.cmd_title(&cmd));
                        self.keys_changed(msg, false);
                    } else if let Some(kp) = self.keys_panel.as_mut() {
                        kp.message = None;
                    }
                }
                _ => {}
            },
            Hit::KeysRow(k) => {
                // 點命令列的空白處：開始指定按鍵（與點 ＋ 相同）
                if let Some(Row::Cmd(..)) = rows.get(k) {
                    return self.keys_click(Hit::KeysPart(k, PART_ADD));
                }
            }
            Hit::KeysButton(0) => {
                self.config.keys = self.config.config_keys.clone();
                self.keys_changed(tr!("已還原所有快速鍵").to_string(), false);
            }
            Hit::KeysButton(_) => self.close_keys_panel(),
            _ => {}
        }
        self.invalidate();
    }

    fn cmd_title(&self, cmd: &str) -> String {
        self.key_commands().into_iter().find(|(c, _)| c == cmd).map_or_else(|| cmd.to_string(), |(_, t)| t)
    }

    fn keys_reset_cmd(&mut self, cmd: &str, title: &str) {
        let defaults = self.config.config_keys.clone();
        let taken = keyedit::reset(&mut self.config.keys, &defaults, cmd);
        if taken.is_empty() {
            self.keys_changed(tr!("已還原「{title}」的預設快速鍵", title), false);
        } else {
            let t: Vec<String> = taken.iter().map(KeyChord::to_string).collect();
            self.keys_changed(tr!("已還原「{title}」；{keys} 已被其他命令使用，沒有還原", title, keys = t.join(", ")), true);
        }
    }

    /// 錄製或依按鍵搜尋中（按鍵交給面板）
    pub fn keys_capturing(&self) -> bool {
        self.keys_panel.as_ref().is_some_and(|k| k.recording.is_some() || k.awaiting_filter)
    }

    /// 錄製中按下的鍵
    pub fn keys_key(&mut self, vk: u16, mods: Mods) {
        // 只有修飾鍵：繼續等
        if matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5 | 0x14 | 0x90 | 0x91) {
            return;
        }
        let Some(kp) = self.keys_panel.as_mut() else { return };
        let plain = !mods.ctrl && !mods.alt && !mods.shift;
        if plain && vk == 0x1B {
            kp.recording = None;
            kp.awaiting_filter = false;
            kp.message = None;
            self.invalidate();
            return;
        }
        let chord = KeyChord { ctrl: mods.ctrl, shift: mods.shift, alt: mods.alt, vk };
        if kp.awaiting_filter {
            kp.awaiting_filter = false;
            kp.key_filter = Some(chord);
            kp.message = None;
            kp.scroll = 0.0;
            self.invalidate();
            return;
        }
        let Some(cmd) = kp.recording.take() else { return };
        let title = self.cmd_title(&cmd);
        match self.config.command_for(chord).map(str::to_string) {
            Some(other) if other == cmd => {
                let msg = tr!("「{title}」已經是 {chord}", title, chord = chord.to_string());
                self.keys_changed(msg, false);
            }
            Some(other) => {
                let other_title = self.cmd_title(&other);
                if let Some(kp) = self.keys_panel.as_mut() {
                    kp.conflict = Some((cmd, chord, other));
                    kp.message = Some((tr!("{chord} 已被「{other}」使用", chord = chord.to_string(), other = other_title), true));
                }
                self.invalidate();
            }
            None => {
                keyedit::add(&mut self.config.keys, &cmd, chord);
                let msg = tr!("「{title}」：{chord}", title, chord = chord.to_string());
                self.keys_changed(msg, false);
            }
        }
    }

    /// 按鍵改變後：顯示結果，並在背景寫入 keys.toml
    fn keys_changed(&mut self, msg: String, warn: bool) {
        if let Some(kp) = self.keys_panel.as_mut() {
            kp.recording = None;
            kp.message = Some((msg, warn));
        }
        if !self.isolated {
            let text = keyedit::to_toml(&keyedit::overrides(&self.config.config_keys, &self.config.keys));
            let n = self.notifier.clone();
            self.pool.spawn(move || {
                zing_core::guard::io("儲存快速鍵");
                if let Err(e) = std::fs::write(crate::settings::keys_path(), text) {
                    n.status(tr!("無法儲存 keys.toml：{e}", e), true);
                }
            });
        }
        self.invalidate();
    }

    fn begin_keys_search(&mut self) {
        let (Some(panel), Some(kp)) = (self.layout.keys_panel, self.keys_panel.as_ref()) else { return };
        let (_, list, _, _) = self.keys_layout(panel);
        let q = self.search_parts(self.keys_row_rect(list, 0, kp.scroll))[PART_QUERY];
        let text = kp.query.clone();
        let len = text.encode_utf16().count();
        window::begin_edit(self, Rect::new(q.l + 26.0, q.t + 1.0, q.r - q.h(), q.b - 1.0), &text, len, EditKind::KeysSearch);
    }

    /// 搜尋框內容改變（EN_CHANGE）
    pub fn keys_query(&mut self, text: String) {
        if let Some(kp) = self.keys_panel.as_mut() {
            kp.query = text;
            kp.key_filter = None;
            kp.scroll = 0.0;
            self.invalidate();
        }
    }

    pub fn keys_wheel(&mut self, panel: Rect, delta: i16) {
        let (_, list, _, _) = self.keys_layout(panel);
        let max = (self.keys_rows().len() as f32 * (self.row_h() + 6.0) - list.h()).max(0.0);
        let step = 3.0 * self.row_h();
        if let Some(kp) = self.keys_panel.as_mut() {
            kp.scroll = (kp.scroll - (delta as i32 / 120).signum() as f32 * step).clamp(0.0, max);
        }
        self.invalidate();
    }

    pub fn paint_keys_panel(&self, r: Rect) {
        let Some(kp) = self.keys_panel.as_ref() else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius;
        g.fill(r, c.surface);
        let (title, list, note, buttons) = self.keys_layout(r);
        let half = title.h() / 2.0;
        g.text(tr!("快速鍵"), Font::Bold, Rect::new(title.l, title.t, title.r, title.t + half), c.text);
        g.text(tr!("點 ＋ 後按下組合鍵即可加上；游標停在按鍵上可移除"), Font::Small, Rect::new(title.l, title.t + half, title.r, title.b), c.text_secondary);
        let rows = self.keys_rows();
        let search = self.keys_row_rect(list, 0, kp.scroll);
        g.push_clip(Rect::new(list.l, search.b, list.r, list.b));
        for (k, row) in rows.iter().enumerate().skip(1) {
            let rr = self.keys_row_rect(list, k, kp.scroll);
            if rr.b < search.b || rr.t > list.b {
                continue;
            }
            match row {
                Row::Header(h) => g.text(h, Font::Bold, Rect::new(rr.l, rr.t + 4.0, rr.r, rr.b), c.text_secondary),
                Row::Cmd(cmd, t) => {
                    let recording = kp.recording.as_deref() == Some(cmd.as_str());
                    let row_hover = matches!(self.hover, Hit::KeysRow(x) | Hit::KeysPart(x, _) if x == k);
                    if recording {
                        g.fill_round(rr, radius, c.accent.with_alpha(40));
                    } else if row_hover {
                        g.fill_round(rr, radius, c.hover);
                    }
                    let parts = self.key_parts(cmd, rr);
                    let left = parts.iter().map(|(_, p)| p.l).fold(rr.r, f32::min);
                    g.text(t, Font::Text, Rect::new(rr.l + 8.0, rr.t, left - 8.0, rr.b), c.text);
                    let changed = keyedit::chords_for(&self.config.keys, cmd) != keyedit::chords_for(&self.config.config_keys, cmd);
                    for (i, p) in parts {
                        let hot = self.hover == Hit::KeysPart(k, i);
                        match i {
                            PART_ADD => {
                                let col = if hot || recording { c.accent } else { c.text_secondary };
                                g.stroke_round(p, 4.0, if hot { c.accent } else { c.border }, 1.0);
                                let w = g.measure("+", Font::Text);
                                g.text("+", Font::Text, Rect::new(p.l + (p.w() - w) / 2.0, p.t, p.r, p.b), col);
                            }
                            PART_RESET => {
                                if changed {
                                    let w = g.measure("↺", Font::Text);
                                    g.text("↺", Font::Text, Rect::new(p.l + (p.w() - w) / 2.0, p.t, p.r, p.b), if hot { c.accent } else { c.text_secondary });
                                }
                            }
                            i => {
                                let chord = keyedit::chords_for(&self.config.keys, cmd)[i].to_string();
                                g.fill_round(p, 4.0, c.background);
                                g.stroke_round(p, 4.0, if hot { c.error } else { c.border }, 1.0);
                                g.text(&chord, Font::Small, Rect::new(p.l + 8.0, p.t, p.r - 16.0, p.b), c.text);
                                if hot {
                                    g.text("×", Font::Small, Rect::new(p.r - 15.0, p.t, p.r, p.b), c.error);
                                }
                            }
                        }
                    }
                    if recording {
                        let msg = tr!("按下組合鍵…");
                        let w = g.measure(msg, Font::Small);
                        g.text(msg, Font::Small, Rect::new(left - 10.0 - w, rr.t, left, rr.b), c.accent);
                    }
                }
                Row::Conflict => {
                    let Some((_, chord, other)) = kp.conflict.as_ref() else { continue };
                    g.fill_round(rr, radius, c.error.with_alpha(30));
                    let parts = self.conflict_parts(rr);
                    let text = tr!("{chord} 已被「{other}」使用", chord = chord.to_string(), other = self.cmd_title(other));
                    g.text(&text, Font::Small, Rect::new(rr.l + 16.0, rr.t, parts[0].l - 6.0, rr.b), c.text);
                    for (p, label) in [(PART_REPLACE, tr!("取代")), (PART_CANCEL, tr!("取消"))] {
                        let b = parts[p];
                        let hot = self.hover == Hit::KeysPart(k, p);
                        g.fill_round(b, radius, if p == PART_REPLACE { c.accent } else if hot { c.hover } else { c.background });
                        g.stroke_round(b, radius, if p == PART_REPLACE { c.accent } else { c.border }, 1.0);
                        let w = g.measure(label, Font::Small);
                        let col = if p == PART_REPLACE { crate::themepanel::contrast(c.accent) } else { c.text };
                        g.text(label, Font::Small, Rect::new(b.l + (b.w() - w) / 2.0, b.t, b.r, b.b), col);
                    }
                }
                Row::Search => {}
            }
        }
        g.pop_clip();
        // 搜尋列
        let [q, by, clear] = self.search_parts(search);
        g.fill_round(q, radius, c.background);
        g.stroke_round(q, radius, if self.hover == Hit::KeysPart(0, PART_QUERY) { c.accent } else { c.border }, 1.0);
        g.text(crate::gfx::glyph::FILTER, Font::GlyphSmall, Rect::new(q.l + 6.0, q.t, q.l + 24.0, q.b), c.text_secondary);
        let editing = matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::KeysSearch));
        if !editing {
            let (text, col) = match (kp.key_filter, kp.query.is_empty()) {
                (Some(k), _) => (tr!("按鍵：{chord}", chord = k.to_string()), c.text),
                (None, false) => (kp.query.clone(), c.text),
                (None, true) => (tr!("搜尋命令").to_string(), c.text_secondary),
            };
            g.text(&text, Font::Small, Rect::new(q.l + 26.0, q.t, clear.l, q.b), col);
        }
        if !kp.query.is_empty() || kp.key_filter.is_some() {
            g.text("×", Font::Text, Rect::new(clear.l + 6.0, clear.t, clear.r, clear.b), c.text_secondary);
        }
        let on = kp.awaiting_filter;
        g.fill_round(by, radius, if on { c.accent.with_alpha(60) } else if self.hover == Hit::KeysPart(0, PART_BY_KEY) { c.hover } else { c.background });
        g.stroke_round(by, radius, if on { c.accent } else { c.border }, 1.0);
        g.text("⌨", Font::Small, Rect::new(by.l + 8.0, by.t, by.l + 26.0, by.b), c.text);
        g.text(tr!("依按鍵搜尋"), Font::Small, Rect::new(by.l + 26.0, by.t, by.r, by.b), c.text);
        // 結果訊息
        if let Some((m, warn)) = &kp.message {
            g.text(m, Font::Small, note, if *warn { c.error } else { c.text_secondary });
        }
        for (k, b) in buttons.iter().enumerate() {
            let primary = k == 1;
            let hot = self.hover == Hit::KeysButton(k);
            g.fill_round(*b, radius, if primary { c.accent } else if hot { c.hover } else { c.background });
            g.stroke_round(*b, radius, if primary { c.accent } else { c.border }, 1.0);
            let label = zing_core::i18n::tr(BUTTONS[k]);
            let w = g.measure(label, Font::Text);
            let col = if primary { crate::themepanel::contrast(c.accent) } else { c.text };
            g.text(label, Font::Text, Rect::new(b.l + ((b.w() - w) / 2.0).max(4.0), b.t, b.r, b.b), col);
        }
    }
}
