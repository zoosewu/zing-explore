# Lua 腳本

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

## 全域函式

與目前狀態、使用者互動。`sel`、`cwd`、`focus` 回傳命令開始時的快照。

### `sel()`

選取的項目。命令開始時的快照，執行中的命令不會改變它

- 回傳：string[] 選取項目的完整路徑（不可變清單）

```lua
for _, p in ipairs(sel()) do msg(p) end
```

### `cwd()`

目前的資料夾（快照）

- 回傳：string 目前資料夾（「此電腦」為空字串）

```lua
msg(cwd())
```

### `focus()`

焦點所在的項目（快照）

- 回傳：string? 焦點項目的完整路徑

```lua
local f = focus()
```

### `go(path)`

切換到資料夾

- `path`：string 資料夾

```lua
go("D:\\Photos")
```

### `msg(text)`

在狀態列顯示訊息

- `text`：string

```lua
msg("完成")
```

### `ask(prompt, default?)`

詢問使用者輸入（等待到使用者回答）

- `prompt`：string 提示
- `default?`：string 預設值
- 回傳：string? 輸入的文字；取消時為 nil

```lua
local name = ask("新資料夾名稱", "新增資料夾")
```

### `pick_folder(title?, default?)`

顯示選擇資料夾的對話方塊（等待到使用者選擇）

- `title?`：string 對話方塊的標題
- `default?`：string 一開始顯示的資料夾
- 回傳：string? 選取的資料夾；取消時為 nil

```lua
local dest = pick_folder("解壓縮到", cwd())
```

### `clipboard(text?)`

讀取或寫入剪貼簿的文字（換行一律是 \n，寫入時轉成 Windows 的 \r\n）

- `text?`：string 要放到剪貼簿的文字；省略時讀取
- 回傳：string|boolean|nil 讀取時為剪貼簿的文字（沒有文字時為 nil）；寫入時為是否成功

```lua
clipboard(table.concat(sel() | map(path.name) | list, "\n"))
local text = clipboard()
```

### `exec(script)`

執行命令腳本

- `script`：string 命令腳本（同設定檔的 cmd，可用 {sel}、{ask:提示} 等變數）

```lua
exec("copy {sel} --to={dest}")
```

### `log(...)`

寫入 scripts\zing.log（除錯用）

- `...`：any

```lua
log("選取", #sel())
```

## 函數式函式庫

惰性求值、不可變、純函式、組合。管線以 `|` 串接：`來源 | 步驟 | 步驟 | 終點`；來源可以是清單（如 `sel()`）或 `seq(...)`。步驟（`map`、`filter`…）只描述要做什麼，到終點（`list`、`each`、`reduce`…）才逐項計算。除了 `each` 與 `is_dir`，這些函式都是純函式；回傳的清單不可修改。管線是運算式，Lua 不允許單獨寫成一行（`sel() | each(f)` 是語法錯誤）：把結果指定給變數，或只為副作用時用方法呼叫 `seq(sel()):each(f)`。

### `seq(source)`

建立惰性串流：串接的步驟在取值時才逐項計算，不產生中間清單

- `source`：table|function 清單或迭代函式
- 回傳：Seq

```lua
local jpgs = seq(sel()):filter(function(p) return path.ext(p) == "jpg" end):take(10):list()
```

### `map(f)`

管線步驟：轉換每一項

- `f`：fun(x: any): any
- 回傳：Stage

```lua
local names = sel() | map(path.name) | list
```

### `filter(pred)`

管線步驟：只留下符合的項目

- `pred`：fun(x: any): boolean
- 回傳：Stage

```lua
local jpgs = sel() | filter(function(p) return path.ext(p) == "jpg" end) | list
```

### `take(n)`

管線步驟：取前 n 項

- `n`：integer
- 回傳：Stage

```lua
local first3 = sel() | take(3) | list
```

### `skip(n)`

管線步驟：略過前 n 項

- `n`：integer
- 回傳：Stage

```lua
local rest = sel() | skip(1) | list
```

### `take_while(pred)`

管線步驟：取到第一個不符合的為止

- `pred`：fun(x: any): boolean
- 回傳：Stage

```lua
local small = seq({1, 2, 5, 1}) | take_while(function(x) return x < 3 end) | list
```

### `sort_by(key)`

管線步驟：依鍵排序（需要讀完整個串流）

- `key`：fun(x: any): any 排序鍵
- 回傳：Stage

```lua
local sorted = sel() | sort_by(path.name) | list
```

### `each(f)`

管線終點：對每一項執行（有副作用的動作放這裡）

- `f`：fun(x: any)
- 回傳：Stage

```lua
seq(sel()):each(msg) -- 只為了副作用時用方法呼叫：Lua 不允許單獨一行 a | b
```

### `reduce(f, init)`

管線終點：累積成一個值

- `f`：fun(acc: any, x: any): any
- `init`：any
- 回傳：Stage

```lua
local sum = seq({1, 2, 3}) | reduce(function(a, x) return a + x end, 0)
```

### `list(s)`

管線終點：取出所有項目成為不可變清單

- `s`：Seq|table
- 回傳：table 不可變清單

```lua
local names = sel() | map(path.name) | list
```

### `count(s)`

管線終點：項目數

- `s`：Seq|table
- 回傳：integer

```lua
local dirs = sel() | filter(is_dir) | count
```

### `first(s)`

管線終點：第一項（沒有時為 nil）

- `s`：Seq|table
- 回傳：any

```lua
local f = sel() | first
```

### `compose(...)`

組合函式（由右到左）：compose(f, g)(x) == f(g(x))

- `...`：function
- 回傳：function

```lua
local stem = compose(string.lower, path.stem)
```

### `pipe(x, ...)`

把值依序傳過函式（由左到右）：pipe(x, f, g) == g(f(x))

- `x`：any
- `...`：function
- 回傳：any

```lua
pipe(focus(), path.name, string.upper)
```

### `partial(f, ...)`

固定前幾個參數

- `f`：function
- `...`：any 先套用的參數
- 回傳：function

```lua
local to_backup = partial(path.join, "D:\\備份")
```

### `freeze(t)`

回傳不可變的唯讀版本（修改時產生錯誤）

- `t`：table
- 回傳：table

```lua
local cfg = freeze({ dest = "D:\\" })
```

### `is_dir(p)`

是否為資料夾（讀取檔案系統）

- `p`：string
- 回傳：boolean

```lua
local dirs = sel() | filter(is_dir) | list
```

### `stat(p)`

檔案資訊：大小、修改／建立／存取時間、屬性（讀取檔案系統）

- `p`：string 檔案或資料夾
- 回傳：{size: integer, modified: integer?, created: integer?, accessed: integer?, is_dir: boolean, readonly: boolean, hidden: boolean, system: boolean}?, string? 時間為 Unix 秒數（可給 os.date）；資料夾的 size 為 0；失敗時為 nil 與錯誤訊息

```lua
local month = os.date("%Y-%m", stat(sel()[1]).modified)
local big = sel() | filter(function(p) return stat(p).size > 100 * 1024 * 1024 end) | list
```

### `path.name(p)`

檔名（含副檔名）

- `p`：string
- 回傳：string

```lua
path.name("C:\\a\\b.txt") --> "b.txt"
```

### `path.stem(p)`

主檔名（不含副檔名）

- `p`：string
- 回傳：string

```lua
path.stem("b.tar.gz") --> "b.tar"
```

### `path.ext(p)`

副檔名（小寫、不含點；沒有時為空字串）

- `p`：string
- 回傳：string

```lua
path.ext("A.JPG") --> "jpg"
```

### `path.dir(p)`

上層資料夾

- `p`：string
- 回傳：string

```lua
path.dir("C:\\a\\b.txt") --> "C:\\a"
```

### `path.join(...)`

以 \ 連接路徑

- `...`：string
- 回傳：string

```lua
path.join(cwd(), "新資料夾")
```

## 事件

在腳本中定義這些名稱的全域函式即生效。

### `on_navigate(path)`

進入資料夾時呼叫（不等待結果）

- `path`：string 進入的資料夾

```lua
function on_navigate(p)
  if p:find("\\Photos") then view.mode("large") end
end
```

### `thumbnail(path)`

決定縮圖來源（每個縮圖呼叫一次，CPU 時間上限 0.3 秒）

- `path`：string 檔案或資料夾
- 回傳：string? 要當作縮圖的圖片路徑；nil 表示使用預設

```lua
function thumbnail(p)
  if path.ext(p) == "mp4" then
    local jpg = path.join(path.dir(p), path.stem(p) .. ".jpg")
    if io.open(jpg) then return jpg end
  end
end
```

## 內建命令

與設定檔、命令面板相同的命令，以函式呼叫。字串、數字參數為位置參數；表格的整數鍵為位置參數（清單會展開），字串鍵為 `--鍵=值`（`true` 為 `--鍵`、`false` 省略）。參數不經變數展開，路徑中的 `{` `}` 不會被誤解。命令送給畫面執行後腳本就繼續，不等命令完成。

### `nav.back()`

上一頁

```lua
nav.back()
```

### `nav.forward()`

下一頁

```lua
nav.forward()
```

### `nav.up()`

上一層

```lua
nav.up()
```

### `nav.open()`

開啟焦點項目（資料夾進入、檔案以預設程式開啟）

```lua
nav.open()
```

### `nav.refresh()`

重新整理

```lua
nav.refresh()
```

### `nav.home()`

回到使用者資料夾

```lua
nav.home()
```

### `nav.goto(path)`

切換到資料夾（同 `go`）

- `path`：string 路徑

```lua
nav["goto"]("C:\\Windows") -- goto 是 Lua 的保留字，要這樣寫；也可以用 go()
```

### `address.edit()`

編輯網址列

```lua
address.edit()
```

### `palette.open()`

開啟命令面板

```lua
palette.open()
```

### `search.open()`

全磁碟搜尋：以名稱、路徑搜尋所有 NTFS 磁碟，可篩選與排序（語法見 README；需要 NTFS 索引服務）

```lua
search.open()
```

### `jobs.toggle()`

顯示／隱藏檔案作業面板

```lua
jobs.toggle()
```

### `jobs.clear()`

清除已完成的檔案作業

```lua
jobs.clear()
```

### `file.copy()`

複製選取項目到剪貼簿

```lua
file.copy()
```

### `file.cut()`

剪下選取項目到剪貼簿

```lua
file.cut()
```

### `file.paste()`

貼上剪貼簿的檔案到目前資料夾

```lua
file.paste()
```

### `file.delete(opts?)`

刪除選取項目

- `opts?`：{permanent?: boolean} permanent 為 true 時不經資源回收筒

```lua
file.delete{ permanent = true }
```

### `file.rename()`

重新命名（選取多個項目時為批次重新命名）

```lua
file.rename()
```

### `file.batchrename()`

批次重新命名選取的項目

```lua
file.batchrename()
```

### `file.newfolder()`

在目前資料夾建立新資料夾

```lua
file.newfolder()
```

### `file.properties()`

顯示選取項目的內容對話框

```lua
file.properties()
```

### `menu.context()`

顯示焦點項目的右鍵選單

```lua
menu.context()
```

### `file.copypath()`

複製選取項目的完整路徑

```lua
file.copypath()
```

### `copy(args)`

複製或移動到指定資料夾（經檔案作業佇列）

- `args`：{to: string, move?: boolean, [integer]: string} to 為目的資料夾；位置參數為來源（省略時為選取項目）
- 另一種寫法：`fun(sources: string[]|string, opts: {to: string, move?: boolean})`

```lua
copy{ to = "D:\\備份", move = true }
```

### `run(program, ...)`

執行程式（在目前資料夾）。主控台程式（含 .bat、.cmd）不顯示主控台視窗，視窗程式照常顯示；設定檔中以 `run --console 程式` 顯示主控台

- `program`：string 程式
- `...`：string|{wait?: boolean, console?: boolean} 參數；表格的 wait = true 等程式結束並回傳結束代碼（只在腳本中，等待不計入時間上限），console = true 顯示主控台視窗（互動式終端機）
- 回傳：integer?, string? 結束代碼（wait 時）；無法啟動時為 nil 與錯誤訊息
- 另一種寫法：`fun(args: {[integer]: string, wait?: boolean, console?: boolean}): integer?, string?`

```lua
run("code", cwd())
local code = run{ "tar", "-xf", "a.zip", "-C", "D:\\out", wait = true }
run{ "pwsh", console = true }
```

### `open(...)`

以預設程式開啟檔案

- `...`：string 檔案

```lua
open("C:\\a.pdf")
```

### `sel.all()`

全選

```lua
sel.all()
```

### `sel.none()`

取消選取

```lua
sel.none()
```

### `sel.invert()`

反向選取

```lua
sel.invert()
```

### `tab.new(opts?)`

開新分頁（預設為目前資料夾）

- `opts?`：{path?: string}

```lua
tab.new{ path = "C:\\" }
```

### `tab.close()`

關閉分頁

```lua
tab.close()
```

### `tab.next()`

下一個分頁

```lua
tab.next()
```

### `tab.prev()`

上一個分頁

```lua
tab.prev()
```

### `pane.split()`

分割出新窗格

```lua
pane.split()
```

### `pane.close()`

關閉窗格

```lua
pane.close()
```

### `pane.focusnext()`

切換到下一個窗格

```lua
pane.focusnext()
```

### `tree.toggle()`

顯示／隱藏樹狀列

```lua
tree.toggle()
```

### `view.sort(opts)`

排序

- `opts`：{by: "name"|"modified"|"type"|"size", asc?: boolean, desc?: boolean}

```lua
view.sort{ by = "size", desc = true }
```

### `view.foldersizes()`

計算資料夾大小

```lua
view.foldersizes()
```

### `thumb.set(opts?)`

手動指定縮圖（圖片複製到 %LOCALAPPDATA%，以 NTFS 檔案 ID 為鍵，改名或移動後仍有效）

- `opts?`：{path?: string, image?: string} path 預設為目前的項目；沒有 image 時開啟選擇圖片的對話框

```lua
thumb.set{ image = "D:\\封面.jpg" }
```

### `thumb.clear(opts?)`

移除手動指定的縮圖

- `opts?`：{path?: string}

```lua
thumb.clear()
```

### `settings.open()`

開啟設定頁（修改會寫回 config.toml 的對應行，保留註解）

```lua
settings.open()
```

### `keys.edit()`

開啟快速鍵編輯器（結果存在程式旁的 keys.toml）

```lua
keys.edit()
```

### `view.preview()`

顯示／隱藏預覽窗格

```lua
view.preview()
```

### `view.mode(mode)`

切換清單／縮圖檢視

- `mode`："list"|"small"|"medium"|"large"|"thumbnails"

```lua
view.mode("medium")
```

### `view.thumbsize(opts)`

調整縮圖大小

- `opts`：{delta: integer} 加減的像素

```lua
view.thumbsize{ delta = 16 }
```

### `view.togglehidden()`

顯示／隱藏隱藏檔案

```lua
view.togglehidden()
```

### `index.install()`

啟用 NTFS 索引服務（需要系統管理員權限）

```lua
index.install()
```

### `index.uninstall()`

移除 NTFS 索引服務

```lua
index.uninstall()
```

### `index.status()`

顯示 NTFS 索引服務狀態

```lua
index.status()
```

### `filter.clear()`

清除篩選

```lua
filter.clear()
```

### `filter.set(opts)`

設定篩選

- `opts`：{text: string} 篩選文字（可用 * ? 萬用字元）

```lua
filter.set{ text = "*.jpg" }
```

### `favorites.add(opts?)`

加入我的最愛（預設為目前資料夾）

- `opts?`：{path?: string}

```lua
favorites.add()
```

### `favorites.remove(opts?)`

從我的最愛移除

- `opts?`：{path?: string}

```lua
favorites.remove{ path = cwd() }
```

### `favorites.move(opts)`

調整我的最愛的順序（記在 session.toml，設定檔中的也能調整）

- `opts`：{by: integer, path?: string} by 為移動的位置數（負數往上）；path 預設為樹狀列焦點所在的我的最愛，否則目前資料夾

```lua
favorites.move{ by = -1 }
```

### `theme.set(name)`

切換主題

- `name`：string 主題名稱

```lua
theme.set("fluent")
```

### `theme.toggle()`

切換深色／淺色（保留風格）

```lua
theme.toggle()
```

### `theme.edit()`

開啟主題編輯器

```lua
theme.edit()
```

### `theme.editfile()`

以文字編輯器開啟目前的主題檔

```lua
theme.editfile()
```

### `theme.import()`

匯入主題

```lua
theme.import()
```

### `theme.export()`

匯出目前的主題

```lua
theme.export()
```

### `config.edit()`

以文字編輯器開啟設定檔

```lua
config.edit()
```

### `config.reload()`

重新載入設定

```lua
config.reload()
```

### `script.reload()`

重新載入 Lua 腳本

```lua
script.reload()
```

### `app.exit()`

結束程式

```lua
app.exit()
```
