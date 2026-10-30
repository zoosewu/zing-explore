//! 內建命令表：每個命令的名稱、Lua 說明、分組、預設圖示只寫在這裡一次。
//!
//! `builtins!` 一列產生 `Cmd` 的一個變體與 `SPECS` 的一項，兩者順序相同；zing-app 的 `App::run` 對 `Cmd` 窮舉，
//! 新增命令時漏了實作會編譯失敗。Lua 文件（`scriptapi`）、命令列與快速鍵面板的分組、圖示都從這裡讀。
//! `PRESETS` 是帶參數的常用呼叫（命令面板的標題、命令列與選單的短名稱），測試確認每個都是存在的命令。

use crate::scriptapi::{Spec, spec};

const NAV: &str = tr_key!("導覽");
const FILE: &str = tr_key!("檔案");
const SELECT: &str = tr_key!("選取");
const VIEW: &str = tr_key!("檢視");
const TABS: &str = tr_key!("分頁與窗格");
const FAVORITES: &str = tr_key!("我的最愛");
const LOOK: &str = tr_key!("外觀");
const TOOLS: &str = tr_key!("工具");
const LIST: &str = tr_key!("清單");
const TREE: &str = tr_key!("樹狀列");
const POPUP: &str = tr_key!("彈出清單");
/// 不屬於任何命令的分組（腳本的命令、`run …`）
pub const OTHER: &str = tr_key!("其他");

macro_rules! builtins {
    ($($id:ident, $group:expr, $icon:expr => $spec:expr;)*) => {
        /// 內建命令
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Cmd {
            $($id,)*
        }

        impl Cmd {
            pub const ALL: &[Cmd] = &[$(Cmd::$id,)*];

            /// 分組（原文，顯示時 `i18n::tr`）：自訂命令列與快速鍵編輯面板共用
            pub fn group(self) -> &'static str {
                match self {
                    $(Cmd::$id => $group,)*
                }
            }

            /// 預設圖示（`cmdbar::GLYPHS` 的名稱）
            pub fn icon(self) -> Option<&'static str> {
                match self {
                    $(Cmd::$id => $icon,)*
                }
            }
        }

        /// 內建命令的說明，順序與 `Cmd` 相同。在 Lua 中以函式呼叫：`nav.up()`、`view.mode("list")`、
        /// `copy{ to = "D:\\", move = true }`。字串、數字參數是位置參數；表格中的整數鍵是位置參數（清單會展開），
        /// 字串鍵是 `--鍵=值`（`true` 為 `--鍵`）。
        pub const SPECS: &[Spec] = &[$($spec,)*];
    };
}

impl Cmd {
    pub fn name(self) -> &'static str {
        SPECS[self as usize].name
    }

    pub fn spec(self) -> &'static Spec {
        &SPECS[self as usize]
    }

    pub fn parse(name: &str) -> Option<Cmd> {
        SPECS.iter().position(|s| s.name == name).map(|i| Cmd::ALL[i])
    }

    /// 命令腳本的第一個字（`view.sort --by=size` → `view.sort`）
    pub fn of_script(script: &str) -> Option<Cmd> {
        Cmd::parse(script.split_whitespace().next().unwrap_or(""))
    }
}

builtins! {
    NavBack, NAV, None => spec("nav.back", &[], tr_key!("上一頁"), "nav.back()");
    NavForward, NAV, None => spec("nav.forward", &[], tr_key!("下一頁"), "nav.forward()");
    NavUp, NAV, None => spec("nav.up", &[], tr_key!("上一層"), "nav.up()");
    NavOpen, NAV, None => spec("nav.open", &[], tr_key!("開啟焦點項目（資料夾進入、檔案以預設程式開啟）"), "nav.open()");
    NavRefresh, NAV, Some("sync") => spec("nav.refresh", &[], tr_key!("重新整理"), "nav.refresh()");
    NavHome, NAV, Some("home") => spec("nav.home", &[], tr_key!("回到使用者資料夾"), "nav.home()");
    NavGoto, NAV, None => spec("nav.goto", &[tr_key!("path string 路徑")], tr_key!("切換到資料夾（同 `go`）"), tr_key!("nav[\"goto\"](\"C:\\\\Windows\") -- goto 是 Lua 的保留字，要這樣寫；也可以用 go()"));
    AddressEdit, NAV, None => spec("address.edit", &[], tr_key!("編輯網址列"), "address.edit()");
    PaletteOpen, OTHER, None => spec("palette.open", &[], tr_key!("開啟命令面板"), "palette.open()");
    SearchOpen, TOOLS, Some("search") => spec("search.open", &[], tr_key!("全磁碟搜尋：以名稱、路徑搜尋所有 NTFS 磁碟，可篩選與排序（語法見 README；需要 NTFS 索引服務）"), "search.open()");
    JobsToggle, TABS, None => spec("jobs.toggle", &[], tr_key!("顯示／隱藏檔案作業面板"), "jobs.toggle()");
    JobsClear, TABS, None => spec("jobs.clear", &[], tr_key!("清除已完成的檔案作業"), "jobs.clear()");
    FileCopy, FILE, Some("copy") => spec("file.copy", &[], tr_key!("複製選取項目到剪貼簿"), "file.copy()");
    FileCut, FILE, Some("cut") => spec("file.cut", &[], tr_key!("剪下選取項目到剪貼簿"), "file.cut()");
    FilePaste, FILE, Some("paste") => spec("file.paste", &[], tr_key!("貼上剪貼簿的檔案到目前資料夾"), "file.paste()");
    FileDelete, FILE, Some("delete") => spec("file.delete", &[tr_key!("opts? {permanent?: boolean} permanent 為 true 時不經資源回收筒")], tr_key!("刪除選取項目"), "file.delete{ permanent = true }");
    FileRename, FILE, Some("rename") => spec("file.rename", &[], tr_key!("重新命名（選取多個項目時為批次重新命名）"), "file.rename()");
    FileBatchrename, FILE, Some("rename") => spec("file.batchrename", &[], tr_key!("批次重新命名選取的項目"), "file.batchrename()");
    FileNewfolder, FILE, Some("new-folder") => spec("file.newfolder", &[], tr_key!("在目前資料夾建立新資料夾"), "file.newfolder()");
    FileProperties, FILE, Some("properties") => spec("file.properties", &[], tr_key!("顯示選取項目的內容對話框"), "file.properties()");
    MenuContext, FILE, None => spec("menu.context", &[], tr_key!("顯示焦點項目的右鍵選單"), "menu.context()");
    FileCopypath, FILE, Some("link") => spec("file.copypath", &[], tr_key!("複製選取項目的完整路徑"), "file.copypath()");
    Copy, FILE, Some("copy") => spec(
        "copy",
        &[
            tr_key!("args {to: string, move?: boolean, [integer]: string} to 為目的資料夾；位置參數為來源（省略時為選取項目）"),
            "overload fun(sources: string[]|string, opts: {to: string, move?: boolean})",
        ],
        tr_key!("複製或移動到指定資料夾（經檔案作業佇列）"),
        tr_key!("copy{ to = \"D:\\\\備份\", move = true }"),
    );
    Run, OTHER, Some("terminal") => spec(
        "run",
        &[
            tr_key!("program string 程式"),
            tr_key!("... string|{wait?: boolean, console?: boolean} 參數；表格的 wait = true 等程式結束並回傳結束代碼（只在腳本中，等待不計入時間上限），console = true 顯示主控台視窗（互動式終端機）"),
            tr_key!("return integer?, string? 結束代碼（wait 時）；無法啟動時為 nil 與錯誤訊息"),
            "overload fun(args: {[integer]: string, wait?: boolean, console?: boolean}): integer?, string?",
        ],
        tr_key!("執行程式（在目前資料夾）。主控台程式（含 .bat、.cmd）不顯示主控台視窗，視窗程式照常顯示；設定檔中以 `run --console 程式` 顯示主控台"),
        "run(\"code\", cwd())\nlocal code = run{ \"tar\", \"-xf\", \"a.zip\", \"-C\", \"D:\\\\out\", wait = true }\nrun{ \"pwsh\", console = true }",
    );
    Open, OTHER, None => spec("open", &[tr_key!("... string 檔案")], tr_key!("以預設程式開啟檔案"), "open(\"C:\\\\a.pdf\")");
    SelAll, SELECT, Some("select-all") => spec("sel.all", &[], tr_key!("全選"), "sel.all()");
    SelNone, SELECT, Some("select-all") => spec("sel.none", &[], tr_key!("取消選取"), "sel.none()");
    SelInvert, SELECT, Some("select-all") => spec("sel.invert", &[], tr_key!("反向選取"), "sel.invert()");
    TabNew, TABS, Some("tab") => spec("tab.new", &["opts? {path?: string}"], tr_key!("開新分頁（預設為目前資料夾）"), "tab.new{ path = \"C:\\\\\" }");
    TabClose, TABS, Some("tab") => spec("tab.close", &[], tr_key!("關閉分頁"), "tab.close()");
    TabNext, TABS, Some("tab") => spec("tab.next", &[], tr_key!("下一個分頁"), "tab.next()");
    TabPrev, TABS, Some("tab") => spec("tab.prev", &[], tr_key!("上一個分頁"), "tab.prev()");
    PaneSplit, TABS, Some("split") => spec("pane.split", &[], tr_key!("分割出新窗格"), "pane.split()");
    PaneClose, TABS, Some("split") => spec("pane.close", &[], tr_key!("關閉窗格"), "pane.close()");
    PaneFocusnext, TABS, Some("split") => spec("pane.focusnext", &[], tr_key!("切換到下一個窗格"), "pane.focusnext()");
    TreeToggle, VIEW, Some("tree") => spec("tree.toggle", &[], tr_key!("顯示／隱藏樹狀列"), "tree.toggle()");
    ViewSort, VIEW, Some("sort") => spec("view.sort", &[tr_key!("opts {by?: \"name\"|\"modified\"|\"type\"|\"size\", asc?: boolean, desc?: boolean} 只給 asc／desc 時維持目前的欄位")], tr_key!("排序"), "view.sort{ by = \"size\", desc = true }");
    ViewFoldersizes, VIEW, Some("size") => spec("view.foldersizes", &[], tr_key!("計算資料夾大小"), "view.foldersizes()");
    ThumbSet, FILE, Some("photo") => spec(
        "thumb.set",
        &[tr_key!("opts? {path?: string, image?: string} path 預設為目前的項目；沒有 image 時開啟選擇圖片的對話框")],
        tr_key!("手動指定縮圖（圖片複製到 %LOCALAPPDATA%，以 NTFS 檔案 ID 為鍵，改名或移動後仍有效）"),
        tr_key!("thumb.set{ image = \"D:\\\\封面.jpg\" }"),
    );
    ThumbClear, FILE, Some("photo") => spec("thumb.clear", &["opts? {path?: string}"], tr_key!("移除手動指定的縮圖"), "thumb.clear()");
    CmdbarEdit, OTHER, Some("settings") => spec("cmdbar.edit", &[], tr_key!("開啟自訂命令列的面板（結果存在程式旁的 toolbar.toml）"), "cmdbar.edit()");
    CmdbarHide, OTHER, None => spec("cmdbar.hide", &[], tr_key!("隱藏命令列（寫回 config.toml 的 command_bar.visible）"), "cmdbar.hide()");
    FileNewfile, FILE, Some("document") => spec("file.newfile", &[], tr_key!("在目前的資料夾新增文字文件並進入重新命名"), "file.newfile()");
    SettingsOpen, OTHER, Some("settings") => spec("settings.open", &[], tr_key!("開啟設定頁（修改會寫回 config.toml 的對應行，保留註解）"), "settings.open()");
    KeysEdit, OTHER, Some("settings") => spec("keys.edit", &[], tr_key!("開啟快速鍵編輯器（結果存在程式旁的 keys.toml）"), "keys.edit()");
    ViewPreview, VIEW, Some("preview") => spec("view.preview", &[], tr_key!("顯示／隱藏預覽窗格"), "view.preview()");
    ViewMode, VIEW, Some("view") => spec("view.mode", &["mode \"list\"|\"small\"|\"medium\"|\"large\"|\"thumbnails\""], tr_key!("切換清單／縮圖檢視"), "view.mode(\"medium\")");
    ViewThumbsize, VIEW, None => spec("view.thumbsize", &[tr_key!("opts {delta: integer} 加減的像素")], tr_key!("調整縮圖大小"), "view.thumbsize{ delta = 16 }");
    ViewTogglehidden, VIEW, Some("hidden") => spec("view.togglehidden", &[], tr_key!("顯示／隱藏隱藏檔案"), "view.togglehidden()");
    IndexInstall, TOOLS, None => spec("index.install", &[], tr_key!("啟用 NTFS 索引服務（需要系統管理員權限）"), "index.install()");
    IndexUninstall, TOOLS, None => spec("index.uninstall", &[], tr_key!("移除 NTFS 索引服務"), "index.uninstall()");
    IndexStatus, TOOLS, None => spec("index.status", &[], tr_key!("顯示 NTFS 索引服務狀態"), "index.status()");
    FilterClear, VIEW, None => spec("filter.clear", &[], tr_key!("清除篩選"), "filter.clear()");
    FilterSet, VIEW, None => spec("filter.set", &[tr_key!("opts {text: string} 篩選文字（可用 * ? 萬用字元）")], tr_key!("設定篩選"), "filter.set{ text = \"*.jpg\" }");
    FavoritesAdd, FAVORITES, Some("star") => spec("favorites.add", &["opts? {path?: string}"], tr_key!("加入我的最愛（預設為目前資料夾）"), "favorites.add()");
    FavoritesRemove, FAVORITES, Some("star") => spec("favorites.remove", &["opts? {path?: string}"], tr_key!("從我的最愛移除"), "favorites.remove{ path = cwd() }");
    FavoritesMove, FAVORITES, None => spec(
        "favorites.move",
        &[tr_key!("opts {by: integer, path?: string} by 為移動的位置數（負數往上）；path 預設為樹狀列焦點所在的我的最愛，否則目前資料夾")],
        tr_key!("調整我的最愛的順序（記在 session.toml，設定檔中的也能調整）"),
        "favorites.move{ by = -1 }",
    );
    ThemeSet, LOOK, None => spec("theme.set", &[tr_key!("name string 主題名稱")], tr_key!("切換主題"), "theme.set(\"fluent\")");
    ThemeToggle, LOOK, Some("edit") => spec("theme.toggle", &[], tr_key!("切換深色／淺色（保留風格）"), "theme.toggle()");
    ThemeEdit, LOOK, Some("edit") => spec("theme.edit", &[], tr_key!("開啟主題編輯器"), "theme.edit()");
    ThemeEditfile, LOOK, None => spec("theme.editfile", &[], tr_key!("以文字編輯器開啟目前的主題檔"), "theme.editfile()");
    ThemeImport, LOOK, None => spec("theme.import", &[], tr_key!("匯入主題"), "theme.import()");
    ThemeExport, LOOK, None => spec("theme.export", &[], tr_key!("匯出目前的主題"), "theme.export()");
    ConfigEdit, OTHER, None => spec("config.edit", &[], tr_key!("以文字編輯器開啟設定檔"), "config.edit()");
    ConfigReload, OTHER, None => spec("config.reload", &[], tr_key!("重新載入設定"), "config.reload()");
    ScriptReload, OTHER, None => spec("script.reload", &[], tr_key!("重新載入 Lua 腳本"), "script.reload()");
    AppExit, OTHER, None => spec("app.exit", &[], tr_key!("結束程式"), "app.exit()");
    UpdateCheck, OTHER, None => spec("update.check", &[], tr_key!("立即檢查新版本（不受每天一次與設定的限制；有任何較新的版本都提示，包括修補版與略過的版本）"), "update.check()");
    WelcomeShow, OTHER, None => spec("welcome.show", &[], tr_key!("開啟歡迎畫面（第一次啟動時顯示的常用設定）"), "welcome.show()");
    ListUp, LIST, None => spec("cursor.up", &[tr_key!("opts? {extend?: boolean, keep?: boolean} extend 延伸選取（Shift）；keep 只移動焦點、不改選取（Ctrl）；兩者都有時延伸並保留原本的選取")], tr_key!("清單：焦點向上移動並選取（縮圖檢視中左右是一格、上下是一列）"), "cursor.up{ extend = true }");
    ListDown, LIST, None => spec("cursor.down", &[tr_key!("opts? {extend?: boolean, keep?: boolean} extend 延伸選取（Shift）；keep 只移動焦點、不改選取（Ctrl）；兩者都有時延伸並保留原本的選取")], tr_key!("清單：焦點向下移動並選取（縮圖檢視中左右是一格、上下是一列）"), "cursor.down{ extend = true }");
    ListLeft, LIST, None => spec("cursor.left", &[tr_key!("opts? {extend?: boolean, keep?: boolean} extend 延伸選取（Shift）；keep 只移動焦點、不改選取（Ctrl）；兩者都有時延伸並保留原本的選取")], tr_key!("清單：焦點向左移動並選取（縮圖檢視中左右是一格、上下是一列）"), "cursor.left{ extend = true }");
    ListRight, LIST, None => spec("cursor.right", &[tr_key!("opts? {extend?: boolean, keep?: boolean} extend 延伸選取（Shift）；keep 只移動焦點、不改選取（Ctrl）；兩者都有時延伸並保留原本的選取")], tr_key!("清單：焦點向右移動並選取（縮圖檢視中左右是一格、上下是一列）"), "cursor.right{ extend = true }");
    ListPageup, LIST, None => spec("cursor.pageup", &[tr_key!("opts? {extend?: boolean, keep?: boolean} extend 延伸選取（Shift）；keep 只移動焦點、不改選取（Ctrl）；兩者都有時延伸並保留原本的選取")], tr_key!("清單：焦點往上一頁移動並選取（縮圖檢視中左右是一格、上下是一列）"), "cursor.pageup{ extend = true }");
    ListPagedown, LIST, None => spec("cursor.pagedown", &[tr_key!("opts? {extend?: boolean, keep?: boolean} extend 延伸選取（Shift）；keep 只移動焦點、不改選取（Ctrl）；兩者都有時延伸並保留原本的選取")], tr_key!("清單：焦點往下一頁移動並選取（縮圖檢視中左右是一格、上下是一列）"), "cursor.pagedown{ extend = true }");
    ListFirst, LIST, None => spec("cursor.first", &[tr_key!("opts? {extend?: boolean, keep?: boolean} extend 延伸選取（Shift）；keep 只移動焦點、不改選取（Ctrl）；兩者都有時延伸並保留原本的選取")], tr_key!("清單：焦點到第一個移動並選取（縮圖檢視中左右是一格、上下是一列）"), "cursor.first{ extend = true }");
    ListLast, LIST, None => spec("cursor.last", &[tr_key!("opts? {extend?: boolean, keep?: boolean} extend 延伸選取（Shift）；keep 只移動焦點、不改選取（Ctrl）；兩者都有時延伸並保留原本的選取")], tr_key!("清單：焦點到最後一個移動並選取（縮圖檢視中左右是一格、上下是一列）"), "cursor.last{ extend = true }");
    ListSelect, LIST, None => spec("cursor.select", &[], tr_key!("清單：只選取焦點項目"), "cursor.select()");
    ListToggle, LIST, None => spec("cursor.toggle", &[], tr_key!("清單：切換焦點項目的選取（其他項目不變）"), "cursor.toggle()");
    FilterBackspace, LIST, None => spec("filter.backspace", &[], tr_key!("篩選文字刪去最後一個字（刪完就結束篩選）"), "filter.backspace()");
    FilterStart, LIST, None => spec("filter.start", &[], tr_key!("開始篩選：之後打的字成為篩選文字（「在清單中打字」設為不做事時使用）"), "filter.start()");
    TreeUp, TREE, None => spec("tree.up", &[], tr_key!("樹狀列：焦點往上"), "tree.up()");
    TreeDown, TREE, None => spec("tree.down", &[], tr_key!("樹狀列：焦點往下"), "tree.down()");
    TreeExpand, TREE, None => spec("tree.expand", &[], tr_key!("樹狀列：展開焦點節點（已展開時移到第一個子節點）"), "tree.expand()");
    TreeCollapse, TREE, None => spec("tree.collapse", &[], tr_key!("樹狀列：收合焦點節點（已收合時移到上層）"), "tree.collapse()");
    TreeOpen, TREE, None => spec("tree.open", &[], tr_key!("樹狀列：前往焦點節點的資料夾"), "tree.open()");
    TreeLeave, TREE, None => spec("tree.leave", &[], tr_key!("樹狀列：鍵盤焦點回到檔案清單"), "tree.leave()");
    TreeMoveup, TREE, None => spec("tree.moveup", &[], tr_key!("樹狀列：焦點所在的我的最愛上移（不是我的最愛時不做事）"), "tree.moveup()");
    TreeMovedown, TREE, None => spec("tree.movedown", &[], tr_key!("樹狀列：焦點所在的我的最愛下移（不是我的最愛時不做事）"), "tree.movedown()");
    UiUp, POPUP, None => spec("ui.up", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：往上"), "ui.up()");
    UiDown, POPUP, None => spec("ui.down", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：往下"), "ui.down()");
    UiLeft, POPUP, None => spec("ui.left", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：往左（命令列、子選單）"), "ui.left()");
    UiRight, POPUP, None => spec("ui.right", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：往右（命令列、子選單）"), "ui.right()");
    UiPageup, POPUP, None => spec("ui.pageup", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：往上一頁"), "ui.pageup()");
    UiPagedown, POPUP, None => spec("ui.pagedown", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：往下一頁"), "ui.pagedown()");
    UiFirst, POPUP, None => spec("ui.first", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：到第一個"), "ui.first()");
    UiLast, POPUP, None => spec("ui.last", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：到最後一個"), "ui.last()");
    UiAccept, POPUP, None => spec("ui.accept", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：確定（執行選取的項目）"), "ui.accept()");
    UiCancel, POPUP, None => spec("ui.cancel", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：取消（關閉）"), "ui.cancel()");
    UiSelect, POPUP, None => spec("ui.select", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：切換焦點項目（開關、選項）"), "ui.select()");
    UiNext, POPUP, None => spec("ui.next", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：下一個欄位"), "ui.next()");
    UiPrev, POPUP, None => spec("ui.prev", &[], tr_key!("彈出清單（選單、命令面板、命令列、右側面板）：上一個欄位"), "ui.prev()");
    SearchSort, TOOLS, None => spec("search.sort", &[tr_key!("opts {by: \"name\"|\"path\"|\"ext\"|\"size\"|\"date\"|\"none\"}")], tr_key!("全磁碟搜尋：依欄位排序（再按一次反向），none 取消排序"), "search.sort{ by = \"size\" }");
    SearchOpenitem, TOOLS, None => spec("search.openitem", &[], tr_key!("全磁碟搜尋：開啟選取的結果（資料夾就進入；Enter 則是前往所在的資料夾）"), "search.openitem()");
    CmdbarFocus, OTHER, None => spec("cmdbar.focus", &[], tr_key!("鍵盤焦點移到命令列（←→ 移動、Enter 執行、Esc 離開）；再執行一次離開"), "cmdbar.focus()");
}

/// 在終端機開啟目前的資料夾
pub const TERMINAL: &str = "run wt.exe -d {path}";

/// 常用的呼叫：`title` 是命令面板的標題（None 表示不放進命令面板），`short` 是命令列與選單上的短名稱
pub struct Preset {
    pub script: &'static str,
    pub title: Option<&'static str>,
    pub short: Option<&'static str>,
}

const fn titled(script: &'static str, title: &'static str) -> Preset {
    Preset { script, title: Some(title), short: None }
}

const fn titled_short(script: &'static str, title: &'static str, short: &'static str) -> Preset {
    Preset { script, title: Some(title), short: Some(short) }
}

const fn short(script: &'static str, short: &'static str) -> Preset {
    Preset { script, title: None, short: Some(short) }
}

/// 命令面板只列不需要額外輸入就能執行的呼叫（依這裡的順序）
pub const PRESETS: &[Preset] = &[
    titled("nav.back", tr_key!("上一頁")),
    titled("nav.forward", tr_key!("下一頁")),
    titled("nav.up", tr_key!("上一層")),
    titled("nav.open", tr_key!("開啟")),
    titled("nav.refresh", tr_key!("重新整理")),
    titled("nav.home", tr_key!("前往使用者資料夾")),
    titled("address.edit", tr_key!("編輯網址列")),
    titled("file.copy", tr_key!("複製")),
    titled("file.cut", tr_key!("剪下")),
    titled("file.paste", tr_key!("貼上")),
    titled_short("file.delete", tr_key!("刪除（移到資源回收筒）"), tr_key!("刪除")),
    titled("file.delete --permanent", tr_key!("永久刪除")),
    titled("file.rename", tr_key!("重新命名")),
    titled_short("file.newfolder", tr_key!("新增資料夾"), tr_key!("資料夾")),
    titled_short("file.newfile", tr_key!("新增文字文件"), tr_key!("文字文件")),
    titled("file.properties", tr_key!("內容")),
    titled("file.copypath", tr_key!("複製路徑")),
    titled("copy --to={dest}", tr_key!("複製到另一個窗格")),
    titled("copy --move --to={dest}", tr_key!("移動到另一個窗格")),
    titled("menu.context", tr_key!("顯示右鍵選單")),
    titled("sel.all", tr_key!("全選")),
    titled("sel.none", tr_key!("取消選取")),
    titled("sel.invert", tr_key!("反向選取")),
    titled("tab.new", tr_key!("新增分頁")),
    titled("tab.close", tr_key!("關閉分頁")),
    titled("tab.next", tr_key!("下一個分頁")),
    titled("tab.prev", tr_key!("上一個分頁")),
    titled("pane.split", tr_key!("分割窗格")),
    titled("pane.close", tr_key!("關閉窗格")),
    titled("pane.focusnext", tr_key!("切換到下一個窗格")),
    titled_short("tree.toggle", tr_key!("顯示／隱藏樹狀列"), tr_key!("樹狀列")),
    titled_short("view.sort --by=name", tr_key!("依名稱排序"), tr_key!("名稱")),
    titled_short("view.sort --by=modified", tr_key!("依修改日期排序"), tr_key!("修改日期")),
    titled_short("view.sort --by=type", tr_key!("依類型排序"), tr_key!("類型")),
    titled_short("view.sort --by=size", tr_key!("依大小排序"), tr_key!("大小")),
    titled_short("view.togglehidden", tr_key!("顯示／隱藏隱藏檔案"), tr_key!("隱藏的項目")),
    titled("view.foldersizes", tr_key!("計算資料夾大小")),
    titled_short("view.preview", tr_key!("顯示／隱藏預覽窗格"), tr_key!("預覽窗格")),
    titled_short("file.batchrename", tr_key!("批次重新命名（選取多個項目時 F2）"), tr_key!("批次重新命名")),
    titled_short("view.mode list", tr_key!("檢視：清單"), tr_key!("清單")),
    titled_short("view.mode small", tr_key!("檢視：小縮圖"), tr_key!("小縮圖")),
    titled_short("view.mode medium", tr_key!("檢視：中縮圖"), tr_key!("中縮圖")),
    titled_short("view.mode large", tr_key!("檢視：大縮圖"), tr_key!("大縮圖")),
    titled_short("thumb.set", tr_key!("設定縮圖…（目前的項目）"), tr_key!("設定縮圖…")),
    titled_short("thumb.clear", tr_key!("移除自訂縮圖（目前的項目）"), tr_key!("移除自訂縮圖")),
    titled("index.install", tr_key!("啟用 NTFS 索引服務（立即顯示資料夾大小，需要系統管理員權限）")),
    titled("index.uninstall", tr_key!("移除 NTFS 索引服務")),
    titled("index.status", tr_key!("NTFS 索引服務狀態")),
    titled_short("search.open", tr_key!("全磁碟搜尋（NTFS 索引）"), tr_key!("全磁碟搜尋")),
    titled("filter.clear", tr_key!("清除篩選")),
    titled_short("jobs.toggle", tr_key!("顯示／隱藏檔案作業"), tr_key!("檔案作業")),
    titled("jobs.clear", tr_key!("清除已完成的檔案作業")),
    titled("favorites.add", tr_key!("加入我的最愛")),
    titled("favorites.remove", tr_key!("從我的最愛移除")),
    titled("favorites.move --by=-1", tr_key!("我的最愛：上移")),
    titled("favorites.move --by=1", tr_key!("我的最愛：下移")),
    titled_short("theme.toggle", tr_key!("切換深色／淺色主題"), tr_key!("切換深色／淺色")),
    titled("theme.set system", tr_key!("主題：緊湊（預設，跟隨系統深淺色）")),
    titled("theme.set dark", tr_key!("主題：緊湊深色")),
    titled("theme.set light", tr_key!("主題：緊湊淺色")),
    titled("theme.set fluent", tr_key!("主題：Fluent（寬鬆圓角，跟隨系統深淺色）")),
    titled("theme.set balanced", tr_key!("主題：折衷（Fluent 外觀、中等密度，跟隨系統深淺色）")),
    titled_short("theme.edit", tr_key!("編輯目前的主題"), tr_key!("自訂主題…")),
    titled("theme.editfile", tr_key!("用文字編輯器開啟目前的主題檔（存檔後自動套用）")),
    titled("theme.import", tr_key!("匯入主題…")),
    titled("theme.export", tr_key!("匯出目前的主題…")),
    titled("settings.open", tr_key!("設定")),
    titled_short("cmdbar.edit", tr_key!("自訂命令列"), tr_key!("自訂命令列…")),
    titled("cmdbar.hide", tr_key!("隱藏命令列")),
    titled_short(TERMINAL, tr_key!("在終端機開啟目前的資料夾"), tr_key!("在終端機開啟")),
    titled_short("keys.edit", tr_key!("編輯快速鍵"), tr_key!("編輯快速鍵…")),
    titled("config.edit", tr_key!("編輯設定檔")),
    titled("config.reload", tr_key!("重新載入設定")),
    titled("update.check", tr_key!("檢查更新")),
    titled_short("welcome.show", tr_key!("歡迎畫面（常用設定）"), tr_key!("歡迎畫面…")),
    titled_short("app.exit", tr_key!("結束 zing-explore"), tr_key!("結束")),
    titled("cursor.up", tr_key!("清單：向上")),
    titled("cursor.up --extend", tr_key!("清單：向上延伸選取")),
    titled("cursor.up --keep", tr_key!("清單：焦點向上（不改選取）")),
    titled("cursor.up --extend --keep", tr_key!("清單：向上延伸選取（保留原本的選取）")),
    titled("cursor.down", tr_key!("清單：向下")),
    titled("cursor.down --extend", tr_key!("清單：向下延伸選取")),
    titled("cursor.down --keep", tr_key!("清單：焦點向下（不改選取）")),
    titled("cursor.down --extend --keep", tr_key!("清單：向下延伸選取（保留原本的選取）")),
    titled("cursor.left", tr_key!("清單：向左")),
    titled("cursor.left --extend", tr_key!("清單：向左延伸選取")),
    titled("cursor.left --keep", tr_key!("清單：焦點向左（不改選取）")),
    titled("cursor.left --extend --keep", tr_key!("清單：向左延伸選取（保留原本的選取）")),
    titled("cursor.right", tr_key!("清單：向右")),
    titled("cursor.right --extend", tr_key!("清單：向右延伸選取")),
    titled("cursor.right --keep", tr_key!("清單：焦點向右（不改選取）")),
    titled("cursor.right --extend --keep", tr_key!("清單：向右延伸選取（保留原本的選取）")),
    titled("cursor.pageup", tr_key!("清單：往上一頁")),
    titled("cursor.pageup --extend", tr_key!("清單：往上一頁延伸選取")),
    titled("cursor.pageup --keep", tr_key!("清單：焦點往上一頁（不改選取）")),
    titled("cursor.pageup --extend --keep", tr_key!("清單：往上一頁延伸選取（保留原本的選取）")),
    titled("cursor.pagedown", tr_key!("清單：往下一頁")),
    titled("cursor.pagedown --extend", tr_key!("清單：往下一頁延伸選取")),
    titled("cursor.pagedown --keep", tr_key!("清單：焦點往下一頁（不改選取）")),
    titled("cursor.pagedown --extend --keep", tr_key!("清單：往下一頁延伸選取（保留原本的選取）")),
    titled("cursor.first", tr_key!("清單：到第一個")),
    titled("cursor.first --extend", tr_key!("清單：到第一個延伸選取")),
    titled("cursor.first --keep", tr_key!("清單：焦點到第一個（不改選取）")),
    titled("cursor.first --extend --keep", tr_key!("清單：到第一個延伸選取（保留原本的選取）")),
    titled("cursor.last", tr_key!("清單：到最後一個")),
    titled("cursor.last --extend", tr_key!("清單：到最後一個延伸選取")),
    titled("cursor.last --keep", tr_key!("清單：焦點到最後一個（不改選取）")),
    titled("cursor.last --extend --keep", tr_key!("清單：到最後一個延伸選取（保留原本的選取）")),
    titled("cursor.select", tr_key!("清單：選取焦點項目")),
    titled("cursor.toggle", tr_key!("清單：切換焦點項目的選取")),
    titled("filter.backspace", tr_key!("篩選：刪去最後一個字")),
    titled("filter.start", tr_key!("開始篩選")),
    titled("tree.up", tr_key!("樹狀列：焦點往上")),
    titled("tree.down", tr_key!("樹狀列：焦點往下")),
    titled("tree.expand", tr_key!("樹狀列：展開")),
    titled("tree.collapse", tr_key!("樹狀列：收合")),
    titled("tree.open", tr_key!("樹狀列：前往")),
    titled("tree.leave", tr_key!("樹狀列：回到檔案清單")),
    titled("tree.moveup", tr_key!("樹狀列：我的最愛上移")),
    titled("tree.movedown", tr_key!("樹狀列：我的最愛下移")),
    titled("ui.up", tr_key!("彈出清單：往上")),
    titled("ui.down", tr_key!("彈出清單：往下")),
    titled("ui.left", tr_key!("彈出清單：往左（命令列、子選單）")),
    titled("ui.right", tr_key!("彈出清單：往右（命令列、子選單）")),
    titled("ui.pageup", tr_key!("彈出清單：往上一頁")),
    titled("ui.pagedown", tr_key!("彈出清單：往下一頁")),
    titled("ui.first", tr_key!("彈出清單：到第一個")),
    titled("ui.last", tr_key!("彈出清單：到最後一個")),
    titled("ui.accept", tr_key!("彈出清單：確定（執行選取的項目）")),
    titled("ui.cancel", tr_key!("彈出清單：取消（關閉）")),
    titled("ui.select", tr_key!("彈出清單：切換焦點項目（開關、選項）")),
    titled("ui.next", tr_key!("彈出清單：下一個欄位")),
    titled("ui.prev", tr_key!("彈出清單：上一個欄位")),
    titled("search.sort --by=name", tr_key!("全磁碟搜尋：依名稱排序")),
    titled("search.sort --by=path", tr_key!("全磁碟搜尋：依路徑排序")),
    titled("search.sort --by=ext", tr_key!("全磁碟搜尋：依副檔名排序")),
    titled("search.sort --by=size", tr_key!("全磁碟搜尋：依大小排序")),
    titled("search.sort --by=date", tr_key!("全磁碟搜尋：依日期排序")),
    titled("search.sort --by=none", tr_key!("全磁碟搜尋：取消排序")),
    titled("search.openitem", tr_key!("全磁碟搜尋：開啟項目")),
    titled("cmdbar.focus", tr_key!("移到命令列")),
    short("view.sort --asc", tr_key!("遞增")),
    short("view.sort --desc", tr_key!("遞減")),
];

/// 命令面板的內建項目：（命令腳本, 標題）
pub fn palette() -> impl Iterator<Item = (&'static str, &'static str)> {
    PRESETS.iter().filter_map(|p| p.title.map(|t| (p.script, t)))
}

pub fn preset(script: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.script == script)
}

/// 命令腳本的分組：內建命令依 `Cmd::group`，其他（腳本的命令、`run …`）為「其他」
pub fn group_of(script: &str) -> &'static str {
    Cmd::of_script(script).map_or(OTHER, Cmd::group)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        assert_eq!(Cmd::ALL.len(), SPECS.len());
        for &c in Cmd::ALL {
            assert_eq!(Cmd::parse(c.name()), Some(c));
        }
        assert_eq!(Cmd::of_script("view.sort --by=size"), Some(Cmd::ViewSort));
        assert_eq!(Cmd::parse("nope"), None);
    }

    #[test]
    fn presets_are_known_commands() {
        let mut seen = std::collections::HashSet::new();
        for p in PRESETS {
            assert!(Cmd::of_script(p.script).is_some(), "{} 不是內建命令", p.script);
            assert!(seen.insert(p.script), "{} 重複", p.script);
            assert!(p.title.is_some() || p.short.is_some(), "{}", p.script);
        }
        assert_eq!(group_of("copy --to={dest}"), FILE);
        assert_eq!(group_of("run notepad.exe"), OTHER);
        assert_eq!(group_of("my_script"), OTHER);
    }

    #[test]
    fn icons_exist() {
        for &c in Cmd::ALL {
            if let Some(i) = c.icon() {
                assert!(crate::cmdbar::glyph(i).is_some(), "{} 的圖示 {i} 不存在", c.name());
            }
        }
    }
}
