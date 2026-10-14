//! 主題編輯器、匯入匯出、以文字編輯器修改主題檔。
//!
//! - 編輯面板（命令 theme.edit）停靠在窗格右側，列出 `zing_core::theme::FIELDS`；每次修改立即套用到整個畫面。
//!   顏色：點色塊開啟系統選色對話框，點數值直接輸入 #rrggbb；數字：輸入或滾輪微調；選項：點一下換下一個。
//!   「另存新主題」寫成 themes\<名稱>.toml（只存與內建基礎不同的欄位）；「儲存」覆蓋目前的使用者主題；
//!   「取消」還原開啟面板前的主題。內建主題不能覆蓋。
//! - 匯入（.zingtheme／.toml，也可以拖進視窗）：驗證後複製到 themes 資料夾並套用；同名時詢問是否覆蓋。
//!   匯出：目前的主題（差異）存成 .zingtheme。
//! - 以文字編輯器開啟（theme.editfile）：用記事本開啟目前的主題檔，每秒檢查修改時間，存檔後自動重新載入。
//!
//! 所有檔案讀寫都在背景；對話框（選色、開啟、另存）在釋放 App 借用後以模態動作執行。

use std::path::PathBuf;
use std::time::SystemTime;

use zing_core::theme::{FIELDS, FieldKind, Theme, choice_label, valid_theme_name};

use crate::app::{App, EditKind, Modal};
use crate::gfx::{Font, Rect};
use crate::services::Msg;
use crate::settings;
use crate::view::Hit;
use crate::window;

/// 面板寬度
pub const PANEL_W: f32 = 380.0;
/// 底部按鈕：另存新主題、儲存、取消、以文字編輯器開啟
pub const BUTTONS: [&str; 4] = ["另存新主題", "儲存", "取消", "文字編輯器"];
/// 主題檔的大小上限（匯入時）
const MAX_THEME_BYTES: u64 = 1 << 20;

pub struct ThemeEditor {
    /// 開啟面板（或上次儲存）時的主題與名稱：取消時還原
    pub original: Theme,
    pub original_name: String,
    pub scroll: f32,
}

/// 面板中的一列：分組標題或欄位
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    Header(&'static str),
    Field(usize),
}

pub fn rows() -> Vec<Row> {
    let mut out = Vec::new();
    let mut group = "";
    for (i, f) in FIELDS.iter().enumerate() {
        if f.group != group {
            group = f.group;
            out.push(Row::Header(group));
        }
        out.push(Row::Field(i));
    }
    out
}

pub struct PanelLayout {
    pub title: Rect,
    pub list: Rect,
    pub buttons: [Rect; 4],
}

fn theme_file(name: &str) -> PathBuf {
    settings::themes_dir().join(format!("{name}.toml"))
}

impl App {
    pub fn is_user_theme(&self) -> bool {
        Theme::resolve_builtin(&self.theme_name, false).is_none()
    }

    pub fn open_theme_editor(&mut self) {
        if self.theme_editor.is_none() {
            self.theme_editor =
                Some(ThemeEditor { original: self.theme.clone(), original_name: self.theme_name.clone(), scroll: 0.0 });
        }
        self.invalidate();
    }

    /// 關閉面板；`revert` 為 true 時還原開啟前的主題
    pub fn close_theme_editor(&mut self, revert: bool) {
        if let Some(ed) = self.theme_editor.take()
            && revert
            && self.theme != ed.original
        {
            self.theme_name = ed.original_name;
            self.apply_theme(ed.original);
        }
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::ThemeField { .. } | EditKind::ThemeSaveAs))
            && let Some(e) = self.edit.take()
        {
            window::destroy_edit(e.hwnd);
        }
        self.invalidate();
    }

    pub fn theme_panel_layout(&self, r: Rect) -> PanelLayout {
        let rh = self.row_h();
        let inner = r.inset(12.0, 10.0);
        let title = Rect::new(inner.l, inner.t, inner.r, inner.t + rh);
        let bh = (rh + 6.0).max(26.0);
        let row2 = Rect::new(inner.l, inner.b - bh, inner.r, inner.b);
        let row1 = Rect::new(inner.l, row2.t - 6.0 - bh, inner.r, row2.t - 6.0);
        let half = (inner.w() - 6.0) / 2.0;
        let buttons = [
            Rect::new(row1.l, row1.t, row1.l + half, row1.b),
            Rect::new(row1.l + half + 6.0, row1.t, row1.r, row1.b),
            Rect::new(row2.l, row2.t, row2.l + half, row2.b),
            Rect::new(row2.l + half + 6.0, row2.t, row2.r, row2.b),
        ];
        let list = Rect::new(inner.l, title.b + 6.0, inner.r, row1.t - 8.0);
        PanelLayout { title, list, buttons }
    }

    /// 欄位列的位置（視窗座標）與其中色塊、數值的範圍
    pub fn theme_row_rects(&self, list: Rect, k: usize, scroll: f32) -> (Rect, Rect, Rect) {
        let rh = self.row_h();
        let y = list.t + k as f32 * rh - scroll;
        let row = Rect::new(list.l, y, list.r, y + rh);
        let value_l = list.l + (list.w() * 0.45).min(150.0);
        let swatch = Rect::new(value_l, y + 4.0, value_l + rh - 8.0, y + rh - 4.0);
        let value = Rect::new(value_l, y, list.r, y + rh);
        (row, swatch, value)
    }

    pub fn theme_hit(&self, lay_panel: Rect, x: f32, y: f32) -> Hit {
        let Some(ed) = self.theme_editor.as_ref() else { return Hit::None };
        let pl = self.theme_panel_layout(lay_panel);
        if let Some(k) = pl.buttons.iter().position(|r| r.contains(x, y)) {
            return Hit::ThemeButton(k);
        }
        if pl.list.contains(x, y) {
            for (k, row) in rows().iter().enumerate() {
                let Row::Field(i) = *row else { continue };
                let (r, swatch, _) = self.theme_row_rects(pl.list, k, ed.scroll);
                if r.contains(x, y) {
                    if FIELDS[i].kind == FieldKind::Color && swatch.contains(x, y) {
                        return Hit::ThemeSwatch(i);
                    }
                    return Hit::ThemeRow(i);
                }
            }
        }
        Hit::ThemePanel
    }

    pub fn theme_click(&mut self, hit: Hit) {
        match hit {
            Hit::ThemeSwatch(i) => {
                let initial = self.theme.field_value(&FIELDS[i]);
                self.modal = Some(Modal::PickColor { field: i, initial });
            }
            Hit::ThemeRow(i) => match FIELDS[i].kind {
                FieldKind::Choice(_) | FieldKind::Bool => {
                    if let Some(t) = self.theme.cycle_field(&FIELDS[i]) {
                        self.apply_theme(t);
                    }
                }
                _ => self.begin_theme_field_edit(i),
            },
            Hit::ThemeButton(0) => self.begin_theme_save_as(),
            Hit::ThemeButton(1) => self.save_theme_overwrite(),
            Hit::ThemeButton(2) => self.close_theme_editor(true),
            Hit::ThemeButton(3) => self.open_theme_file(),
            _ => {}
        }
    }

    /// 滾輪：數字欄位上微調，其他地方捲動清單
    pub fn theme_wheel(&mut self, panel: Rect, x: f32, y: f32, delta: i16) {
        let notches = (delta as i32 / 120).signum();
        if let Hit::ThemeRow(i) = self.theme_hit(panel, x, y)
            && let Some(t) = self.theme.nudge_field(&FIELDS[i], notches)
        {
            self.apply_theme(t);
            return;
        }
        let pl = self.theme_panel_layout(panel);
        let max = (rows().len() as f32 * self.row_h() - pl.list.h()).max(0.0);
        if let Some(ed) = self.theme_editor.as_mut() {
            ed.scroll = (ed.scroll - notches as f32 * 3.0 * self.theme.metrics.row_height).clamp(0.0, max);
        }
        self.invalidate();
    }

    fn begin_theme_field_edit(&mut self, i: usize) {
        let Some(panel) = self.layout.theme_panel else { return };
        let Some(ed) = self.theme_editor.as_ref() else { return };
        let pl = self.theme_panel_layout(panel);
        let Some(k) = rows().iter().position(|r| *r == Row::Field(i)) else { return };
        let (_, swatch, value) = self.theme_row_rects(pl.list, k, ed.scroll);
        let l = if FIELDS[i].kind == FieldKind::Color { swatch.r + 6.0 } else { value.l };
        let text = self.theme.field_value(&FIELDS[i]);
        let len = text.encode_utf16().count();
        window::begin_edit(self, Rect::new(l, value.t, value.r, value.b), &text, len, EditKind::ThemeField { field: i });
    }

    fn begin_theme_save_as(&mut self) {
        let Some(panel) = self.layout.theme_panel else { return };
        let pl = self.theme_panel_layout(panel);
        let b = pl.buttons[0];
        let r = Rect::new(pl.list.l, b.t - 8.0 - self.row_h(), pl.list.r, b.t - 8.0);
        let suggest = if self.is_user_theme() { format!("{} 複本", self.theme_name) } else { "我的主題".to_string() };
        let len = suggest.encode_utf16().count();
        window::begin_edit(self, r, &suggest, len, EditKind::ThemeSaveAs);
    }

    /// 欄位輸入框確認
    pub fn commit_theme_field(&mut self, i: usize, text: &str) {
        match self.theme.with_field(&FIELDS[i], text) {
            Ok(t) => self.apply_theme(t),
            Err(e) => self.set_status(e, true),
        }
    }

    /// 選色對話框的結果
    pub fn theme_pick_color(&mut self, i: usize, hex: String) {
        self.commit_theme_field(i, &hex);
    }

    /// 另存新主題：名稱不能與內建或已存在的主題相同
    pub fn save_theme_as(&mut self, name: &str) {
        let name = name.trim().to_string();
        if let Err(e) = valid_theme_name(&name) {
            self.set_status(e, true);
            return;
        }
        let mut t = self.theme.clone();
        t.name = name.clone();
        let text = t.diff_toml(&t.base());
        self.write_theme(name, text, false);
    }

    /// 儲存：覆蓋目前的使用者主題（內建主題要另存）
    pub fn save_theme_overwrite(&mut self) {
        if !self.is_user_theme() {
            self.set_status("內建主題不能覆蓋，請用「另存新主題」", true);
            return;
        }
        let mut t = self.theme.clone();
        t.name = self.theme_name.clone();
        let text = t.diff_toml(&t.base());
        let name = self.theme_name.clone();
        self.write_theme(name, text, true);
    }

    /// 在背景寫入 themes\<name>.toml；`overwrite` 為 false 時已存在就失敗
    pub fn write_theme(&mut self, name: String, text: String, overwrite: bool) {
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            zing_core::guard::io("寫入主題檔");
            let path = theme_file(&name);
            let result = if !overwrite && path.exists() {
                Err(format!("已有名為「{name}」的主題"))
            } else {
                let _ = std::fs::create_dir_all(settings::themes_dir());
                std::fs::write(&path, text).map_err(|e| format!("無法寫入 {}：{e}", path.display()))
            };
            n.send(Msg::ThemeSaved { name, result });
        });
    }

    pub fn on_theme_saved(&mut self, name: String, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.set_status(format!("已儲存主題「{name}」"), false);
                self.load_theme_by_name(name);
            }
            Err(e) => self.set_status(e, true),
        }
    }

    /// 在背景讀取並套用主題（theme.set、存檔後、檔案變更後）
    pub fn load_theme_by_name(&mut self, name: String) {
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            let (theme, warn) = settings::resolve_theme(&name);
            let result = match warn {
                Some(w) => Err(w),
                None => Ok(theme),
            };
            n.send(Msg::ThemeLoaded { name, result });
        });
    }

    pub fn on_theme_loaded(&mut self, name: String, result: Result<Theme, String>) {
        match result {
            Ok(theme) => {
                self.theme_name = name;
                // 編輯面板開著：存檔後的狀態成為新的還原點
                if let Some(ed) = self.theme_editor.as_mut() {
                    ed.original = theme.clone();
                    ed.original_name = self.theme_name.clone();
                }
                self.apply_theme(theme);
            }
            Err(e) => self.set_status(e, true),
        }
    }

    /// 以文字編輯器開啟目前的主題檔，並在檔案變更時自動重新載入
    pub fn open_theme_file(&mut self) {
        if !self.is_user_theme() {
            self.set_status("內建主題沒有主題檔：請先「另存新主題」再用文字編輯器修改", true);
            return;
        }
        let path = theme_file(&self.theme_name);
        let file = Some(format!("\"{}\"", path.display()));
        crate::services::launch(&self.notifier, "notepad.exe".into(), file, None, None, crate::services::Show::Normal);
        self.watch_theme_file();
    }

    /// 開始監看目前主題檔的修改時間（TIMER_THEME_WATCH，每秒一次，在背景讀取）
    pub fn watch_theme_file(&mut self) {
        self.theme_watch = Some((self.theme_name.clone(), None));
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), crate::app::TIMER_THEME_WATCH, 1000, None);
        }
    }

    pub fn stop_theme_watch(&mut self) {
        self.theme_watch = None;
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), crate::app::TIMER_THEME_WATCH);
        }
    }

    pub fn on_theme_watch_timer(&mut self) {
        let Some((name, _)) = self.theme_watch.clone() else {
            self.stop_theme_watch();
            return;
        };
        // 已換成其他主題就不再監看
        if name != self.theme_name {
            self.stop_theme_watch();
            return;
        }
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            let mtime = std::fs::metadata(theme_file(&name)).and_then(|m| m.modified()).ok();
            n.send(Msg::ThemeFileTime { name, mtime });
        });
    }

    pub fn on_theme_file_time(&mut self, name: String, mtime: Option<SystemTime>) {
        let Some((watched, last)) = self.theme_watch.as_mut() else { return };
        if *watched != name || mtime.is_none() {
            return;
        }
        let changed = last.is_some() && *last != mtime;
        *last = mtime;
        if changed {
            self.load_theme_by_name(name);
        }
    }

    /// 匯入：讀取、驗證後複製到 themes 資料夾並套用；同名時詢問是否覆蓋
    pub fn import_theme_file(&mut self, path: String) {
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            zing_core::guard::io("讀取主題檔");
            let result = (|| -> Result<(String, String, bool), String> {
                let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                if meta.len() > MAX_THEME_BYTES {
                    return Err("檔案太大，不是主題檔".into());
                }
                let text = std::fs::read_to_string(&path).map_err(|e| format!("無法讀取：{e}"))?;
                let theme = Theme::from_toml(&text)?;
                let stem = zing_core::path::file_name(&path).rsplit_once('.').map_or("", |x| x.0).to_string();
                let mut name = if valid_theme_name(&theme.name).is_ok() { theme.name.clone() } else { stem };
                if valid_theme_name(&name).is_err() {
                    name = format!("{} 匯入", name.trim());
                }
                valid_theme_name(&name)?;
                let exists = theme_file(&name).exists();
                Ok((name, text, exists))
            })();
            n.send(Msg::ThemeImport(result));
        });
    }

    pub fn on_theme_import(&mut self, result: Result<(String, String, bool), String>) {
        match result {
            Ok((name, text, true)) => self.modal = Some(Modal::ConfirmThemeOverwrite { name, text }),
            Ok((name, text, false)) => self.write_theme(name, text, false),
            Err(e) => self.set_status(format!("無法匯入主題：{e}"), true),
        }
    }

    /// 匯出：目前的主題（只含與內建基礎不同的欄位）
    pub fn export_theme(&mut self) {
        let mut t = self.theme.clone();
        t.name = self.theme_name.clone();
        let text = t.diff_toml(&t.base());
        self.modal = Some(Modal::ExportTheme { name: self.theme_name.clone(), text });
    }

    pub fn write_export(&mut self, path: String, text: String) {
        let n = self.notifier.clone();
        self.pool.spawn(move || {
            zing_core::guard::io("匯出主題");
            match std::fs::write(&path, text) {
                Ok(()) => n.status(format!("已匯出主題到 {path}"), false),
                Err(e) => n.status(format!("無法匯出主題：{e}"), true),
            }
        });
    }

    pub fn paint_theme_editor(&self, r: Rect) {
        let Some(ed) = self.theme_editor.as_ref() else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        g.fill(r, c.surface);
        let pl = self.theme_panel_layout(r);
        let kind = if self.is_user_theme() { "" } else { "（內建，需另存）" };
        g.text(&format!("編輯主題：{}{kind}", self.theme_name), Font::Bold, pl.title, c.text);
        g.push_clip(pl.list);
        let rh = self.row_h();
        for (k, row) in rows().iter().enumerate() {
            let (rr, swatch, value) = self.theme_row_rects(pl.list, k, ed.scroll);
            if rr.b < pl.list.t || rr.t > pl.list.b {
                continue;
            }
            match *row {
                Row::Header(h) => {
                    g.text(h, Font::Bold, Rect::new(rr.l, rr.t + 2.0, rr.r, rr.b), c.text_secondary);
                }
                Row::Field(i) => {
                    let f = &FIELDS[i];
                    if self.hover == Hit::ThemeRow(i) || self.hover == Hit::ThemeSwatch(i) {
                        g.fill_round(rr, self.theme.metrics.radius, c.hover);
                    }
                    g.text(f.label, Font::Text, Rect::new(rr.l + 8.0, rr.t, value.l - 6.0, rr.b), c.text);
                    let v = self.theme.field_value(f);
                    match f.kind {
                        FieldKind::Color => {
                            if let Ok(col) = zing_core::theme::Color::parse(&v) {
                                g.fill_round(swatch, 3.0, col);
                                g.stroke_round(swatch, 3.0, c.border, 1.0);
                            }
                            g.text(&v, Font::Mono, Rect::new(swatch.r + 8.0, rr.t, rr.r - 6.0, rr.b), c.text_secondary);
                        }
                        FieldKind::Choice(_) | FieldKind::Bool => {
                            g.text(&format!("{} ⟳", choice_label(&v)), Font::Text, Rect::new(value.l, rr.t, rr.r - 6.0, rr.b), c.accent);
                        }
                        _ => g.text(&v, Font::Text, Rect::new(value.l, rr.t, rr.r - 6.0, rr.b), c.text_secondary),
                    }
                    let _ = rh;
                }
            }
        }
        g.pop_clip();
        let can_save = self.is_user_theme();
        for (k, b) in pl.buttons.iter().enumerate() {
            let enabled = k != 1 || can_save;
            let fill = if self.hover == Hit::ThemeButton(k) && enabled { c.hover } else { c.background };
            g.fill_round(*b, self.theme.metrics.radius, fill);
            g.stroke_round(*b, self.theme.metrics.radius, if k == 0 { c.accent } else { c.border }, 1.0);
            let col = if enabled { c.text } else { c.text_secondary.with_alpha(120) };
            let w = g.measure(BUTTONS[k], Font::Text);
            let l = b.l + ((b.w() - w) / 2.0).max(4.0);
            g.text(BUTTONS[k], Font::Text, Rect::new(l, b.t, b.r - 4.0, b.b), col);
        }
    }
}

/// 選色對話框：初始值 #rrggbb，回傳選取的 #rrggbb（取消為 None）
pub fn pick_color(owner: windows::Win32::Foundation::HWND, initial: &str) -> Option<String> {
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
    thread_local! {
        static CUSTOM: std::cell::RefCell<[COLORREF; 16]> = const { std::cell::RefCell::new([COLORREF(0x00ffffff); 16]) };
    }
    let c = zing_core::theme::Color::parse(initial).unwrap_or(zing_core::theme::Color::rgb(0));
    CUSTOM.with(|custom| {
        let mut custom = custom.borrow_mut();
        let mut cc = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: owner,
            rgbResult: COLORREF(c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        if !unsafe { ChooseColorW(&mut cc) }.as_bool() {
            return None;
        }
        let v = cc.rgbResult.0;
        Some(format!("#{:02x}{:02x}{:02x}", v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff))
    })
}

/// 開啟或另存主題檔的對話框；`save` 時 `name` 為預設檔名。回傳選取的路徑
pub fn theme_file_dialog(owner: windows::Win32::Foundation::HWND, save: bool, name: &str) -> Option<String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, SIGDN_FILESYSPATH};
    use windows::core::{Interface, PCWSTR, w};
    unsafe {
        let dialog: IFileDialog = if save {
            CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?.cast().ok()?
        } else {
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?.cast().ok()?
        };
        let filters = [
            COMDLG_FILTERSPEC { pszName: w!("zing-explore 主題"), pszSpec: w!("*.zingtheme;*.toml") },
            COMDLG_FILTERSPEC { pszName: w!("所有檔案"), pszSpec: w!("*.*") },
        ];
        let _ = dialog.SetFileTypes(&filters);
        if save {
            let _ = dialog.SetDefaultExtension(w!("zingtheme"));
            let file = zing_core::text::wide_z(&format!("{name}.zingtheme"));
            let _ = dialog.SetFileName(PCWSTR(file.as_ptr()));
        }
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
        s
    }
}
