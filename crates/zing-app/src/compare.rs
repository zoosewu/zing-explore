//! 資料夾比對與同步（`folder.compare`）：比對兩個窗格目前的資料夾（含子資料夾），在視窗中央的對話框列出有差異的項目，
//! 依方向同步。比對與同步計畫在 `zing_core::foldercompare`；這裡是對話框（沿用右側面板的機制，`SidePanel::Compare`）、
//! 背景比對與送出同步的作業。
//!
//! - 比對在自己的執行緒（可能很久），列舉經 `archive::list_folder`（壓縮檔也能比對），以 generation 丟棄過時的結果。
//! - 「比對內容」：大小相同、時間不同的檔案在背景逐位元組比對，相同的從清單拿掉。
//! - 同步：先以 `App::confirm` 列出將複製、覆蓋、刪除的數目，確認後送出一個 `FileOp::Sync`（進作業佇列、可復原；
//!   被覆蓋的舊檔與刪除的項目送進資源回收筒）。作業完成後自動重新比對。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use zing_core::foldercompare::{self, Diff, Direction, Item, Kind, Side};
use zing_core::path;

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::menu::{Action, MenuItem};
use crate::services::Msg;
use crate::shell::FileOp;
use crate::sidepanel::Line;
use crate::view::Hit;

pub const DIALOG_W: f32 = 980.0;
pub const DIALOG_H: f32 = 640.0;
/// 底部按鈕：同步、關閉
pub const BUTTONS: [&str; 2] = [tr_key!("同步"), tr_key!("關閉")];

/// 選項列（第 0 列，固定在清單上方）的部分
pub const PART_DIRECTION: usize = 0;
pub const PART_MIRROR: usize = 1;
pub const PART_CONTENT: usize = 2;
pub const PART_RERUN: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// 比對中：已比對的項目數
    Running(usize),
    Done,
    Failed(String),
}

pub struct ComparePanel {
    pub scroll: f32,
    pub left: String,
    pub right: String,
    pub state: State,
    pub diffs: Vec<Diff>,
    pub direction: Direction,
    /// 鏡像（只用於單向）：目的地與來源完全相同
    pub mirror: bool,
    /// 比對內容中
    pub checking: bool,
    /// 最後一次操作的結果
    pub message: Option<String>,
    generation: u64,
    cancel: Arc<AtomicBool>,
}

impl Drop for ComparePanel {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn direction_label(d: Direction) -> &'static str {
    match d {
        Direction::LeftToRight => tr_key!("左 → 右"),
        Direction::RightToLeft => tr_key!("左 ← 右"),
        Direction::Both => tr_key!("雙向"),
    }
}

fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::OnlyLeft => tr_key!("只在左邊"),
        Kind::OnlyRight => tr_key!("只在右邊"),
        Kind::LeftNewer => tr_key!("左邊較新"),
        Kind::RightNewer => tr_key!("右邊較新"),
        Kind::Different => tr_key!("大小不同"),
        Kind::TypeMismatch => tr_key!("類型不同"),
    }
}

/// 左右兩邊的完整路徑
fn abs(cp: &ComparePanel, side: Side, rel: &str) -> String {
    path::join(if side == Side::Left { &cp.left } else { &cp.right }, rel)
}

fn other(side: Side) -> Side {
    if side == Side::Left { Side::Right } else { Side::Left }
}

impl App {
    pub fn compare_panel(&self) -> Option<&ComparePanel> {
        match &self.side {
            Some(crate::sidepanel::SidePanel::Compare(p)) => Some(p),
            _ => None,
        }
    }

    pub fn compare_panel_mut(&mut self) -> Option<&mut ComparePanel> {
        match &mut self.side {
            Some(crate::sidepanel::SidePanel::Compare(p)) => Some(p),
            _ => None,
        }
    }

    /// 為什麼不能比對（命令列按鈕的停用原因）
    pub fn compare_disabled(&self) -> Option<&'static str> {
        if self.panes.len() < 2 {
            return Some(tr!("只有一個窗格"));
        }
        let (l, r) = (&self.panes[0].tab().path, &self.panes[1].tab().path);
        if l.is_empty() || r.is_empty() {
            return Some(tr!("「此電腦」不能比對"));
        }
        if l.eq_ignore_ascii_case(r) {
            return Some(tr!("兩個窗格是同一個資料夾"));
        }
        None
    }

    /// `folder.compare`：比對左右兩個窗格目前的資料夾
    pub fn open_compare(&mut self) {
        if let Some(why) = self.compare_disabled() {
            self.set_status(why, true);
            return;
        }
        let panel = ComparePanel {
            scroll: 0.0,
            left: self.panes[0].tab().path.clone(),
            right: self.panes[1].tab().path.clone(),
            state: State::Running(0),
            diffs: Vec::new(),
            direction: Direction::LeftToRight,
            mirror: false,
            checking: false,
            message: None,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        self.open_side(crate::sidepanel::SidePanel::Compare(panel));
        self.start_compare();
    }

    /// （重新）在背景比對
    pub fn start_compare(&mut self) {
        let n = self.notifier.clone();
        let Some(cp) = self.compare_panel_mut() else { return };
        cp.cancel.store(true, Ordering::Relaxed);
        cp.cancel = Arc::new(AtomicBool::new(false));
        cp.generation += 1;
        cp.state = State::Running(0);
        cp.diffs.clear();
        cp.checking = false;
        cp.scroll = 0.0;
        let (left, right, generation, cancel) = (cp.left.clone(), cp.right.clone(), cp.generation, cp.cancel.clone());
        let _ = std::thread::Builder::new().name("zing-compare".into()).spawn(move || {
            let list = |rel: &str, side: Side| {
                let root = if side == Side::Left { &left } else { &right };
                let dir = if rel.is_empty() { root.clone() } else { path::join(root, rel) };
                crate::archive::list_folder(&dir, false, &cancel, |_| {})
                    .map(|l| {
                        (0..l.len())
                            .map(|i| Item { name: l.name_string(i), dir: l.entries[i].is_dir(), size: l.entries[i].size, modified: l.entries[i].modified })
                            .collect()
                    })
                    .map_err(|e| tr!("{dir}：{e}", dir, e = e.to_string()))
            };
            let mut last = Instant::now();
            let result = foldercompare::compare(list, &cancel, |scanned| {
                if last.elapsed() > Duration::from_millis(200) {
                    last = Instant::now();
                    n.send(Msg::CompareProgress { generation, scanned });
                }
            });
            if !cancel.load(Ordering::Relaxed) {
                n.send(Msg::CompareDone { generation, result });
            }
        });
        self.invalidate();
    }

    pub fn on_compare_progress(&mut self, generation: u64, scanned: usize) {
        if let Some(cp) = self.compare_panel_mut().filter(|c| c.generation == generation) {
            cp.state = State::Running(scanned);
            self.invalidate();
        }
    }

    pub fn on_compare_done(&mut self, generation: u64, result: Result<Vec<Diff>, String>) {
        let Some(cp) = self.compare_panel_mut().filter(|c| c.generation == generation) else { return };
        match result {
            Ok(diffs) => {
                cp.diffs = diffs;
                cp.state = State::Done;
            }
            Err(e) => cp.state = State::Failed(e),
        }
        self.side_settle();
        self.invalidate();
    }

    /// 「比對內容」：大小相同、時間不同的檔案逐位元組比對
    fn compare_content(&mut self) {
        let n = self.notifier.clone();
        let Some(cp) = self.compare_panel_mut().filter(|c| c.state == State::Done && !c.checking) else { return };
        let pairs: Vec<(String, String, String)> = cp
            .diffs
            .iter()
            .filter(|d| matches!(d.kind, Kind::LeftNewer | Kind::RightNewer) && d.left.as_ref().map(|i| i.size) == d.right.as_ref().map(|i| i.size))
            .map(|d| (d.rel.clone(), abs(cp, Side::Left, &d.rel), abs(cp, Side::Right, &d.rel)))
            .collect();
        if pairs.is_empty() {
            cp.message = Some(tr!("沒有大小相同、只有時間不同的檔案").to_string());
            self.invalidate();
            return;
        }
        cp.checking = true;
        cp.message = Some(tr!("比對 {n} 個檔案的內容…", n = pairs.len()));
        let (generation, cancel) = (cp.generation, cp.cancel.clone());
        let _ = std::thread::Builder::new().name("zing-compare-content".into()).spawn(move || {
            let same: Vec<String> = pairs.into_iter().filter(|(_, a, b)| foldercompare::same_content(a, b, &cancel)).map(|(rel, _, _)| rel).collect();
            if !cancel.load(Ordering::Relaxed) {
                n.send(Msg::CompareContent { generation, same });
            }
        });
        self.invalidate();
    }

    pub fn on_compare_content(&mut self, generation: u64, same: Vec<String>) {
        let Some(cp) = self.compare_panel_mut().filter(|c| c.generation == generation) else { return };
        cp.checking = false;
        cp.diffs.retain(|d| !same.contains(&d.rel));
        cp.message = Some(tr!("{n} 個檔案的內容相同，已從清單拿掉", n = same.len()));
        self.side_settle();
        self.invalidate();
    }

    /// 檔案作業完成：動到比對的資料夾就重新比對（同步之後看到結果）
    pub fn compare_after_op(&mut self, dirs: &[String]) {
        let Some(cp) = self.compare_panel() else { return };
        let touched = dirs.iter().any(|d| crate::model::within(d, &cp.left) || crate::model::within(d, &cp.right));
        if touched {
            self.start_compare();
        }
    }

    /// 「同步」：依方向產生計畫，確認後送出
    fn compare_sync(&mut self) {
        let Some(cp) = self.compare_panel_mut() else { return };
        if cp.state != State::Done {
            return;
        }
        let plan = foldercompare::plan(&cp.diffs, cp.direction, cp.mirror && cp.direction != Direction::Both);
        if plan.copies.is_empty() && plan.removes.is_empty() {
            cp.message = Some(tr!("沒有要同步的項目").to_string());
            self.invalidate();
            return;
        }
        let copies: Vec<(String, String)> = plan
            .copies
            .iter()
            .map(|(side, rel)| (abs(cp, *side, rel), path::parent(&abs(cp, other(*side), rel)).unwrap_or_default()))
            .collect();
        let removes: Vec<String> = plan.removes.iter().map(|(side, rel)| abs(cp, *side, rel)).collect();
        let mut msg = tr!("同步「{left}」與「{right}」：", left = path::file_name(&cp.left), right = path::file_name(&cp.right));
        msg.push('\n');
        msg.push_str(&tr!(
            "複製 {copy} 個、覆蓋 {over} 個、刪除 {del} 個項目（被覆蓋與刪除的送進資源回收筒）",
            copy = plan.copies.len() - plan.overwrites,
            over = plan.overwrites,
            del = plan.removes.len() - plan.overwrites
        ));
        if plan.skipped > 0 {
            msg.push('\n');
            msg.push_str(&tr!("{n} 個差異不同步（方向相反、無法判斷新舊或類型不同）", n = plan.skipped));
        }
        self.confirm(msg, tr_key!("同步"), move |a| a.submit_op(FileOp::Sync { copies, removes }));
    }

    fn open_direction_menu(&mut self, anchor: Rect) {
        let Some(cp) = self.compare_panel() else { return };
        let current = cp.direction;
        let items = [Direction::LeftToRight, Direction::RightToLeft, Direction::Both]
            .into_iter()
            .map(|d| MenuItem { checked: d == current, ..MenuItem::new(zing_core::i18n::tr(direction_label(d)), Action::CompareDirection(d)) })
            .collect();
        self.open_menu(items, anchor);
    }

    pub fn set_compare_direction(&mut self, d: Direction) {
        if let Some(cp) = self.compare_panel_mut() {
            cp.direction = d;
            cp.message = None;
        }
        self.invalidate();
    }

    // ------------------------------------------------------------ 版面與命中

    /// （標題, 兩個路徑, 清單, 摘要, 底部按鈕）
    pub fn compare_layout(&self, r: Rect) -> (Rect, Rect, Rect, Rect, [Rect; 2]) {
        let rh = self.row_h();
        let inner = r.inset(16.0, 12.0);
        let title = Rect::new(inner.l, inner.t, inner.r, inner.t + rh);
        let paths = Rect::new(inner.l, title.b + 2.0, inner.r, title.b + 2.0 + rh * 2.0);
        let buttons = crate::sidepanel::footer_buttons(inner, rh);
        let note = Rect::new(inner.l, buttons[0].t - 6.0 - rh, inner.r, buttons[0].t - 6.0);
        let list = Rect::new(inner.l, paths.b + 6.0, inner.r, note.t - 4.0);
        (title, paths, list, note, buttons)
    }

    pub fn compare_row_h(&self) -> f32 {
        self.row_h() + 4.0
    }

    fn compare_row_rect(&self, list: Rect, k: usize, scroll: f32) -> Rect {
        crate::sidepanel::row_rect(list, k, self.compare_row_h(), scroll, 1)
    }

    /// 選項列的方向、鏡像、比對內容、重新比對
    fn compare_option_parts(&self, r: Rect) -> [Rect; 4] {
        let g = &self.gfx;
        let (t, b) = (r.t + 3.0, r.b - 3.0);
        let dir_w = [Direction::LeftToRight, Direction::RightToLeft, Direction::Both]
            .into_iter()
            .map(|d| g.measure(zing_core::i18n::tr(direction_label(d)), Font::Text))
            .fold(0.0, f32::max)
            + 40.0;
        let direction = Rect::new(r.l, t, r.l + dir_w, b);
        let mirror_w = g.measure(tr!("鏡像：刪除目的地多出的項目"), Font::Text) + 56.0;
        let mirror = Rect::new(direction.r + 12.0, t, direction.r + 12.0 + mirror_w, b);
        let rerun_w = g.measure(tr!("重新比對"), Font::Text) + 24.0;
        let rerun = Rect::new(r.r - rerun_w, t, r.r, b);
        let content_w = g.measure(tr!("比對內容"), Font::Text) + 24.0;
        let content = Rect::new(rerun.l - 6.0 - content_w, t, rerun.l - 6.0, b);
        [direction, mirror, content, rerun]
    }

    pub fn compare_rows(&self) -> usize {
        self.compare_panel().map_or(0, |cp| 1 + cp.diffs.len())
    }

    pub fn compare_hit(&self, panel: Rect, x: f32, y: f32) -> Hit {
        let Some(cp) = self.compare_panel() else { return Hit::None };
        let (_, _, list, _, buttons) = self.compare_layout(panel);
        if let Some(k) = buttons.iter().position(|b| b.contains(x, y)) {
            return Hit::CompareButton(k);
        }
        if !list.contains(x, y) {
            return Hit::ComparePanel;
        }
        let options = self.compare_row_rect(list, 0, cp.scroll);
        if options.contains(x, y) {
            return self.compare_option_parts(options).iter().position(|p| p.contains(x, y)).map_or(Hit::ComparePanel, Hit::ComparePart);
        }
        // 差異的列只是列出，不能按
        Hit::ComparePanel
    }

    /// 鍵盤目標（`side_targets`）：第 0 列是選項，差異的列不能按（只看），下方是底部按鈕
    pub fn compare_targets(&self, panel: Rect, line: Line) -> Vec<(Hit, Rect)> {
        let Some(cp) = self.compare_panel() else { return Vec::new() };
        let (_, _, list, _, buttons) = self.compare_layout(panel);
        match line {
            Line::Bottom(_) => vec![(Hit::CompareButton(0), buttons[0]), (Hit::CompareButton(1), buttons[1])],
            Line::Row(0) => self.compare_option_parts(self.compare_row_rect(list, 0, cp.scroll)).into_iter().enumerate().map(|(i, r)| (Hit::ComparePart(i), r)).collect(),
            _ => Vec::new(),
        }
    }

    pub fn compare_click(&mut self, hit: Hit) {
        let Some(panel) = self.layout.side_panel else { return };
        match hit {
            Hit::ComparePart(PART_DIRECTION) => {
                let (_, _, list, _, _) = self.compare_layout(panel);
                let scroll = self.compare_panel().map_or(0.0, |c| c.scroll);
                let anchor = self.compare_option_parts(self.compare_row_rect(list, 0, scroll))[PART_DIRECTION];
                self.open_direction_menu(anchor);
            }
            Hit::ComparePart(PART_MIRROR) => {
                if let Some(cp) = self.compare_panel_mut().filter(|c| c.direction != Direction::Both) {
                    cp.mirror = !cp.mirror;
                    cp.message = None;
                }
            }
            Hit::ComparePart(PART_CONTENT) => self.compare_content(),
            Hit::ComparePart(PART_RERUN) => self.start_compare(),
            Hit::CompareButton(0) => self.compare_sync(),
            Hit::CompareButton(_) => self.close_side_panels(),
            _ => {}
        }
        self.invalidate();
    }

    // ------------------------------------------------------------ 繪製

    pub fn paint_compare(&self, r: Rect) {
        let Some(cp) = self.compare_panel() else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius;
        let (title, paths, list, note, buttons) = self.compare_layout(r);
        g.text(tr!("比對資料夾"), Font::Bold, title, c.text);
        let rh = self.row_h();
        let half = |k: f32| Rect::new(paths.l, paths.t + k * rh, paths.r, paths.t + (k + 1.0) * rh);
        g.text(&tr!("左：{path}", path = cp.left), Font::Small, half(0.0), c.text_secondary);
        g.text(&tr!("右：{path}", path = cp.right), Font::Small, half(1.0), c.text_secondary);
        // 差異的列
        let options = self.compare_row_rect(list, 0, cp.scroll);
        g.push_clip(Rect::new(list.l, options.b, list.r, list.b));
        let meta_w = 190.0;
        let kind_w = 96.0;
        for (i, d) in cp.diffs.iter().enumerate() {
            let rr = self.compare_row_rect(list, i + 1, cp.scroll);
            if rr.b < options.b || rr.t > list.b {
                continue;
            }
            g.text(zing_core::i18n::tr(kind_label(d.kind)), Font::Small, Rect::new(rr.l + 8.0, rr.t, rr.l + kind_w, rr.b), c.text_secondary);
            let depth = d.rel.matches('\\').count() as f32;
            let name = path::file_name(&d.rel);
            let name = if d.dir() { format!("{name}\\") } else { name.to_string() };
            let right_meta = Rect::new(rr.r - meta_w, rr.t, rr.r - 8.0, rr.b);
            let left_meta = Rect::new(right_meta.l - meta_w, rr.t, right_meta.l - 8.0, rr.b);
            let x = rr.l + kind_w + 4.0 + depth * 14.0;
            // 上層資料夾以淡色顯示在名稱前面，看得出位置
            let parent = path::parent(&d.rel).filter(|p| !p.is_empty());
            let mut x2 = x;
            if let Some(p) = &parent {
                let p = format!("{p}\\");
                g.text(&p, Font::Text, Rect::new(x, rr.t, left_meta.l - 8.0, rr.b), c.text_secondary);
                x2 = (x + g.measure(&p, Font::Text)).min(left_meta.l - 40.0);
            }
            g.text(&name, Font::Text, Rect::new(x2, rr.t, left_meta.l - 8.0, rr.b), if d.dir() { c.folder } else { c.text });
            let meta = |it: &Option<Item>| match it {
                None => String::from("—"),
                Some(it) if it.dir => tr!("資料夾").to_string(),
                Some(it) => format!("{}  {}", zing_core::format::size_short(it.size), zing_core::format::filetime_local(it.modified)),
            };
            g.text(&meta(&d.left), Font::SmallRight, left_meta, c.text_secondary);
            g.text(&meta(&d.right), Font::SmallRight, right_meta, c.text_secondary);
        }
        g.pop_clip();
        // 選項列
        let parts = self.compare_option_parts(options);
        let hot = |p: usize| self.hover == Hit::ComparePart(p);
        let dir = parts[PART_DIRECTION];
        g.fill_round(dir, radius, if hot(PART_DIRECTION) { c.hover } else { c.background });
        g.stroke_round(dir, radius, c.border, 1.0);
        g.text(&format!("{} ▾", zing_core::i18n::tr(direction_label(cp.direction))), Font::Text, Rect::new(dir.l + 10.0, dir.t, dir.r - 4.0, dir.b), c.text);
        let mirror = parts[PART_MIRROR];
        let enabled = cp.direction != Direction::Both;
        let track_r = self.paint_switch(mirror, mirror.l + 4.0, cp.mirror && enabled, enabled);
        g.text(tr!("鏡像：刪除目的地多出的項目"), Font::Text, Rect::new(track_r + 8.0, mirror.t, mirror.r, mirror.b), if enabled { c.text } else { c.text_secondary });
        for (p, label) in [(PART_CONTENT, tr!("比對內容")), (PART_RERUN, tr!("重新比對"))] {
            let b = parts[p];
            g.fill_round(b, radius, if hot(p) { c.hover } else { c.background });
            g.stroke_round(b, radius, c.border, 1.0);
            let w = g.measure(label, Font::Text);
            g.text(label, Font::Text, Rect::new(b.l + (b.w() - w) / 2.0, b.t, b.r, b.b), c.text);
        }
        // 摘要
        let summary = match &cp.state {
            State::Running(n) => tr!("比對中…（已比對 {n} 個項目）", n),
            State::Failed(e) => tr!("無法比對：{e}", e),
            State::Done if cp.diffs.is_empty() => tr!("兩個資料夾相同").to_string(),
            State::Done => {
                let count = |k: Kind| cp.diffs.iter().filter(|d| d.kind == k).count();
                let mut parts = vec![tr!("{n} 個差異", n = cp.diffs.len())];
                for k in [Kind::OnlyLeft, Kind::OnlyRight, Kind::LeftNewer, Kind::RightNewer, Kind::Different, Kind::TypeMismatch] {
                    let n = count(k);
                    if n > 0 {
                        parts.push(format!("{} {n}", zing_core::i18n::tr(kind_label(k))));
                    }
                }
                parts.join("　")
            }
        };
        let summary = match &cp.message {
            Some(m) => format!("{summary}　·　{m}"),
            None => summary,
        };
        let failed = matches!(cp.state, State::Failed(_));
        g.text(&summary, Font::Small, note, if failed { c.error } else { c.text_secondary });
        self.paint_footer_buttons(&buttons, BUTTONS, Some(0), |k| self.hover == Hit::CompareButton(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_cover_every_choice() {
        for d in [Direction::LeftToRight, Direction::RightToLeft, Direction::Both] {
            assert!(!direction_label(d).is_empty());
        }
        assert_eq!(other(Side::Left), Side::Right);
    }
}
