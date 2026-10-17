# zing-explore

繁體中文 | [English](README.en.md) | [日本語](README.ja.md)

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
- 全磁碟搜尋（`Ctrl+F`）：以名稱、路徑即時搜尋所有本機 NTFS 磁碟，可依類型、大小、修改日期篩選與排序（需要 NTFS 索引服務，見下方「資料夾大小與全磁碟搜尋」）。
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
  時用它當資料夾縮圖；也可在設定檔用 `[[thumbnailer]]` 指定外部命令產生縮圖（見下方「自訂」）。
  個別檔案或資料夾可在右鍵選單「設定縮圖…」手動指定圖片（改名、在同一磁碟區內移動後仍有效；「移除自訂縮圖」還原）
- 我的最愛：`Ctrl+D` 或右鍵「加入我的最愛」，在樹狀列右鍵移除；在樹狀列按 `Ctrl+↑`／`Ctrl+↓` 或右鍵「上移／下移」調整順序
  （順序記在 `session.toml`，設定檔中的我的最愛也能調整）
- 資料夾內容變更自動更新
- 三種內建風格，各有深色／淺色並可跟隨系統：緊湊（預設，高密度、直角、完整路徑）、Fluent（寬鬆圓角）、
  折衷（Fluent 外觀、中等密度）；命令面板「主題：…」切換，`Ctrl+Shift+T` 切換深淺色時保留風格；可載入自訂主題檔
- 設定頁（`Ctrl+,` 或工具列右上角的齒輪）：介面語言、主題、樹狀列與命令面板的位置、顯示隱藏檔案、啟動資料夾、資料夾大小等常用設定，
  修改立即生效，並寫回 `config.toml` 中的那一行（保留檔案的註解與排列）；也能從這裡開啟快速鍵、主題編輯器與設定檔
- 所有操作都能用滑鼠完成，不必記命令：工具列下方的命令列（Windows 11 檔案總管的樣式）有新增▾、剪下、複製、貼上、重新命名、刪除、
  排序▾、檢視▾，其他操作都在「⋯」裡（分組的子選單，包括 Lua 腳本的命令）。不能用的按鈕變灰色；游標停在按鈕上會顯示名稱、
  快速鍵與不能用的原因；選單中快速鍵靠右顯示。視窗變窄時先收起文字，再把按鈕移進「⋯」。命令面板（`Ctrl+Shift+P`）留給熟悉命令的人
- 自訂命令列（「⋯ → 自訂命令列…」或在命令列上按右鍵）：拖曳排序、加入任何內建命令或腳本、改名稱、從 Fluent 圖示中挑或用自己的圖片檔；
  在「⋯」的項目上按右鍵可直接「加到命令列」。結果存在 `zing.exe` 旁的 `toolbar.toml`。設定頁可隱藏命令列或只顯示圖示
- 快速鍵編輯器（「⋯ → 編輯快速鍵…」）：命令依分組列出，點「＋」後按下組合鍵即可加上；已被其他命令使用時先詢問「取代」或「取消」；
  游標停在按鍵上可移除、↺ 還原、「全部還原」；可依名稱或「依按鍵搜尋」。結果存在 `zing.exe` 旁的 `keys.toml`（優先於 `config.toml`，不會改寫設定檔）
- 進階：快速鍵與命令可寫在設定檔，命令可帶變數（`{sel}`、`{dest}`、`{path}`…；`{ask:提示}` 執行時詢問）
- 介面語言：繁體中文、英文、日文，預設跟隨 Windows 的顯示語言，可在設定檔指定 `language = "en"`（`auto`／`zh-TW`／`en`／`ja`）
- 資料夾大小：進入資料夾就顯示每個子資料夾的大小，可依大小排序（見下方「資料夾大小」）
- 樹狀列：看得到的節點先在背景載入子資料夾，點擊前就知道能不能展開（沒有子資料夾就不顯示箭頭），
  點了立即展開；網路磁碟、隨身碟不預先載入，一律顯示箭頭。展開中的資料夾在別處新增、刪除、改名子資料夾時自動更新
- 檔案作業佇列（`Ctrl+J`）：複製／移動／刪除依序執行，顯示進度、速度（MB/s）與剩餘時間，可暫停、取消；重新命名等小作業不必排隊
- 命令面板（`Ctrl+Shift+P`）：模糊搜尋所有命令（中文標題或命令名稱，如 `tog hid`），顯示快速鍵，最近用過的排前面；輸入 `>` 開頭可直接執行命令腳本，`=` 開頭可計算 Lua 運算式
- Lua 腳本：`scripts\*.lua` 裡的全域函式就是命令（見下方「Lua 腳本」）

## 自訂

設定檔：`zing.exe` 旁的 `config.toml`（`Ctrl+,` 開啟設定頁，設定頁的「編輯設定檔」或命令面板以記事本開啟，`Ctrl+Shift+R` 重新載入）。設定、主題與工作階段都放在程式所在的資料夾（可攜式），
重灌系統或換電腦時連同程式一起複製即可；請放在一般使用者可寫入的位置（不要放在 Program Files）。
範本見 [`crates/zing-core/src/default_config.toml`](crates/zing-core/src/default_config.toml)。

```toml
language = "auto"       # auto（跟隨 Windows）| zh-TW | en | ja
theme = "system"        # system | dark | light（緊湊）、fluent | balanced（跟隨系統）、fluent-dark…、自訂主題名稱

[[key]]
key = "F6"
cmd = "copy --move --to={dest}"

[[button]]               # 加在命令列的「⋯」前面（還沒有 toolbar.toml 時；之後用「自訂命令列」調整）
label = "用 VS Code 開啟"
cmd = "run code {sel}"
```

`run` 執行主控台程式（含 `.bat`、`.cmd`）時不顯示主控台視窗，視窗程式照常顯示；要開互動式的終端機時加上 `--console`，
例如 `run --console pwsh`。

`{ask:提示}` 在執行前顯示輸入框，回答後代入（同一個提示只問一次，`Esc` 取消整個命令），例如
`cmd = "run git commit -m {ask:提交訊息}"`。

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

完整的函式說明見 [docs/lua.md](docs/lua.md)（由程式碼產生，永遠與程式一致；另有 [English](docs/lua.en.md)、[日本語](docs/lua.ja.md)）。

主題（命令面板）：
- 「編輯目前的主題」（或設定頁的「自訂主題…」）：右側面板，修改立即套用到整個畫面。不必知道色碼：
  風格與深淺色從選單挑（換了會保留已改的部分）；顏色點一下展開色票，或「更多顏色…」、直接輸入色碼；數字用滑桿、−／＋或滾輪；
  字型從系統安裝的字型中挑；「檔案顏色」依類別（圖片、影片、音樂…）新增；改過的欄位旁有 ↺ 可還原。
  「儲存」在內建主題上會詢問名稱存成自己的主題（只存與內建基礎不同的欄位）；「取消」還原
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

- **NTFS 索引服務（建議）**：命令面板執行「啟用 NTFS 索引服務」（會跳出 UAC）。更新 zing 後，若服務是舊版，zing 啟動時會自動更新它（同樣跳出 UAC 確認；取消的話下次啟動再詢問）。
  服務讀取每顆本機 NTFS 磁碟的主檔案表（MFT）建立索引，之後跟著 NTFS 變更日誌更新，所以任何資料夾的大小都能立即顯示，
  也會隨檔案變動自動更新；全磁碟搜尋（`Ctrl+F`）也由它提供。
  做法與 Everything 相同：只有服務（`zing-index`，LocalSystem）有讀取磁碟區的權限，`zing.exe` 維持一般權限，
  每次查詢服務都以呼叫者的身分確認能列出該資料夾；搜尋結果只包含你能列出的資料夾中的項目。
  - 索引的記憶體約每 100 萬筆記錄 47 MB（含所有檔名與修改時間）；搜尋 100 萬個名稱約 10–20 ms
  - 第一次建立索引約數秒到數十秒；之後開機從快照與變更日誌接續，不必重新掃描
  - 大小是邏輯大小；有硬連結的檔案只算一次（檔案總管「內容」會重複計算，`C:\Windows` 因此差數 GB）；隱藏與系統檔案都計入
  - 服務執行檔安裝在 `%ProgramFiles%\zing-explore`，快照與記錄檔在服務帳號的
    `C:\Windows\System32\config\systemprofile\AppData\Local\zing-explore`
  - 命令列：`zing-index --status`、`--install`、`--uninstall`；以系統管理員身分執行
    `zing-index --scan C: --verify C:\Users` 可量測讀取 MFT 的時間並與逐資料夾掃描比對
- **沒有索引時**（服務未啟用、非 NTFS 磁碟、網路磁碟）：設定 `[folder_sizes] auto = true`（預設）時，進入本機固定磁碟上的資料夾
  就以背景優先權逐資料夾計算；`Ctrl+Shift+S` 計算選取的（或全部）資料夾。結果快取在 `%LOCALAPPDATA%\zing-explore\folder-sizes.tsv`。

### 全磁碟搜尋的語法

以空白分隔多個詞，每個都要符合；`"…"` 包住含空白的詞，`!` 開頭表示排除。不分大小寫（含 é、Σ、全形字母等）。

| 寫法 | 意思 |
| --- | --- |
| `報告 2024` | 名稱同時含「報告」與「2024」 |
| `*.jpg`、`img_????.png` | 萬用字元比對整個名稱（`?` 是一個字元） |
| `src\ main`、`c:\users\*\桌面\*` | 含 `\` 的詞比對完整路徑 |
| `!node_modules\`、`!*.tmp` | 排除 |
| `ext:jpg;png` | 副檔名（只有檔案） |
| `size:>10mb`、`size:<=1k`、`size:1mb..1gb` | 大小（資料夾為整個資料夾的大小） |
| `dm:today`、`dm:yesterday`、`dm:7d`、`dm:2024-05`、`dm:>2024-05-01`、`dm:2024-01..2024-03` | 修改日期 |
| `file:`、`folder:` | 只找檔案／資料夾 |
| `sort:size`、`sort:date`、`sort:name`、`sort:path`、`sort:ext`（可加 `-asc`／`-desc`） | 排序；大小、日期預設由大到小；名稱、路徑、副檔名為自然排序（`file2` 在 `file10` 前） |

在搜尋面板按 `Ctrl+1`–`Ctrl+5` 依名稱、路徑、副檔名、大小、日期排序（改寫查詢中的 `sort:`，再按一次反向），`Ctrl+0` 取消排序；
輸入框右側顯示目前的排序與結果數。

沒有指定排序時結果依磁碟、建立順序邊找邊列出（最快）；排序時要先比對完所有磁碟，最多約 3 秒，超過時只排序已找到的部分（顯示「500+ 個」）。
資料夾的修改日期只在它自己的記錄有變動時更新（新增或刪除裡面的項目不一定會更新）。

## 效能

每次變更都要通過 `pwsh tools/perf.ps1` 與 `pwsh tools/smoke.ps1`（門檻與規則見 [CLAUDE.md](CLAUDE.md)）。

## 翻譯

介面文字以繁體中文為原文，翻譯在 `crates/zing-core/i18n/en.toml`、`ja.toml`（`"原文" = "翻譯"`）。
`cargo test -p zing-core i18n` 會檢查每個原文都有翻譯、參數名稱一致；`ZING_I18N_MISSING=1` 時列出缺少的條目。
Lua API 文件（`docs/lua*.md`）由 `scriptapi.rs` 與同一份翻譯產生。
