//! 分頁在視窗之間移動（多視窗，hub.rs）：
//! - 拖曳分頁時游標離開視窗：游標下是另一個 zing 視窗就請它在分頁列標示插入的位置（`Msg::TabHover`），
//!   放開時把分頁交給它（`Msg::TabDrop`，那個視窗依游標位置插入）；游標下沒有 zing 視窗就在那裡開新視窗。
//! - 命令 `tab.newwindow`（分頁的右鍵選單）：把分頁移到新視窗。
//!
//! 視窗之間只經 Hub 送訊息、不等待；目標視窗已關閉時 Hub 改開新視窗，分頁不會不見。

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, GetClassNameW, GetClientRect, GetWindowThreadProcessId, WindowFromPoint};

use crate::app::App;

/// 交給另一個視窗的分頁（只帶路徑與上一頁、下一頁；那個視窗重新列舉）
#[derive(Debug)]
pub struct TabDrop {
    pub path: String,
    pub back: Vec<String>,
    pub forward: Vec<String>,
    /// 放開時游標的位置（螢幕座標，實體像素）
    pub screen: (i32, i32),
}

/// 游標下是哪個視窗
#[derive(Clone, Copy, Debug, PartialEq)]
enum Under {
    Own,
    /// 這個程序的另一個 zing 視窗
    Other(isize),
    /// 沒有 zing 視窗（桌面、其他程式）
    Nothing,
}

/// 冒煙測試：代替 `WindowFromPoint`（使用者的其他視窗可能蓋在測試視窗上）；0 表示沒有 zing 視窗
#[cfg(feature = "smoke")]
pub static TEST_WINDOW_AT: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);

impl App {
    /// 拖曳中的分頁移動到 (x, y)（DIP）：在視窗外時回傳游標的螢幕座標，並更新其他視窗的插入標示
    pub fn tab_drag_outside(&mut self, x: f32, y: f32) -> Option<(i32, i32)> {
        let pt = self.client_to_screen(x, y);
        let screen = (pt.x, pt.y);
        match self.under(screen) {
            Under::Own => {
                self.set_tab_hover_target(None, screen);
                None
            }
            Under::Other(h) => {
                self.set_tab_hover_target(Some(h), screen);
                Some(screen)
            }
            Under::Nothing => {
                self.set_tab_hover_target(None, screen);
                Some(screen)
            }
        }
    }

    /// 在視窗外放開拖曳中的分頁
    pub fn tab_drag_end(&mut self, p: usize, t: usize, screen: (i32, i32)) {
        self.set_tab_hover_target(None, screen);
        match self.under(screen) {
            Under::Own => {}
            Under::Other(h) => {
                let Some(mut drop) = self.detach_tab(p, t, true) else { return };
                drop.screen = screen;
                crate::hub::to_window(h, crate::services::Msg::TabDrop(Box::new(drop)));
            }
            // 唯一的分頁拖到外面時不做事（視窗本身就是那個分頁）
            Under::Nothing => self.tab_to_new_window(p, t, Some(screen)),
        }
    }

    /// 把分頁移到新視窗（`tab.newwindow`；拖到沒有 zing 視窗的地方）。`screen`：新視窗的左上角附近；None 時錯開
    pub fn tab_to_new_window(&mut self, p: usize, t: usize, screen: Option<(i32, i32)>) {
        let Some(drop) = self.detach_tab(p, t, false) else {
            return self.set_status(tr!("只有一個分頁"), false);
        };
        let scale = self.gfx.scale();
        let rect = crate::window::placement(self.hwnd).map(|w| match screen {
            // 游標落在新視窗的分頁列上（大約第一個分頁的位置）
            Some((sx, sy)) => (sx - (60.0 * scale) as i32, sy - (12.0 * scale) as i32, w.w, w.h),
            None => {
                let step = (32.0 * scale) as i32;
                (w.x + step, w.y + step, w.w, w.h)
            }
        });
        crate::hub::open_window(crate::hub::WindowInit {
            restore: Some(crate::settings::WindowSession {
                panes: vec![crate::settings::PaneState { tabs: vec![drop.path], active: 0 }],
                ..Default::default()
            }),
            rect,
            isolated: self.isolated,
            inherit: self.inherit_for_new_window(),
            ..Default::default()
        });
    }

    /// 從這個視窗拿走分頁（不記進「最近關閉的分頁」）。唯一的分頁：`close_window` 時關閉視窗，否則不拿
    fn detach_tab(&mut self, p: usize, t: usize, close_window: bool) -> Option<TabDrop> {
        let total: usize = self.panes.iter().map(|pane| pane.tabs.len()).sum();
        let tab = &self.panes[p].tabs[t];
        let drop = TabDrop { path: tab.path.clone(), back: tab.back.clone(), forward: tab.forward.clone(), screen: (0, 0) };
        if total <= 1 {
            if !close_window {
                return None;
            }
            crate::window::close(self.hwnd);
            return Some(drop);
        }
        let tab = if self.panes[p].tabs.len() > 1 {
            self.take_tab(p, t)
        } else {
            let pane = self.panes.remove(p);
            self.active_pane = self.active_pane.min(self.panes.len() - 1);
            pane.tabs.into_iter().next()?
        };
        tab.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.sync_tree();
        self.invalidate();
        Some(drop)
    }

    /// 另一個視窗拖曳中的分頁經過（`screen`）或離開（None）：分頁列標示插入的位置
    pub fn on_tab_hover(&mut self, screen: Option<(i32, i32)>) {
        self.tab_insert = screen.and_then(|s| self.tab_insert_at(s));
        self.invalidate();
    }

    /// 另一個視窗放過來的分頁：插在游標所在的分頁列，不在分頁列上時加在作用中的窗格最後
    pub fn on_tab_drop(&mut self, drop: TabDrop) {
        self.tab_insert = None;
        let (p, t) = self.tab_insert_at(drop.screen).unwrap_or_else(|| (self.active_pane, self.panes[self.active_pane].tabs.len()));
        let mut tab = self.new_tab(drop.path);
        tab.back = drop.back;
        tab.forward = drop.forward;
        let pane = &mut self.panes[p];
        let t = t.min(pane.tabs.len());
        pane.tabs.insert(t, tab);
        pane.active = t;
        self.active_pane = p;
        self.load(p, t);
        self.sync_tree();
        self.invalidate();
    }

    /// 螢幕座標落在哪個窗格的分頁列、插在第幾個
    fn tab_insert_at(&mut self, screen: (i32, i32)) -> Option<(usize, usize)> {
        let (x, y) = self.screen_to_client(screen);
        self.layout = self.compute_layout();
        let p = self.layout.panes.iter().position(|pl| pl.strip.contains(x, y))?;
        let t = self.layout.panes[p].tabs.iter().filter(|r| (r.l + r.r) / 2.0 < x).count();
        Some((p, t))
    }

    /// 拖曳經過的另一個視窗改變：舊的拿掉標示、新的顯示
    fn set_tab_hover_target(&mut self, target: Option<isize>, screen: (i32, i32)) {
        if let Some(old) = self.tab_hover_target.filter(|&h| Some(h) != target) {
            crate::hub::to_window(old, crate::services::Msg::TabHover(None));
        }
        if let Some(h) = target {
            crate::hub::to_window(h, crate::services::Msg::TabHover(Some(screen)));
        }
        self.tab_hover_target = target;
    }

    /// 游標下的視窗：上層是另一個 zing 視窗就是它；否則在自己的用戶區內就是自己
    /// （其他程式的視窗蓋在上面時，在用戶區內拖曳照樣是視窗內的拖曳）
    fn under(&self, screen: (i32, i32)) -> Under {
        match self.window_at(screen) {
            Under::Other(h) => Under::Other(h),
            _ if self.client_contains(screen) => Under::Own,
            other => other,
        }
    }

    fn window_at(&self, screen: (i32, i32)) -> Under {
        #[cfg(feature = "smoke")]
        if let Some(h) = *TEST_WINDOW_AT.lock().unwrap() {
            return if h == 0 { Under::Nothing } else { Under::Other(h) };
        }
        let root = unsafe { GetAncestor(WindowFromPoint(POINT { x: screen.0, y: screen.1 }), GA_ROOT) };
        if root == self.hwnd {
            return Under::Own;
        }
        let mut class = [0u16; 32];
        let n = unsafe { GetClassNameW(root, &mut class) } as usize;
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(root, Some(&mut pid)) };
        if String::from_utf16_lossy(&class[..n]) == "zing.main" && pid == std::process::id() {
            Under::Other(root.0 as isize)
        } else {
            Under::Nothing
        }
    }

    fn client_contains(&self, screen: (i32, i32)) -> bool {
        let mut r = RECT::default();
        let mut pt = POINT { x: screen.0, y: screen.1 };
        let _ = unsafe { GetClientRect(self.hwnd, &mut r) };
        let _ = unsafe { ScreenToClient(self.hwnd, &mut pt) };
        pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
    }

    fn screen_to_client(&self, screen: (i32, i32)) -> (f32, f32) {
        let mut pt = POINT { x: screen.0, y: screen.1 };
        let _ = unsafe { ScreenToClient(self.hwnd, &mut pt) };
        let s = self.gfx.scale();
        (pt.x as f32 / s, pt.y as f32 / s)
    }
}
