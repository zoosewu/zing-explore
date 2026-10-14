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
- 直接打字即時篩選（支援 `*.rs` 萬用字元），`Esc` 清除
- 麵包屑網址列（`Ctrl+L` 直接輸入路徑）
- 複製／剪下／貼上（與檔案總管互通）、刪除到資源回收筒、重新命名（`F2`）、新資料夾（`Ctrl+Shift+N`）
- 系統右鍵選單（含第三方擴充功能，Shift+F10 / 選單鍵也可開啟）、拖放進出
- 系統檔案圖示（背景載入，執行檔與捷徑顯示各自的圖示）
- 我的最愛：`Ctrl+D` 或右鍵「加入我的最愛」，在樹狀列右鍵移除
- 資料夾內容變更自動更新
- 三種內建風格，各有深色／淺色並可跟隨系統：緊湊（預設，高密度、直角、完整路徑）、Fluent（寬鬆圓角）、
  折衷（Fluent 外觀、中等密度）；命令面板「主題：…」切換，`Ctrl+Shift+T` 切換深淺色時保留風格；可載入自訂主題檔
- 快速鍵、工具列按鈕全部由設定檔定義，命令可帶變數（`{sel}`、`{dest}`、`{path}`…）
- 資料夾大小：進入資料夾就顯示每個子資料夾的大小，可依大小排序（見下方「資料夾大小」）
- 樹狀列：看得到的節點先在背景載入子資料夾，點擊前就知道能不能展開（沒有子資料夾就不顯示箭頭），
  點了立即展開；網路磁碟、隨身碟不預先載入，一律顯示箭頭
- 檔案作業佇列（`Ctrl+J`）：複製／移動／刪除依序執行，顯示進度與剩餘時間，可暫停、取消；重新命名等小作業不必排隊
- 命令面板（`Ctrl+Shift+P`）：模糊搜尋所有命令（中文標題或命令名稱，如 `tog hid`），顯示快速鍵，最近用過的排前面；輸入 `>` 開頭可直接執行命令腳本

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

## 資料夾大小

- **NTFS 索引服務（建議）**：命令面板執行「啟用 NTFS 索引服務」（會跳出 UAC）。服務讀取每顆本機 NTFS 磁碟的
  主檔案表（MFT）建立索引，之後跟著 NTFS 變更日誌更新，所以任何資料夾的大小都能立即顯示，也會隨檔案變動自動更新。
  做法與 Everything 相同：只有服務（`zing-index`，LocalSystem）有讀取磁碟區的權限，`zing.exe` 維持一般權限，
  每次查詢服務都以呼叫者的身分確認能列出該資料夾。
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
