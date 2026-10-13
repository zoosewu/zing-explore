# 專案：Windows 11 檔案總管（fx）

Rust + 原生 Win32（`windows-rs`）+ Direct2D/DirectWrite 自繪 UI，腳本使用 Lua（尚未實作）。
規格文件：https://claude.ai/code/artifact/335464a0-428e-4b77-b122-94f641201d9f

## 核心開發注意事項：效能是硬性門檻

每次開發（新功能、修正、重構、加相依套件）都必須關注並測試以下指標。
任一指標超標，或相對基準退步（超過 5% 且超過該指標的絕對容差），就不算完成、不合併。

| 指標 | 門檻 | 測法 |
| --- | --- | --- |
| 冷啟動到可操作 | < 150 ms | 程序建立 → 第一幀呈現，10 次中位數 |
| 閒置記憶體（單視窗） | < 30 MB | 啟動 5 秒後讀 Private Working Set |
| 開啟 10 萬檔資料夾首屏 | < 500 ms | 固定測試資料夾，量「進入 → 首屏繪完」 |
| UI 幀時間 | p99 < 16 ms | 捲動 300 幀，3 次取中位數 |
| 執行檔大小 | < 10 MB | release build |
| 主題切換 | < 50 ms | 切換 → 重繪完成 |

### 每次變更的流程

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets      # 必須 0 警告
pwsh tools/perf.ps1                         # 建置 release 並檢查全部指標，失敗時結束代碼 1
cargo bench -p fx-core --bench core         # 動到排序／篩選／列舉時
```

- 把 `tools/perf.ps1` 的結果表貼進 commit / PR 說明。
- 只有在確認是改善（或刻意接受的取捨）時才執行 `pwsh tools/perf.ps1 -UpdateBaseline` 更新 `perf/baseline.json`，並在說明中寫明原因。
- frame_p99 受 OS/GPU 偶發停頓影響，同一版本重跑約有 ±1.5 ms 差異，因此退步容差為 2 ms；繪製成本的實際退步由 frame_p50（容差 0.5 ms）把關。

### 規則

- UI 執行緒禁止：檔案 I/O、網路、等待鎖、會阻塞的 Shell 呼叫。工作交給 `services::Pool`，結果經 `Notifier` 送回。
- 所有背景任務都必須可取消（換資料夾時中止舊任務），結果分批送回 UI；以 generation 丟棄過時結果。
- 清單一律虛擬化：只處理可見的列；昂貴的欄位（圖示、縮圖、屬性）延遲到捲到才計算。
- 右鍵選單、內容對話方塊、剪貼簿一律交給 `ShellHost`（shellhost.rs）：獨立的 STA 執行緒，UI 只送請求不等待，結果以訊息送回。主機忙碌超過 3 秒時下一個請求會改用新主機。
- `ShellHost` 的視窗必須是 message-only，執行緒上不可留下頂層視窗：系統啟用視窗時會同步送 WM_ACTIVATEAPP 給程序內所有頂層視窗，主機卡住會連帶卡住 UI。選單的擁有者視窗只在選單顯示期間存在。
- 拖曳出去與放置目標受 COM 限制必須在 UI 執行緒，透過 `App::modal` 在釋放 App 借用後執行；放置目標每次拖曳只解析一次路徑（快取）。
- 測試卡住的 Shell 擴充功能：debug build 設定 `FX_TEST_SHELL_DELAY_MS=8000`，第一個右鍵選單會延遲；UI 必須仍可操作，3 秒後再按右鍵會改用新主機。
- 新增相依套件前，評估對執行檔大小與啟動時間的影響。
- 啟動路徑上不放非必要初始化，能延後的就延後（OLE、拖放在第一幀之後初始化）。
- 第一幀用 WARP 繪製，硬體 GPU 裝置在第一幀呈現後於背景建立再切換；不要在第一幀前碰硬體裝置（驅動初始化約 130 ms，且會鎖住 DXGI）。
- 分析工具：`--perf-out` 模式的 `phase_*` 時間點、Windows Performance Recorder/Analyzer（ETW）、`criterion`；需要符號時用 `cargo build --profile profiling`。

### 已知的效能缺口（待處理）

- 拖放（拖出的資料物件、放置目標）仍在 UI 執行緒解析路徑，這是 COM 的限制；網路路徑在拖曳經過時可能短暫延遲。
- 捲動測試偶有 20–35 ms 的單幀停頓（frame_max），p99 不受影響，原因未查明。
- 尚未有 debug build 的「UI 執行緒 I/O」防呆檢查。

## 專案結構

- `crates/fx-core`：與 UI 無關、可測試的核心（列舉、排序／篩選、命令語言、快速鍵、主題、設定）。
- `crates/fx-app`：Win32 視窗、繪製（`gfx`、`view`）、輸入、Shell 整合（`shell`）、背景服務（`services`）、效能模式（`perf`）。
- 使用者資料：`%APPDATA%\fx\config.toml`（設定）、`themes\*.toml`（主題）、`session.toml`（上次的視窗與分頁）。

## 已確認的決策

- 語言 Rust；腳本 Lua。
- 預設佈局：單窗格 + 樹狀列；佈局可自訂（樹狀列可移動、關閉、換位置；窗格可增加）。
- 主題系統：可匯入匯出，主題檔只含資料、不含腳本。
- 介面風格（Fluent 或緊湊）尚未決定，以內建主題實現。
- 自用為主，但會開源。
