//! 檔案作業佇列介面：進度、暫停、取消。
//!
//! 複製／移動／刪除由 shell.rs 的作業執行緒依序執行，進度以 `Msg::Job` 送回（最多每 100 ms 一次）。
//! UI 只透過 `JobCtl` 的 atomic 旗標控制工作，不等待任何鎖。
//! 面板在工作執行超過 0.4 秒時自動顯示（小作業不會閃一下），全部成功後 3 秒自動收起；
//! 失敗的工作留著，直到使用者清除。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fx_core::path;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::app::{App, TIMER_JOBS};
use crate::gfx::{Font, Rect, glyph};
use crate::shell::FileOp;
use crate::view::{Hit, Layout};

pub type JobId = u64;

/// UI 與作業執行緒共用的控制旗標
#[derive(Default)]
pub struct JobCtl {
    pub cancel: AtomicBool,
    pub pause: AtomicBool,
}

pub enum JobEvent {
    Running,
    Paused,
    /// `fraction` 0–1；`current` 目前處理的項目；`done` 已處理的項目數（含資料夾內的項目）
    Progress { fraction: f32, current: String, done: u32 },
    Finished { error: Option<String>, cancelled: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobStatus {
    Queued,
    Running,
    Paused,
    Done,
    Failed(String),
    Cancelled,
}

pub struct Job {
    pub id: JobId,
    pub title: String,
    pub glyph: &'static str,
    pub status: JobStatus,
    pub fraction: f32,
    pub current: String,
    pub done: u32,
    /// 不含暫停的執行時間
    active: Duration,
    since: Option<Instant>,
    finished_at: Option<Instant>,
    pub ctl: Arc<JobCtl>,
}

impl Job {
    pub fn is_finished(&self) -> bool {
        matches!(self.status, JobStatus::Done | JobStatus::Failed(_) | JobStatus::Cancelled)
    }

    fn elapsed(&self) -> Duration {
        self.active + self.since.map_or(Duration::ZERO, |s| s.elapsed())
    }

    /// 依目前的進度速率估算剩餘時間
    fn eta(&self) -> Option<Duration> {
        let t = self.elapsed().as_secs_f32();
        (self.fraction > 0.02 && t > 1.0).then(|| Duration::from_secs_f32(t * (1.0 - self.fraction) / self.fraction))
    }

    fn paused(&self) -> bool {
        !self.is_finished() && self.ctl.pause.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Jobs {
    pub list: Vec<Job>,
    pub visible: bool,
    /// 使用者手動收起：自動顯示不再打開，直到全部工作結束
    dismissed: bool,
}

impl Jobs {
    pub fn active(&self) -> usize {
        self.list.iter().filter(|j| !j.is_finished()).count()
    }
}

/// 面板最多顯示幾列
const MAX_ROWS: usize = 4;
const HEADER_H: f32 = 30.0;
const ROW_H: f32 = 50.0;
const BTN: f32 = 28.0;
/// 執行超過這個時間才自動顯示面板
const SHOW_AFTER: Duration = Duration::from_millis(400);
/// 成功的工作在完成後保留多久
const KEEP_DONE: Duration = Duration::from_secs(3);

fn describe(op: &FileOp) -> (String, &'static str) {
    let what = |paths: &[String]| match paths {
        [one] => format!("「{}」", path::file_name(one)),
        many => format!(" {} 個項目", many.len()),
    };
    let folder = |p: &str| {
        let n = path::file_name(p);
        if n.is_empty() { p.to_string() } else { n.to_string() }
    };
    match op {
        FileOp::Transfer { sources, dest, mov: false } => (format!("複製{}到「{}」", what(sources), folder(dest)), "\u{E8C8}"),
        FileOp::Transfer { sources, dest, mov: true } => (format!("移動{}到「{}」", what(sources), folder(dest)), "\u{E8C6}"),
        FileOp::Delete { paths, permanent: false } => (format!("刪除{}", what(paths)), "\u{E74D}"),
        FileOp::Delete { paths, permanent: true } => (format!("永久刪除{}", what(paths)), "\u{E74D}"),
        _ => (String::new(), glyph::FILE),
    }
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} 秒"),
        60..3600 => format!("{} 分 {} 秒", s / 60, s % 60),
        _ => format!("{} 小時 {} 分", s / 3600, s % 3600 / 60),
    }
}

impl App {
    /// 送出檔案作業；耗時的作業加入佇列介面
    pub fn submit_op(&mut self, op: FileOp) {
        let (title, glyph) = describe(&op);
        if let Some((id, ctl)) = self.fileops.submit(op) {
            self.jobs.list.push(Job {
                id,
                title,
                glyph,
                status: JobStatus::Queued,
                fraction: 0.0,
                current: String::new(),
                done: 0,
                active: Duration::ZERO,
                since: None,
                finished_at: None,
                ctl,
            });
            self.invalidate();
        }
    }

    pub fn on_job_event(&mut self, id: JobId, event: JobEvent) {
        let Some(job) = self.jobs.list.iter_mut().find(|j| j.id == id) else { return };
        match event {
            JobEvent::Running => {
                job.status = JobStatus::Running;
                job.since.get_or_insert_with(Instant::now);
                self.schedule_jobs_tick(SHOW_AFTER);
            }
            JobEvent::Paused => {
                job.status = JobStatus::Paused;
                if let Some(s) = job.since.take() {
                    job.active += s.elapsed();
                }
            }
            JobEvent::Progress { fraction, current, done } => {
                job.fraction = fraction;
                job.current = current;
                job.done = done;
            }
            JobEvent::Finished { error, cancelled } => {
                if let Some(s) = job.since.take() {
                    job.active += s.elapsed();
                }
                job.finished_at = Some(Instant::now());
                job.status = match (error, cancelled) {
                    (_, true) => JobStatus::Cancelled,
                    (Some(e), _) => JobStatus::Failed(e),
                    (None, _) => {
                        job.fraction = 1.0;
                        JobStatus::Done
                    }
                };
                // 失敗一定要讓使用者看到
                if matches!(job.status, JobStatus::Failed(_)) {
                    self.jobs.visible = true;
                }
                self.schedule_jobs_tick(KEEP_DONE);
                if self.pending_exit && self.jobs.active() == 0 {
                    crate::window::close(self.hwnd);
                }
            }
        }
        self.invalidate();
    }

    fn schedule_jobs_tick(&self, after: Duration) {
        unsafe { SetTimer(Some(self.hwnd), TIMER_JOBS, after.as_millis() as u32, None) };
    }

    /// 計時器：自動顯示面板、移除已完成的工作、自動收起
    pub fn jobs_tick(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_JOBS);
        }
        if self.jobs.active() > 0 {
            let long = self.jobs.list.iter().any(|j| !j.is_finished() && j.elapsed() >= SHOW_AFTER);
            if long && !self.jobs.dismissed {
                self.jobs.visible = true;
            } else if !self.jobs.visible {
                self.schedule_jobs_tick(Duration::from_millis(200));
            }
        }
        // 成功與取消的工作保留一段時間後移除；失敗的留著
        let expired = |j: &Job| {
            matches!(j.status, JobStatus::Done | JobStatus::Cancelled) && j.finished_at.is_some_and(|t| t.elapsed() >= KEEP_DONE)
        };
        self.jobs.list.retain(|j| !expired(j));
        if let Some(next) = self.jobs.list.iter().filter_map(|j| match j.status {
            JobStatus::Done | JobStatus::Cancelled => j.finished_at.map(|t| KEEP_DONE.saturating_sub(t.elapsed())),
            _ => None,
        }).min()
        {
            self.schedule_jobs_tick(next.max(Duration::from_millis(50)));
        }
        if self.jobs.list.is_empty() {
            self.jobs.visible = false;
            self.jobs.dismissed = false;
        }
        self.invalidate();
    }

    pub fn jobs_toggle(&mut self) {
        self.jobs.visible = !self.jobs.visible;
        self.jobs.dismissed = !self.jobs.visible;
        self.invalidate();
    }

    pub fn jobs_clear(&mut self) {
        self.jobs.list.retain(|j| !j.is_finished());
        if self.jobs.list.is_empty() {
            self.jobs.visible = false;
        }
        self.invalidate();
    }

    pub fn job_toggle_pause(&mut self, id: JobId) {
        if let Some(j) = self.jobs.list.iter().find(|j| j.id == id && !j.is_finished()) {
            j.ctl.pause.fetch_xor(true, Ordering::Relaxed);
        }
        self.invalidate();
    }

    pub fn job_cancel(&mut self, id: JobId) {
        if let Some(j) = self.jobs.list.iter().find(|j| j.id == id && !j.is_finished()) {
            j.ctl.cancel.store(true, Ordering::Relaxed);
        }
        self.invalidate();
    }

    /// 取消全部（結束程式時）
    pub fn cancel_all_jobs(&mut self) {
        for j in &self.jobs.list {
            j.ctl.cancel.store(true, Ordering::Relaxed);
        }
    }

    // ------------------------------------------------------------ 版面與繪製

    /// 面板高度（不顯示時為 0）
    pub fn jobs_panel_height(&self) -> f32 {
        if !self.jobs.visible {
            return 0.0;
        }
        HEADER_H + self.jobs.list.len().clamp(1, MAX_ROWS) as f32 * ROW_H + 4.0
    }

    /// 顯示的工作：進行中的在前，其次是最近完成的
    fn shown_jobs(&self) -> Vec<&Job> {
        let mut v: Vec<&Job> = self.jobs.list.iter().filter(|j| !j.is_finished()).collect();
        v.extend(self.jobs.list.iter().rev().filter(|j| j.is_finished()));
        v.truncate(MAX_ROWS);
        v
    }

    fn job_rows(&self, panel: Rect) -> Vec<(JobId, Rect)> {
        self.shown_jobs()
            .iter()
            .enumerate()
            .map(|(i, j)| {
                let t = panel.t + HEADER_H + i as f32 * ROW_H;
                (j.id, Rect::new(panel.l + 8.0, t, panel.r - 8.0, t + ROW_H))
            })
            .collect()
    }

    /// 列右側的按鈕：暫停／繼續、取消
    fn job_buttons(row: Rect) -> (Rect, Rect) {
        let cy = row.t + 18.0;
        let cancel = Rect::new(row.r - BTN, cy - BTN / 2.0, row.r, cy + BTN / 2.0);
        let pause = Rect::new(cancel.l - BTN - 4.0, cancel.t, cancel.l - 4.0, cancel.b);
        (pause, cancel)
    }

    fn jobs_header_buttons(&self, panel: Rect) -> (Option<Rect>, Rect) {
        let cy = panel.t + HEADER_H / 2.0;
        let close = Rect::new(panel.r - 8.0 - BTN, cy - 12.0, panel.r - 8.0, cy + 12.0);
        let clear = self.jobs.list.iter().any(Job::is_finished).then(|| {
            let w = self.gfx.measure("清除已完成", Font::Small) + 16.0;
            Rect::new(close.l - 6.0 - w, cy - 12.0, close.l - 6.0, cy + 12.0)
        });
        (clear, close)
    }

    /// 狀態列上的進度（面板收起且有進行中的工作時）
    pub fn jobs_indicator_text(&self) -> Option<String> {
        let active: Vec<&Job> = self.jobs.list.iter().filter(|j| !j.is_finished()).collect();
        if self.jobs.visible || active.is_empty() {
            return None;
        }
        let pct = (active.iter().map(|j| j.fraction).sum::<f32>() / active.len() as f32 * 100.0).round();
        Some(format!("檔案作業：{} 個進行中 {pct}%", active.len()))
    }

    pub fn jobs_hit(&self, lay: &Layout, x: f32, y: f32) -> Option<Hit> {
        if lay.jobs_indicator.is_some_and(|r| r.contains(x, y)) {
            return Some(Hit::JobsIndicator);
        }
        let panel = lay.jobs.filter(|r| r.contains(x, y))?;
        let (clear, close) = self.jobs_header_buttons(panel);
        if close.contains(x, y) {
            return Some(Hit::JobsClose);
        }
        if clear.is_some_and(|r| r.contains(x, y)) {
            return Some(Hit::JobsClear);
        }
        for (id, row) in self.job_rows(panel) {
            let finished = self.jobs.list.iter().any(|j| j.id == id && j.is_finished());
            let (pause, cancel) = Self::job_buttons(row);
            if !finished && pause.contains(x, y) {
                return Some(Hit::JobPause(id));
            }
            if !finished && cancel.contains(x, y) {
                return Some(Hit::JobCancel(id));
            }
        }
        Some(Hit::JobsPanel)
    }

    /// 某個工作的暫停、取消按鈕中心（冒煙測試用）
    #[cfg(feature = "smoke")]
    pub fn job_button_pos(&self, id: JobId, cancel: bool) -> Option<(f32, f32)> {
        let panel = self.compute_layout().jobs?;
        let (_, row) = self.job_rows(panel).into_iter().find(|(j, _)| *j == id)?;
        let (p, c) = Self::job_buttons(row);
        let r = if cancel { c } else { p };
        Some(((r.l + r.r) / 2.0, (r.t + r.b) / 2.0))
    }

    pub fn paint_jobs(&self, lay: &Layout) {
        let Some(panel) = lay.jobs else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        let radius = self.theme.metrics.radius;
        g.fill(panel, c.surface);
        g.hline(0.0, panel.r, panel.t, c.border);

        let head = Rect::new(panel.l + 14.0, panel.t, panel.r, panel.t + HEADER_H);
        let active = self.jobs.active();
        let summary = if active > 0 { format!("檔案作業　{active} 個進行中") } else { "檔案作業".to_string() };
        g.text(&summary, Font::Bold, head, c.text);
        let (clear, close) = self.jobs_header_buttons(panel);
        if let Some(r) = clear {
            if self.hover == Hit::JobsClear {
                g.fill_round(r, radius, c.hover);
            }
            g.text("清除已完成", Font::Small, Rect::new(r.l + 8.0, r.t, r.r, r.b), c.text);
        }
        if self.hover == Hit::JobsClose {
            g.fill_round(close, radius, c.hover);
        }
        g.text(glyph::CLOSE, Font::GlyphSmall, close, c.text_secondary);

        if self.jobs.list.is_empty() {
            let r = Rect::new(panel.l + 14.0, panel.t + HEADER_H, panel.r, panel.t + HEADER_H + ROW_H);
            g.text("沒有檔案作業", Font::Text, r, c.text_secondary);
            return;
        }
        let shown = self.shown_jobs();
        for (job, (_, row)) in shown.iter().zip(self.job_rows(panel)) {
            let (pause_r, cancel_r) = Self::job_buttons(row);
            let finished = job.is_finished();
            let paused = job.paused();
            // 狀態文字
            let (status, status_color) = match &job.status {
                JobStatus::Done => ("完成".to_string(), c.text_secondary),
                JobStatus::Cancelled => ("已取消".to_string(), c.text_secondary),
                JobStatus::Failed(e) => (format!("失敗：{e}"), c.error),
                _ if paused => (format!("已暫停　{:.0}%", job.fraction * 100.0), c.text_secondary),
                JobStatus::Queued => ("排隊中".to_string(), c.text_secondary),
                _ => {
                    let mut s = format!("{:.0}%", job.fraction * 100.0);
                    if let Some(eta) = job.eta() {
                        s.push_str(&format!("　剩餘 {}", format_duration(eta)));
                    }
                    (s, c.text)
                }
            };
            let right = if finished { row.r } else { pause_r.l - 8.0 };
            let status_w = g.measure(&status, Font::Small).min(row.w() * 0.45);
            g.text(&status, Font::SmallRight, Rect::new(right - status_w - 4.0, row.t + 4.0, right, row.t + 32.0), status_color);
            // 標題與目前項目
            let text_l = row.l + 30.0;
            let text_r = right - status_w - 12.0;
            g.text(job.glyph, Font::Glyph, Rect::new(row.l, row.t + 4.0, row.l + 26.0, row.t + 32.0), c.text_secondary);
            g.text(&job.title, Font::Text, Rect::new(text_l, row.t + 4.0, text_r, row.t + 32.0), c.text);
            if !finished && (!job.current.is_empty() || job.done > 0) {
                let sub = if job.done > 0 { format!("{}（已處理 {} 項）", job.current, job.done) } else { job.current.clone() };
                g.text(&sub, Font::Small, Rect::new(text_l, row.t + 26.0, right, row.t + 42.0), c.text_secondary);
            }
            // 進度條
            let bar = Rect::new(text_l, row.b - 6.0, right, row.b - 3.0);
            g.fill_round(bar, 1.5, c.border);
            let fill_color = match job.status {
                JobStatus::Failed(_) => c.error,
                _ if paused || job.status == JobStatus::Cancelled => c.text_secondary,
                _ => c.accent,
            };
            if job.fraction > 0.0 {
                g.fill_round(Rect::new(bar.l, bar.t, bar.l + bar.w() * job.fraction, bar.b), 1.5, fill_color);
            }
            // 按鈕
            if !finished {
                for (r, hit, glyph) in [
                    (pause_r, Hit::JobPause(job.id), if paused { "\u{E768}" } else { "\u{E769}" }),
                    (cancel_r, Hit::JobCancel(job.id), "\u{E711}"),
                ] {
                    if self.hover == hit {
                        g.fill_round(r, radius, c.hover);
                    }
                    g.text(glyph, Font::GlyphSmall, r, c.text);
                }
            }
        }
    }
}
