//! 檔案作業的復原與重做（`file.undo`／`file.redo`，預設 Ctrl+Z／Ctrl+Y，與檔案總管相同）。
//!
//! - 作業完成時，檔案作業的執行緒依實際的結果產生 `Undo`（`undo_of`）：複製、移動、刪除的結果來自 Shell 的進度回呼
//!   （新建立的項目、移到資源回收筒裡的項目），重新命名與新增的結果來自作業本身。
//! - 復原：重新命名改回原名、移動移回原資料夾、複製與新增刪除到資源回收筒、刪除到資源回收筒的還原（`FileOp::Restore`）。
//!   永久刪除不能復原。
//! - 重做：再送一次原本的作業（它完成時又產生新的 `Undo`）。新的一般作業清掉重做的堆疊。
//! - 復原與重做的作業同樣經 `App::submit_op_from`（暫停監看、作業佇列），`Origin` 告訴完成時要不要記錄。

use zing_core::path;

use crate::app::App;
use crate::shell::{FileOp, Origin};

/// 堆疊的上限
const MAX: usize = 50;

/// 復原一個作業：要送出的作業（移回不同的資料夾時有好幾個）與重做時再送的原本的作業
#[derive(Clone, Debug)]
pub struct Undo {
    /// 原本的作業（「重新命名」「移動 3 個項目」…），狀態列與提示用
    pub label: String,
    pub ops: Vec<FileOp>,
    pub redo: FileOp,
}

/// `op` 成功之後怎麼復原。`created` 是新增的項目名稱，`log` 是進度回呼記下的（原本的路徑, 新的項目）：
/// 複製、移動是新的完整路徑，刪除是資源回收筒裡的項目（解析名稱）
pub fn undo_of(op: &FileOp, created: Option<&str>, log: &[(String, String)]) -> Option<Undo> {
    let name = |p: &str| path::file_name(p).to_string();
    let parent = |p: &str| path::parent(p).unwrap_or_default();
    let (label, ops) = match op {
        FileOp::Rename { path: p, new_name } => {
            let now = path::join(&parent(p), new_name);
            (tr!("重新命名「{name}」", name = name(p)), vec![FileOp::Rename { path: now, new_name: name(p) }])
        }
        FileOp::BatchRename { dir, pairs } => {
            let back = pairs.iter().map(|(o, n)| (n.clone(), o.clone())).collect();
            (tr!("重新命名 {n} 個項目", n = pairs.len()), vec![FileOp::BatchRename { dir: dir.clone(), pairs: back }])
        }
        FileOp::NewFolder { dir } | FileOp::NewFile { dir } => {
            let created = path::join(dir, created?);
            (tr!("新增「{name}」", name = name(&created)), vec![FileOp::Delete { paths: vec![created], permanent: false }])
        }
        FileOp::Transfer { sources, dest, mov: false } => {
            // 只取直接複製到目的地的項目（資料夾裡的項目跟著資料夾刪除）
            let copies: Vec<String> = log.iter().filter(|(_, new)| parent(new).eq_ignore_ascii_case(dest)).map(|(_, new)| new.clone()).collect();
            if copies.is_empty() {
                return None;
            }
            (tr!("複製 {n} 個項目", n = sources.len()), vec![FileOp::Delete { paths: copies, permanent: false }])
        }
        FileOp::Transfer { sources, mov: true, .. } => {
            // 依原本的資料夾分組移回去
            let mut groups: Vec<(String, Vec<String>)> = Vec::new();
            for (old, new) in log.iter().filter(|(old, _)| sources.iter().any(|s| s.eq_ignore_ascii_case(old))) {
                let back = parent(old);
                match groups.iter_mut().find(|(d, _)| d.eq_ignore_ascii_case(&back)) {
                    Some((_, v)) => v.push(new.clone()),
                    None => groups.push((back, vec![new.clone()])),
                }
            }
            if groups.is_empty() {
                return None;
            }
            let ops = groups.into_iter().map(|(dest, sources)| FileOp::Transfer { sources, dest, mov: true }).collect();
            (tr!("移動 {n} 個項目", n = sources.len()), ops)
        }
        FileOp::Delete { paths, permanent: false } => {
            let items: Vec<(String, String, String)> = log
                .iter()
                .filter(|(old, _)| paths.iter().any(|p| p.eq_ignore_ascii_case(old)))
                .map(|(old, recycled)| (recycled.clone(), parent(old), name(old)))
                .collect();
            if items.is_empty() {
                return None;
            }
            (tr!("刪除 {n} 個項目", n = paths.len()), vec![FileOp::Restore { items }])
        }
        // 永久刪除不能復原；還原本身由重做處理（再刪一次）
        FileOp::Delete { permanent: true, .. } | FileOp::Restore { .. } => return None,
    };
    Some(Undo { label, ops, redo: op.clone() })
}

impl App {
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// 最後一個作業的說明（命令的提示、狀態列）
    pub fn undo_label(&self) -> Option<&str> {
        self.undo_stack.last().map(|u| u.label.as_str())
    }

    pub fn undo(&mut self) {
        let Some(u) = self.undo_stack.pop() else { return };
        self.set_status(tr!("復原：{what}", what = u.label), false);
        self.redo_stack.push((u.label.clone(), u.redo));
        for op in u.ops {
            self.submit_op_from(op, Origin::Undo);
        }
    }

    pub fn redo(&mut self) {
        let Some((label, op)) = self.redo_stack.pop() else { return };
        self.set_status(tr!("重做：{what}", what = label), false);
        self.submit_op_from(op, Origin::Redo);
    }

    /// 作業完成：一般作業與重做記進復原的堆疊；新的一般作業清掉重做的堆疊
    pub fn record_undo(&mut self, origin: Origin, undo: Option<Undo>) {
        match origin {
            Origin::Normal => {
                if let Some(u) = undo {
                    self.redo_stack.clear();
                    self.push_undo(u);
                }
            }
            Origin::Redo => {
                if let Some(u) = undo {
                    self.push_undo(u);
                }
            }
            Origin::Undo => {}
        }
    }

    fn push_undo(&mut self, u: Undo) {
        if self.undo_stack.len() >= MAX {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(u);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_undoes_to_the_old_name() {
        let op = FileOp::Rename { path: r"C:\a\old.txt".into(), new_name: "new.txt".into() };
        let u = undo_of(&op, None, &[]).unwrap();
        match &u.ops[..] {
            [FileOp::Rename { path, new_name }] => assert_eq!((path.as_str(), new_name.as_str()), (r"C:\a\new.txt", "old.txt")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn move_goes_back_to_each_source_folder() {
        let op = FileOp::Transfer { sources: vec![r"C:\a\1.txt".into(), r"C:\b\2.txt".into()], dest: r"D:\x".into(), mov: true };
        let log = [
            (r"C:\a\1.txt".to_string(), r"D:\x\1.txt".to_string()),
            (r"C:\b\2.txt".to_string(), r"D:\x\2.txt".to_string()),
        ];
        let u = undo_of(&op, None, &log).unwrap();
        let dests: Vec<&str> = u.ops.iter().map(|o| match o {
            FileOp::Transfer { dest, mov: true, .. } => dest.as_str(),
            other => panic!("{other:?}"),
        }).collect();
        assert_eq!(dests, [r"C:\a", r"C:\b"]);
    }

    #[test]
    fn copy_undo_deletes_only_top_level_copies() {
        let op = FileOp::Transfer { sources: vec![r"C:\a\dir".into()], dest: r"D:\x".into(), mov: false };
        let log = [(r"C:\a\dir".to_string(), r"D:\x\dir".to_string()), (r"C:\a\dir\f.txt".to_string(), r"D:\x\dir\f.txt".to_string())];
        match &undo_of(&op, None, &log).unwrap().ops[..] {
            [FileOp::Delete { paths, permanent: false }] => assert_eq!(paths, &[r"D:\x\dir".to_string()]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn permanent_delete_cannot_be_undone() {
        assert!(undo_of(&FileOp::Delete { paths: vec![r"C:\a".into()], permanent: true }, None, &[]).is_none());
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    use crate::services::{Msg, Notifier};
    use crate::shell::FileOps;
    use std::time::Duration;

    /// 等下一個 `FileOpDone`（其他訊息略過）
    fn done(rx: &std::sync::mpsc::Receiver<Msg>) -> (Option<String>, Option<Box<Undo>>) {
        loop {
            match rx.recv_timeout(Duration::from_secs(30)).expect("檔案作業逾時") {
                Msg::FileOpDone { error, undo, .. } => return (error, undo),
                _ => continue,
            }
        }
    }

    /// 刪除到資源回收筒後復原（從回收筒還原到原處）、改名後復原：以真的 Shell 執行，會用到資源回收筒
    /// `cargo test -p zing-app undo_with_shell -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn undo_with_shell() {
        let dir = zing_core::testdir::dir().join(format!("zing-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("復原測試.txt");
        std::fs::write(&file, "x").unwrap();
        let file_s = file.to_string_lossy().into_owned();
        let (n, rx) = Notifier::new(windows::Win32::Foundation::HWND::default());
        let mut ops = FileOps::new(n, false);

        // 改名、復原
        ops.submit(FileOp::Rename { path: file_s.clone(), new_name: "改名後.txt".into() }, Origin::Normal);
        let (err, undo) = done(&rx);
        assert_eq!(err, None);
        assert!(dir.join("改名後.txt").exists());
        for op in undo.expect("改名沒有產生復原").ops {
            ops.submit(op, Origin::Undo);
            assert_eq!(done(&rx).0, None);
        }
        assert!(file.exists(), "改名沒有復原");

        // 刪除到資源回收筒、復原
        ops.submit(FileOp::Delete { paths: vec![file_s.clone()], permanent: false }, Origin::Normal);
        let (err, undo) = done(&rx);
        assert_eq!(err, None);
        assert!(!file.exists(), "沒有刪除");
        let undo = undo.expect("刪除沒有產生復原");
        println!("復原：{} {:?}", undo.label, undo.ops);
        for op in undo.ops {
            ops.submit(op, Origin::Undo);
            assert_eq!(done(&rx).0, None);
        }
        assert!(file.exists(), "沒有從資源回收筒還原");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
