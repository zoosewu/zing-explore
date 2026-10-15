//! Lua 腳本 API 的單一說明來源：內建命令、全域函式、函數式函式庫、事件。
//!
//! `docs/lua.md`（使用者文件）與 `zing.d.lua`（Lua Language Server 的自動補全定義）都由這裡產生；
//! 測試會確認 `docs/lua.md` 與這裡一致、zing-app 的命令分派與 `COMMANDS` 一致、內建函式庫的每個全域函式都有說明。
//! 修改 API 時改這裡，再以 `ZING_UPDATE_DOCS=1 cargo test -p zing-core` 更新文件。

/// 一個說明項目
pub struct Spec {
    pub name: &'static str,
    /// Lua Language Server 的參數與回傳型別（`---@param`、`---@return` 的內容），每行一個
    pub sig: &'static [&'static str],
    pub summary: &'static str,
    pub example: &'static str,
}

const fn spec(name: &'static str, sig: &'static [&'static str], summary: &'static str, example: &'static str) -> Spec {
    Spec { name, sig, summary, example }
}

/// 內建命令。在 Lua 中以函式呼叫：`nav.up()`、`view.mode("list")`、`copy{ to = "D:\\", move = true }`。
/// 字串、數字參數是位置參數；表格中的整數鍵是位置參數（清單會展開），字串鍵是 `--鍵=值`（`true` 為 `--鍵`）。
pub const COMMANDS: &[Spec] = &[
    spec("nav.back", &[], "上一頁", "nav.back()"),
    spec("nav.forward", &[], "下一頁", "nav.forward()"),
    spec("nav.up", &[], "上一層", "nav.up()"),
    spec("nav.open", &[], "開啟焦點項目（資料夾進入、檔案以預設程式開啟）", "nav.open()"),
    spec("nav.refresh", &[], "重新整理", "nav.refresh()"),
    spec("nav.home", &[], "回到使用者資料夾", "nav.home()"),
    spec("nav.goto", &["path string 路徑"], "切換到資料夾（同 `go`）", "nav.goto(\"C:\\\\Windows\")"),
    spec("address.edit", &[], "編輯網址列", "address.edit()"),
    spec("palette.open", &[], "開啟命令面板", "palette.open()"),
    spec("jobs.toggle", &[], "顯示／隱藏檔案作業面板", "jobs.toggle()"),
    spec("jobs.clear", &[], "清除已完成的檔案作業", "jobs.clear()"),
    spec("file.copy", &[], "複製選取項目到剪貼簿", "file.copy()"),
    spec("file.cut", &[], "剪下選取項目到剪貼簿", "file.cut()"),
    spec("file.paste", &[], "貼上剪貼簿的檔案到目前資料夾", "file.paste()"),
    spec("file.delete", &["opts? {permanent?: boolean} permanent 為 true 時不經資源回收筒"], "刪除選取項目", "file.delete{ permanent = true }"),
    spec("file.rename", &[], "重新命名（選取多個項目時為批次重新命名）", "file.rename()"),
    spec("file.batchrename", &[], "批次重新命名選取的項目", "file.batchrename()"),
    spec("file.newfolder", &[], "在目前資料夾建立新資料夾", "file.newfolder()"),
    spec("file.properties", &[], "顯示選取項目的內容對話框", "file.properties()"),
    spec("menu.context", &[], "顯示焦點項目的右鍵選單", "menu.context()"),
    spec("file.copypath", &[], "複製選取項目的完整路徑", "file.copypath()"),
    spec(
        "copy",
        &["args {to: string, move?: boolean, [integer]: string} to 為目的資料夾；位置參數為來源（省略時為選取項目）"],
        "複製或移動到指定資料夾（經檔案作業佇列）",
        "copy{ to = \"D:\\\\備份\", move = true }",
    ),
    spec(
        "run",
        &[
            "program string 程式",
            "... string|{wait?: boolean, console?: boolean} 參數；表格的 wait = true 等程式結束並回傳結束代碼（只在腳本中，等待不計入時間上限），console = true 顯示主控台視窗（互動式終端機）",
            "return integer?, string? 結束代碼（wait 時）；無法啟動時為 nil 與錯誤訊息",
        ],
        "執行程式（在目前資料夾）。主控台程式（含 .bat、.cmd）不顯示主控台視窗，視窗程式照常顯示；設定檔中以 `run --console 程式` 顯示主控台",
        "run(\"code\", cwd())\nlocal code = run{ \"tar\", \"-xf\", \"a.zip\", \"-C\", \"D:\\\\out\", wait = true }\nrun{ \"pwsh\", console = true }",
    ),
    spec("open", &["... string 檔案"], "以預設程式開啟檔案", "open(\"C:\\\\a.pdf\")"),
    spec("sel.all", &[], "全選", "sel.all()"),
    spec("sel.none", &[], "取消選取", "sel.none()"),
    spec("sel.invert", &[], "反向選取", "sel.invert()"),
    spec("tab.new", &["opts? {path?: string}"], "開新分頁（預設為目前資料夾）", "tab.new{ path = \"C:\\\\\" }"),
    spec("tab.close", &[], "關閉分頁", "tab.close()"),
    spec("tab.next", &[], "下一個分頁", "tab.next()"),
    spec("tab.prev", &[], "上一個分頁", "tab.prev()"),
    spec("pane.split", &[], "分割出新窗格", "pane.split()"),
    spec("pane.close", &[], "關閉窗格", "pane.close()"),
    spec("pane.focusnext", &[], "切換到下一個窗格", "pane.focusnext()"),
    spec("tree.toggle", &[], "顯示／隱藏樹狀列", "tree.toggle()"),
    spec("view.sort", &["opts {by: \"name\"|\"modified\"|\"type\"|\"size\", asc?: boolean, desc?: boolean}"], "排序", "view.sort{ by = \"size\", desc = true }"),
    spec("view.foldersizes", &[], "計算資料夾大小", "view.foldersizes()"),
    spec("view.preview", &[], "顯示／隱藏預覽窗格", "view.preview()"),
    spec("view.mode", &["mode \"list\"|\"small\"|\"medium\"|\"large\"|\"thumbnails\""], "切換清單／縮圖檢視", "view.mode(\"medium\")"),
    spec("view.thumbsize", &["opts {delta: integer} 加減的像素"], "調整縮圖大小", "view.thumbsize{ delta = 16 }"),
    spec("view.togglehidden", &[], "顯示／隱藏隱藏檔案", "view.togglehidden()"),
    spec("index.install", &[], "啟用 NTFS 索引服務（需要系統管理員權限）", "index.install()"),
    spec("index.uninstall", &[], "移除 NTFS 索引服務", "index.uninstall()"),
    spec("index.status", &[], "顯示 NTFS 索引服務狀態", "index.status()"),
    spec("filter.clear", &[], "清除篩選", "filter.clear()"),
    spec("filter.set", &["opts {text: string} 篩選文字（可用 * ? 萬用字元）"], "設定篩選", "filter.set{ text = \"*.jpg\" }"),
    spec("favorites.add", &["opts? {path?: string}"], "加入我的最愛（預設為目前資料夾）", "favorites.add()"),
    spec("favorites.remove", &["opts? {path?: string}"], "從我的最愛移除", "favorites.remove{ path = cwd() }"),
    spec("theme.set", &["name string 主題名稱"], "切換主題", "theme.set(\"fluent\")"),
    spec("theme.toggle", &[], "切換深色／淺色（保留風格）", "theme.toggle()"),
    spec("theme.edit", &[], "開啟主題編輯器", "theme.edit()"),
    spec("theme.editfile", &[], "以文字編輯器開啟目前的主題檔", "theme.editfile()"),
    spec("theme.import", &[], "匯入主題", "theme.import()"),
    spec("theme.export", &[], "匯出目前的主題", "theme.export()"),
    spec("config.edit", &[], "以文字編輯器開啟設定檔", "config.edit()"),
    spec("config.reload", &[], "重新載入設定", "config.reload()"),
    spec("script.reload", &[], "重新載入 Lua 腳本", "script.reload()"),
    spec("app.exit", &[], "結束程式", "app.exit()"),
];

/// 與目前狀態、使用者互動的全域函式
pub const GLOBALS: &[Spec] = &[
    spec("sel", &["return string[] 選取項目的完整路徑（不可變清單）"], "選取的項目。命令開始時的快照，執行中的命令不會改變它", "for _, p in ipairs(sel()) do msg(p) end"),
    spec("cwd", &["return string 目前資料夾（「此電腦」為空字串）"], "目前的資料夾（快照）", "msg(cwd())"),
    spec("focus", &["return string? 焦點項目的完整路徑"], "焦點所在的項目（快照）", "local f = focus()"),
    spec("go", &["path string 資料夾"], "切換到資料夾", "go(\"D:\\\\Photos\")"),
    spec("msg", &["text string"], "在狀態列顯示訊息", "msg(\"完成\")"),
    spec("ask", &["prompt string 提示", "default? string 預設值", "return string? 輸入的文字；取消時為 nil"], "詢問使用者輸入（等待到使用者回答）", "local name = ask(\"新資料夾名稱\", \"新增資料夾\")"),
    spec(
        "pick_folder",
        &["title? string 對話方塊的標題", "default? string 一開始顯示的資料夾", "return string? 選取的資料夾；取消時為 nil"],
        "顯示選擇資料夾的對話方塊（等待到使用者選擇）",
        "local dest = pick_folder(\"解壓縮到\", cwd())",
    ),
    spec("exec", &["script string 命令腳本（同設定檔的 cmd，可用 {sel} 等變數）"], "執行命令腳本", "exec(\"copy {sel} --to={dest}\")"),
    spec("log", &["... any"], "寫入 scripts\\zing.log（除錯用）", "log(\"選取\", #sel())"),
];

/// 函數式函式庫：惰性串流、不可變、純函式、組合
pub const PRELUDE: &[Spec] = &[
    spec("seq", &["source table|function 清單或迭代函式", "return Seq"], "建立惰性串流：串接的步驟在取值時才逐項計算，不產生中間清單", "seq(sel()):filter(is_image):take(10):list()"),
    spec("map", &["f fun(x: any): any", "return Stage"], "管線步驟：轉換每一項", "sel() | map(path.name) | list"),
    spec("filter", &["pred fun(x: any): boolean", "return Stage"], "管線步驟：只留下符合的項目", "sel() | filter(function(p) return path.ext(p) == \"jpg\" end) | list"),
    spec("take", &["n integer", "return Stage"], "管線步驟：取前 n 項", "sel() | take(3) | list"),
    spec("skip", &["n integer", "return Stage"], "管線步驟：略過前 n 項", "sel() | skip(1) | list"),
    spec("take_while", &["pred fun(x: any): boolean", "return Stage"], "管線步驟：取到第一個不符合的為止", "seq({1, 2, 5, 1}) | take_while(function(x) return x < 3 end) | list"),
    spec("sort_by", &["key fun(x: any): any 排序鍵", "return Stage"], "管線步驟：依鍵排序（需要讀完整個串流）", "sel() | sort_by(path.name) | list"),
    spec("each", &["f fun(x: any)", "return Stage"], "管線終點：對每一項執行（有副作用的動作放這裡）", "sel() | each(msg)"),
    spec("reduce", &["f fun(acc: any, x: any): any", "init any", "return Stage"], "管線終點：累積成一個值", "seq({1, 2, 3}) | reduce(function(a, x) return a + x end, 0)"),
    spec("list", &["s Seq|table", "return table 不可變清單"], "管線終點：取出所有項目成為不可變清單", "sel() | map(path.name) | list"),
    spec("count", &["s Seq|table", "return integer"], "管線終點：項目數", "sel() | filter(is_dir) | count"),
    spec("first", &["s Seq|table", "return any"], "管線終點：第一項（沒有時為 nil）", "sel() | first"),
    spec("compose", &["... function", "return function"], "組合函式（由右到左）：compose(f, g)(x) == f(g(x))", "local stem = compose(string.lower, path.stem)"),
    spec("pipe", &["x any", "... function", "return any"], "把值依序傳過函式（由左到右）：pipe(x, f, g) == g(f(x))", "pipe(focus(), path.name, string.upper)"),
    spec("partial", &["f function", "... any 先套用的參數", "return function"], "固定前幾個參數", "local to_backup = partial(path.join, \"D:\\\\備份\")"),
    spec("freeze", &["t table", "return table"], "回傳不可變的唯讀版本（修改時產生錯誤）", "local cfg = freeze({ dest = \"D:\\\\\" })"),
    spec("is_dir", &["p string", "return boolean"], "是否為資料夾（讀取檔案系統）", "sel() | filter(is_dir) | list"),
    spec("path.name", &["p string", "return string"], "檔名（含副檔名）", "path.name(\"C:\\\\a\\\\b.txt\") --> \"b.txt\""),
    spec("path.stem", &["p string", "return string"], "主檔名（不含副檔名）", "path.stem(\"b.tar.gz\") --> \"b.tar\""),
    spec("path.ext", &["p string", "return string"], "副檔名（小寫、不含點；沒有時為空字串）", "path.ext(\"A.JPG\") --> \"jpg\""),
    spec("path.dir", &["p string", "return string"], "上層資料夾", "path.dir(\"C:\\\\a\\\\b.txt\") --> \"C:\\\\a\""),
    spec("path.join", &["... string", "return string"], "以 \\ 連接路徑", "path.join(cwd(), \"新資料夾\")"),
];

/// 事件：在腳本中定義這些名稱的全域函式即生效
pub const HOOKS: &[Spec] = &[
    spec("on_navigate", &["path string 進入的資料夾"], "進入資料夾時呼叫（不等待結果）", "function on_navigate(p)\n  if p:find(\"\\\\Photos\") then view.mode(\"large\") end\nend"),
    spec("thumbnail", &["path string 檔案或資料夾", "return string? 要當作縮圖的圖片路徑；nil 表示使用預設"], "決定縮圖來源（每個縮圖呼叫一次，CPU 時間上限 0.3 秒）", "function thumbnail(p)\n  if path.ext(p) == \"mp4\" then\n    local jpg = path.join(path.dir(p), path.stem(p) .. \".jpg\")\n    if io.open(jpg) then return jpg end\n  end\nend"),
];

fn lua_type(sig: &str) -> (&str, &str) {
    // 「名稱 型別 說明」→ (名稱, 其餘)
    sig.split_once(' ').unwrap_or((sig, "any"))
}

/// docs/lua.md 的內容
pub fn markdown() -> String {
    let mut s = String::new();
    s.push_str(MD_HEAD);
    let section = |s: &mut String, title: &str, intro: &str, items: &[Spec]| {
        s.push_str(&format!("\n## {title}\n\n{intro}\n"));
        for it in items {
            let args: Vec<String> = it
                .sig
                .iter()
                .filter(|x| !x.starts_with("return "))
                .map(|x| lua_type(x).0.to_string())
                .collect();
            s.push_str(&format!("\n### `{}({})`\n\n{}\n", it.name, args.join(", "), it.summary));
            for x in it.sig {
                let (n, rest) = lua_type(x);
                if n == "return" {
                    s.push_str(&format!("\n- 回傳：{rest}"));
                } else {
                    s.push_str(&format!("\n- `{n}`：{rest}"));
                }
            }
            if !it.sig.is_empty() {
                s.push('\n');
            }
            s.push_str(&format!("\n```lua\n{}\n```\n", it.example));
        }
    };
    section(&mut s, "全域函式", "與目前狀態、使用者互動。`sel`、`cwd`、`focus` 回傳命令開始時的快照。", GLOBALS);
    section(
        &mut s,
        "函數式函式庫",
        "惰性求值、不可變、純函式、組合。管線以 `|` 串接：`來源 | 步驟 | 步驟 | 終點`；來源可以是清單（如 `sel()`）或 `seq(...)`。\
         步驟（`map`、`filter`…）只描述要做什麼，到終點（`list`、`each`、`reduce`…）才逐項計算。\
         除了 `each` 與 `is_dir`，這些函式都是純函式；回傳的清單不可修改。",
        PRELUDE,
    );
    section(&mut s, "事件", "在腳本中定義這些名稱的全域函式即生效。", HOOKS);
    section(
        &mut s,
        "內建命令",
        "與設定檔、命令面板相同的命令，以函式呼叫。字串、數字參數為位置參數；表格的整數鍵為位置參數（清單會展開），\
         字串鍵為 `--鍵=值`（`true` 為 `--鍵`、`false` 省略）。參數不經變數展開，路徑中的 `{` `}` 不會被誤解。\
         命令送給畫面執行後腳本就繼續，不等命令完成。",
        COMMANDS,
    );
    s
}

const MD_HEAD: &str = r#"# Lua 腳本

> 這份文件由 `crates/zing-core/src/scriptapi.rs` 產生，請勿直接修改；
> 修改 API 後執行 `ZING_UPDATE_DOCS=1 cargo test -p zing-core` 更新。

腳本放在 `zing.exe` 旁的 `scripts\*.lua`，啟動後在背景載入（依檔名順序），「重新載入 Lua 腳本」（`script.reload`）可重新載入。
腳本在專用的執行緒執行，不會讓畫面卡住；單次執行用掉的 CPU 時間超過 30 秒會被中止（等待 `ask()`、外部程式、檔案 I/O 不算在內，只攔下失控的迴圈）。錯誤顯示在狀態列，並記錄在 `scripts\error.log`（含檔名、行號與呼叫堆疊）。
腳本可以使用 Lua 5.4 完整的標準函式庫（含 `io`、`os`）。`os.execute`、`io.popen` 改為在目前資料夾執行且不顯示主控台視窗
（`io.popen` 的讀取模式可以邊執行邊讀取輸出，寫入模式寫進命令的標準輸入；`close()` 等命令結束並回傳結束狀態）。

## 自訂命令

全域函式就是命令：出現在命令面板（標題取自函式上方的 `---` 註解），可在設定檔綁定快速鍵或按鈕（`cmd = "函式名稱"`）。
`local` 函式只是輔助用，不會成為命令。函式名稱只能用英文；標題可以是中文。

```lua
--- 把選取的相片移到「相片\年-月」
function sort_photos()
  local dest = ask("目的資料夾", path.join(cwd(), "相片"))
  if not dest then return end
  sel() | filter(function(p) return path.ext(p) == "jpg" end) | each(function(p)
    copy{ p, to = dest, move = true }
  end)
end
```

## 自動補全與除錯

- 載入腳本時會在 `scripts\.zing\zing.d.lua` 寫入所有 API 的型別定義，並在 `scripts\.luarc.json` 設定好。
  以 VS Code 開啟 `scripts` 資料夾並安裝 Lua 擴充功能（Lua Language Server），即有自動補全、說明與型別檢查。
- 命令面板輸入 `=` 開頭的 Lua 運算式會立即執行並在狀態列顯示結果，例如 `= #sel()`、`= sel() | map(path.ext) | list`。
- `log(...)` 寫入 `scripts\zing.log`；除錯器連線時也顯示在 VS Code 的「偵錯主控台」。

### 中斷點除錯

zing 內建 Lua 除錯器（Debug Adapter Protocol），以 VS Code 連線：

1. 安裝擴充功能：在 zing 的原始碼資料夾執行 `pwsh tools/install-vscode-ext.ps1`，重新載入 VS Code。
2. 以 VS Code 開啟 `scripts` 資料夾（zing 已寫好 `.vscode\launch.json`），開著 zing 按 F5 附加。
3. 在腳本設定中斷點，然後在 zing 執行命令：停在中斷點時可以看呼叫堆疊、區域變數、上值、全域變數，
   在「偵錯主控台」或監看式對目前的框架求值，單步執行（不進入、進入、跳出）、繼續或暫停。

- 支援條件中斷點、記錄點（`{運算式}` 會代入值）；「腳本錯誤時暫停」預設開啟，在錯誤發生的當下停住，堆疊還在。
- 暫停期間 zing 的畫面照常可以操作，其他腳本命令排隊等繼續後才執行；暫停的時間不計入 CPU 時間上限。
- 重新載入腳本時中斷點會保留。
- 只有目前的 Windows 使用者能連線（具名管道 `\\.\pipe\zing-lua-debug-使用者名稱`，拒絕遠端連線）；同時只能有一個 zing 接受除錯連線。
"#;

/// Lua Language Server 的定義檔（scripts\.zing\zing.d.lua）
pub fn luals_meta() -> String {
    let mut s = String::from("---@meta\n-- zing-explore Lua API（由 zing 自動產生，請勿修改）\n\n");
    s.push_str("---@class Seq 惰性串流\n---@operator bor(fun(s: Seq): any): any\nlocal Seq = {}\n");
    for m in ["map", "filter", "take", "skip", "take_while", "sort_by"] {
        s.push_str(&format!("---@return Seq\nfunction Seq:{m}(...) end\n"));
    }
    for m in ["list", "each", "reduce", "count", "first"] {
        s.push_str(&format!("function Seq:{m}(...) end\n"));
    }
    s.push_str("\n---@alias Stage fun(s: Seq|table): any\n\n");
    let mut tables: Vec<&str> = Vec::new();
    for group in [GLOBALS, PRELUDE, HOOKS, COMMANDS] {
        for it in group {
            if let Some((t, _)) = it.name.split_once('.')
                && !tables.contains(&t)
            {
                tables.push(t);
            }
        }
    }
    for t in &tables {
        s.push_str(&format!("{t} = {t} or {{}}\n"));
    }
    s.push('\n');
    for group in [GLOBALS, PRELUDE, HOOKS, COMMANDS] {
        for it in group {
            s.push_str(&format!("--- {}\n", it.summary));
            for line in it.example.lines() {
                s.push_str(&format!("--- `{line}`\n"));
            }
            let mut params = Vec::new();
            for x in it.sig {
                let (n, rest) = lua_type(x);
                if n == "return" {
                    s.push_str(&format!("---@return {rest}\n"));
                } else {
                    s.push_str(&format!("---@param {n} {rest}\n"));
                    params.push(n.trim_end_matches('?'));
                }
            }
            s.push_str(&format!("function {}({}) end\n\n", it.name, params.join(", ")));
        }
    }
    s
}

/// scripts\.vscode\launch.json：以 F5 附加到 zing 的除錯器（tools/vscode-zing-lua）
pub const LAUNCH_JSON: &str = r#"{
  "version": "0.2.0",
  "configurations": [
    { "type": "zing-lua", "request": "attach", "name": "附加到 zing" }
  ]
}
"#;

/// scripts\.luarc.json：讓 Lua Language Server 認得 Lua 5.4 與定義檔
pub const LUARC: &str = r#"{
  "runtime.version": "Lua 5.4",
  "workspace.library": [".zing"],
  "diagnostics.globals": []
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_are_up_to_date() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/lua.md");
        let want = markdown();
        if std::env::var_os("ZING_UPDATE_DOCS").is_some() {
            std::fs::write(path, &want).unwrap();
        }
        let have = std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "docs/lua.md 與 scriptapi.rs 不一致：執行 ZING_UPDATE_DOCS=1 cargo test -p zing-core 更新");
    }

    #[test]
    fn names_are_unique_and_meta_mentions_all() {
        let mut names = std::collections::HashSet::new();
        let meta = luals_meta();
        for group in [GLOBALS, PRELUDE, HOOKS, COMMANDS] {
            for it in group {
                assert!(names.insert(it.name), "重複：{}", it.name);
                assert!(meta.contains(&format!("function {}(", it.name)), "{}", it.name);
            }
        }
    }
}
