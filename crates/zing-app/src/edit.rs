//! 主視窗的單行輸入框（`App::edit`，一次一個）：每種輸入框的行為都在這裡——
//! 建立時決定的按鍵與失去焦點的行為（`subclass_data`）、內容改變（`edit_changed`）、結束（`finish_edit`）、
//! 是否屬於右側面板（`in_side_panel`）。Win32 的輸入框本身（建立、子類別、位置）在 window.rs，
//! 那裡只把訊息轉來這裡；批次重新命名的兩個輸入框不佔用 `App::edit`（batch.rs）。
//! 這裡的 match 都不用 `_`：新增種類時編譯器會要求每一項都決定。

use windows::Win32::Foundation::HWND;
use zing_core::path;

use crate::app::App;
use crate::services::{Msg, TabId};
use crate::shell::FileOp;
use crate::window;

pub enum EditKind {
    Rename { tab: TabId, path: String },
    /// 樹狀列的節點（資料夾）
    TreeRename { path: String },
    Address,
    Palette,
    /// 主題編輯器：欄位的值、另存新主題的名稱
    ThemeField { field: usize },
    ThemeSaveAs,
    /// 主題編輯器：檔案顏色的萬用字元樣式
    ThemeRule { rule: usize },
    /// 設定頁的文字欄位
    Pref { field: usize },
    /// 自訂命令列：按鈕的名稱、自訂命令的命令腳本
    BarLabel { item: usize },
    BarCustom,
    /// 快速鍵編輯面板的搜尋框（內容改變時即時篩選）
    KeysSearch,
    /// 自訂命令列的搜尋框（輸入時即時篩選）
    BarSearch,
    /// Lua 腳本的 ask()
    Ask,
}

pub struct Edit {
    pub hwnd: HWND,
    pub kind: EditKind,
    /// 重新命名：以 D2D 畫在輸入框外的框線（DIP）；`fit` 為 Some 時框的寬度跟著文字，最寬到這個右緣
    pub frame: Option<crate::gfx::Rect>,
    pub fit: Option<f32>,
}

/// 子類別資料的旗標：方向鍵、PageUp/PageDown、Ctrl+數字交給命令面板
pub const LIST_KEYS: usize = 1;
/// 子類別資料的旗標：失去焦點視為取消（沒有這個旗標時視為確定）
pub const BLUR_CANCELS: usize = 2;

impl EditKind {
    /// 建立輸入框時存進子類別資料，`edit_proc` 依此處理按鍵（在輸入框自己的訊息處理中不借用 App）
    pub fn subclass_data(&self) -> usize {
        match self {
            EditKind::Palette => LIST_KEYS | BLUR_CANCELS,
            EditKind::Ask => BLUR_CANCELS,
            EditKind::Rename { .. }
            | EditKind::TreeRename { .. }
            | EditKind::Address
            | EditKind::ThemeField { .. }
            | EditKind::ThemeSaveAs
            | EditKind::ThemeRule { .. }
            | EditKind::Pref { .. }
            | EditKind::BarLabel { .. }
            | EditKind::BarCustom
            | EditKind::KeysSearch
            | EditKind::BarSearch => 0,
        }
    }

    /// 面板裡的輸入框：關閉面板時一起關閉
    pub fn in_side_panel(&self) -> bool {
        match self {
            EditKind::Pref { .. }
            | EditKind::ThemeField { .. }
            | EditKind::ThemeSaveAs
            | EditKind::ThemeRule { .. }
            | EditKind::BarLabel { .. }
            | EditKind::BarCustom
            | EditKind::BarSearch
            | EditKind::KeysSearch => true,
            EditKind::Rename { .. } | EditKind::TreeRename { .. } | EditKind::Address | EditKind::Palette | EditKind::Ask => false,
        }
    }
}

impl App {
    /// 輸入框的內容改變（EN_CHANGE）：即時篩選的搜尋框、跟著文字調整寬度的重新命名框
    pub fn edit_changed(&mut self, hwnd: HWND, text: String) {
        let Some(e) = self.edit.as_ref().filter(|e| e.hwnd == hwnd) else { return };
        match e.kind {
            EditKind::Palette => self.palette_query(text),
            EditKind::KeysSearch => self.keys_query(text),
            EditKind::BarSearch => self.bar_query(text),
            EditKind::Rename { .. } | EditKind::TreeRename { .. } => window::rename_text_changed(self, &text),
            EditKind::Address
            | EditKind::ThemeField { .. }
            | EditKind::ThemeSaveAs
            | EditKind::ThemeRule { .. }
            | EditKind::Pref { .. }
            | EditKind::BarLabel { .. }
            | EditKind::BarCustom
            | EditKind::Ask => {}
        }
    }

    /// 編輯框結束（Enter / 失去焦點為確認，Esc 為取消）。`hwnd` 為送出通知的編輯框：
    /// 通知是 post 的，抵達前編輯框可能已關閉、甚至換成另一個，這時忽略
    pub fn finish_edit(&mut self, commit: bool, hwnd: HWND) {
        if self.edit.as_ref().is_none_or(|e| e.hwnd != hwnd) {
            return;
        }
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::Ask)) {
            let text = self.edit.as_ref().map(|e| window::edit_text(e.hwnd));
            self.finish_ask(commit, text);
            return;
        }
        if matches!(self.edit.as_ref().map(|e| &e.kind), Some(EditKind::Palette)) {
            if commit {
                self.run_palette(None);
            } else {
                self.close_palette();
            }
            return;
        }
        let Some(edit) = self.edit.take() else { return };
        let text = window::edit_text(edit.hwnd);
        window::destroy_edit(edit.hwnd);
        if commit {
            match edit.kind {
                EditKind::Rename { tab, path: full } => {
                    let new_name = text.trim().to_string();
                    let old_name = path::file_name(&full).to_string();
                    if !new_name.is_empty() && new_name != old_name && self.find_tab(tab).is_some() {
                        self.submit_op(FileOp::Rename { path: full, new_name });
                    }
                }
                // 改名後開在它裡面的分頁、我的最愛跟著換（`on_renamed`），節點由樹狀列的監看更新
                EditKind::TreeRename { path: full } => {
                    let new_name = text.trim().to_string();
                    if !new_name.is_empty() && new_name != path::file_name(&full) {
                        self.submit_op(FileOp::Rename { path: full, new_name });
                    }
                }
                EditKind::Address => {
                    let target = text.trim().to_string();
                    let pane = self.active_pane;
                    if target.is_empty() || target == "此電腦" || target == tr!("此電腦") {
                        self.navigate(pane, String::new(), true);
                    } else {
                        // 檢查是否為資料夾會碰磁碟（網路路徑可能很慢），交給背景
                        let normalized = path::normalize(&target);
                        let cwd = self.tab().path.clone();
                        let n = self.notifier.clone();
                        self.pool.spawn(move || {
                            zing_core::guard::io("檢查網址列路徑");
                            let is_dir = std::path::Path::new(&normalized).is_dir();
                            n.send(Msg::AddressResolved { pane, target, normalized, is_dir, cwd });
                        });
                    }
                }
                EditKind::ThemeField { field } => self.commit_theme_field(field, &text),
                EditKind::ThemeSaveAs => self.save_theme_as(&text),
                EditKind::ThemeRule { rule } => self.commit_rule_pattern(rule, &text),
                EditKind::Pref { field } => self.set_pref(field, text.trim().to_string()),
                EditKind::BarLabel { item } => self.commit_bar_label(item, &text),
                EditKind::BarCustom => self.commit_bar_custom(&text),
                EditKind::KeysSearch | EditKind::BarSearch => {}
                // 在函式開頭已處理
                EditKind::Palette | EditKind::Ask => {}
            }
        }
        window::focus_main(self.hwnd);
        self.invalidate();
    }
}
