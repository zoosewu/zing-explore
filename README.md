# zing-explore

輕量、快速、可自訂的 Windows 11 檔案總管（執行檔 `zing.exe`）。Rust + Win32 + Direct2D，執行檔約 1 MB。

## 建置與執行

需要 Rust（MSVC 工具鏈）與 Visual Studio Build Tools（C++ 與 Windows SDK）。

```powershell
cargo build --release
.\target\release\zing.exe
```

`cargo build --release` 同時建置 `zing-index.exe`（NTFS 索引服務，見下方「資料夾大小」）。

## 目前功能（MVP）

- 單窗格 + 樹狀列，可分割成多個窗格（`Ctrl+\`），每個窗格有分頁（`Ctrl+T`）
- 詳細清單：名稱／修改日期／類型／大小，點欄位標題排序、拖曳欄位邊界調整寬度；每個資料夾記住自己的排序方式
- 直接打字即時篩選（支援 `*.rs` 萬用字元，中文輸入法的組字視窗對齊篩選標籤），`Esc` 清除
- 全磁碟搜尋（`Ctrl+F`）：以名稱即時搜尋所有本機 NTFS 磁碟（需要 NTFS 索引服務，見下方「資料夾大小」）。
  空白分隔多個詞（都要符合），`*` `?` 萬用字元，`"…"` 包住含空白的詞；`Enter` 前往所在的資料夾並選取，`Ctrl+Enter` 開啟
- 麵包屑網址列（`Ctrl+L` 直接輸入路徑）
- 複製／剪下／貼上（與檔案總管互通）、刪除到資源回收筒、重新命名（`F2`）、新資料夾（`Ctrl+Shift+N`）
- 批次重新命名（選取多個項目時 `F2`）：搜尋與取代（文字或規則運算式，`$1` 引用群組，`\w`、`\d`、`\p{Han}` 支援中文）、範圍（主檔名／副檔名／整個名稱）、
  編號 `{n}`、`{n:3}`（補零）、`{n:3:10}`（從 10 開始，只計算符合的項目）、大小寫轉換；即時預覽並標出衝突
  （重名、不合法字元、保留名稱），有衝突時不能執行。新舊名稱重疊（例如 01→02、02→03）時自動經過暫時名稱
- 系統右鍵選單（含第三方擴充功能，Shift+F10 / 選單鍵也可開啟）、拖放進出
- 系統檔案圖示（背景載入，執行檔與捷徑顯示各自的圖示）
- 預覽窗格（`Alt+P`）：文字檔（自動判斷 UTF-8／UTF-16／Big5）、PDF／Office／影片等以 Windows 預覽處理常式顯示
  （與檔案總管相同；沒有或沒回應時改顯示大縮圖）、圖片縮圖、資料夾的項目數，
  下方顯示與檔案總管詳細資料窗格相同的屬性；寬度可拖曳調整
- 縮圖檢視：`Ctrl+Shift+1`–`4` 切換清單／小／中／大縮圖，`Ctrl+滾輪` 連續調整（48–256 px），每個資料夾分別記住。
  縮圖來自 Windows（與檔案總管相同，安裝的縮圖處理常式都能用）；資料夾裡有 `folder.jpg/png` 或 `cover.jpg/png`
  時用它當資料夾縮圖；也可在設定檔用 `[[thumbnailer]]` 指定外部命令產生縮圖（見下方「自訂」）
- 我的最愛：`Ctrl+D` 或右鍵「加入我的最愛」，在樹狀列右鍵移除
- 資料夾內容變更自動更新
- 三種內建風格，各有深色／淺色並可跟隨系統：緊湊（預設，高密度、直角、完整路徑）、Fluent（寬鬆圓角）、
  折衷（Fluent 外觀、中等密度）；命令面板「主題：…」切換，`Ctrl+Shift+T` 切換深淺色時保留風格；可載入自訂主題檔
- 快速鍵、工具列按鈕全部由設定檔定義，命令可帶變數（`{sel}`、`{dest}`、`{path}`…）
- 資料夾大小：進入資料夾就顯示每個子資料夾的大小，可依大小排序（見下方「資料夾大小」）
- 樹狀列：看得到的節點先在背景載入子資料夾，點擊前就知道能不能展開（沒有子資料夾就不顯示箭頭），
  點了立即展開；網路磁碟、隨身碟不預先載入，一律顯示箭頭。展開中的資料夾在別處新增、刪除、改名子資料夾時自動更新
- 檔案作業佇列（`Ctrl+J`）：複製／移動／刪除依序執行，顯示進度與剩餘時間，可暫停、取消；重新命名等小作業不必排隊
- 命令面板（`Ctrl+Shift+P`）：模糊搜尋所有命令（中文標題或命令名稱，如 `tog hid`），顯示快速鍵，最近用過的排前面；輸入 `>` 開頭可直接執行命令腳本，`=` 開頭可計算 Lua 運算式
- Lua 腳本：`scripts\*.lua` 裡的全域函式就是命令（見下方「Lua 腳本」）

## 自訂

設定檔：`zing.exe` 旁的 `config.toml`（`Ctrl+,` 開啟，`Ctrl+Shift+R` 重新載入）。設定、主題與工作階段都放在程式所在的資料夾（可攜式），
重灌系統或換電腦時連同程式一起複製即可；請放在一般使用者可寫入的位置（不要放在 Program Files）。
範本見 [`crates/zing-core/src/default_config.toml`](crates/zing-core/src/default_config.toml)。

```toml
theme = "system"        # system | dark | light（緊湊）、fluent | balanced（跟隨系統）、fluent-dark…、自訂主題名稱

[[key]]
key = "F6"
cmd = "copy --move --to={dest}"

[[button]]
label = "用 VS Code 開啟"
cmd = "run code {sel}"
```

`run` 執行主控台程式（含 `.bat`、`.cmd`）時不顯示主控台視窗，視窗程式照常顯示；要開互動式的終端機時加上 `--console`，
例如 `run --console pwsh`。

自訂縮圖產生器：檔名符合 `match` 的檔案改用命令產生的 PNG（`{in}` 來源、`{out}` 輸出、`{size}` 邊長），
結果快取在 `%LOCALAPPDATA%\zing-explore\thumbs`，檔案修改後自動重做；命令最多執行 10 秒。

```toml
[[thumbnailer]]
match = "*.stl;*.3mf"
cmd = 'stl-thumb -s {size} "{in}" "{out}"'
```

## Lua 腳本

腳本放在 `zing.exe` 旁的 `scripts\*.lua`（Lua 5.4），啟動後在背景載入，命令面板「重新載入 Lua 腳本」可重新載入。
全域函式就是命令，上方的 `---` 註解是命令面板顯示的標題；`on_navigate(path)`、`thumbnail(path)` 是事件。
內建命令都能直接呼叫（`nav.refresh()`、`copy(sel(), { to = "D:\\備份" })`），清單可用 `|` 串接惰性的處理步驟：

```lua
--- 把選取的圖片移到「圖片」資料夾
function move_images()
  local imgs = sel() | filter(function(p) return path.ext(p) == "jpg" or path.ext(p) == "png" end) | list
  if #imgs == 0 then return msg("沒有選取圖片") end
  copy(imgs, { to = path.join(cwd(), "圖片"), move = true })
end
```

- 每次執行有 CPU 時間上限（命令 30 秒、縮圖規則 0.3 秒；等待輸入、外部程式不算），腳本在背景執行，不會卡住視窗
- `pick_folder()` 顯示選擇資料夾的對話方塊；`run{ ..., wait = true }`、`os.execute`、`io.popen` 執行外部程式時不顯示主控台視窗
- 錯誤顯示在狀態列，完整的呼叫堆疊記在 `scripts\error.log`；`log()` 寫入 `scripts\zing.log`
- 載入時在 `scripts\.zing\` 產生 LuaLS 定義檔與 `.luarc.json`：用 VS Code 的 Lua 擴充功能開啟 `scripts` 資料夾就有自動補全與說明
- 中斷點除錯：執行 `pwsh tools/install-vscode-ext.ps1` 安裝 VS Code 擴充功能，以 VS Code 開啟 `scripts` 資料夾，開著 zing 按 F5 附加；
  支援中斷點、條件中斷點、記錄點、單步、呼叫堆疊、變數、求值，腳本出錯時停在錯誤發生處

範例（複製到 `zing.exe` 旁的 `scripts\`，命令面板執行「重新載入 Lua 腳本」）：

- [extract.lua](examples/scripts/extract.lua)：批次解壓縮到指定資料夾，使用 Windows 內建的 tar.exe
- [rename_pattern.lua](examples/scripts/rename_pattern.lua)：以 Lua 樣式批次重新命名（保留副檔名、名稱衝突加序號），可復原
- [checksum.lua](examples/scripts/checksum.lua)：計算 SHA-256 寫成 `.sha256` 檔（單一檔案時也複製到剪貼簿），或以 `.sha256` 檔驗證（certutil、`io.popen`）
- [organize.lua](examples/scripts/organize.lua)：依副檔名移到「圖片」「影片」「文件」等子資料夾，或依修改日期移到「年-月」（`stat()`）
- [folder_view.lua](examples/scripts/folder_view.lua)：事件函式，相片資料夾自動切換成縮圖、資料夾以封面圖片當縮圖

完整的函式說明見 [docs/lua.md](docs/lua.md)（由程式碼產生，永遠與程式一致）。

主題（命令面板）：
- 「編輯目前的主題」：右側面板列出配色、字型、尺寸、外觀，修改立即套用；顏色可點色塊選色或輸入 `#rrggbb`，
  數字可滾輪微調。「另存新主題」只存與內建基礎不同的欄位；內建主題不能覆蓋；「取消」還原
- 「用文字編輯器開啟目前的主題檔」：存檔後自動套用
- 「匯入主題…」「匯出目前的主題…」：`.zingtheme` 檔，也可以直接把 `.zingtheme` 拖進視窗匯入
- 最後選用的主題會記住；修改設定檔的 `theme` 並重新載入時以設定檔為準

主題檔放在 `zing.exe` 旁的 `themes\<名稱>.toml`，只需寫要改的欄位，其餘沿用 `style`（compact | fluent | balanced）
與 `mode`（dark | light）選定的內建主題：

```toml
style = "fluent"
mode = "dark"
[color]
accent = "#4cc2ff"
[metrics]
row_height = 30
[chrome]
address = "path"        # path（完整路徑）| breadcrumb（麵包屑）
```

## 資料夾大小與全磁碟搜尋

- **NTFS 索引服務（建議）**：命令面板執行「啟用 NTFS 索引服務」（會跳出 UAC；更新 zing 後再執行一次即可更新服務）。
  服務讀取每顆本機 NTFS 磁碟的主檔案表（MFT）建立索引，之後跟著 NTFS 變更日誌更新，所以任何資料夾的大小都能立即顯示，
  也會隨檔案變動自動更新；全磁碟搜尋（`Ctrl+F`）也由它提供。
  做法與 Everything 相同：只有服務（`zing-index`，LocalSystem）有讀取磁碟區的權限，`zing.exe` 維持一般權限，
  每次查詢服務都以呼叫者的身分確認能列出該資料夾；搜尋結果只包含你能列出的資料夾中的項目。
  - 索引的記憶體約每 100 萬筆記錄 45 MB（含所有檔名）；搜尋 100 萬個名稱約 10 ms
  - 第一次建立索引約數秒到數十秒；之後開機從快照與變更日誌接續，不必重新掃描
  - 大小是邏輯大小；有硬連結的檔案只算一次（檔案總管「內容」會重複計算，`C:\Windows` 因此差數 GB）；隱藏與系統檔案都計入
  - 服務執行檔安裝在 `%ProgramFiles%\zing-explore`，快照與記錄檔在服務帳號的
    `C:\Windows\System32\config\systemprofile\AppData\Local\zing-explore`
  - 命令列：`zing-index --status`、`--install`、`--uninstall`；以系統管理員身分執行
    `zing-index --scan C: --verify C:\Users` 可量測讀取 MFT 的時間並與逐資料夾掃描比對
- **沒有索引時**（服務未啟用、非 NTFS 磁碟、網路磁碟）：設定 `[folder_sizes] auto = true`（預設）時，進入本機固定磁碟上的資料夾
  就以背景優先權逐資料夾計算；`Ctrl+Shift+S` 計算選取的（或全部）資料夾。結果快取在 `%LOCALAPPDATA%\zing-explore\folder-sizes.tsv`。

## 效能

每次變更都要通過 `pwsh tools/perf.ps1` 與 `pwsh tools/smoke.ps1`（門檻與規則見 [CLAUDE.md](CLAUDE.md)）。
