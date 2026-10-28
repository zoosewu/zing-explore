//! 命令列（工具列下方的按鈕列，Windows 11 檔案總管的樣式）：按鈕清單、下拉選單的內容、圖示。
//!
//! - 按鈕清單存在程式旁的 `toolbar.toml`（命令列的自訂面板寫的檔案）；沒有這個檔案時用 `defaults()`，
//!   再加上 config.toml 的 `[[button]]` 中命令列與選單裡還沒有的命令（舊版的工具列按鈕）。
//! - 特殊的命令：`-` 分隔線；`menu.new`、`menu.sort`、`menu.view`、`menu.more` 開啟下拉選單。
//! - 圖示：空字串用命令的預設圖示；`glyph:名稱` 是 `GLYPHS` 中的 Fluent 圖示；其他視為圖片檔的路徑。

use serde::{Deserialize, Serialize};

pub const SEPARATOR: &str = "-";
pub const MENU_NEW: &str = "menu.new";
pub const MENU_SORT: &str = "menu.sort";
pub const MENU_VIEW: &str = "menu.view";
pub const MENU_MORE: &str = "menu.more";
/// 在終端機開啟目前的資料夾（舊版預設工具列的「終端機」按鈕）
pub use crate::builtin::TERMINAL;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub cmd: String,
    /// 空字串：用命令的名稱（`label_for`）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// 空字串：用命令的預設圖示；`glyph:名稱` 或圖片檔的路徑
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
}

impl Item {
    pub fn new(cmd: &str) -> Self {
        Item { cmd: cmd.into(), label: String::new(), icon: String::new() }
    }

    pub fn is_separator(&self) -> bool {
        self.cmd == SEPARATOR
    }

    pub fn is_menu(&self) -> bool {
        matches!(self.cmd.as_str(), MENU_NEW | MENU_SORT | MENU_VIEW | MENU_MORE)
    }

    /// 顯示的名稱
    pub fn title(&self) -> String {
        if self.label.is_empty() { label_for(&self.cmd).to_string() } else { crate::i18n::tr_str(&self.label).into_owned() }
    }

    /// 圖示：Fluent 字碼或圖片路徑
    pub fn icon(&self) -> Icon<'_> {
        if self.icon.is_empty() {
            return default_icon(&self.cmd).map_or(Icon::None, Icon::Glyph);
        }
        match self.icon.strip_prefix("glyph:") {
            Some(name) => glyph(name).map_or(Icon::None, Icon::Glyph),
            None => Icon::Image(&self.icon),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon<'a> {
    None,
    /// Segoe Fluent Icons 的字元
    Glyph(&'static str),
    Image(&'a str),
}

pub fn defaults() -> Vec<Item> {
    [
        MENU_NEW,
        SEPARATOR,
        "file.cut",
        "file.copy",
        "file.paste",
        "file.rename",
        "file.delete",
        SEPARATOR,
        MENU_SORT,
        MENU_VIEW,
        SEPARATOR,
        MENU_MORE,
    ]
    .into_iter()
    .map(Item::new)
    .collect()
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    item: Vec<Item>,
}

/// 讀取 `toolbar.toml`（None：沒有這個檔案）。`buttons`：config.toml 的 `[[button]]`
pub fn load(src: Option<&str>, buttons: &[crate::config::Button]) -> (Vec<Item>, Option<String>) {
    match src.map(toml::from_str::<File>) {
        Some(Ok(f)) => (f.item, None),
        Some(Err(e)) => (with_buttons(defaults(), buttons), Some(tr!("toolbar.toml 格式錯誤，已改用預設的命令列：{e}", e))),
        None => (with_buttons(defaults(), buttons), None),
    }
}

/// 舊版工具列的按鈕：命令列與選單裡還沒有的才加在「⋯」前面
fn with_buttons(mut items: Vec<Item>, buttons: &[crate::config::Button]) -> Vec<Item> {
    for b in buttons {
        if b.cmd.trim().is_empty() || items.iter().any(|i| i.cmd == b.cmd) || in_menus(&b.cmd) {
            continue;
        }
        let at = items.iter().position(|i| i.cmd == MENU_MORE).unwrap_or(items.len());
        items.insert(at, Item { cmd: b.cmd.clone(), label: b.label.clone(), icon: String::new() });
    }
    items
}

pub fn to_toml(items: &[Item]) -> String {
    let body = toml::to_string(&File { item: items.to_vec() }).unwrap_or_default();
    format!("{}\n{body}", tr!("# zing-explore 的命令列（由「自訂命令列」寫入）。刪除這個檔案即還原預設。"))
}

/// 下拉選單中的一項
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Entry {
    Cmd(&'static str),
    Separator,
    /// 子選單：（名稱, 內容）
    Sub(&'static str, &'static [Entry]),
}

use Entry::{Cmd, Separator as Sep, Sub};

pub const NEW_MENU: &[Entry] = &[Cmd("file.newfolder"), Cmd("file.newfile")];
pub const SORT_MENU: &[Entry] = &[
    Cmd("view.sort --by=name"),
    Cmd("view.sort --by=modified"),
    Cmd("view.sort --by=type"),
    Cmd("view.sort --by=size"),
    Sep,
    Cmd("view.sort --asc"),
    Cmd("view.sort --desc"),
];
pub const VIEW_MENU: &[Entry] = &[
    Cmd("view.mode list"),
    Cmd("view.mode small"),
    Cmd("view.mode medium"),
    Cmd("view.mode large"),
    Sep,
    Cmd("view.preview"),
    Cmd("tree.toggle"),
    Cmd("view.togglehidden"),
    Sep,
    Cmd("view.foldersizes"),
];
pub const MORE_MENU: &[Entry] = &[
    Sub(
        tr_key!("檔案"),
        &[
            Cmd("file.batchrename"),
            Cmd("file.delete --permanent"),
            Cmd("file.copypath"),
            Cmd("file.properties"),
            Sep,
            Cmd("thumb.set"),
            Cmd("thumb.clear"),
            Sep,
            Cmd(TERMINAL),
        ],
    ),
    Sub(tr_key!("選取"), &[Cmd("sel.all"), Cmd("sel.none"), Cmd("sel.invert")]),
    Sub(
        tr_key!("分頁與窗格"),
        &[
            Cmd("tab.new"),
            Cmd("tab.close"),
            Sep,
            Cmd("pane.split"),
            Cmd("pane.close"),
            Cmd("pane.focusnext"),
            Sep,
            Cmd("copy --to={dest}"),
            Cmd("copy --move --to={dest}"),
        ],
    ),
    Sub(tr_key!("我的最愛"), &[Cmd("favorites.add"), Cmd("favorites.remove")]),
    Sub(
        tr_key!("工具"),
        &[Cmd("search.open"), Cmd("view.foldersizes"), Sep, Cmd("jobs.toggle"), Cmd("jobs.clear"), Sep, Cmd("index.status")],
    ),
    Sub(tr_key!("外觀"), &[Cmd("theme.toggle"), Cmd("theme.edit"), Sep, Cmd("theme.import"), Cmd("theme.export")]),
    Sep,
    Cmd("cmdbar.edit"),
    Cmd("keys.edit"),
    Cmd("settings.open"),
    Cmd("update.check"),
    Sep,
    Cmd("app.exit"),
];

pub fn menu(cmd: &str) -> &'static [Entry] {
    match cmd {
        MENU_NEW => NEW_MENU,
        MENU_SORT => SORT_MENU,
        MENU_VIEW => VIEW_MENU,
        _ => MORE_MENU,
    }
}

/// 命令是否已出現在某個下拉選單中
pub fn in_menus(cmd: &str) -> bool {
    fn walk(list: &[Entry], cmd: &str) -> bool {
        list.iter().any(|e| match e {
            Cmd(c) => *c == cmd,
            Sub(_, sub) => walk(sub, cmd),
            Sep => false,
        })
    }
    [NEW_MENU, SORT_MENU, VIEW_MENU, MORE_MENU].iter().any(|m| walk(m, cmd))
}

/// 下拉選單按鈕的名稱（命令的短名稱在 `builtin::PRESETS`）
const LABELS: &[(&str, &str)] = &[
    (MENU_NEW, tr_key!("新增")),
    (MENU_SORT, tr_key!("排序")),
    (MENU_VIEW, tr_key!("檢視")),
    (MENU_MORE, tr_key!("更多")),
];

/// 命令的顯示名稱：短名稱、命令面板的標題，都沒有就是命令本身
pub fn label_for(cmd: &str) -> &str {
    if let Some((_, l)) = LABELS.iter().find(|(c, _)| *c == cmd) {
        return crate::i18n::tr(l);
    }
    crate::builtin::preset(cmd).and_then(|p| p.short.or(p.title)).map_or(cmd, |t| crate::i18n::tr(t))
}

/// Fluent 圖示（自訂按鈕時可選）：（名稱, 字元）
pub const GLYPHS: &[(&str, &str)] = &[
    ("add", "\u{E710}"),
    ("folder", "\u{E8B7}"),
    ("new-folder", "\u{E8F4}"),
    ("document", "\u{E8A5}"),
    ("cut", "\u{E8C6}"),
    ("copy", "\u{E8C8}"),
    ("paste", "\u{E77F}"),
    ("rename", "\u{E8AC}"),
    ("delete", "\u{E74D}"),
    ("sort", "\u{E8CB}"),
    ("view", "\u{E8A9}"),
    ("more", "\u{E712}"),
    ("terminal", "\u{E756}"),
    ("settings", "\u{E713}"),
    ("search", "\u{E721}"),
    ("star", "\u{E734}"),
    ("heart", "\u{EB51}"),
    ("home", "\u{E80F}"),
    ("download", "\u{E896}"),
    ("upload", "\u{E898}"),
    ("sync", "\u{E895}"),
    ("share", "\u{E72D}"),
    ("link", "\u{E71B}"),
    ("mail", "\u{E715}"),
    ("photo", "\u{EB9F}"),
    ("video", "\u{E714}"),
    ("music", "\u{E8D6}"),
    ("zip", "\u{F012}"),
    ("code", "\u{E943}"),
    ("play", "\u{E768}"),
    ("tag", "\u{E8EC}"),
    ("lock", "\u{E72E}"),
    ("cloud", "\u{E753}"),
    ("globe", "\u{E774}"),
    ("edit", "\u{E70F}"),
    ("info", "\u{E946}"),
    ("properties", "\u{E90F}"),
    ("select-all", "\u{E8B3}"),
    ("split", "\u{E89F}"),
    ("preview", "\u{E8FF}"),
    ("tree", "\u{E8A4}"),
    ("hidden", "\u{E7B3}"),
    ("size", "\u{E9F5}"),
    ("tab", "\u{EC6C}"),
    ("lightning", "\u{E945}"),
    ("script", "\u{E9D5}"),
];

pub fn glyph(name: &str) -> Option<&'static str> {
    GLYPHS.iter().find(|(n, _)| *n == name).map(|(_, g)| *g)
}

/// 命令的預設圖示
pub fn default_icon(cmd: &str) -> Option<&'static str> {
    let name = match cmd {
        MENU_NEW => "add",
        MENU_SORT => "sort",
        MENU_VIEW => "view",
        MENU_MORE => "more",
        _ => crate::builtin::Cmd::of_script(cmd)?.icon()?,
    };
    glyph(name)
}

pub use crate::builtin::group_of;

/// 自訂面板「可加入的命令」的分組：（分組名稱, 命令）。腳本的命令由程式另外加上
pub fn catalog() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut out: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    let cmds = [MENU_NEW, MENU_SORT, MENU_VIEW, MENU_MORE, "file.newfile", TERMINAL, "cmdbar.edit"]
        .into_iter()
        .chain(crate::builtin::palette().map(|(c, _)| c));
    for cmd in cmds {
        let g = if cmd.starts_with("menu.") && crate::builtin::Cmd::of_script(cmd).is_none() { tr_key!("選單") } else { group_of(cmd) };
        match out.iter_mut().find(|(n, _)| *n == g) {
            Some((_, v)) => {
                if !v.contains(&cmd) {
                    v.push(cmd)
                }
            }
            None => out.push((g, vec![cmd])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Button;

    #[test]
    fn defaults_and_old_buttons() {
        let old = [
            Button { label: "新資料夾".into(), cmd: "file.newfolder".into() },
            Button { label: "終端機".into(), cmd: TERMINAL.into() },
            Button { label: "記事本".into(), cmd: "run notepad.exe".into() },
        ];
        let (items, w) = load(None, &old);
        assert!(w.is_none());
        // 新資料夾在「新增」選單、終端機在「⋯」裡：不重複；其他的加在「⋯」前面
        let cmds: Vec<&str> = items.iter().map(|i| i.cmd.as_str()).collect();
        assert_eq!(cmds[cmds.len() - 2..], ["run notepad.exe", MENU_MORE]);
        assert_eq!(items.len(), defaults().len() + 1);
        assert_eq!(items[items.len() - 2].title(), "記事本");
    }

    #[test]
    fn roundtrip() {
        let mut items = defaults();
        items.push(Item { cmd: "run x.exe".into(), label: "X".into(), icon: "glyph:star".into() });
        items.push(Item { cmd: "run y.exe".into(), label: "Y".into(), icon: r"C:\圖示\y.png".into() });
        let text = to_toml(&items);
        let (back, w) = load(Some(&text), &[]);
        assert!(w.is_none());
        assert_eq!(back, items);
        assert_eq!(back[back.len() - 2].icon(), Icon::Glyph("\u{E734}"));
        assert_eq!(back[back.len() - 1].icon(), Icon::Image(r"C:\圖示\y.png"));
        let (_, w) = load(Some("item = 3"), &[]);
        assert!(w.is_some());
    }

    #[test]
    fn labels_and_icons() {
        assert_eq!(Item::new("file.copy").title(), "複製");
        assert_eq!(Item::new(MENU_SORT).title(), "排序");
        assert_eq!(Item::new("unknown.cmd").title(), "unknown.cmd");
        for i in defaults().iter().filter(|i| !i.is_separator()) {
            assert_ne!(i.icon(), Icon::None, "{}", i.cmd);
        }
        // 每個選單項目都有名稱（不是命令本身）
        fn walk(list: &[Entry]) {
            for e in list {
                match e {
                    Cmd(c) => assert_ne!(label_for(c), *c, "{c} 沒有名稱"),
                    Sub(_, s) => walk(s),
                    Sep => {}
                }
            }
        }
        for m in [NEW_MENU, SORT_MENU, VIEW_MENU, MORE_MENU] {
            walk(m);
        }
        assert!(in_menus("sel.invert") && !in_menus("nav.back"));
        let groups = catalog();
        assert!(groups.iter().any(|(_, v)| v.contains(&"file.copy")));
    }
}
