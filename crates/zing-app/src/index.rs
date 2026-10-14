//! 與 NTFS 索引服務（zing-index）的互動：資料夾大小、樹狀列的子資料夾、安裝與移除。
//!
//! - 進入資料夾（列舉完成）時向服務查詢所有子資料夾的大小；服務沒有執行、磁碟沒有索引（非 NTFS、
//!   網路磁碟…）或索引還在建立時，改用逐資料夾掃描（`sizes.rs`，設定 `[folder_sizes] auto`）。
//! - 每 2 秒以變動標記輪詢目前顯示的資料夾：沒有變動時服務只回「沒變」，有變動才送新的大小。
//! - 查詢在單一背景執行緒進行（每次約 1 ms），UI 不等待；隔離模式（效能、冒煙測試）不使用索引，結果才可重現。

use std::time::{SystemTime, UNIX_EPOCH};

use zing_core::dirsize::DirSize;
use zing_core::indexproto::{self, QueryError, Request, Response};
use zing_core::text::{natural_cmp, wide_z};

use crate::app::App;
use crate::model::Children;
use crate::services::{Msg, Pool};
use crate::sizes::{SizeInfo, SizeState, key};

/// 從索引取得子資料夾（樹狀列用）；服務不可用時回傳 None
pub fn subdirs(path: &str, show_hidden: bool) -> Option<Children> {
    let Ok(Response::Children { children, .. }) = indexproto::query(&Request::Children { path: path.into(), since: 0 })
    else {
        return None;
    };
    let mut v: Vec<(Vec<u16>, String, Option<bool>)> = children
        .into_iter()
        .filter(|c| show_hidden || !c.hidden)
        .map(|c| {
            let sub = if show_hidden { c.subdirs } else { c.visible_subdirs };
            (wide_z(&c.name), c.name, sub)
        })
        .collect();
    v.sort_by(|a, b| natural_cmp(&a.0, &b.0));
    let (names, subdirs) = v.into_iter().map(|(_, n, s)| (n, s)).unzip();
    Some(Children::new(names, subdirs))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl App {
    fn index_pool(&mut self) -> &Pool {
        self.sizes.query_pool.get_or_insert_with(|| Pool::background(1, "zing-index-query"))
    }

    /// 向索引服務查詢 `parent` 底下所有子資料夾的大小；`since` 為上次的變動標記（0 表示一定要回內容）
    fn query_index_sizes(&mut self, parent: String, since: u64) {
        if !self.sizes.inflight.insert(key(&parent)) {
            return;
        }
        let n = self.notifier.clone();
        self.index_pool().spawn(move || {
            let result = indexproto::query(&Request::Children { path: parent.clone(), since });
            n.send(Msg::IndexSizes { parent, result });
        });
    }

    /// 進入資料夾（列舉完成）時：有索引就查詢，否則依設定逐資料夾計算
    pub fn folder_sizes_on_enter(&mut self, p: usize, t: usize) {
        let path = self.panes[p].tabs[t].path.clone();
        if path.is_empty() || t != self.panes[p].active {
            return;
        }
        if self.isolated {
            self.auto_folder_sizes(p, t);
            return;
        }
        self.query_index_sizes(path, 0);
    }

    pub fn on_index_sizes(&mut self, parent: String, result: Result<Response, QueryError>) {
        let parent_key = key(&parent);
        self.sizes.inflight.remove(&parent_key);
        match result {
            Ok(Response::Children { touched, children, .. }) => {
                self.sizes.index_touched.insert(parent_key.clone(), touched);
                let when = now_secs();
                for c in children {
                    let t = c.totals;
                    let size = DirSize { bytes: t.bytes, files: t.files, dirs: t.dirs, denied: 0 };
                    self.sizes.set_info(&parent, &c.name, SizeInfo { size, state: SizeState::Fresh, when });
                }
                self.sizes.mark_dirty(parent_key);
                self.apply_size_updates();
                self.invalidate();
            }
            Ok(Response::Same { .. }) => {}
            Ok(Response::Scanning { percent }) => {
                // 索引建立完成前先逐資料夾計算，之後輪詢時改用索引
                self.sizes.index_touched.insert(parent_key, 0);
                self.set_status(format!("索引服務正在建立索引（{percent}%），暫時逐資料夾計算大小"), false);
                self.fallback_sizes(&parent);
            }
            Ok(_) | Err(_) => {
                if matches!(result, Err(QueryError::NotRunning)) && !self.sizes.hinted {
                    self.sizes.hinted = true;
                    self.set_status(
                        "提示：啟用 NTFS 索引服務可立即顯示所有資料夾大小（命令面板：啟用 NTFS 索引服務）",
                        false,
                    );
                }
                self.sizes.index_touched.remove(&parent_key);
                self.fallback_sizes(&parent);
            }
        }
    }

    /// 沒有索引時：顯示這個資料夾的分頁依設定逐資料夾計算
    fn fallback_sizes(&mut self, parent: &str) {
        for p in 0..self.panes.len() {
            let t = self.panes[p].active;
            if self.panes[p].tabs[t].path.eq_ignore_ascii_case(parent) {
                self.auto_folder_sizes(p, t);
            }
        }
    }

    /// 定時（TIMER_INDEX）：目前顯示、且有索引的資料夾向服務確認是否有變動
    pub fn poll_index(&mut self) {
        let paths: Vec<String> = self
            .panes
            .iter()
            .map(|pane| pane.tab().path.clone())
            .filter(|p| self.sizes.index_touched.contains_key(&key(p)))
            .collect();
        for path in paths {
            let since = self.sizes.index_touched.get(&key(&path)).copied().unwrap_or(0);
            self.query_index_sizes(path, since);
        }
    }

    /// 命令：index.install、index.uninstall、index.status
    pub fn index_command(&mut self, cmd: &str) {
        let exe = crate::settings::app_dir().join("zing-index.exe");
        let n = self.notifier.clone();
        if cmd == "index.status" {
            self.index_pool().spawn(move || {
                let (text, error) = match indexproto::query(&Request::Status) {
                    Ok(Response::Status(vols)) => {
                        let parts: Vec<String> = vols
                            .iter()
                            .map(|v| match v.state.as_str() {
                                "ready" => format!("{} 就緒（{} 個資料夾）", v.volume, v.dirs),
                                "scanning" => format!("{} 建立中 {}%", v.volume, v.detail),
                                _ => format!("{} 錯誤：{}", v.volume, v.detail),
                            })
                            .collect();
                        (format!("索引服務：{}", parts.join("、")), false)
                    }
                    Err(QueryError::NotRunning) => ("索引服務沒有執行（命令面板：啟用 NTFS 索引服務）".into(), false),
                    Ok(_) => ("索引服務回應異常".into(), true),
                    Err(QueryError::Failed(e)) => (format!("無法查詢索引服務：{e}"), true),
                };
                n.send(Msg::IndexAction { text, error, changed: false });
            });
            return;
        }
        if !exe.is_file() {
            self.set_status(format!("找不到 {}", exe.display()), true);
            return;
        }
        let install = cmd == "index.install";
        self.set_status(
            if install {
                "正在啟用索引服務（需要系統管理員權限）…"
            } else {
                "正在移除索引服務…"
            },
            false,
        );
        // 直接以系統管理員身分（UAC）執行 zing-index，等它結束並依結束代碼回報；
        // 不用 std::process::Command（會多帶約 0.1 MB 的程式碼進執行檔）
        std::thread::spawn(move || {
            let arg = if install { "--install" } else { "--uninstall" };
            let (text, error) = match run_elevated(&exe.to_string_lossy(), arg) {
                Ok(0) => (if install { "已啟用索引服務，正在建立索引" } else { "已移除索引服務" }.to_string(), false),
                Ok(code) => (
                    format!(
                        "索引服務操作失敗（結束代碼 {code}），詳見 {}",
                        r"C:\Windows\System32\config\systemprofile\AppData\Local\zing-explore\index.log"
                    ),
                    true,
                ),
                Err(None) => ("已取消".to_string(), false),
                Err(Some(e)) => (format!("無法執行 zing-index：{e}"), true),
            };
            n.send(Msg::IndexAction { text, error, changed: !error });
        });
    }

    pub fn on_index_action(&mut self, text: String, error: bool, changed: bool) {
        self.set_status(text, error);
        if changed {
            // 服務剛安裝或移除：重新查詢目前顯示的資料夾
            self.sizes.index_touched.clear();
            for p in 0..self.panes.len() {
                let t = self.panes[p].active;
                self.folder_sizes_on_enter(p, t);
            }
        }
    }
}

/// 以 UAC 執行程式（不顯示主控台視窗）並等待結束，回傳結束代碼；使用者取消時為 Err(None)
fn run_elevated(file: &str, args: &str) -> Result<u32, Option<String>> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
    use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::PCWSTR;

    let (file, args, verb) = (wide_z(file), wide_z(args), wide_z("runas"));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            return Err((e.code() != ERROR_CANCELLED.to_hresult()).then(|| e.message()));
        }
        let _ = WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        Ok(code)
    }
}
