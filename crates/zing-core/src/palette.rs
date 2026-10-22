//! 命令面板：模糊比對（內建的項目在 `builtin::palette`）。
//!
//! 查詢字串忽略空白與大小寫，依序比對標題或命令名稱中的字元（子序列）；
//! 連續命中、字首、單字開頭有加分。空查詢時最近用過的排在前面。

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
        crate::builtin::palette().map(|(cmd, title)| Entry { title: title.into(), cmd: cmd.into(), key: None }).collect()
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
        assert_eq!(e[r[2]].cmd, crate::builtin::PRESETS[0].script);
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

}
