//! 快速鍵編輯器（`keys.edit`）：沿用命令面板的輸入框與清單（`Palette::keys`）。
//!
//! - 輸入文字篩選命令；`Enter`（或點一列）後按下新的組合鍵，取代這個命令原本的按鍵；按鍵已被其他命令使用時
//!   從那個命令移除。錄製中 `Esc` 取消（所以 `Esc` 本身不能指定）。
//! - `Ctrl+Delete` 移除這個命令的按鍵，`Ctrl+R` 還原預設（預設＋config.toml）。
//! - 每次變更立即生效，並在背景把與預設不同的按鍵寫進程式旁的 `keys.toml`（`zing_core::keyedit`）；
//!   隔離模式（冒煙測試）不寫檔。
//! - 錄製時的按鍵由輸入框的子類別程序（window.rs 的 `edit_proc`）攔下，轉給 `keys_record`。

use zing_core::keyedit;
use zing_core::keymap::KeyChord;
use zing_core::palette::{BUILTIN, Entry, filter};

use crate::app::App;
use crate::palette::Palette;
use crate::window;

#[derive(Default)]
pub struct KeyEdit {
    /// 正在等使用者按下新的組合鍵
    pub recording: bool,
    /// 最後一次操作的結果（顯示在面板底部）；第二個值為 true 表示警告
    pub message: Option<(String, bool)>,
}

impl App {
    /// 所有命令（內建、工具列按鈕、腳本、設定檔中綁定的其他命令）與它們目前的按鍵
    fn key_entries(&self) -> Vec<Entry> {
        let mut v: Vec<Entry> = BUILTIN.iter().map(|&(cmd, title)| Entry { title: zing_core::i18n::tr(title).into(), cmd: cmd.into(), key: None }).collect();
        v.insert(0, Entry { title: tr!("命令面板").into(), cmd: "palette.open".into(), key: None });
        for b in &self.config.buttons {
            if !v.iter().any(|e| e.cmd == b.cmd) {
                v.push(Entry { title: tr!("按鈕：{label}", label = zing_core::i18n::tr_str(&b.label)), cmd: b.cmd.clone(), key: None });
            }
        }
        for (name, title) in &self.scripts.commands {
            if !v.iter().any(|e| &e.cmd == name) {
                v.push(Entry { title: tr!("腳本：{title}", title), cmd: name.clone(), key: None });
            }
        }
        for (_, cmd) in self.config.keys.iter().chain(&self.config.config_keys) {
            if !v.iter().any(|e| &e.cmd == cmd) {
                v.push(Entry { title: cmd.clone(), cmd: cmd.clone(), key: None });
            }
        }
        for e in &mut v {
            e.key = self.keys_text(&e.cmd);
        }
        v
    }

    fn keys_text(&self, cmd: &str) -> Option<String> {
        let k = keyedit::chords_for(&self.config.keys, cmd);
        (!k.is_empty()).then(|| k.iter().map(KeyChord::to_string).collect::<Vec<_>>().join(", "))
    }

    pub fn open_key_editor(&mut self) {
        self.close_palette();
        let entries = self.key_entries();
        let matches = filter(&entries, "", &[]);
        self.palette = Some(Palette {
            entries,
            query: String::new(),
            matches,
            sel: 0,
            top: 0,
            search: None,
            keys: Some(KeyEdit::default()),
        });
        if let Some(r) = self.palette_edit_rect() {
            window::begin_palette_edit(self, r);
        }
        self.invalidate();
    }

    fn key_edit(&mut self) -> Option<&mut KeyEdit> {
        self.palette.as_mut()?.keys.as_mut()
    }

    pub fn keys_mode(&self) -> bool {
        self.palette.as_ref().is_some_and(|p| p.keys.is_some())
    }

    pub fn keys_recording(&self) -> bool {
        self.palette.as_ref().and_then(|p| p.keys.as_ref()).is_some_and(|k| k.recording)
    }

    /// 選取的命令（`matches` 的第 `sel` 項）
    fn keys_selected(&self) -> Option<Entry> {
        let p = self.palette.as_ref()?;
        p.matches.get(p.sel).and_then(|&k| p.entries.get(k)).cloned()
    }

    /// Enter 或點一列：開始錄製（`index` 為 `matches` 索引，None 為目前選取的）
    pub fn keys_begin_record(&mut self, index: Option<usize>) {
        let Some(p) = self.palette.as_mut() else { return };
        if let Some(i) = index.filter(|&i| i < p.matches.len()) {
            p.sel = i;
        }
        if p.matches.is_empty() {
            return;
        }
        if let Some(k) = p.keys.as_mut() {
            k.recording = true;
            k.message = None;
        }
        self.invalidate();
    }

    pub fn keys_cancel_record(&mut self) {
        if let Some(k) = self.key_edit() {
            k.recording = false;
        }
        self.invalidate();
    }

    /// 錄製到的組合鍵：指定給選取的命令
    pub fn keys_record(&mut self, chord: KeyChord) {
        let Some(e) = self.keys_selected() else { return };
        let prev = keyedit::assign(&mut self.config.keys, &e.cmd, chord);
        let msg = match prev {
            Some(p) => {
                let other = self.palette.as_ref().and_then(|pl| pl.entries.iter().find(|x| x.cmd == p)).map_or(p, |x| x.title.clone());
                (tr!("{chord} 已從「{other}」移除，改為「{title}」", chord, other, title = e.title), true)
            }
            None => (tr!("「{title}」：{chord}", title = e.title, chord), false),
        };
        self.keys_changed(msg);
    }

    /// Ctrl+Delete：移除選取命令的按鍵
    pub fn keys_clear(&mut self) {
        let Some(e) = self.keys_selected() else { return };
        let msg = if keyedit::clear(&mut self.config.keys, &e.cmd) {
            (tr!("已移除「{title}」的快速鍵", title = e.title), false)
        } else {
            (tr!("「{title}」沒有快速鍵", title = e.title), false)
        };
        self.keys_changed(msg);
    }

    /// Ctrl+R：還原選取命令的預設按鍵
    pub fn keys_reset(&mut self) {
        let Some(e) = self.keys_selected() else { return };
        let defaults = self.config.config_keys.clone();
        let taken = keyedit::reset(&mut self.config.keys, &defaults, &e.cmd);
        let msg = if taken.is_empty() {
            (tr!("已還原「{title}」的預設快速鍵", title = e.title), false)
        } else {
            let t: Vec<String> = taken.iter().map(KeyChord::to_string).collect();
            (tr!("已還原「{title}」；{keys} 已被其他命令使用，沒有還原", title = e.title, keys = t.join(", ")), true)
        };
        self.keys_changed(msg);
    }

    /// 按鍵改變後：更新清單上的按鍵、在背景寫入 keys.toml
    fn keys_changed(&mut self, msg: (String, bool)) {
        let texts: Vec<Option<String>> =
            self.palette.as_ref().map(|p| p.entries.iter().map(|e| self.keys_text(&e.cmd)).collect()).unwrap_or_default();
        if let Some(p) = self.palette.as_mut() {
            for (e, t) in p.entries.iter_mut().zip(texts) {
                e.key = t;
            }
            if let Some(k) = p.keys.as_mut() {
                k.recording = false;
                k.message = Some(msg);
            }
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

    /// 面板底部的說明或最後一次操作的結果
    pub fn keys_footer(&self) -> Option<(String, bool)> {
        let k = self.palette.as_ref()?.keys.as_ref()?;
        Some(match &k.message {
            Some(m) => m.clone(),
            None => (tr!("Enter 或點一列後按下新的快速鍵　Ctrl+Delete 移除　Ctrl+R 還原預設　Esc 關閉").into(), false),
        })
    }
}
