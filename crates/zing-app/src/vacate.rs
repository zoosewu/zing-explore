//! 移走中的路徑（`Vacating`）：檔案作業正要移走（改名、移動、刪除）的資料夾，以及因此對分頁與監看的決定。
//!
//! Windows 拒絕移走底下有開啟中監看的資料夾，所以作業前暫停 `FileOp::vacated` 之下的監看、結束後恢復；
//! 作業進行中「資料夾不見了」的回報可能是誤報（Shell 在磁碟上改好名稱之後還要一段時間才結束）。
//! 這裡只做路徑的判斷、回傳要做的事（`Effect`），真的監看、背景確認、分頁的載入由 App 執行。

use crate::model::{replace_prefix, within};

/// 分頁的位置：（窗格, 分頁）
pub type At = (usize, usize);

/// App 傳進來的分頁現況
pub struct TabSpot<'a> {
    pub at: At,
    pub path: &'a str,
    pub watching: bool,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// 停止這個分頁的監看
    Unwatch(At),
    /// 開始監看這個分頁
    Watch(At),
    /// 資料夾改名：分頁換成新路徑、保留內容並重新載入（重新載入時依 `blocks_watch` 開始監看）
    Renamed { at: At, path: String },
    /// 資料夾不見了：分頁移到仍存在的上層
    Gone { at: At, path: String },
    /// 在背景確認這個路徑還在（`services::check_gone`）
    Check(String),
    /// 樹狀列的監看依 `blocks_watch` 重新設定
    SyncTreeWatch,
}

/// 這個視窗目前移走中的路徑（可重複：兩個作業移走同一個資料夾就要結束兩次）
#[derive(Default)]
pub struct Vacating {
    paths: Vec<String>,
}

impl Vacating {
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// `p` 在移走中的資料夾之下，不能監看（不含資料夾本身：監看它自己不妨礙它被改名、移動）
    pub fn blocks_watch(&self, p: &str) -> bool {
        self.paths.iter().any(|v| !v.eq_ignore_ascii_case(p) && within(p, v))
    }

    /// 「`gone` 不見了」可能是作業還沒結束的誤報（含資料夾本身：改名的正是它）。
    /// 作業結束時改名的分頁跟著換（`finish`），刪除、移動由完成後的確認（`check_under`）處理
    fn hides_gone(&self, gone: &str) -> bool {
        self.paths.iter().any(|v| within(gone, v))
    }

    /// 作業開始移走 `vacated`：停止在它們之下的監看
    pub fn pause(&mut self, vacated: Vec<String>, tabs: &[TabSpot]) -> Vec<Effect> {
        if vacated.is_empty() {
            return Vec::new();
        }
        self.paths.extend(vacated);
        let mut out: Vec<Effect> = tabs.iter().filter(|t| t.watching && self.blocks_watch(t.path)).map(|t| Effect::Unwatch(t.at)).collect();
        out.push(Effect::SyncTreeWatch);
        out
    }

    /// 作業結束（成功、失敗或取消）：先把改名的分頁換成新路徑，再恢復 `vacated` 之下的監看（為空時只套用改名）
    pub fn finish(&mut self, vacated: &[String], renamed: Option<(&str, &str)>, tabs: &[TabSpot]) -> Vec<Effect> {
        let mut out = Vec::new();
        let mut moved: Vec<At> = Vec::new();
        if let Some((old, new)) = renamed {
            for t in tabs {
                if let Some(path) = replace_prefix(t.path, old, new) {
                    moved.push(t.at);
                    out.push(Effect::Renamed { at: t.at, path });
                }
            }
        }
        if vacated.is_empty() {
            return out;
        }
        for v in vacated {
            if let Some(k) = self.paths.iter().position(|w| w == v) {
                self.paths.swap_remove(k);
            }
        }
        // 改名的分頁重新載入時就會開始監看
        out.extend(
            tabs.iter()
                .filter(|t| !t.watching && !t.path.is_empty() && !moved.contains(&t.at) && !self.blocks_watch(t.path))
                .map(|t| Effect::Watch(t.at)),
        );
        out.push(Effect::SyncTreeWatch);
        out
    }

    /// `gone` 已不存在：開在它（或它裡面）的分頁移到仍存在的上層 `existing`；作業還在移走它時不理會
    pub fn folder_gone(&self, gone: &str, existing: &str, tabs: &[TabSpot]) -> Vec<Effect> {
        if self.hides_gone(gone) {
            return Vec::new();
        }
        tabs.iter().filter(|t| within(t.path, gone)).map(|t| Effect::Gone { at: t.at, path: existing.to_string() }).collect()
    }

    /// `dirs` 的子資料夾有變動（刪除、改名、移走）：開在它們之下（不含它們本身）的分頁要確認還在，
    /// 同一個路徑只確認一次（資料夾監看看不到自己被刪除或改名）
    pub fn check_under(dirs: &[String], tabs: &[TabSpot]) -> Vec<Effect> {
        let mut seen: Vec<&str> = Vec::new();
        for t in tabs {
            let under = dirs.iter().any(|d| !d.eq_ignore_ascii_case(t.path) && within(t.path, d));
            if under && !seen.iter().any(|s| s.eq_ignore_ascii_case(t.path)) {
                seen.push(t.path);
            }
        }
        seen.into_iter().map(|p| Effect::Check(p.to_string())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(at: At, path: &str, watching: bool) -> TabSpot<'_> {
        TabSpot { at, path, watching }
    }

    fn v(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pause_stops_watches_under_but_not_the_folder_itself() {
        let mut vac = Vacating::default();
        let tabs = [spot((0, 0), r"C:\a", true), spot((0, 1), r"C:\a\b", true), spot((1, 0), r"C:\ab", true), spot((1, 1), r"C:\a\c", false)];
        let out = vac.pause(v(&[r"C:\a"]), &tabs);
        assert_eq!(out, vec![Effect::Unwatch((0, 1)), Effect::SyncTreeWatch]);
        assert!(vac.blocks_watch(r"c:\A\B"));
        assert!(!vac.blocks_watch(r"C:\a"));
        assert!(!vac.blocks_watch(r"C:\ab"));
    }

    #[test]
    fn nothing_vacated_does_nothing() {
        let mut vac = Vacating::default();
        let tabs = [spot((0, 0), r"C:\a", true)];
        assert!(vac.pause(Vec::new(), &tabs).is_empty());
        assert!(vac.finish(&[], None, &tabs).is_empty());
    }

    #[test]
    fn finish_resumes_only_unblocked_tabs() {
        let mut vac = Vacating::default();
        vac.pause(v(&[r"C:\a", r"C:\x"]), &[]);
        let tabs = [spot((0, 0), r"C:\a\b", false), spot((0, 1), r"C:\x\y", false), spot((0, 2), r"C:\z", true), spot((0, 3), "", false)];
        let out = vac.finish(&v(&[r"C:\a"]), None, &tabs);
        assert_eq!(out, vec![Effect::Watch((0, 0)), Effect::SyncTreeWatch]);
        assert!(!vac.is_empty());
        let out = vac.finish(&v(&[r"C:\x"]), None, &tabs);
        assert_eq!(out, vec![Effect::Watch((0, 0)), Effect::Watch((0, 1)), Effect::SyncTreeWatch]);
        assert!(vac.is_empty());
    }

    #[test]
    fn same_folder_vacated_twice_needs_two_finishes() {
        let mut vac = Vacating::default();
        vac.pause(v(&[r"C:\a"]), &[]);
        vac.pause(v(&[r"C:\a"]), &[]);
        vac.finish(&v(&[r"C:\a"]), None, &[]);
        assert!(vac.blocks_watch(r"C:\a\b"));
        vac.finish(&v(&[r"C:\a"]), None, &[]);
        assert!(vac.is_empty());
    }

    #[test]
    fn rename_retargets_before_resuming_and_skips_watch_for_retargeted() {
        let mut vac = Vacating::default();
        vac.pause(v(&[r"C:\a"]), &[]);
        let tabs = [spot((0, 0), r"C:\a\b", false), spot((0, 1), r"C:\a", true), spot((1, 0), r"C:\other", false)];
        let out = vac.finish(&v(&[r"C:\a"]), Some((r"C:\a", r"C:\n")), &tabs);
        assert_eq!(
            out,
            vec![
                Effect::Renamed { at: (0, 0), path: r"C:\n\b".into() },
                Effect::Renamed { at: (0, 1), path: r"C:\n".into() },
                Effect::Watch((1, 0)),
                Effect::SyncTreeWatch,
            ]
        );
    }

    #[test]
    fn rename_without_local_pause_only_retargets() {
        // 其他視窗送出的作業：監看經 Hub 暫停、恢復，這裡只套用改名
        let mut vac = Vacating::default();
        let tabs = [spot((0, 0), r"C:\a\b", true)];
        let out = vac.finish(&[], Some((r"C:\a", r"C:\n")), &tabs);
        assert_eq!(out, vec![Effect::Renamed { at: (0, 0), path: r"C:\n\b".into() }]);
    }

    #[test]
    fn gone_report_during_operation_is_ignored() {
        // 改名的作業還沒結束時，樹狀列監看的確認回報資料夾不見了（fdcae16 的競態）
        let mut vac = Vacating::default();
        vac.pause(v(&[r"C:\a"]), &[]);
        let tabs = [spot((0, 0), r"C:\a\b", false)];
        assert!(vac.folder_gone(r"C:\a", r"C:\", &tabs).is_empty());
        assert!(vac.folder_gone(r"C:\a\b", r"C:\", &tabs).is_empty());
        // 作業結束後同樣的回報就處理
        vac.finish(&v(&[r"C:\a"]), None, &tabs);
        assert_eq!(vac.folder_gone(r"C:\a", r"C:\", &tabs), vec![Effect::Gone { at: (0, 0), path: r"C:\".into() }]);
    }

    #[test]
    fn gone_moves_the_folder_and_everything_inside() {
        let vac = Vacating::default();
        let tabs = [spot((0, 0), r"C:\a", true), spot((0, 1), r"C:\a\b", true), spot((1, 0), r"C:\ab", true)];
        assert_eq!(
            vac.folder_gone(r"c:\A", r"C:\", &tabs),
            vec![Effect::Gone { at: (0, 0), path: r"C:\".into() }, Effect::Gone { at: (0, 1), path: r"C:\".into() }]
        );
    }

    #[test]
    fn check_under_skips_the_dir_itself_and_duplicates() {
        let tabs = [spot((0, 0), r"C:\a", true), spot((0, 1), r"C:\a\b", true), spot((1, 0), r"c:\A\B", true), spot((1, 1), r"C:\ab\c", true)];
        assert_eq!(Vacating::check_under(&v(&[r"C:\a"]), &tabs), vec![Effect::Check(r"C:\a\b".into())]);
    }
}
