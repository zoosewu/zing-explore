//! Lua 腳本 API 的單一說明來源：內建命令、全域函式、函數式函式庫、事件。
//!
//! `docs/lua.md`（使用者文件，另有 `lua.en.md`、`lua.ja.md`）與 `zing.d.lua`（Lua Language Server 的自動補全定義）
//! 都由這裡產生；說明文字以 `tr_key!` 標記，翻譯在 `i18n/*.toml`。
//! 內建命令的說明在 `builtin`（命令表），這裡只有全域函式、函式庫與事件。
//! 測試會確認文件與這裡一致、內建函式庫的每個全域函式都有說明。
//! 修改 API 時改這裡或 `builtin`（並補上翻譯），再以 `ZING_UPDATE_DOCS=1 cargo test -p zing-core` 更新文件。

use crate::i18n::{Lang, tr_in};

/// 一個說明項目
pub struct Spec {
    pub name: &'static str,
    /// Lua Language Server 的參數與回傳型別（`---@param`、`---@return` 的內容），每行一個
    pub sig: &'static [&'static str],
    pub summary: &'static str,
    pub example: &'static str,
}

pub(crate) const fn spec(name: &'static str, sig: &'static [&'static str], summary: &'static str, example: &'static str) -> Spec {
    Spec { name, sig, summary, example }
}

/// 內建命令（`builtin::SPECS`）
pub const COMMANDS: &[Spec] = crate::builtin::SPECS;

/// 與目前狀態、使用者互動的全域函式
pub const GLOBALS: &[Spec] = &[
    spec("sel", &[tr_key!("return string[] 選取項目的完整路徑（不可變清單）")], tr_key!("選取的項目。命令開始時的快照，執行中的命令不會改變它"), "for _, p in ipairs(sel()) do msg(p) end"),
    spec("cwd", &[tr_key!("return string 目前資料夾（「此電腦」為空字串）")], tr_key!("目前的資料夾（快照）"), "msg(cwd())"),
    spec("focus", &[tr_key!("return string? 焦點項目的完整路徑")], tr_key!("焦點所在的項目（快照）"), "local f = focus()"),
    spec("go", &[tr_key!("path string 資料夾")], tr_key!("切換到資料夾"), "go(\"D:\\\\Photos\")"),
    spec("msg", &["text string"], tr_key!("在狀態列顯示訊息"), tr_key!("msg(\"完成\")")),
    spec("ask", &[tr_key!("prompt string 提示"), tr_key!("default? string 預設值"), tr_key!("return string? 輸入的文字；取消時為 nil")], tr_key!("詢問使用者輸入（等待到使用者回答）"), tr_key!("local name = ask(\"新資料夾名稱\", \"新增資料夾\")")),
    spec(
        "pick_folder",
        &[tr_key!("title? string 對話方塊的標題"), tr_key!("default? string 一開始顯示的資料夾"), tr_key!("return string? 選取的資料夾；取消時為 nil")],
        tr_key!("顯示選擇資料夾的對話方塊（等待到使用者選擇）"),
        tr_key!("local dest = pick_folder(\"解壓縮到\", cwd())"),
    ),
    spec(
        "clipboard",
        &[
            tr_key!("text? string 要放到剪貼簿的文字；省略時讀取"),
            tr_key!("return string|boolean|nil 讀取時為剪貼簿的文字（沒有文字時為 nil）；寫入時為是否成功"),
        ],
        tr_key!("讀取或寫入剪貼簿的文字（換行一律是 \\n，寫入時轉成 Windows 的 \\r\\n）"),
        "clipboard(table.concat(sel() | map(path.name) | list, \"\\n\"))\nlocal text = clipboard()",
    ),
    spec("exec", &[tr_key!("script string 命令腳本（同設定檔的 cmd，可用 {sel}、{ask:提示} 等變數）")], tr_key!("執行命令腳本"), "exec(\"copy {sel} --to={dest}\")"),
    spec("log", &["... any"], tr_key!("寫入 scripts\\zing.log（除錯用）"), tr_key!("log(\"選取\", #sel())")),
];

/// 函數式函式庫：惰性串流、不可變、純函式、組合
pub const PRELUDE: &[Spec] = &[
    spec("seq", &[tr_key!("source table|function 清單或迭代函式"), "return Seq"], tr_key!("建立惰性串流：串接的步驟在取值時才逐項計算，不產生中間清單"), "local jpgs = seq(sel()):filter(function(p) return path.ext(p) == \"jpg\" end):take(10):list()"),
    spec("map", &["f fun(x: any): any", "return Stage"], tr_key!("管線步驟：轉換每一項"), "local names = sel() | map(path.name) | list"),
    spec("filter", &["pred fun(x: any): boolean", "return Stage"], tr_key!("管線步驟：只留下符合的項目"), "local jpgs = sel() | filter(function(p) return path.ext(p) == \"jpg\" end) | list"),
    spec("take", &["n integer", "return Stage"], tr_key!("管線步驟：取前 n 項"), "local first3 = sel() | take(3) | list"),
    spec("skip", &["n integer", "return Stage"], tr_key!("管線步驟：略過前 n 項"), "local rest = sel() | skip(1) | list"),
    spec("take_while", &["pred fun(x: any): boolean", "return Stage"], tr_key!("管線步驟：取到第一個不符合的為止"), "local small = seq({1, 2, 5, 1}) | take_while(function(x) return x < 3 end) | list"),
    spec("sort_by", &[tr_key!("key fun(x: any): any 排序鍵"), "return Stage"], tr_key!("管線步驟：依鍵排序（需要讀完整個串流）"), "local sorted = sel() | sort_by(path.name) | list"),
    spec("each", &["f fun(x: any)", "return Stage"], tr_key!("管線終點：對每一項執行（有副作用的動作放這裡）"), tr_key!("seq(sel()):each(msg) -- 只為了副作用時用方法呼叫：Lua 不允許單獨一行 a | b")),
    spec("reduce", &["f fun(acc: any, x: any): any", "init any", "return Stage"], tr_key!("管線終點：累積成一個值"), "local sum = seq({1, 2, 3}) | reduce(function(a, x) return a + x end, 0)"),
    spec("list", &["s Seq|table", tr_key!("return table 不可變清單")], tr_key!("管線終點：取出所有項目成為不可變清單"), "local names = sel() | map(path.name) | list"),
    spec("count", &["s Seq|table", "return integer"], tr_key!("管線終點：項目數"), "local dirs = sel() | filter(is_dir) | count"),
    spec("first", &["s Seq|table", "return any"], tr_key!("管線終點：第一項（沒有時為 nil）"), "local f = sel() | first"),
    spec("compose", &["... function", "return function"], tr_key!("組合函式（由右到左）：compose(f, g)(x) == f(g(x))"), "local stem = compose(string.lower, path.stem)"),
    spec("pipe", &["x any", "... function", "return any"], tr_key!("把值依序傳過函式（由左到右）：pipe(x, f, g) == g(f(x))"), "pipe(focus(), path.name, string.upper)"),
    spec("partial", &["f function", tr_key!("... any 先套用的參數"), "return function"], tr_key!("固定前幾個參數"), tr_key!("local to_backup = partial(path.join, \"D:\\\\備份\")")),
    spec("freeze", &["t table", "return table"], tr_key!("回傳不可變的唯讀版本（修改時產生錯誤）"), "local cfg = freeze({ dest = \"D:\\\\\" })"),
    spec("is_dir", &["p string", "return boolean"], tr_key!("是否為資料夾（讀取檔案系統）"), "local dirs = sel() | filter(is_dir) | list"),
    spec(
        "stat",
        &[
            tr_key!("p string 檔案或資料夾"),
            tr_key!("return {size: integer, modified: integer?, created: integer?, accessed: integer?, is_dir: boolean, readonly: boolean, hidden: boolean, system: boolean}?, string? 時間為 Unix 秒數（可給 os.date）；資料夾的 size 為 0；失敗時為 nil 與錯誤訊息"),
        ],
        tr_key!("檔案資訊：大小、修改／建立／存取時間、屬性（讀取檔案系統）"),
        "local month = os.date(\"%Y-%m\", stat(sel()[1]).modified)\nlocal big = sel() | filter(function(p) return stat(p).size > 100 * 1024 * 1024 end) | list",
    ),
    spec("path.name", &["p string", "return string"], tr_key!("檔名（含副檔名）"), "path.name(\"C:\\\\a\\\\b.txt\") --> \"b.txt\""),
    spec("path.stem", &["p string", "return string"], tr_key!("主檔名（不含副檔名）"), "path.stem(\"b.tar.gz\") --> \"b.tar\""),
    spec("path.ext", &["p string", "return string"], tr_key!("副檔名（小寫、不含點；沒有時為空字串）"), "path.ext(\"A.JPG\") --> \"jpg\""),
    spec("path.dir", &["p string", "return string"], tr_key!("上層資料夾"), "path.dir(\"C:\\\\a\\\\b.txt\") --> \"C:\\\\a\""),
    spec("path.join", &["... string", "return string"], tr_key!("以 \\ 連接路徑"), tr_key!("path.join(cwd(), \"新資料夾\")")),
];

/// 事件：在腳本中定義這些名稱的全域函式即生效
pub const HOOKS: &[Spec] = &[
    spec("on_navigate", &[tr_key!("path string 進入的資料夾")], tr_key!("進入資料夾時呼叫（不等待結果）"), "function on_navigate(p)\n  if p:find(\"\\\\Photos\") then view.mode(\"large\") end\nend"),
    spec("thumbnail", &[tr_key!("path string 檔案或資料夾"), tr_key!("return string? 要當作縮圖的圖片路徑；nil 表示使用預設")], tr_key!("決定縮圖來源（每個縮圖呼叫一次，CPU 時間上限 0.3 秒）"), "function thumbnail(p)\n  if path.ext(p) == \"mp4\" then\n    local jpg = path.join(path.dir(p), path.stem(p) .. \".jpg\")\n    if io.open(jpg) then return jpg end\n  end\nend"),
];

fn lua_type(sig: &str) -> (&str, &str) {
    // 「名稱 型別 說明」→ (名稱, 其餘)
    sig.split_once(' ').unwrap_or((sig, "any"))
}

/// 「型別 說明」→ (型別, 說明)。型別裡可以有括號、大括號與「: 」「, 」後的空白
fn split_type(rest: &str) -> (&str, &str) {
    let mut depth = 0i32;
    let mut prev = ' ';
    for (i, c) in rest.char_indices() {
        match c {
            '(' | '{' | '<' | '[' => depth += 1,
            ')' | '}' | '>' | ']' => depth -= 1,
            ' ' if depth == 0 && prev != ':' && prev != ',' => return (&rest[..i], rest[i + 1..].trim()),
            _ => {}
        }
        prev = c;
    }
    (rest, "")
}

/// 以 sig 組出 LuaLS 的函式型別：fun(a: T, b?: U): R
fn fun_type(it: &Spec) -> String {
    let mut params = Vec::new();
    let mut ret = String::new();
    for x in it.sig {
        let (n, rest) = lua_type(x);
        match n {
            "return" => ret = format!(": {}", split_type(rest).0),
            "overload" => {}
            _ => params.push(format!("{n}: {}", split_type(rest).0)),
        }
    }
    format!("fun({}){ret}", params.join(", "))
}

/// Lua 的保留字不能用在 `a.名稱`，要寫成 `a["名稱"]`
fn lua_field(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not",
        "or", "repeat", "return", "then", "true", "until", "while",
    ];
    match name.split_once('.') {
        Some((t, f)) if KEYWORDS.contains(&f) => format!("{t}[\"{f}\"]"),
        _ => name.to_string(),
    }
}

/// 文件檔名：docs/lua.md、docs/lua.en.md、docs/lua.ja.md
pub fn doc_file(lang: Lang) -> String {
    format!("lua{}.md", lang.suffix())
}

/// docs/lua*.md 的內容
pub fn markdown(lang: Lang) -> String {
    let t = |s: &'static str| tr_in(lang, s);
    let mut s = String::new();
    s.push_str(match lang {
        Lang::ZhTw => MD_HEAD,
        Lang::En => MD_HEAD_EN,
        Lang::Ja => MD_HEAD_JA,
    });
    let (ret_label, overload_label, colon) = (t(tr_key!("回傳：")), t(tr_key!("另一種寫法：")), t(tr_key!("：")));
    let section = |s: &mut String, title: &str, intro: &str, items: &[Spec]| {
        s.push_str(&format!("\n## {title}\n\n{intro}\n"));
        for it in items {
            let args: Vec<String> = it
                .sig
                .iter()
                .filter(|x| !x.starts_with("return ") && !x.starts_with("overload "))
                .map(|x| lua_type(x).0.to_string())
                .collect();
            s.push_str(&format!("\n### `{}({})`\n\n{}\n", it.name, args.join(", "), t(it.summary)));
            for x in it.sig {
                let (n, rest) = lua_type(t(x));
                match n {
                    "return" => s.push_str(&format!("\n- {ret_label}{rest}")),
                    "overload" => s.push_str(&format!("\n- {overload_label}`{rest}`")),
                    _ => s.push_str(&format!("\n- `{n}`{colon}{rest}")),
                }
            }
            if !it.sig.is_empty() {
                s.push('\n');
            }
            s.push_str(&format!("\n```lua\n{}\n```\n", t(it.example)));
        }
    };
    section(
        &mut s,
        t(tr_key!("全域函式")),
        t(tr_key!("與目前狀態、使用者互動。`sel`、`cwd`、`focus` 回傳命令開始時的快照。")),
        GLOBALS,
    );
    section(
        &mut s,
        t(tr_key!("函數式函式庫")),
        t(tr_key!("惰性求值、不可變、純函式、組合。管線以 `|` 串接：`來源 | 步驟 | 步驟 | 終點`；來源可以是清單（如 `sel()`）或 `seq(...)`。步驟（`map`、`filter`…）只描述要做什麼，到終點（`list`、`each`、`reduce`…）才逐項計算。除了 `each` 與 `is_dir`，這些函式都是純函式；回傳的清單不可修改。管線是運算式，Lua 不允許單獨寫成一行（`sel() | each(f)` 是語法錯誤）：把結果指定給變數，或只為副作用時用方法呼叫 `seq(sel()):each(f)`。")),
        PRELUDE,
    );
    section(&mut s, t(tr_key!("事件")), t(tr_key!("在腳本中定義這些名稱的全域函式即生效。")), HOOKS);
    section(
        &mut s,
        t(tr_key!("內建命令")),
        t(tr_key!("與設定檔、命令面板相同的命令，以函式呼叫。字串、數字參數為位置參數；表格的整數鍵為位置參數（清單會展開），字串鍵為 `--鍵=值`（`true` 為 `--鍵`、`false` 省略）。參數不經變數展開，路徑中的 `{` `}` 不會被誤解。命令送給畫面執行後腳本就繼續，不等命令完成。")),
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
  local photos = sel() | filter(function(p) return path.ext(p) == "jpg" end) | list
  copy(photos, { to = dest, move = true })
end
```

## 自動補全與除錯

- 載入腳本時會在 `scripts\.zing\zing.d.lua` 寫入所有 API 的型別定義，並在 `scripts\.luarc.json` 設定好。
  以 VS Code 開啟 `scripts` 資料夾並安裝 Lua 擴充功能（Lua Language Server），即有自動補全、說明與型別檢查。
- 命令面板輸入 `=` 開頭的 Lua 運算式會立即執行並在狀態列顯示結果，例如 `= #sel()`、`= sel() | map(path.ext) | list`。
- `log(...)` 寫入 `scripts\zing.log`；除錯器連線時也顯示在 VS Code 的「偵錯主控台」。

### 中斷點除錯

zing 內建 Lua 除錯器（Debug Adapter Protocol），以 VS Code 連線：

1. 安裝擴充功能：在 zing 的原始碼資料夾執行 `pwsh tools/install-vscode-ext.ps1`（會一起安裝 Lua Language Server），重新載入 VS Code。
2. 以 VS Code 開啟 `scripts` 資料夾（zing 已寫好 `.vscode\launch.json`），開著 zing 按 F5 附加。
3. 在腳本設定中斷點，然後在 zing 執行命令：停在中斷點時可以看呼叫堆疊、區域變數、上值、全域變數，
   在「偵錯主控台」或監看式對目前的框架求值，單步執行（不進入、進入、跳出）、繼續或暫停。

- 支援條件中斷點、記錄點（`{運算式}` 會代入值）；「腳本錯誤時暫停」預設開啟，在錯誤發生的當下停住，堆疊還在。
- 暫停期間 zing 的畫面照常可以操作，其他腳本命令排隊等繼續後才執行；暫停的時間不計入 CPU 時間上限。
- 重新載入腳本時中斷點會保留。
- 只有目前的 Windows 使用者能連線（具名管道 `\\.\pipe\zing-lua-debug-使用者名稱`，拒絕遠端連線）；同時只能有一個 zing 接受除錯連線。
"#;

const MD_HEAD_EN: &str = r#"# Lua scripts

> This document is generated from `crates/zing-core/src/scriptapi.rs`; do not edit it directly.
> After changing the API, run `ZING_UPDATE_DOCS=1 cargo test -p zing-core` to update it.

Scripts live in `scripts\*.lua` next to `zing.exe` and are loaded in the background at startup (in file name order); "Reload Lua scripts" (`script.reload`) reloads them.
Scripts run on a dedicated thread and never freeze the UI; a single run that uses more than 30 seconds of CPU time is stopped (waiting for `ask()`, external programs and file I/O does not count, so only runaway loops are caught). Errors are shown in the status bar and logged to `scripts\error.log` (with file name, line number and call stack).
Scripts can use the complete Lua 5.4 standard library (including `io` and `os`). `os.execute` and `io.popen` run in the current folder without showing a console window
(in read mode `io.popen` can read the output while the command runs; in write mode it writes to the command's standard input; `close()` waits for the command to finish and returns its exit status).

## Custom commands

Global functions are commands: they appear in the command palette (the title comes from the `---` comment above the function) and can be bound to keys or buttons in the config file (`cmd = "function_name"`).
`local` functions are helpers only and do not become commands. Function names must be ASCII; titles can be in any language.

```lua
--- Move the selected photos to "Photos\year-month"
function sort_photos()
  local dest = ask("Destination folder", path.join(cwd(), "Photos"))
  if not dest then return end
  local photos = sel() | filter(function(p) return path.ext(p) == "jpg" end) | list
  copy(photos, { to = dest, move = true })
end
```

## Completion and debugging

- When scripts are loaded, type definitions for the whole API are written to `scripts\.zing\zing.d.lua`, and `scripts\.luarc.json` is set up.
  Open the `scripts` folder in VS Code and install the Lua extension (Lua Language Server) to get completion, documentation and type checking.
- In the command palette, a Lua expression starting with `=` runs immediately and shows the result in the status bar, for example `= #sel()` or `= sel() | map(path.ext) | list`.
- `log(...)` writes to `scripts\zing.log`; while a debugger is attached it also appears in VS Code's Debug Console.

### Breakpoint debugging

zing has a built-in Lua debugger (Debug Adapter Protocol); connect with VS Code:

1. Install the extension: in the zing source folder run `pwsh tools/install-vscode-ext.ps1` (it also installs the Lua Language Server), then reload VS Code.
2. Open the `scripts` folder in VS Code (zing already wrote `.vscode\launch.json`) and, with zing running, press F5 to attach.
3. Set breakpoints in a script, then run the command in zing: when stopped you can inspect the call stack, locals, upvalues and globals,
   evaluate in the current frame in the Debug Console or watches, step (over, into, out), continue or pause.

- Conditional breakpoints and logpoints (`{expression}` is replaced by its value) are supported; "Pause on script errors" is on by default and stops right where the error happens, with the stack intact.
- While paused, zing's UI stays usable; other script commands are queued until you continue. Time spent paused does not count toward the CPU time limit.
- Breakpoints are kept when scripts are reloaded.
- Only the current Windows user can connect (named pipe `\\.\pipe\zing-lua-debug-username`, remote connections refused); only one zing at a time accepts debugger connections.
"#;

const MD_HEAD_JA: &str = r#"# Lua スクリプト

> この文書は `crates/zing-core/src/scriptapi.rs` から生成されます。直接編集しないでください。
> API を変更したら `ZING_UPDATE_DOCS=1 cargo test -p zing-core` を実行して更新します。

スクリプトは `zing.exe` の隣の `scripts\*.lua` に置き、起動後にバックグラウンドで読み込まれます（ファイル名の順）。「Lua スクリプトを再読み込み」（`script.reload`）で読み込み直せます。
スクリプトは専用のスレッドで実行され、画面が固まることはありません。1 回の実行で CPU 時間を 30 秒以上使うと中止されます（`ask()`、外部プログラム、ファイル I/O の待ち時間は含まれないので、暴走したループだけが止まります）。エラーはステータスバーに表示され、`scripts\error.log` に記録されます（ファイル名、行番号、呼び出し履歴つき）。
スクリプトでは Lua 5.4 の標準ライブラリをすべて使えます（`io`、`os` を含む）。`os.execute`、`io.popen` は現在のフォルダーで、コンソールウィンドウを表示せずに実行されます
（`io.popen` の読み取りモードでは実行中に出力を読め、書き込みモードではコマンドの標準入力に書き込みます。`close()` はコマンドの終了を待って終了状態を返します）。

## カスタムコマンド

グローバル関数がそのままコマンドになります：コマンドパレットに表示され（タイトルは関数の上の `---` コメント）、設定ファイルでショートカットキーやボタンに割り当てられます（`cmd = "関数名"`）。
`local` 関数は補助用で、コマンドにはなりません。関数名には英字だけを使えます。タイトルは日本語でもかまいません。

```lua
--- 選択した写真を「写真\年-月」に移動
function sort_photos()
  local dest = ask("移動先のフォルダー", path.join(cwd(), "写真"))
  if not dest then return end
  local photos = sel() | filter(function(p) return path.ext(p) == "jpg" end) | list
  copy(photos, { to = dest, move = true })
end
```

## 補完とデバッグ

- スクリプトを読み込むと `scripts\.zing\zing.d.lua` にすべての API の型定義が書き込まれ、`scripts\.luarc.json` も設定されます。
  VS Code で `scripts` フォルダーを開き、Lua 拡張機能（Lua Language Server）を入れると、補完、説明、型チェックが使えます。
- コマンドパレットで `=` から始まる Lua の式を入力するとすぐに実行され、結果がステータスバーに表示されます。例：`= #sel()`、`= sel() | map(path.ext) | list`。
- `log(...)` は `scripts\zing.log` に書き込みます。デバッガーの接続中は VS Code の「デバッグ コンソール」にも表示されます。

### ブレークポイントによるデバッグ

zing には Lua デバッガー（Debug Adapter Protocol）が組み込まれており、VS Code から接続します：

1. 拡張機能をインストール：zing のソースフォルダーで `pwsh tools/install-vscode-ext.ps1` を実行し（Lua Language Server も一緒に入ります）、VS Code を再読み込みします。
2. VS Code で `scripts` フォルダーを開き（zing が `.vscode\launch.json` を用意しています）、zing を開いたまま F5 でアタッチします。
3. スクリプトにブレークポイントを設定し、zing でコマンドを実行します：停止中は呼び出し履歴、ローカル変数、上位値、グローバル変数を確認でき、
   「デバッグ コンソール」やウォッチで現在のフレームの式を評価したり、ステップ実行（ステップオーバー、ステップイン、ステップアウト）、続行、一時停止ができます。

- 条件付きブレークポイントとログポイント（`{式}` が値に置き換わる）に対応しています。「スクリプトのエラーで一時停止」は既定でオンで、エラーが起きたその場で止まり、スタックも残っています。
- 一時停止中も zing の画面は通常どおり操作でき、ほかのスクリプトのコマンドは続行するまで待機します。一時停止の時間は CPU 時間の上限に含まれません。
- スクリプトを再読み込みしてもブレークポイントは保持されます。
- 接続できるのは現在の Windows ユーザーだけです（名前付きパイプ `\\.\pipe\zing-lua-debug-ユーザー名`、リモート接続は拒否）。デバッガーの接続を受け付ける zing は同時に 1 つだけです。
"#;

/// Lua Language Server 的定義檔（scripts\.zing\zing.d.lua），說明文字用 `lang`
pub fn luals_meta(lang: Lang) -> String {
    let t = |s: &'static str| tr_in(lang, s);
    let mut s = format!("---@meta\n-- zing-explore Lua API{}\n\n", t(tr_key!("（由 zing 自動產生，請勿修改）")));
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
    let all = || [GLOBALS, PRELUDE, HOOKS, COMMANDS].into_iter().flatten();
    let doc = |s: &mut String, it: &Spec| {
        s.push_str(&format!("--- {}\n", t(it.summary)));
        for line in t(it.example).lines() {
            s.push_str(&format!("--- `{line}`\n"));
        }
    };
    for t in &tables {
        // 同名的函式（sel()、filter()）：命名空間是可以呼叫的 class
        if let Some(f) = all().find(|it| it.name == *t) {
            doc(&mut s, f);
            s.push_str(&format!("---@class zing.{t}\n---@overload {}\n{t} = {{}}\n\n", fun_type(f)));
        } else {
            // 宣告成 class：拼錯的欄位（path.nmae）才會有警告
            s.push_str(&format!("---@class zing.{t}\n{t} = {{}}\n"));
        }
    }
    s.push('\n');
    for it in all() {
        if tables.contains(&it.name) {
            continue;
        }
        doc(&mut s, it);
        let mut params = Vec::new();
        for x in it.sig {
            let (n, rest) = lua_type(t(x));
            match n {
                "return" => s.push_str(&format!("---@return {rest}\n")),
                "overload" => s.push_str(&format!("---@overload {rest}\n")),
                _ => {
                    s.push_str(&format!("---@param {n} {rest}\n"));
                    params.push(n.trim_end_matches('?'));
                }
            }
        }
        let name = lua_field(it.name);
        if name == it.name {
            s.push_str(&format!("function {name}({}) end\n\n", params.join(", ")));
        } else {
            s.push_str(&format!("{name} = function({}) end\n\n", params.join(", ")));
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
  "diagnostics.globals": [],
  "diagnostics.disable": ["lowercase-global"]
}
"#;

/// 舊版 zing 寫的 .luarc.json：內容完全相同（使用者沒改過）時換成新版
pub const LUARC_OLD: &[&str] = &[r#"{
  "runtime.version": "Lua 5.4",
  "workspace.library": [".zing"],
  "diagnostics.globals": []
}
"#];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_are_up_to_date() {
        for lang in Lang::ALL {
            let path = format!("{}/../../docs/{}", env!("CARGO_MANIFEST_DIR"), doc_file(lang));
            let want = markdown(lang);
            if std::env::var_os("ZING_UPDATE_DOCS").is_some() {
                std::fs::write(&path, &want).unwrap();
            }
            let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
            assert!(have == want, "{path} 與 scriptapi.rs 不一致：執行 ZING_UPDATE_DOCS=1 cargo test -p zing-core 更新");
        }
        // tools/check-lua-api.ps1：匯出定義檔給 Lua Language Server 檢查（每種語言一個資料夾）
        if let Some(dir) = std::env::var_os("ZING_EXPORT_LUA_META") {
            for lang in Lang::ALL {
                let dir = std::path::Path::new(&dir).join(lang.code());
                std::fs::create_dir_all(dir.join(".zing")).unwrap();
                std::fs::write(dir.join(".zing").join("zing.d.lua"), luals_meta(lang)).unwrap();
                std::fs::write(dir.join(".luarc.json"), LUARC).unwrap();
            }
        }
    }

    #[test]
    fn names_are_unique_and_meta_mentions_all() {
        let mut names = std::collections::HashSet::new();
        let meta = luals_meta(Lang::ZhTw);
        for group in [GLOBALS, PRELUDE, HOOKS, COMMANDS] {
            for it in group {
                assert!(names.insert(it.name), "重複：{}", it.name);
                let field = lua_field(it.name);
                let found = meta.contains(&format!("function {field}("))
                    || meta.contains(&format!("{field} = function("))
                    || meta.contains(&format!("---@class zing.{field}\n"));
                assert!(found, "{}", it.name);
            }
        }
    }
}
