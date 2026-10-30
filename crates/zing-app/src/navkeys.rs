//! 可以設定快速鍵的導覽：清單（`cursor.*`、`filter.*`；不叫 `list.*`：Lua 的 `list` 是管線的函式）、樹狀列（`tree.*`）、彈出清單（`ui.*`）的命令，
//! 以及按鍵屬於哪個情境（`zing_core::keymap::Scope`）。預設按鍵在 `zing_core` 的 default_keys.toml，與原本寫死的行為相同。
//!
//! - 彈出清單（下拉選單、歡迎畫面、命令列、右側面板、命令面板）：按鍵先在 `popup` 情境找命令，`ui.*` 轉成
//!   原本的標準按鍵交給各自的處理（`App::ui`）。沒有綁定的標準按鍵（方向鍵、Enter、Esc…）視為已解除、不做事，
//!   其他按鍵照原樣交給它們（例如命令面板輸入框的文字）。
//! - 清單篩選中（`filter` 情境），沒有 Ctrl、Alt 的字元鍵一律是篩選文字，不查快速鍵。
//! - 在清單中直接打字（`list.typing`）：篩選、跳到名稱開頭相符的項目，或不做事（`filter.start` 開始篩選）。

use std::time::{Duration, Instant};

use zing_core::config::Typing;
use zing_core::keymap::{KeyChord, Scope};

use crate::app::{App, Focus};
use crate::model::NodeKind;

/// 彈出清單的動作（`ui.*` 命令）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAction {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    First,
    Last,
    Accept,
    Cancel,
    Select,
    Next,
    Prev,
}

impl UiAction {
    const ALL: [(UiAction, &'static str, u16, bool); 13] = [
        (UiAction::Up, "ui.up", 0x26, false),
        (UiAction::Down, "ui.down", 0x28, false),
        (UiAction::Left, "ui.left", 0x25, false),
        (UiAction::Right, "ui.right", 0x27, false),
        (UiAction::PageUp, "ui.pageup", 0x21, false),
        (UiAction::PageDown, "ui.pagedown", 0x22, false),
        (UiAction::First, "ui.first", 0x24, false),
        (UiAction::Last, "ui.last", 0x23, false),
        (UiAction::Accept, "ui.accept", 0x0D, false),
        (UiAction::Cancel, "ui.cancel", 0x1B, false),
        (UiAction::Select, "ui.select", 0x20, false),
        (UiAction::Next, "ui.next", 0x09, false),
        (UiAction::Prev, "ui.prev", 0x09, true),
    ];

    pub fn parse(cmd: &str) -> Option<UiAction> {
        let name = cmd.split_whitespace().next()?;
        Self::ALL.iter().find(|(_, n, _, _)| *n == name).map(|(a, ..)| *a)
    }

    /// 原本的標準按鍵：（虛擬鍵碼, Shift）
    pub fn key(self) -> (u16, bool) {
        Self::ALL.iter().find(|(a, ..)| *a == self).map(|&(_, _, vk, shift)| (vk, shift)).unwrap_or((0, false))
    }

    /// 這個按鍵是不是某個動作的標準按鍵（沒有綁定時視為已解除）
    fn standard(chord: KeyChord) -> bool {
        !chord.ctrl && !chord.alt && Self::ALL.iter().any(|&(_, _, vk, shift)| vk == chord.vk && shift == chord.shift)
    }
}

/// 彈出清單中按下的按鍵
pub enum PopupKey {
    /// 綁定的命令（`ui.*`、`search.*`…）
    Cmd(String),
    /// 已解除的標準按鍵：不做事
    Unbound,
    /// 照原樣交給彈出清單
    Raw,
}

/// 清單的移動方向
#[derive(Clone, Copy)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    First,
    Last,
}

/// `search.sort --by=…` 的欄位：Some(None) 是取消排序；不是已知的欄位時 None
pub fn sort_key_of(cmd: &str) -> Option<Option<zing_core::namequery::SortKey>> {
    let mut args = cmd.split_whitespace().skip(1);
    let by = args.find_map(|a| a.strip_prefix("--by=").or_else(|| (!a.starts_with("--")).then_some(a)));
    match by.unwrap_or("none") {
        "none" => Some(None),
        w => zing_core::namequery::SortKey::ALL.into_iter().find(|k| k.word() == w).map(Some),
    }
}

/// 「跳到名稱開頭相符的項目」：這段時間內打的字接成一個字首
const JUMP_RESET: Duration = Duration::from_millis(1000);

impl App {
    /// 一般按鍵（不在彈出清單中）屬於哪個情境
    pub fn key_scope(&self) -> Scope {
        match self.focus {
            Focus::List if !self.tab().filter.is_empty() || self.filter_armed => Scope::Filter,
            Focus::List => Scope::List,
            Focus::Tree => Scope::Tree,
            Focus::Side => Scope::Global,
        }
    }

    /// 彈出清單中按下的按鍵：`scope` 是 popup 或 search
    pub fn popup_key(&self, chord: KeyChord, scope: Scope) -> PopupKey {
        match self.config.command_in(scope, chord) {
            Some(cmd) => PopupKey::Cmd(cmd.to_string()),
            None if UiAction::standard(chord) => PopupKey::Unbound,
            None => PopupKey::Raw,
        }
    }

    /// 彈出清單中按鍵綁定的命令：`ui.*` 交給 `ui`（回傳有沒有處理，沒有處理時呼叫端照樣找全域的快速鍵），
    /// 其他命令直接執行
    pub fn popup_command(&mut self, cmd: &str) -> bool {
        match UiAction::parse(cmd) {
            Some(a) => self.ui(a),
            None => {
                self.exec(cmd);
                true
            }
        }
    }

    /// 執行彈出清單的動作（`ui.*`）：交給開著的下拉選單、歡迎畫面、命令列、命令面板或右側面板
    pub fn ui(&mut self, a: UiAction) -> bool {
        let (vk, shift) = a.key();
        if self.menu.is_some() {
            return self.menu_key(vk);
        }
        if self.welcome.is_some() {
            return self.welcome_key(vk, shift);
        }
        if self.bar_focus.is_some() {
            return self.cmdbar_key(vk);
        }
        if self.palette.is_some() {
            let page = crate::palette::MAX_ROWS as isize;
            match a {
                UiAction::Up => self.palette_move(-1),
                UiAction::Down => self.palette_move(1),
                UiAction::PageUp => self.palette_move(-page),
                UiAction::PageDown => self.palette_move(page),
                UiAction::First => self.palette_move(isize::MIN / 2),
                UiAction::Last => self.palette_move(isize::MAX / 2),
                UiAction::Accept => self.run_palette(None),
                UiAction::Cancel => self.close_palette(),
                _ => return false,
            }
            return true;
        }
        if self.focus == Focus::Side {
            if a == UiAction::Cancel {
                if self.side_escape() {
                    self.side_settle();
                }
                return true;
            }
            return self.side_key(vk, false);
        }
        false
    }

    // ------------------------------------------------------------ 清單

    /// 移動焦點：`extend` 延伸選取（Shift），`keep` 只移動焦點（Ctrl），兩者都有時延伸並保留原本的選取
    pub fn list_move(&mut self, dir: Move, extend: bool, keep: bool) {
        let p = self.active_pane;
        let list = self.layout.panes.get(p).map(|pl| pl.list);
        let grid = list.and_then(|l| self.grid(p, l));
        // 縮圖：上下一列是 cols 項，一頁是可見列數 × cols；清單檢視左右不移動
        let cols = grid.map_or(1, |g| g.cols);
        let line_h = grid.map_or(self.row_h(), |g| g.cell_h);
        let page = list.map_or(10, |l| (l.h() / line_h).floor().max(1.0) as usize) * cols;
        let tab = self.panes[p].tab_mut();
        let n = tab.view.len();
        if n == 0 {
            return;
        }
        let last = n - 1;
        let f = tab.focus;
        let t = match dir {
            Move::Up => f.saturating_sub(cols),
            Move::Down => (f + cols).min(last),
            Move::Left if grid.is_some() => f.saturating_sub(1),
            Move::Right if grid.is_some() => (f + 1).min(last),
            Move::Left | Move::Right => return,
            Move::PageUp => f.saturating_sub(page),
            Move::PageDown => (f + page).min(last),
            Move::First => 0,
            Move::Last => last,
        };
        if extend {
            tab.select_range(t, keep);
        } else if keep {
            tab.focus = t;
        } else {
            tab.select_only(t);
        }
        self.ensure_visible(p);
        self.invalidate();
    }

    /// 只選取焦點項目（`toggle`：切換焦點項目，其他不變）
    pub fn list_select(&mut self, toggle: bool) {
        let tab = self.tab_mut();
        if tab.view.is_empty() {
            return;
        }
        let f = tab.focus;
        if toggle { tab.toggle(f) } else { tab.select_only(f) }
        self.invalidate();
    }

    pub fn filter_backspace(&mut self) {
        let mut f = self.tab().filter.clone();
        f.pop();
        self.set_filter(f);
    }

    /// 之後打的字成為篩選文字（任何非字元鍵取消）
    pub fn filter_start(&mut self) {
        if self.focus != Focus::List || !self.tab().filter.is_empty() {
            return;
        }
        self.filter_armed = true;
        self.set_status(tr!("篩選：輸入文字（按其他鍵取消）"), false);
        self.invalidate();
    }

    /// 清單中打的字元（沒有綁定快速鍵的）：依 `list.typing` 篩選、跳到相符的項目或不做事
    pub fn list_typed(&mut self, c: char) {
        if self.tab().filter.is_empty() && !self.filter_armed {
            match self.config.list.typing {
                Typing::Filter => {}
                Typing::Jump => return self.jump_to(c),
                Typing::None => return,
            }
            if c == ' ' {
                return;
            }
        }
        self.filter_armed = false;
        let mut f = self.tab().filter.clone();
        f.push(c);
        self.set_filter(f);
    }

    /// 跳到名稱開頭相符的項目：連續打的字接成字首；重複打同一個字元時輪流跳到下一個以它開頭的項目
    fn jump_to(&mut self, c: char) {
        let now = Instant::now();
        let fresh = self.jump.as_ref().is_none_or(|(_, t)| now.duration_since(*t) > JUMP_RESET);
        let mut prefix = if fresh { String::new() } else { self.jump.take().map(|(s, _)| s).unwrap_or_default() };
        prefix.extend(c.to_lowercase());
        // 同一個字元重複：從下一個項目開始找、只比對一個字元
        let repeat = prefix.chars().count() > 1 && prefix.chars().all(|x| x == prefix.chars().next().unwrap_or(x));
        let key: String = if repeat { prefix.chars().take(1).collect() } else { prefix.clone() };
        self.jump = Some((prefix, now));
        let p = self.active_pane;
        let tab = self.panes[p].tab();
        let n = tab.view.len();
        if n == 0 {
            return;
        }
        let start = if repeat || fresh { tab.focus + 1 } else { tab.focus };
        let found = (0..n).map(|k| (start + k) % n).find(|&i| {
            let name = String::from_utf16_lossy(tab.listing.name(tab.view[i] as usize)).to_lowercase();
            name.starts_with(&key)
        });
        if let Some(i) = found {
            self.panes[p].tab_mut().select_only(i);
            self.ensure_visible(p);
            self.invalidate();
        }
    }

    // ------------------------------------------------------------ 樹狀列

    fn tree_focus(&self) -> Option<usize> {
        let n = self.tree.nodes.len();
        (n > 0).then(|| self.tree.focus.min(n - 1))
    }

    fn tree_moved(&mut self) {
        let f = self.tree.focus;
        self.ensure_tree_visible(f);
        self.invalidate();
    }

    pub fn tree_step(&mut self, down: bool) {
        let Some(i) = self.tree_focus() else { return };
        let n = self.tree.nodes.len();
        self.tree.focus = if down { (i + 1).min(n - 1) } else { i.saturating_sub(1) };
        self.tree_moved();
    }

    /// 展開；已展開時移到第一個子節點
    pub fn tree_expand(&mut self) {
        let Some(i) = self.tree_focus() else { return };
        if self.tree.nodes[i].expanded {
            self.tree.focus = (i + 1).min(self.tree.nodes.len() - 1);
        } else {
            self.toggle_tree_node(i);
        }
        self.tree_moved();
    }

    /// 收合；已收合時移到上層
    pub fn tree_collapse(&mut self) {
        let Some(i) = self.tree_focus() else { return };
        if self.tree.nodes[i].expanded {
            self.toggle_tree_node(i);
        } else {
            let d = self.tree.nodes[i].depth;
            if let Some(parent) = self.tree.nodes[..i].iter().rposition(|x| x.depth < d) {
                self.tree.focus = parent;
            }
        }
        self.tree_moved();
    }

    pub fn tree_open(&mut self) {
        if let Some(i) = self.tree_focus() {
            self.open_tree_node(i);
            self.tree_moved();
        }
    }

    pub fn tree_leave(&mut self) {
        self.focus = Focus::List;
        self.invalidate();
    }

    /// 焦點所在的我的最愛上移／下移（不是我的最愛時不做事）
    pub fn tree_move_favorite(&mut self, by: isize) {
        let Some(i) = self.tree_focus() else { return };
        if self.tree.nodes[i].kind != NodeKind::Favorite {
            return;
        }
        let p = self.tree.nodes[i].path.clone();
        if let Err(e) = self.move_favorite(&p, by) {
            self.set_status(e, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_keys() {
        use zing_core::namequery::SortKey;
        assert_eq!(sort_key_of("search.sort --by=size"), Some(Some(SortKey::Size)));
        assert_eq!(sort_key_of("search.sort --by=none"), Some(None));
        assert_eq!(sort_key_of("search.sort"), Some(None));
        assert_eq!(sort_key_of("search.sort --by=color"), None);
    }

    #[test]
    fn ui_actions() {
        assert_eq!(UiAction::parse("ui.down"), Some(UiAction::Down));
        assert_eq!(UiAction::parse("ui.prev"), Some(UiAction::Prev));
        assert_eq!(UiAction::parse("cursor.down"), None);
        assert_eq!(UiAction::Prev.key(), (0x09, true));
        let k = |s: &str| KeyChord::parse(s).unwrap();
        assert!(UiAction::standard(k("Down")));
        assert!(UiAction::standard(k("Shift+Tab")));
        assert!(!UiAction::standard(k("Shift+Down")));
        assert!(!UiAction::standard(k("Ctrl+J")));
        assert!(!UiAction::standard(k("J")));
        // 每個動作都有預設的按鍵（default_keys.toml）
        let (cfg, _) = zing_core::config::Config::load(None);
        for (a, name, ..) in UiAction::ALL {
            assert!(cfg.keys.iter().any(|(c, cmd)| c.when == Scope::Popup && cmd == name), "{a:?}");
        }
    }
}
