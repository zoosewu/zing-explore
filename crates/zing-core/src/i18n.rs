//! 介面文字的翻譯。原文（繁體中文）就是鍵，翻譯在 `i18n/en.toml`、`i18n/ja.toml`（`"原文" = "翻譯"`），
//! 由 build.rs 編譯成依原文排序的靜態表，查詢時二分搜尋：啟動時不必解析檔案，繁體中文直接回傳原文。
//!
//! - `tr!("原文")` 回傳 `&'static str`；`tr!("已加入：{p}", p)`、`tr!("{n} 個", n = list.len())` 代入參數，回傳 `String`。
//!   參數以名稱代入，翻譯可以調整順序；不支援 `{:.1}` 這類格式，請先格式化成字串。
//! - 常數表裡的原文（命令面板的標題、Lua API 的說明…）用 `tr_key!("…")` 標記，使用時再 `tr()`。
//! - `cargo test -p zing-core i18n` 檢查程式中所有 `tr!`／`tr_key!`／`tr(` 的原文在每個翻譯檔都有、沒有多餘的條目、
//!   參數名稱一致；`ZING_I18N_MISSING=1` 時把缺少的條目印出來（可直接貼進翻譯檔）。
//! - 語言：設定檔的 `language`（`auto` 跟隨 Windows 的顯示語言），程式啟動時以 `set` 設定。

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    ZhTw,
    En,
    Ja,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::ZhTw, Lang::En, Lang::Ja];

    /// 設定檔中的寫法；`auto`（或空白）跟隨 Windows 的顯示語言，無法辨識時回傳 None
    pub fn parse(s: &str) -> Option<Lang> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "" | "auto" | "system" => Some(system()),
            "zh-tw" | "zh-hant" | "zh" | "tw" => Some(Lang::ZhTw),
            "en" | "en-us" | "en-gb" | "english" => Some(Lang::En),
            "ja" | "ja-jp" | "jp" | "japanese" => Some(Lang::Ja),
            _ => None,
        }
    }

    /// 文件檔名的後綴（`README.en.md`）；繁體中文為空字串
    pub fn suffix(self) -> &'static str {
        match self {
            Lang::ZhTw => "",
            Lang::En => ".en",
            Lang::Ja => ".ja",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::ZhTw => "zh-TW",
            Lang::En => "en",
            Lang::Ja => "ja",
        }
    }
}

/// Windows 的顯示語言：中文（含簡體）→ 繁體中文，日文 → 日文，其他 → 英文
pub fn system() -> Lang {
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    match id & 0x3FF {
        0x04 => Lang::ZhTw,
        0x11 => Lang::Ja,
        _ => Lang::En,
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::En,
        2 => Lang::Ja,
        _ => Lang::ZhTw,
    }
}

include!(concat!(env!("OUT_DIR"), "/i18n_tables.rs"));

fn table(lang: Lang) -> &'static [(&'static str, &'static str)] {
    match lang {
        Lang::ZhTw => &[],
        Lang::En => EN,
        Lang::Ja => JA,
    }
}

/// 翻譯成目前的語言；沒有翻譯時回傳原文
pub fn tr(s: &'static str) -> &'static str {
    tr_in(lang(), s)
}

pub fn tr_in(lang: Lang, s: &'static str) -> &'static str {
    lookup(lang, s).unwrap_or(s)
}

/// 非 'static 的原文（例如由常數表複製來的 String）
pub fn tr_str(s: &str) -> std::borrow::Cow<'_, str> {
    match lookup(lang(), s) {
        Some(t) => t.into(),
        None => s.into(),
    }
}

fn lookup(lang: Lang, s: &str) -> Option<&'static str> {
    let t = table(lang);
    t.binary_search_by(|(k, _)| (*k).cmp(s)).ok().map(|i| t[i].1)
}

/// 代入 `{名稱}` 參數；`{{`、`}}` 為大括號本身
pub fn fill(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let Some(end) = tail.find('}').filter(|_| tail.starts_with('{')) {
            let name = &tail[1..end];
            match args.iter().find(|(k, _)| *k == name) {
                Some((_, v)) => out.push_str(&v.to_string()),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        } else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// `tr!("原文")`、`tr!("…{a}…{b}…", a, b = expr)`：見模組說明
#[macro_export]
macro_rules! tr {
    ($s:literal) => {
        $crate::i18n::tr($s)
    };
    ($s:literal, $($k:ident $(= $v:expr)?),+ $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($s), &[$((stringify!($k), &$crate::__tr_arg!($k $(= $v)?) as &dyn ::std::fmt::Display)),+])
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __tr_arg {
    ($k:ident) => {
        $k
    };
    ($k:ident = $v:expr) => {
        $v
    };
}

/// 標記常數表中的原文（不翻譯，使用時再 `tr()`）：讓完整性檢查找得到
#[macro_export]
macro_rules! tr_key {
    ($s:literal) => {
        $s
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_args() {
        assert_eq!(fill("已加入：{p}（{n} 個）", &[("p", &"C:\\a"), ("n", &3)]), "已加入：C:\\a（3 個）");
        assert_eq!(fill("{{x}} {y}", &[]), "{x} {y}");
        let p = "D:\\b";
        assert_eq!(crate::tr!("測試 {p} {n}", p, n = 1 + 1), "測試 D:\\b 2");
    }

    #[test]
    fn tables_sorted_and_lookup() {
        for l in [Lang::En, Lang::Ja] {
            let t = table(l);
            assert!(t.windows(2).all(|w| w[0].0 < w[1].0), "{l:?} 的表沒有排序或有重複");
        }
        assert_eq!(tr_in(Lang::ZhTw, "重新整理"), "重新整理");
        assert_ne!(tr_in(Lang::En, "重新整理"), "重新整理");
        assert_eq!(tr_in(Lang::En, "不存在的原文-zing"), "不存在的原文-zing");
    }

    /// 程式中所有的原文（`tr!(`、`tr_key!(`、`tr(`、Lua 的 `__tr(`）
    fn source_keys() -> std::collections::BTreeMap<String, String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let re = regex::Regex::new(r#"\b(?:tr!|tr_key!|tr|__tr)\(\s*"((?:[^"\\]|\\.)*)""#).unwrap();
        let mut keys = std::collections::BTreeMap::new();
        let mut stack = vec![root.join("zing-core/src"), root.join("zing-app/src"), root.join("zing-index/src")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                if !matches!(ext, "rs" | "lua") {
                    continue;
                }
                // 略過註解（說明文字裡的範例）
                let src: String = std::fs::read_to_string(&p)
                    .unwrap()
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//") && !l.trim_start().starts_with("--"))
                    .collect::<Vec<_>>()
                    .join("\n");
                for c in re.captures_iter(&src) {
                    let raw = &c[1];
                    keys.insert(unescape(raw), p.file_name().unwrap().to_string_lossy().to_string());
                }
            }
        }
        // 這個檔案自己的測試字串
        keys.retain(|k, f| !(f == "i18n.rs" && (k.starts_with("測試") || k.contains("不存在的原文"))));
        keys
    }

    fn unescape(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars();
        while let Some(c) = it.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('\'') => out.push('\''),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        }
        out
    }

    fn placeholders(s: &str) -> Vec<String> {
        let re = regex::Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();
        // `${名稱}` 是批次重新命名的語法（引用群組），不是參數
        let s = regex::Regex::new(r"\$\{[^}]*\}").unwrap().replace_all(s, "");
        let mut v: Vec<String> = re.captures_iter(&s.replace("{{", "").replace("}}", "")).map(|c| c[1].to_string()).collect();
        v.sort();
        v.dedup();
        v
    }

    #[test]
    fn catalogs_complete() {
        let keys = source_keys();
        let mut problems = Vec::new();
        let print = std::env::var_os("ZING_I18N_MISSING").is_some();
        for l in [Lang::En, Lang::Ja] {
            let t = table(l);
            let mut missing = Vec::new();
            for (k, file) in &keys {
                match t.binary_search_by(|(x, _)| (*x).cmp(k)) {
                    Ok(i) => {
                        if placeholders(k) != placeholders(t[i].1) {
                            problems.push(format!("{l:?}：參數不一致：{k} → {}", t[i].1));
                        }
                    }
                    Err(_) => missing.push((k, file)),
                }
            }
            for (k, _) in t {
                if !keys.contains_key(*k) {
                    problems.push(format!("{l:?}：程式中已沒有這個原文：{k}"));
                }
            }
            if print && !missing.is_empty() {
                println!("# ---- {l:?} 缺少 {} 個", missing.len());
                for (k, f) in &missing {
                    println!("{} = \"\"  # {f}", toml::Value::String((*k).clone()));
                }
            }
            problems.extend(missing.iter().map(|(k, f)| format!("{l:?}：缺少翻譯（{f}）：{k}")));
        }
        assert!(problems.is_empty(), "{} 個問題：\n{}", problems.len(), problems.join("\n"));
    }
}
