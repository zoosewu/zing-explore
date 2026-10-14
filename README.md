# zing-explore

輕量、快速、可自訂的 Windows 11 檔案總管（執行檔 `zing.exe`）。Rust + Win32 + Direct2D，執行檔約 1 MB。

## 建置與執行

需要 Rust（MSVC 工具鏈）與 Visual Studio Build Tools（C++ 與 Windows SDK）。

```powershell
cargo build --release
.\target\release\zing.exe
```

## 目前功能（MVP）

- 單窗格 + 樹狀列，可分割成多個窗格（`Ctrl+\`），每個窗格有分頁（`Ctrl+T`）
- 詳細清單：名稱／修改日期／類型／大小，點欄位標題排序、拖曳欄位邊界調整寬度
- 直接打字即時篩選（支援 `*.rs` 萬用字元），`Esc` 清除
- 麵包屑網址列（`Ctrl+L` 直接輸入路徑）
- 複製／剪下／貼上（與檔案總管互通）、刪除到資源回收筒、重新命名（`F2`）、新資料夾（`Ctrl+Shift+N`）
- 系統右鍵選單（含第三方擴充功能，Shift+F10 / 選單鍵也可開啟）、拖放進出
- 系統檔案圖示（背景載入，執行檔與捷徑顯示各自的圖示）
- 我的最愛：`Ctrl+D` 或右鍵「加入我的最愛」，在樹狀列右鍵移除
- 資料夾內容變更自動更新
- 深色／淺色主題、跟隨系統，可載入自訂主題檔
- 快速鍵、工具列按鈕全部由設定檔定義，命令可帶變數（`{sel}`、`{dest}`、`{path}`…）
- 資料夾大小（`Ctrl+Shift+S`）：背景計算選取的或全部資料夾，可依大小排序；設定 `[folder_sizes] auto = true` 可在進入資料夾時自動計算（只限本機固定磁碟）
- 檔案作業佇列（`Ctrl+J`）：複製／移動／刪除依序執行，顯示進度與剩餘時間，可暫停、取消；重新命名等小作業不必排隊
- 命令面板（`Ctrl+Shift+P`）：模糊搜尋所有命令（中文標題或命令名稱，如 `tog hid`），顯示快速鍵，最近用過的排前面；輸入 `>` 開頭可直接執行命令腳本

## 自訂

設定檔：`zing.exe` 旁的 `config.toml`（`Ctrl+,` 開啟，`Ctrl+Shift+R` 重新載入）。設定、主題與工作階段都放在程式所在的資料夾（可攜式），
重灌系統或換電腦時連同程式一起複製即可；請放在一般使用者可寫入的位置（不要放在 Program Files）。
範本見 [`crates/zing-core/src/default_config.toml`](crates/zing-core/src/default_config.toml)。

```toml
theme = "dark"          # dark | light | system | 自訂主題名稱

[[key]]
key = "F6"
cmd = "copy --move --to={dest}"

[[button]]
label = "用 VS Code 開啟"
cmd = "run code {sel}"
```

主題檔放在 `zing.exe` 旁的 `themes\<名稱>.toml`，只需寫要改的欄位，其餘沿用內建值。

## 效能

每次變更都要通過 `pwsh tools/perf.ps1` 與 `pwsh tools/smoke.ps1`（門檻與規則見 [CLAUDE.md](CLAUDE.md)）。
