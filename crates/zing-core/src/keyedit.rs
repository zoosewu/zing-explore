//! 快速鍵編輯器（`keys.edit`）的操作：指定、移除、還原預設，以及要寫進 `keys.toml` 的內容。
//!
//! 生效的快速鍵 = 預設設定 → `config.toml` 的 `[[key]]` → `keys.toml`（後者優先）。編輯器只改寫 `keys.toml`，
//! 內容是「與預設＋config.toml 不同的按鍵」：改成別的命令或解除（`cmd = ""`）。`config.toml` 與其中的註解不動。

use crate::config::KeyBinding;
use crate::keymap::{KeyChord, Scope};

/// 一個命令的按鍵（依設定中的順序）
pub fn chords_for(keys: &[(KeyChord, String)], cmd: &str) -> Vec<KeyChord> {
    keys.iter().filter(|(_, c)| c == cmd).map(|(k, _)| *k).collect()
}

/// 把 `chord` 指定給 `cmd`，取代 `cmd` 原本的按鍵；回傳原本使用這個按鍵的其他命令
pub fn assign(keys: &mut Vec<(KeyChord, String)>, cmd: &str, chord: KeyChord) -> Option<String> {
    let prev = keys.iter().position(|(c, _)| *c == chord).map(|i| keys.remove(i).1).filter(|p| p != cmd);
    keys.retain(|(_, c)| c != cmd);
    keys.push((chord, cmd.to_string()));
    prev
}

/// 再加一個按鍵給 `cmd`（保留原本的按鍵）；回傳原本使用這個按鍵的其他命令
pub fn add(keys: &mut Vec<(KeyChord, String)>, cmd: &str, chord: KeyChord) -> Option<String> {
    let prev = keys.iter().position(|(c, _)| *c == chord).map(|i| keys.remove(i).1).filter(|p| p != cmd);
    keys.push((chord, cmd.to_string()));
    prev
}

/// 移除 `cmd` 的一個按鍵
pub fn remove(keys: &mut Vec<(KeyChord, String)>, cmd: &str, chord: KeyChord) -> bool {
    let n = keys.len();
    keys.retain(|(c, x)| !(*c == chord && x == cmd));
    keys.len() != n
}

/// 移除 `cmd` 的所有按鍵；回傳有沒有移除
pub fn clear(keys: &mut Vec<(KeyChord, String)>, cmd: &str) -> bool {
    let n = keys.len();
    keys.retain(|(_, c)| c != cmd);
    keys.len() != n
}

/// 還原 `cmd` 的預設按鍵（`defaults` = 預設設定＋config.toml）。預設的按鍵已被其他命令使用時不搶回來，
/// 回傳這些按鍵
pub fn reset(keys: &mut Vec<(KeyChord, String)>, defaults: &[(KeyChord, String)], cmd: &str) -> Vec<KeyChord> {
    keys.retain(|(_, c)| c != cmd);
    let mut taken = Vec::new();
    for k in chords_for(defaults, cmd) {
        if keys.iter().any(|(c, _)| *c == k) {
            taken.push(k);
        } else {
            keys.push((k, cmd.to_string()));
        }
    }
    taken
}

/// 與 `defaults` 不同的按鍵：寫進 `keys.toml` 的內容（依情境、按鍵排序，結果固定）
pub fn overrides(defaults: &[(KeyChord, String)], keys: &[(KeyChord, String)]) -> Vec<KeyBinding> {
    let find = |list: &[(KeyChord, String)], k: KeyChord| list.iter().find(|(c, _)| *c == k).map(|(_, cmd)| cmd.clone());
    let mut chords: Vec<KeyChord> = defaults.iter().chain(keys).map(|(k, _)| *k).collect();
    chords.sort_by_key(|k| (k.when, k.to_string()));
    chords.dedup();
    chords
        .into_iter()
        .filter_map(|k| {
            let now = find(keys, k);
            let when = (k.when != Scope::Global).then(|| k.when.name().to_string());
            (now != find(defaults, k)).then(|| KeyBinding { key: k.to_string(), cmd: now.unwrap_or_default(), when })
        })
        .collect()
}

/// `keys.toml` 的內容
pub fn to_toml(bindings: &[KeyBinding]) -> String {
    let mut s = String::from(
        "# 由快速鍵編輯器（命令面板「編輯快速鍵」，keys.edit）產生，優先於 config.toml 的 [[key]]。\n\
         # cmd 為空字串表示解除這個按鍵。\n",
    );
    for b in bindings {
        s.push_str(&format!("\n[[key]]\nkey = {}\ncmd = {}\n", quote(&b.key), quote(&b.cmd)));
        if let Some(w) = &b.when {
            s.push_str(&format!("when = {}\n", quote(w)));
        }
    }
    s
}

fn quote(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_remove_one() {
        let k = |s: &str| KeyChord::parse(s).unwrap();
        let mut keys = vec![(k("F5"), "nav.refresh".to_string()), (k("Ctrl+C"), "file.copy".to_string())];
        assert_eq!(add(&mut keys, "nav.refresh", k("Ctrl+F9")), None);
        assert_eq!(chords_for(&keys, "nav.refresh"), vec![k("F5"), k("Ctrl+F9")]);
        // 搶走其他命令的按鍵
        assert_eq!(add(&mut keys, "nav.refresh", k("Ctrl+C")).as_deref(), Some("file.copy"));
        assert!(chords_for(&keys, "file.copy").is_empty());
        assert!(remove(&mut keys, "nav.refresh", k("F5")));
        assert!(!remove(&mut keys, "nav.refresh", k("F5")));
        assert_eq!(chords_for(&keys, "nav.refresh"), vec![k("Ctrl+F9"), k("Ctrl+C")]);
    }
    use crate::config::Config;

    fn k(s: &str) -> KeyChord {
        KeyChord::parse(s).unwrap()
    }

    fn list(v: &[(&str, &str)]) -> Vec<(KeyChord, String)> {
        v.iter().map(|&(a, b)| (k(a), b.to_string())).collect()
    }

    #[test]
    fn assign_clear_reset() {
        let defaults = list(&[("Ctrl+C", "file.copy"), ("Ctrl+Insert", "file.copy"), ("F5", "nav.refresh")]);
        let mut keys = defaults.clone();
        // 指定已被使用的按鍵：從原本的命令移除，並取代 file.copy 原本的兩個按鍵
        assert_eq!(assign(&mut keys, "file.copy", k("F5")).as_deref(), Some("nav.refresh"));
        assert_eq!(chords_for(&keys, "file.copy"), [k("F5")]);
        assert!(chords_for(&keys, "nav.refresh").is_empty());
        assert_eq!(assign(&mut keys, "file.copy", k("F5")), None);
        // 還原 nav.refresh：F5 已被 file.copy 使用，不搶回來
        assert_eq!(reset(&mut keys, &defaults, "nav.refresh"), [k("F5")]);
        assert!(chords_for(&keys, "nav.refresh").is_empty());
        assert!(reset(&mut keys, &defaults, "file.copy").is_empty());
        assert_eq!(chords_for(&keys, "file.copy"), [k("Ctrl+C"), k("Ctrl+Insert")]);
        assert_eq!(reset(&mut keys, &defaults, "nav.refresh"), []);
        assert!(overrides(&defaults, &keys).is_empty());
        assert!(clear(&mut keys, "file.copy"));
        assert!(!clear(&mut keys, "file.copy"));
    }

    #[test]
    fn scoped_overrides_round_trip() {
        let (base, _) = Config::load(None);
        let mut keys = base.config_keys.clone();
        let down = k("Down").in_scope(Scope::List);
        // 只改清單的 Down，樹狀列與彈出清單的不受影響
        assert_eq!(assign(&mut keys, "cursor.down", k("J").in_scope(Scope::List)), None);
        assert!(chords_for(&keys, "cursor.down").iter().all(|c| c.when == Scope::List));
        assert!(!keys.iter().any(|(c, _)| *c == down));
        let o = overrides(&base.config_keys, &keys);
        let pairs: Vec<(&str, &str, Option<&str>)> = o.iter().map(|b| (b.key.as_str(), b.cmd.as_str(), b.when.as_deref())).collect();
        assert_eq!(pairs, [("Down", "", Some("list")), ("J", "cursor.down", Some("list"))]);
        let (cfg, warn) = Config::load_with_keys(None, Some(&to_toml(&o)));
        assert!(warn.is_empty(), "{warn:?}");
        assert_eq!(cfg.command_in(Scope::List, k("J")), Some("cursor.down"));
        assert_eq!(cfg.command_in(Scope::List, k("Down")), None);
        assert_eq!(cfg.command_in(Scope::Tree, k("Down")), Some("tree.down"));
        assert_eq!(cfg.command_in(Scope::Global, k("J")), None);
    }

    #[test]
    fn overrides_round_trip() {
        let defaults = list(&[("Ctrl+C", "file.copy"), ("F5", "nav.refresh"), ("F2", "file.rename")]);
        let mut keys = defaults.clone();
        assign(&mut keys, "nav.refresh", k("Ctrl+R"));
        clear(&mut keys, "file.rename");
        let o = overrides(&defaults, &keys);
        let pairs: Vec<(&str, &str)> = o.iter().map(|b| (b.key.as_str(), b.cmd.as_str())).collect();
        assert_eq!(pairs, [("Ctrl+R", "nav.refresh"), ("F2", ""), ("F5", "")]);
        // 寫出的 keys.toml 讀回後得到同樣的按鍵
        let (cfg, warn) = Config::load_with_keys(None, Some(&to_toml(&o)));
        assert!(warn.is_empty(), "{warn:?}");
        assert_eq!(cfg.command_for(k("Ctrl+R")), Some("nav.refresh"));
        assert_eq!(cfg.command_for(k("F5")), None);
        assert_eq!(cfg.command_for(k("F2")), None);
        assert_eq!(cfg.command_for(k("Ctrl+C")), Some("file.copy"));
        // 引號與反斜線
        let s = to_toml(&[KeyBinding { key: "Ctrl+Q".into(), cmd: r#"run "C:\a b\x.exe""#.into(), when: None }]);
        let (cfg, _) = Config::load_with_keys(None, Some(&s));
        assert_eq!(cfg.command_for(k("Ctrl+Q")), Some(r#"run "C:\a b\x.exe""#));
    }
}
