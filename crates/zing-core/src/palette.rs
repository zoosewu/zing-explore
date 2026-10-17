//! 命令面板：內建命令清單與模糊比對。
//!
//! 查詢字串忽略空白與大小寫，依序比對標題或命令名稱中的字元（子序列）；
//! 連續命中、字首、單字開頭有加分。空查詢時最近用過的排在前面。

/// 內建命令（命令腳本, 標題）。只列不需要額外輸入就能執行的命令。
pub const BUILTIN: &[(&str, &str)] = &[
    ("nav.back", tr_key!("上一頁")),
    ("nav.forward", tr_key!("下一頁")),
    ("nav.up", tr_key!("上一層")),
    ("nav.open", tr_key!("開啟")),
    ("nav.refresh", tr_key!("重新整理")),
    ("nav.home", tr_key!("前往使用者資料夾")),
    ("address.edit", tr_key!("編輯網址列")),
    ("file.copy", tr_key!("複製")),
    ("file.cut", tr_key!("剪下")),
    ("file.paste", tr_key!("貼上")),
    ("file.delete", tr_key!("刪除（移到資源回收筒）")),
    ("file.delete --permanent", tr_key!("永久刪除")),
    ("file.rename", tr_key!("重新命名")),
    ("file.newfolder", tr_key!("新增資料夾")),
    ("file.newfile", tr_key!("新增文字文件")),
    ("file.properties", tr_key!("內容")),
    ("file.copypath", tr_key!("複製路徑")),
    ("copy --to={dest}", tr_key!("複製到另一個窗格")),
    ("copy --move --to={dest}", tr_key!("移動到另一個窗格")),
    ("menu.context", tr_key!("顯示右鍵選單")),
    ("sel.all", tr_key!("全選")),
    ("sel.none", tr_key!("取消選取")),
    ("sel.invert", tr_key!("反向選取")),
    ("tab.new", tr_key!("新增分頁")),
    ("tab.close", tr_key!("關閉分頁")),
    ("tab.next", tr_key!("下一個分頁")),
    ("tab.prev", tr_key!("上一個分頁")),
    ("pane.split", tr_key!("分割窗格")),
    ("pane.close", tr_key!("關閉窗格")),
    ("pane.focusnext", tr_key!("切換到下一個窗格")),
    ("tree.toggle", tr_key!("顯示／隱藏樹狀列")),
    ("view.sort --by=name", tr_key!("依名稱排序")),
    ("view.sort --by=modified", tr_key!("依修改日期排序")),
    ("view.sort --by=type", tr_key!("依類型排序")),
    ("view.sort --by=size", tr_key!("依大小排序")),
    ("view.togglehidden", tr_key!("顯示／隱藏隱藏檔案")),
    ("view.foldersizes", tr_key!("計算資料夾大小")),
    ("view.preview", tr_key!("顯示／隱藏預覽窗格")),
    ("file.batchrename", tr_key!("批次重新命名（選取多個項目時 F2）")),
    ("view.mode list", tr_key!("檢視：清單")),
    ("view.mode small", tr_key!("檢視：小縮圖")),
    ("view.mode medium", tr_key!("檢視：中縮圖")),
    ("view.mode large", tr_key!("檢視：大縮圖")),
    ("thumb.set", tr_key!("設定縮圖…（目前的項目）")),
    ("thumb.clear", tr_key!("移除自訂縮圖（目前的項目）")),
    ("index.install", tr_key!("啟用 NTFS 索引服務（立即顯示資料夾大小，需要系統管理員權限）")),
    ("index.uninstall", tr_key!("移除 NTFS 索引服務")),
    ("index.status", tr_key!("NTFS 索引服務狀態")),
    ("search.open", tr_key!("全磁碟搜尋（NTFS 索引）")),
    ("filter.clear", tr_key!("清除篩選")),
    ("jobs.toggle", tr_key!("顯示／隱藏檔案作業")),
    ("jobs.clear", tr_key!("清除已完成的檔案作業")),
    ("favorites.add", tr_key!("加入我的最愛")),
    ("favorites.remove", tr_key!("從我的最愛移除")),
    ("favorites.move --by=-1", tr_key!("我的最愛：上移（樹狀列 Ctrl+↑）")),
    ("favorites.move --by=1", tr_key!("我的最愛：下移（樹狀列 Ctrl+↓）")),
    ("theme.toggle", tr_key!("切換深色／淺色主題")),
    ("theme.set system", tr_key!("主題：緊湊（預設，跟隨系統深淺色）")),
    ("theme.set dark", tr_key!("主題：緊湊深色")),
    ("theme.set light", tr_key!("主題：緊湊淺色")),
    ("theme.set fluent", tr_key!("主題：Fluent（寬鬆圓角，跟隨系統深淺色）")),
    ("theme.set balanced", tr_key!("主題：折衷（Fluent 外觀、中等密度，跟隨系統深淺色）")),
    ("theme.edit", tr_key!("編輯目前的主題")),
    ("theme.editfile", tr_key!("用文字編輯器開啟目前的主題檔（存檔後自動套用）")),
    ("theme.import", tr_key!("匯入主題…")),
    ("theme.export", tr_key!("匯出目前的主題…")),
    ("settings.open", tr_key!("設定")),
    ("cmdbar.edit", tr_key!("自訂命令列")),
    ("cmdbar.hide", tr_key!("隱藏命令列")),
    ("run wt.exe -d {path}", tr_key!("在終端機開啟目前的資料夾")),
    ("keys.edit", tr_key!("編輯快速鍵")),
    ("config.edit", tr_key!("編輯設定檔")),
    ("config.reload", tr_key!("重新載入設定")),
    ("app.exit", tr_key!("結束 zing-explore")),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    /// 執行的命令腳本
    pub cmd: String,
    /// 快速鍵說明（例如 `Ctrl+Shift+N`）
    pub key: Option<String>,
}

fn chars(s: &str) -> Vec<char> {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

fn is_boundary(c: char) -> bool {
    matches!(c, ' ' | '.' | '-' | '_' | '/' | '：' | '（' | '(' | '=')
}

/// `query` 是否為 `text` 的子序列（忽略空白與大小寫）；回傳分數，越高越好
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q = chars(query);
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    best_score(&q, &t)
}

fn best_score(q: &[char], t: &[char]) -> Option<i32> {
    // 從每個可能的起點各做一次貪婪比對，取最高分（文字很短，成本可忽略）
    let mut best = None;
    for start in 0..t.len() {
        if t[start] != q[0] {
            continue;
        }
        let mut s = 0;
        let mut ti = start;
        let mut prev: Option<usize> = None;
        let mut ok = true;
        for &qc in q {
            while ti < t.len() && t[ti] != qc {
                ti += 1;
            }
            if ti == t.len() {
                ok = false;
                break;
            }
            s += 1;
            if prev.is_some_and(|p| p + 1 == ti) {
                s += 5;
            }
            if ti == 0 || is_boundary(t[ti - 1]) {
                s += 3;
            }
            prev = Some(ti);
            ti += 1;
        }
        if !ok {
            break;
        }
        if start == 0 {
            s += 6;
        }
        // 比對越分散扣分越多
        s -= (prev.unwrap_or(start) - start) as i32 / 4;
        best = best.max(Some(s));
    }
    best
}

/// 依查詢排序。回傳 `entries` 的索引。`recent` 為最近執行的命令（最新在前）。
pub fn filter(entries: &[Entry], query: &str, recent: &[String]) -> Vec<usize> {
    let rank = |cmd: &str| recent.iter().position(|r| r == cmd).unwrap_or(usize::MAX);
    let mut scored: Vec<(i32, usize, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let s = score(query, &e.title).max(score(query, &e.cmd))?;
            Some((s, rank(&e.cmd), i))
        })
        .collect();
    // 分數高者優先；同分時最近用過的優先，其餘維持原順序
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    scored.into_iter().map(|(_, _, i)| i).collect()
}

/// 最近使用清單：移到最前面，最多保留 `max` 筆
pub fn touch_recent(recent: &mut Vec<String>, cmd: &str, max: usize) {
    recent.retain(|r| r != cmd);
    recent.insert(0, cmd.to_string());
    recent.truncate(max);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        BUILTIN.iter().map(|&(cmd, title)| Entry { title: title.into(), cmd: cmd.into(), key: None }).collect()
    }

    fn top(query: &str, recent: &[String]) -> String {
        let e = entries();
        let r = filter(&e, query, recent);
        e[r[0]].cmd.clone()
    }

    #[test]
    fn subsequence_and_case() {
        assert!(score("tghd", "view.togglehidden").is_some());
        assert!(score("TOG HID", "view.togglehidden").is_some());
        assert!(score("xyz", "view.togglehidden").is_none());
        assert!(score("", "anything") == Some(0));
    }

    #[test]
    fn ranking() {
        assert_eq!(top("隱藏檔", &[]), "view.togglehidden");
        assert_eq!(top("新增資料夾", &[]), "file.newfolder");
        assert_eq!(top("tog hid", &[]), "view.togglehidden");
        assert_eq!(top("重新命名", &[]), "file.rename");
        // 連續命中勝過分散命中
        assert!(score("copy", "file.copy").unwrap() > score("copy", "c-o-p-y").unwrap());
        // 字首勝過中間
        assert!(score("nav", "nav.back").unwrap() > score("nav", "x.nav").unwrap());
    }

    #[test]
    fn recent_first_when_empty_or_tied() {
        let recent = vec!["config.reload".to_string(), "tab.new".to_string()];
        let e = entries();
        let r = filter(&e, "", &recent);
        assert_eq!(e[r[0]].cmd, "config.reload");
        assert_eq!(e[r[1]].cmd, "tab.new");
        assert_eq!(e[r[2]].cmd, BUILTIN[0].0);
        assert_eq!(r.len(), e.len());
    }

    #[test]
    fn touch_moves_to_front_and_caps() {
        let mut r = vec!["a".to_string(), "b".to_string()];
        touch_recent(&mut r, "b", 2);
        assert_eq!(r, ["b", "a"]);
        touch_recent(&mut r, "c", 2);
        assert_eq!(r, ["c", "b"]);
    }

    #[test]
    fn builtin_commands_are_unique() {
        let mut cmds: Vec<_> = BUILTIN.iter().map(|b| b.0).collect();
        cmds.sort();
        cmds.dedup();
        assert_eq!(cmds.len(), BUILTIN.len());
    }
}
