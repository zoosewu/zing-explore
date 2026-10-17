//! 主題的檔案操作：儲存、匯入匯出、以文字編輯器修改主題檔。編輯面板在 themepanel.rs。
//!
//! - 「另存新主題」寫成 themes\<名稱>.toml（只存與內建基礎不同的欄位）；「儲存」覆蓋目前的使用者主題。內建主題不能覆蓋。
//! - 匯入（.zingtheme／.toml，也可以拖進視窗）：驗證後複製到 themes 資料夾並套用；同名時詢問是否覆蓋。
//!   匯出：目前的主題（差異）存成 .zingtheme。
//! - 以文字編輯器開啟（theme.editfile）：用記事本開啟目前的主題檔，每秒檢查修改時間，存檔後自動重新載入。
//!
//! 所有檔案讀寫都在背景；對話框（選色、開啟、另存）在釋放 App 借用後以模態動作執行。

use std::path::PathBuf;
use std::time::SystemTime;

use zing_core::theme::{Theme, valid_theme_name};

use crate::app::{App, Modal};
use crate::services::Msg;
use crate::settings;

/// 主題檔的大小上限（匯入時）
const MAX_THEME_BYTES: u64 = 1 << 20;

fn theme_file(name: &str) -> PathBuf {
    settings::themes_dir().join(format!("{name}.toml"))
}

impl App {
    pub fn is_user_theme(&self) -> bool {
        Theme::resolve_builtin(&self.theme_name, false).is_none()
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
            self.begin_theme_save_as();
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
                Err(tr!("已有名為「{name}」的主題", name))
            } else {
                let _ = std::fs::create_dir_all(settings::themes_dir());
                std::fs::write(&path, text).map_err(|e| tr!("無法寫入 {path}：{e}", path = path.display(), e))
            };
            n.send(Msg::ThemeSaved { name, result });
        });
    }

    pub fn on_theme_saved(&mut self, name: String, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.set_status(tr!("已儲存主題「{name}」", name), false);
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
            self.set_status(tr!("內建主題沒有主題檔：請先按「儲存」存成自己的主題，再用文字編輯器修改"), true);
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
                    return Err(tr!("檔案太大，不是主題檔").into());
                }
                let text = std::fs::read_to_string(&path).map_err(|e| tr!("無法讀取：{e}", e))?;
                let theme = Theme::from_toml(&text)?;
                let stem = zing_core::path::file_name(&path).rsplit_once('.').map_or("", |x| x.0).to_string();
                let mut name = if valid_theme_name(&theme.name).is_ok() { theme.name.clone() } else { stem };
                if valid_theme_name(&name).is_err() {
                    name = tr!("{name} 匯入", name = name.trim());
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
            Err(e) => self.set_status(tr!("無法匯入主題：{e}", e), true),
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
                Ok(()) => n.status(tr!("已匯出主題到 {path}", path), false),
                Err(e) => n.status(tr!("無法匯出主題：{e}", e), true),
            }
        });
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
        let (themes, all) = (zing_core::text::wide_z(tr!("zing-explore 主題")), zing_core::text::wide_z(tr!("所有檔案")));
        let filters = [
            COMDLG_FILTERSPEC { pszName: PCWSTR(themes.as_ptr()), pszSpec: w!("*.zingtheme;*.toml") },
            COMDLG_FILTERSPEC { pszName: PCWSTR(all.as_ptr()), pszSpec: w!("*.*") },
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
