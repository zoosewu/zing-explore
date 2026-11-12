//! UI 執行緒 I/O 防呆（只在 debug build 生效，release 編譯為空）。
//!
//! 會阻塞的操作（檔案 I/O、路徑解析、Shell 呼叫…）開頭呼叫 [`io`]。若在 UI 執行緒上執行，
//! 就透過 [`set_reporter`] 設定的回呼回報（zing-explore 顯示在狀態列）。確有必要的例外（啟動時讀設定、
//! COM 規定必須在 UI 執行緒的拖放）用 [`allow_io`] 包住並寫明原因。

#[cfg(debug_assertions)]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::panic::Location;
    use std::sync::Mutex;

    static VIOLATIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    type Reporter = Box<dyn Fn(String)>;

    // 每個視窗一條 UI 執行緒：是否為 UI 執行緒、回報到哪個視窗都依執行緒
    thread_local! {
        static ALLOWED: Cell<u32> = const { Cell::new(0) };
        static UI_THREAD: Cell<bool> = const { Cell::new(false) };
        static REPORTER: RefCell<Option<Reporter>> = const { RefCell::new(None) };
    }

    /// 這條執行緒是 UI 執行緒（每個視窗的執行緒各呼叫一次）
    pub fn mark_ui_thread() {
        UI_THREAD.with(|u| u.set(true));
    }

    /// 這條 UI 執行緒的回報方式（顯示在它的視窗）
    pub fn set_reporter(f: impl Fn(String) + 'static) {
        REPORTER.with(|r| *r.borrow_mut() = Some(Box::new(f)));
    }

    pub fn on_ui_thread() -> bool {
        UI_THREAD.with(Cell::get)
    }

    #[track_caller]
    pub fn io(what: &str) {
        if !on_ui_thread() || ALLOWED.with(Cell::get) > 0 {
            return;
        }
        report(format!("UI 執行緒執行了可能阻塞的操作：{what}（{}）", Location::caller()));
    }

    /// 直接回報（例如 UI 訊息處理太久）
    pub fn report(msg: String) {
        if let Ok(mut v) = VIOLATIONS.lock() {
            v.push(msg.clone());
        }
        // 回報器正在執行（重入）時不再回報
        REPORTER.with(|r| {
            if let Ok(r) = r.try_borrow()
                && let Some(f) = r.as_ref()
            {
                f(msg);
            }
        });
    }

    pub struct AllowIo(());

    impl Drop for AllowIo {
        fn drop(&mut self) {
            ALLOWED.with(|a| a.set(a.get() - 1));
        }
    }

    pub fn allow_io(_reason: &'static str) -> AllowIo {
        ALLOWED.with(|a| a.set(a.get() + 1));
        AllowIo(())
    }

    pub fn take_violations() -> Vec<String> {
        VIOLATIONS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }
}

#[cfg(not(debug_assertions))]
mod imp {
    pub fn mark_ui_thread() {}
    pub fn set_reporter(_f: impl Fn(String) + 'static) {}
    pub fn on_ui_thread() -> bool {
        false
    }
    #[inline(always)]
    pub fn io(_what: &str) {}
    pub fn report(_msg: String) {}
    pub struct AllowIo(());
    #[inline(always)]
    pub fn allow_io(_reason: &'static str) -> AllowIo {
        AllowIo(())
    }
    pub fn take_violations() -> Vec<String> {
        Vec::new()
    }
}

pub use imp::*;

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn reports_only_on_ui_thread_without_allowance() {
        mark_ui_thread();
        take_violations();
        io("測試讀檔");
        {
            let _g = allow_io("測試例外");
            io("允許的讀檔");
        }
        std::thread::spawn(|| io("背景讀檔")).join().unwrap();
        let v = take_violations();
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("測試讀檔"));
    }
}
