//! 啟動時開啟哪些分頁（設定 `startup`），以及從外部開啟的資料夾（命令列參數、其他程式轉來的路徑）。
//!
//! - `zing.exe <路徑>`：資料夾、磁碟機（設為預設的檔案總管後由 Windows 傳入），`shell:MyComputerFolder` 是「此電腦」（Win+E）。
//!   其他 `shell:`、`::{CLSID}`（控制台、資源回收筒等虛擬資料夾）交給 Windows 檔案總管。
//! - 開啟的資料夾已經在某個分頁中就切換到那個分頁，否則在作用中的窗格加開一個分頁。

use crate::config::Startup;
use crate::path;

/// 命令列參數要開啟的位置
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// zing 自己開啟（空字串是「此電腦」）
    Folder(String),
    /// 虛擬資料夾等 zing 不支援的位置：交給 explorer.exe
    Explorer(String),
}

/// 「此電腦」的 Shell 寫法（Win+E 的命令帶這個參數）
pub const THIS_PC: &str = "shell:MyComputerFolder";
const THIS_PC_CLSID: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";

/// 第一個不是 `--` 開頭的參數（`args[0]` 是程式本身）
pub fn parse_args(args: &[String]) -> Option<Target> {
    let arg = args.iter().skip(1).find(|a| !a.starts_with("--"))?;
    let a = arg.trim().trim_matches('"');
    if a.eq_ignore_ascii_case(THIS_PC) || a.eq_ignore_ascii_case(THIS_PC_CLSID) || a.eq_ignore_ascii_case(&format!("shell:{THIS_PC_CLSID}")) {
        return Some(Target::Folder(String::new()));
    }
    let lower = a.to_ascii_lowercase();
    if lower.starts_with("shell:") || a.starts_with("::") {
        return Some(Target::Explorer(a.to_string()));
    }
    Some(Target::Folder(path::normalize(a)))
}

/// 分頁在設定檔（`start_tabs`）中的寫法：「此電腦」寫成 `shell:MyComputerFolder`
pub fn tab_value(path: &str) -> String {
    if path.is_empty() { THIS_PC.to_string() } else { path.to_string() }
}

/// `tab_value` 的反向
pub fn tab_path(value: &str) -> String {
    if value.trim().eq_ignore_ascii_case(THIS_PC) { String::new() } else { path::normalize(value) }
}

/// 一個窗格的分頁（路徑）與作用中的分頁
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneTabs {
    pub tabs: Vec<String>,
    pub active: usize,
}

/// 開著 `target` 的分頁（窗格, 分頁），不分大小寫
pub fn find<'a, P, T>(panes: P, target: &str) -> Option<(usize, usize)>
where
    P: IntoIterator<Item = T>,
    T: IntoIterator<Item = &'a str>,
{
    panes.into_iter().enumerate().find_map(|(p, tabs)| tabs.into_iter().position(|t| t.eq_ignore_ascii_case(target)).map(|t| (p, t)))
}

/// 啟動時的窗格與作用中的窗格。
/// `session`：上次的工作階段；`home`：沒有可還原的分頁時開啟的資料夾；`open`：命令列要開啟的資料夾
pub fn initial(
    startup: Startup,
    start_tabs: &[String],
    session: &[PaneTabs],
    session_active: usize,
    home: &str,
    open: Option<&str>,
) -> (Vec<PaneTabs>, usize) {
    let one = |tabs: Vec<String>| vec![PaneTabs { tabs, active: 0 }];
    let (mut panes, mut active) = match startup {
        // 指定以資料夾啟動時，新分頁就是那個資料夾
        Startup::New if open.is_some() => (Vec::new(), 0),
        Startup::New => (one(vec![String::new()]), 0),
        Startup::Tabs => {
            let tabs: Vec<String> = start_tabs.iter().map(|t| tab_path(t)).collect();
            (if tabs.is_empty() { Vec::new() } else { one(tabs) }, 0)
        }
        Startup::Session => {
            let panes: Vec<PaneTabs> = session
                .iter()
                .filter(|p| !p.tabs.is_empty())
                .map(|p| PaneTabs { tabs: p.tabs.clone(), active: p.active.min(p.tabs.len() - 1) })
                .collect();
            let active = session_active.min(panes.len().saturating_sub(1));
            (panes, active)
        }
    };
    match open {
        Some(target) => match find(panes.iter().map(|p| p.tabs.iter().map(String::as_str)), target) {
            Some((p, t)) => {
                panes[p].active = t;
                active = p;
            }
            None if panes.is_empty() => panes = one(vec![target.to_string()]),
            None => {
                let pane = &mut panes[active];
                let at = (pane.active + 1).min(pane.tabs.len());
                pane.tabs.insert(at, target.to_string());
                pane.active = at;
            }
        },
        None if panes.is_empty() => panes = one(vec![home.to_string()]),
        None => {}
    }
    (panes, active)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        std::iter::once("zing.exe").chain(a.iter().copied()).map(String::from).collect()
    }

    fn pane(tabs: &[&str], active: usize) -> PaneTabs {
        PaneTabs { tabs: tabs.iter().map(|t| t.to_string()).collect(), active }
    }

    #[test]
    fn parses_arguments() {
        assert_eq!(parse_args(&args(&[])), None);
        assert_eq!(parse_args(&args(&["--perf-out=x.json"])), None);
        assert_eq!(parse_args(&args(&["C:\\Users\\a\\"])), Some(Target::Folder("C:\\Users\\a".into())));
        assert_eq!(parse_args(&args(&["--x", "D:"])), Some(Target::Folder("D:\\".into())));
        assert_eq!(parse_args(&args(&["\"E:\\a b\""])), Some(Target::Folder("E:\\a b".into())));
        assert_eq!(parse_args(&args(&["\\\\server\\share"])), Some(Target::Folder("\\\\server\\share".into())));
        assert_eq!(parse_args(&args(&[THIS_PC])), Some(Target::Folder(String::new())));
        assert_eq!(parse_args(&args(&["shell:mycomputerfolder"])), Some(Target::Folder(String::new())));
        assert_eq!(parse_args(&args(&["::{20d04fe0-3aea-1069-a2d8-08002b30309d}"])), Some(Target::Folder(String::new())));
        assert_eq!(parse_args(&args(&["shell:RecycleBinFolder"])), Some(Target::Explorer("shell:RecycleBinFolder".into())));
        let cp = "::{26EE0668-A00A-44D7-9371-BEB064C98683}";
        assert_eq!(parse_args(&args(&[cp])), Some(Target::Explorer(cp.into())));
    }

    #[test]
    fn finds_open_tab() {
        let panes = [vec!["C:\\a", "D:\\"], vec!["", "C:\\B"]];
        assert_eq!(find(panes.iter().map(|p| p.iter().copied()), "c:\\b"), Some((1, 1)));
        assert_eq!(find(panes.iter().map(|p| p.iter().copied()), ""), Some((1, 0)));
        assert_eq!(find(panes.iter().map(|p| p.iter().copied()), "C:\\"), None);
    }

    #[test]
    fn session_startup() {
        let session = [pane(&["C:\\a", "C:\\b"], 1), pane(&[], 0), pane(&["D:\\"], 0)];
        let (p, a) = initial(Startup::Session, &[], &session, 2, "H:\\", None);
        assert_eq!((p.clone(), a), (vec![pane(&["C:\\a", "C:\\b"], 1), pane(&["D:\\"], 0)], 1), "空的窗格略過");
        // 開啟的資料夾已經在分頁中：切換過去
        let (p, a) = initial(Startup::Session, &[], &session, 0, "H:\\", Some("c:\\A"));
        assert_eq!((p[0].active, a), (0, 0));
        // 否則加在作用中的分頁後面
        let (p, a) = initial(Startup::Session, &[], &session, 0, "H:\\", Some("E:\\x"));
        assert_eq!((p[0].clone(), a), (pane(&["C:\\a", "C:\\b", "E:\\x"], 2), 0));
        // 沒有工作階段
        assert_eq!(initial(Startup::Session, &[], &[], 0, "H:\\", None), (vec![pane(&["H:\\"], 0)], 0));
        assert_eq!(initial(Startup::Session, &[], &[], 3, "H:\\", Some("E:\\x")), (vec![pane(&["E:\\x"], 0)], 0));
        // 記錄的作用中分頁超出範圍
        assert_eq!(initial(Startup::Session, &[], &[pane(&["C:\\"], 5)], 0, "H:\\", None).0, vec![pane(&["C:\\"], 0)]);
    }

    #[test]
    fn new_and_fixed_tabs() {
        let session = [pane(&["C:\\a"], 0)];
        assert_eq!(initial(Startup::New, &[], &session, 0, "H:\\", None), (vec![pane(&[""], 0)], 0));
        assert_eq!(initial(Startup::New, &[], &session, 0, "H:\\", Some("E:\\x")), (vec![pane(&["E:\\x"], 0)], 0));
        assert_eq!((tab_value(""), tab_path(THIS_PC), tab_value("C:\\"), tab_path("C:")), (THIS_PC.into(), String::new(), "C:\\".into(), "C:\\".into()));
        let pc = [THIS_PC.to_string()];
        assert_eq!(initial(Startup::Tabs, &pc, &session, 0, "H:\\", None).0, vec![pane(&[""], 0)]);
        let fixed = ["C:\\Users\\".to_string(), "D:".to_string()];
        assert_eq!(initial(Startup::Tabs, &fixed, &session, 0, "H:\\", None), (vec![pane(&["C:\\Users", "D:\\"], 0)], 0));
        assert_eq!(initial(Startup::Tabs, &fixed, &session, 0, "H:\\", Some("d:\\")), (vec![pane(&["C:\\Users", "D:\\"], 1)], 0));
        assert_eq!(initial(Startup::Tabs, &fixed, &session, 0, "H:\\", Some("E:\\")).0, vec![pane(&["C:\\Users", "E:\\", "D:\\"], 1)]);
        // 沒有指定任何分頁
        assert_eq!(initial(Startup::Tabs, &[], &session, 0, "H:\\", None), (vec![pane(&["H:\\"], 0)], 0));
    }
}
