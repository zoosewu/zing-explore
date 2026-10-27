# 專案：Windows 11 檔案總管 zing-explore（執行檔 zing.exe）

Rust + 原生 Win32（`windows-rs`）+ Direct2D/DirectWrite 自繪 UI，腳本使用 Lua 5.4（`mlua`）。
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
pwsh tools/smoke.ps1                        # 自動化 UI 冒煙測試（debug build，含 I/O 防呆與卡頓監看）
cargo bench -p zing-core --bench core         # 動到排序／篩選／列舉時
```

- 把 `tools/perf.ps1` 的結果表貼進 commit / PR 說明。
- `tools/smoke.ps1` 在程序內送出鍵盤、滑鼠、拖放訊息（`--smoke-*` 模式，見 `smoke.rs`），不動真實游標、不搶前景，
  使用者操作電腦時也能跑；截圖存在 `target\smoke\`。新增會改變操作行為的功能時，在 `smoke.rs` 加上對應步驟。
  剪貼簿：開始前備份使用者的剪貼簿（所有記憶體區塊格式），測試寫入的內容帶 `ZingSmokeTestContent` 標記且不進入剪貼簿歷程，
  結束時剪貼簿上仍有標記就還原（不論步驟成敗），沒有標記但已改變（使用者自己複製）就不動。目前只測文字（Lua `clipboard()`、
  複製路徑）；檔案的複製／貼上（OLE 資料物件）、右鍵選單、刪除（會搶前景、留東西在資源回收筒）仍需手動測試。
- 只有在確認是改善（或刻意接受的取捨）時才執行 `pwsh tools/perf.ps1 -UpdateBaseline` 更新 `perf/baseline.json`，並在說明中寫明原因。
- 發佈：在 master 上把 `Cargo.toml` 的 `version` 改好、push `v<版本>` 標籤，`.github/workflows/release.yml` 會跑 clippy、
  單元測試、release 的冒煙測試、效能檢查，全部通過才建立 GitHub Release（`zing-explore-windows-x64.zip`、`.sha256`、`perf.json`；
  檔名不含版本，`releases/latest/download/` 永遠是最新版）。CI 的效能基準是上一個 Release 附的 `perf.json`（不是
  `perf/baseline.json`，機器不同），相對容差 15%；刻意接受的退步以手動執行（選標籤、勾選 allow_regression）發佈。
  工具鏈固定在 `rust-toolchain.toml`。
- 測試資料（單元測試、冒煙測試、效能測試的 10 萬檔資料夾、隔離模式的快取）放在環境變數 `ZING_TEST_DIR` 指定的資料夾，
  沒有設定時用系統暫存資料夾；Rust 端一律經 `zing_core::testdir::dir()`，不要直接用 `std::env::temp_dir()`。
- 效能模式的視窗固定在 (100, 100)、1280×800，結果才可重現。
- 效能模式等硬體 GPU 裝置就緒後才量捲動（最多 3 秒）；等待期間背景結果造成的重繪不會提早結束等待。
  結果的 `scroll_warp` 為 1 表示是用 WARP（軟體）量的，數字不能和基準比較。
- frame_p99 受 OS/GPU 偶發停頓影響，同一版本重跑約有 ±1.5 ms 差異，因此退步容差為 2 ms；繪製成本的實際退步由 frame_p50（容差 0.5 ms）把關。

### 規則

- UI 執行緒禁止：檔案 I/O、網路、等待鎖、會阻塞的 Shell 呼叫。工作交給 `services::Pool`，結果經 `Notifier` 送回。
- 會阻塞的操作開頭呼叫 `zing_core::guard::io("說明")`。debug build 在 UI 執行緒呼叫時，狀態列會顯示錯誤與程式位置；確有必要的例外用 `let _allow = guard::allow_io("原因")` 包住並寫明原因。debug build 另有卡頓監看：UI 訊息處理超過 100 ms 會回報。
- 不必模擬輸入也能測命令：`zing --perf-out=結果.json --perf-dir=資料夾 --perf-idle-ms=2000 --perf-exec="命令"`，debug build 的防呆回報會寫進結果的 `guard_violations`（應為空陣列）。注意 `--perf-exec` 會真的執行命令，不要拿有外部副作用的命令（如 `config.edit`、檔案作業）在真實資料上測。
- 所有背景任務都必須可取消（換資料夾時中止舊任務），結果分批送回 UI；以 generation 丟棄過時結果。
- 檔案作業一律經 `App::submit_op`：複製／移動／刪除進作業佇列（依序執行、顯示進度，可暫停／取消），
  重新命名與新增資料夾走另一條執行緒，不會被大量複製擋住。UI 只透過 `JobCtl` 的 atomic 旗標控制工作，不等待鎖；
  暫停／取消在 Shell 的進度回呼（`shell.rs` 的 `Progress`）中生效。冒煙測試用 `shell::TEST_PROGRESS_DELAY_MS` 放慢複製。
- 清單一律虛擬化：只處理可見的列；昂貴的欄位（圖示、縮圖、屬性）延遲到捲到才計算。
  繪製時不掃過整個清單：選取數由 `Tab` 在選取、清單、篩選改變時更新（`selected` 只經 `Tab` 的方法修改，
  debug build 讀取時會以掃描結果核對）。
- 縮圖同樣只取看得到的格子（`thumbs.rs`）：繪製時登記，繪製結束時取消已不可見、還沒開始的請求；
  本機兩條、網路磁碟與光碟一條執行緒（背景模式）。清單與縮圖的幾何一律經 `App::grid`／`App::item_rect`，
  不要再自行以 `row_h` 換算位置。
- 單行輸入框（`App::edit`，一次一個）的每種行為都在 `edit.rs` 的 `EditKind`：按鍵與失去焦點（`subclass_data`）、
  內容改變（`edit_changed`）、結束（`finish_edit`）、是否屬於右側面板；那裡的 match 不用 `_`。window.rs 只建立輸入框、轉送訊息。
- 批次重新命名（`batch.rs`、`zing_core::batchrename`）：兩個輸入框不佔用 `app.edit`；輸入框的按鍵以 `WM_APP_BATCH`
  post 給主視窗處理（不在輸入框自己的訊息處理中改變面板）。
- 預覽窗格（`preview.rs`）：焦點停留 150 ms 才讀取，在專用的 STA 執行緒只處理最新的請求，以 generation 丟棄過時結果。
  Windows 預覽處理常式（`prevhost.rs`）在專用 STA 執行緒上、以**沒有擁有者**的頂層彈出視窗承載（跨執行緒的父子或擁有者
  關係都會連結輸入佇列）；UI 只用 PostThreadMessage、SWP_ASYNCWINDOWPOS、ShowWindowAsync 操作它，從不等待；
  看門狗 3 秒，逾時就放棄該執行緒並改顯示縮圖。
- 樹狀列同樣只對看得到的節點預先載入子資料夾（`App::schedule_tree_probes`，在繪製前送出、第一幀之後才開始），
  在背景模式的執行緒池執行；有索引時問索引，否則只在本機固定磁碟列舉，網路磁碟、隨身碟不預先載入。
- 不急的背景工作（資料夾大小、預先載入）用 `Pool::background`：磁碟與 CPU 優先權最低，不影響使用者正在做的事。
- 資料夾大小與樹狀列優先向 NTFS 索引服務（`crates/zing-index`）查詢，沒有服務時才逐資料夾掃描。隔離模式
  （效能、冒煙測試）不使用索引，結果才可重現。服務以 LocalSystem 執行，修改時遵守：每個查詢先以呼叫者身分
  （`ImpersonateNamedPipeClient`）開啟資料夾，無權列出就拒絕；只回答大小與名稱（不提供檔案內容）；
  全磁碟搜尋的每個結果都先以呼叫者身分開啟它所在的資料夾，能列出且是索引中的同一個資料夾才回傳；管線拒絕遠端連線；
  修改 zing-index（行為、協定、快照格式）時把 `indexproto::SERVICE_VERSION` 加 1：zing 啟動時比對已安裝服務的版號，
  較舊就以 UAC 執行程式旁的 `zing-index --install` 更新；
  服務執行檔只放在系統管理員才能寫入的位置；快照與記錄檔只放在 SYSTEM 與系統管理員能讀取的位置。
- 全磁碟搜尋（`search.rs`）沿用命令面板（`Palette::search`），輸入停頓後在工作池查詢，以 generation 丟棄過時的結果；
  冒煙測試以 `search::TEST_RESULTS` 代替服務（`test_search` 在程序內比對與排序）。查詢語法（路徑、篩選、排序）與比對在
  `zing_core::namequery`，索引的名稱、修改時間與搜尋在 `zing_core::volindex`，排序在服務端（`server.rs` 的 `search`）
  （改動時跑 `cargo bench -p zing-core --bench core -- ntfs`，`search_1m_*` 不可明顯退步）。
- 樹狀列展開中的節點由 `services::TreeWatch` 監看（一條執行緒、只看子資料夾名稱，最多 63 個），變更合併 250 ms 後
  以 `TreeLoad::Refresh` 重新載入並 `reconcile`。
- 分頁開著的資料夾被刪除或改名（資料夾監看看不到自己被刪除）：上層的子資料夾變更（樹狀列監看）、檔案作業完成時，
  在背景確認它下面的分頁（`services::check_gone`）；重新整理時找不到也一樣（`Msg::ListingError` 的 `gone`）。
  不在了就移到仍存在的上層（`App::on_folder_gone`）。只限列舉成功過的路徑（`Tab::shown`），輸入錯的路徑照樣顯示錯誤。
- 資料夾底下有開啟中的監看（分頁、樹狀列）時，Windows 拒絕改名、移動、刪除到資源回收筒（監看它自己不影響）。
  `App::submit_op` 送出前暫停 `FileOp::vacated` 之下的監看（`pause_watches`），`Msg::FileOpDone` 的 `vacated` 恢復；
  新增會移走項目的作業時同步更新 `vacated`。
- 右鍵選單、內容對話方塊、剪貼簿一律交給 `ShellHost`（shellhost.rs）：獨立的 STA 執行緒，UI 只送請求不等待，結果以訊息送回。主機忙碌超過 3 秒時下一個請求會改用新主機。
  右鍵選單最慢的是 `QueryContextMenu`（Windows 查詢動詞，文字檔約 200 ms；第一次載入擴充功能可達數秒），所以游標在清單停住 150 ms
  就送 `ShellJob::Prepare` 先建好（`shell::prepare_menu`），按下右鍵時項目相同、剪貼簿序號沒變、沒按 Shift 就直接顯示（`show_menu`）。
  停住與按下右鍵的對象（項目、資料夾）一律經 `Tab::menu_target`（樹狀列 `App::tree_menu_target`），不要另外計算，否則預先準備會悄悄失效。
  鍵盤（Shift+F10、選單鍵）同理：清單的按鍵停住 150 ms 就以 `Tab::key_menu_target`（樹狀列同上）準備，`menu.context` 用同一個函式。
  預先準備中（PREPARING）超過 10 秒才算卡住。各階段耗時：`cargo test --release -p zing-app ctx_menu_timing -- --ignored --nocapture`。
- 清單與樹狀列的右鍵選單預設是兩層（`ctxmenu.rs`，設定 `context_menu.style`）：第一層用 `menu.rs` 自繪（程式的命令＋`entry_menu_extras`＋
  第三方擴充功能；樹狀列的項目只對節點動作，是 `MENU_TREE_*` 而不是命令），立即出現；「顯示更多選項」、Shift+右鍵、「此電腦」才開完整的 Windows 選單。第三方擴充功能在 `shellext.rs`：
  預先準備時在 ShellHost 載入、計時、讀出項目（`Msg::MenuExtensions`），選了再以同一個物件執行（`ShellJob::InvokeExt`）；
  還沒準備好就不放（選單不會在顯示後變長）。緩慢的（依檔案／資料夾／背景分開判斷）不放進第一層（`context_menu.hide_slow`），`allow_slow` 列出的 CLSID 除外
  （設定頁的表格逐一切換，`hide_slow` 關閉時表格為灰階）。
  這台電腦會放進第一層的項目與耗時：`cargo test --release -p zing-app ext_items_here -- --ignored --nocapture`。
- `ShellHost` 的視窗必須是 message-only，執行緒上不可留下頂層視窗：系統啟用視窗時會同步送 WM_ACTIVATEAPP 給程序內所有頂層視窗，主機卡住會連帶卡住 UI。選單的擁有者視窗只在選單顯示期間存在。
- 拖曳出去與放置目標受 COM 限制必須在 UI 執行緒，透過 `App::modal` 在釋放 App 借用後執行；放置目標每次拖曳只解析一次路徑（快取）。
- 測試卡住的 Shell 擴充功能：debug build 設定 `ZING_TEST_SHELL_DELAY_MS=8000`，第一個右鍵選單會延遲；UI 必須仍可操作，3 秒後再按右鍵會改用新主機。
- 新增相依套件前，評估對執行檔大小與啟動時間的影響。
- 介面文字（含錯誤訊息、命令面板標題、Lua API 說明、zing-index 的命令列輸出）一律經 `tr!`（`zing_core::i18n`，原文為繁體中文）；
  常數表裡的原文用 `tr_key!` 標記、顯示時 `i18n::tr()`；Lua（prelude.lua、debugger.lua）用 `tr("…", { 名稱 = 值 })`。
  新增或修改原文時同步補上 `crates/zing-core/i18n/en.toml`、`ja.toml`，`cargo test -p zing-core i18n` 會檢查缺漏、多餘與參數名稱
  （`ZING_I18N_MISSING=1` 列出缺少的條目）。不翻譯：記錄檔、測試、`guard::io` 的說明、協定字串（`indexproto::UNPARSABLE`）。
  隔離模式（效能、冒煙測試）固定用繁體中文；冒煙測試另有切換成英文、日文的步驟（截圖 `14-英文介面`、`15-日文介面`）。
- 設定頁（`prefspage.rs`，命令 `settings.open`）：先選分類（`Pref::group`，順序依 `PREFS`）再選項目。項目定義在
  `zing_core::prefs::PREFS`，新增設定時同步加上 `Config::pref_value`／
  `set_pref`；改變後的副作用只寫在 `App::apply_config`（比較新舊設定，設定頁與 `config.reload` 共用）。寫回 config.toml 只改那一行（`prefs::write_pref`，保留註解），改不了（行內表、
  多行字串）就回報錯誤、不寫入；隔離模式不寫檔。與主題編輯面板共用右側位置，兩者只開一個。
- 介面要讓不會寫程式的人也能用：能用 UI 調整的就不要求改設定檔或寫命令（選單、開關、滑桿、色票，改了立即看到效果）；
  設定檔與命令腳本留給進階使用者。主題編輯面板在 `themepanel.rs`（檔案操作在 `themeedit.rs`）。
  命令面板視為只有工程師會用：每個操作都要能從命令列（`cmdbar.rs`，內容在 `zing_core::cmdbar`）、「⋯」選單、右鍵或面板完成
  （搜尋本來就是輸入框，除外）。內建命令只在 `zing_core::builtin` 宣告一次（`builtins!` 一列：`Cmd` 變體、分組、預設圖示、Lua 說明），
  `App::run` 對 `Cmd` 窮舉，漏了實作會編譯失敗；命令面板的標題與命令列的短名稱在同一檔的 `PRESETS`。
  新增命令時同步加進 `zing_core::cmdbar` 的選單（或確認已在其中）；
  不能用時在 `App::cmd_disabled` 回傳原因（按鈕變灰、提示顯示原因）。圖示按鈕都要有提示（`tooltip.rs`）。
  下拉選單用 `menu.rs`（D2D 自繪、跟著主題）。命令列可只用鍵盤操作（F10，`App::bar_focus`、`cmdbar_key`），新的按鈕種類
  也要能用鍵盤啟動。右側面板（設定、主題、自訂命令列、快速鍵）是 `App::side` 一個欄位（`sidepanel.rs`），一次只開一個；
  宿主只經過 `side_hit`／`side_click`／`side_wheel`／`side_escape`／`paint_side`，捲動、列的位置、捲到看得見（`side_reveal`，
  冒煙測試也用它找列）、底部按鈕是共用的。新面板加一個 `SidePanel` 變體與這些分派，不要再在 view.rs、input.rs 各接一次。
  鍵盤（`Focus::Side`：開啟面板或按 Tab 進入）：面板只提供一行中可按的目標（`side_targets`，命中與位置），
  ↑↓←→、Home／End、Enter／空白鍵、焦點框都在 sidepanel.rs；改了面板的命中（`*_hit`）就同步改 `*_targets`，
  冒煙測試「每一行的鍵盤目標與滑鼠命中一致」會檢查。
- 執行外部程式一律經 `services::start`／`services::launch`：`Show::HideConsole`（`run`、Lua 的 `run`）時主控台程式以
  CREATE_NO_WINDOW 執行；開啟檔案、網址列輸入維持 `Show::Normal`（與檔案總管相同）。Lua 的 `os.execute`、`io.popen`
  在 prelude 換成不顯示主控台視窗的版本。
- 冒煙測試工具自己的檔案 I/O（建立測試檔、清理）用 `harness_io` 包住：花的時間不算 UI 停頓（測試資料夾可能在慢的硬碟上），
  同一個訊息中被測程式碼的時間照樣監看；檢查階段整段扣除。
- Lua 腳本（`script.rs`、`prelude.lua`）：
  - 腳本只在腳本執行緒執行；UI 只送工作，腳本要求的動作以訊息送回。每次呼叫都要設定時間上限。
  - release 是 `panic = "abort"`，Lua 的錯誤不可穿過 Rust 的堆疊框架：Rust 提供給 Lua 的函式一律經 `script::lua_fn`
    （參數只收 `Value`、不回傳錯誤）；不用 mlua 的 `set_hook`（時間上限在 prelude 用 `debug.sethook`）；Rust 端讀寫表格只用
    `raw_get`／`raw_set`。`clippy.toml` 的 `disallowed-methods` 會擋下違規；冒煙測試的 debug build 用 `debug-abort` profile
    （同為 abort），違規在 debug 冒煙測試就會當掉。
  - 除錯器：傳輸在 `luadebug.rs`（具名管道，只允許目前使用者、拒絕遠端），除錯邏輯（DAP、JSON、中斷點、單步、變數）在
    `debugger.lua`，VS Code 擴充功能在 `tools/vscode-zing-lua`（管道名稱兩邊要一致）。暫停時腳本執行緒只處理除錯訊息，
    其他工作排隊（`Queue`）、縮圖規則直接回答 None。prelude 的鉤子同時負責時間上限與除錯器，`debugger.lua` 呼叫 `stop`
    時不可寫成尾呼叫（`return stop(...)`），否則框架層級會錯。
  - API 的說明只寫在 `zing_core::scriptapi`（命令的在 `zing_core::builtin`）：新增或修改命令、全域函式、prelude 函式時同步更新（並補上翻譯），再以
    `ZING_UPDATE_DOCS=1 cargo test -p zing-core` 重新產生 `docs/lua.md`、`lua.en.md`、`lua.ja.md`（測試會檢查是否最新）；
    `tools/smoke.ps1` 每次先執行 `tools/check-lua-api.ps1`：
    以 Lua Language Server 檢查定義檔、`examples/scripts` 與 `docs/lua.md` 的每個範例，必須沒有任何問題
    （範例必須是合法的 Lua：管線是運算式，不能單獨成為一行）；沒有安裝 LuaLS 時只警告，修改 API 時務必安裝後執行。
  - `examples/scripts` 的每個範例在 `script.rs` 都有單元測試（`example_*`，以真的檔案與外部程式執行）。
- 啟動路徑上不放非必要初始化，能延後的就延後（OLE、拖放在第一幀之後初始化）。
- 第一幀用 WARP 繪製，硬體 GPU 裝置在第一幀呈現後於背景建立再切換；不要在第一幀前碰硬體裝置（驅動初始化約 130 ms，且會鎖住 DXGI）。
- 分析工具：`--perf-out` 模式的 `phase_*` 時間點、Windows Performance Recorder/Analyzer（ETW）、`criterion`；需要符號時用 `cargo build --profile profiling`。

### 已知的效能缺口（待處理）

- 拖放（拖出的資料物件、放置目標）仍在 UI 執行緒解析路徑，這是 COM 的限制；網路路徑在拖曳經過時可能短暫延遲。
  放置目標每次拖曳只解析一次（冒煙測試確認：來回經過 12 次只解析 2 次，DragOver 約 1.5 ms）。
- 放下：一般檔案（CF_HDROP）的複製／移動交給 `FileOps` 在背景執行，`Drop` 約 1 ms 返回；效果沿用 Shell 在最後一次
  DragOver 決定的（與檔案總管規則相同），移動時以「最佳化移動」回報來源（回傳 DROPEFFECT_NONE、設定 Performed DropEffect）。
  右鍵拖曳、建立捷徑、虛擬檔案（非 CF_HDROP）、資源回收筒內的項目仍交給 Shell 的 `Drop`，會在 UI 執行緒同步執行
  （約 120–190 ms），以 `window::exempt_stall()` 排除在卡頓監看外。
- 捲動時偶有一幀約 17–20 ms（約一個 60 Hz 更新週期）的停頓，p99 不受影響。已查明時間全部花在 `Present`
  （等待桌面合成器），CPU 繪製約 2 ms、EndDraw 約 0.15 ms；連續呈現約 70–100 ms 後發生、閒置後會再發生。
  是否發生取決於桌面當下的狀態（同一版本同一條件，一輪 12/12、另一輪 0/3），緩衝數量、FLIP_SEQUENTIAL、
  blt 模式都無法穩定消除，所以沒有修改。下一步：用 PresentMon／ETW 確認是否為呈現模式切換（independent flip）。
  效能報告的 `frames_over_8ms` 與結果 JSON 的 `worst_*` 欄位會顯示每次的狀況。

## 專案結構

- `crates/zing-core`：與 UI 無關、可測試的核心（列舉、排序／篩選、命令語言、快速鍵、主題、設定）。
- `crates/zing-app`：Win32 視窗、繪製（`gfx`、`view`）、輸入、Shell 整合（`shell`）、背景服務（`services`）、效能模式（`perf`）、
  索引服務用戶端（`index`）、Lua 腳本（`script`、`prelude.lua`、除錯器 `luadebug`、`debugger.lua`）。
- `crates/zing-index`：NTFS 索引服務 `zing-index.exe`（Windows 服務）。讀取 MFT 建立資料夾大小索引、跟著變更日誌更新，
  經 named pipe（協定在 `zing_core::indexproto`）回答查詢；MFT 解析與索引資料結構在 `zing_core::ntfs`、`zing_core::volindex`，
  以構造的記錄做單元測試與基準測試（`cargo bench -p zing-core --bench core -- ntfs`）。讀取磁碟區需要系統管理員權限，
  冒煙測試無法涵蓋：實機驗證用系統管理員終端機執行 `zing-index --scan C: --verify 資料夾`。
  圖示的原始檔是 `assets/zing.svg`（24 px 以下用 `zing-small.svg`），修改後執行 `pwsh tools/make-icon.ps1` 重新產生 `zing.ico`；
  圖示與版本資訊由 `build.rs` 以 Windows SDK 的 `rc.exe` 編譯進執行檔。
- 資料存放位置：
  - 使用者的設定與習慣放在 `zing.exe` 所在的資料夾（可攜式，重灌系統不受影響）：`config.toml`（設定）、
    `themes\*.toml`（主題）、`session.toml`（視窗、分頁、我的最愛與其順序、命令面板最近使用）、
    `keys.toml`（快速鍵編輯器寫的按鍵，只含與預設＋config.toml 不同的，見 `zing_core::keyedit`）、
    `toolbar.toml`（命令列的按鈕，「自訂命令列」寫的，見 `zing_core::cmdbar`）。路徑一律經 `settings::app_dir()`。
  - 系統產生、可重建的資料（資料夾大小快取、縮圖快取…）放在 `%LOCALAPPDATA%\zing-explore\`（只屬於這台電腦、不漫遊，
    刪除只需重新計算），不要放在程式旁或 `%APPDATA%`。目前有：`folder-sizes.tsv`（沒有索引時的資料夾大小，見 `sizes.rs`）、
    `folders.tsv`（每個資料夾的檢視設定，最多 5,000 個，見 `views.rs`、`zing_core::folderview`）、
    `thumbs\`（外部命令產生的縮圖 PNG，上限 256 MB，見 `thumbs.rs`；Windows 的縮圖只放記憶體，上限 64 MB）、
    `custom-thumbs\`（手動指定的縮圖，檔名是「磁碟區序號-NTFS 檔案 ID」：鍵只在這台電腦有意義，所以不放在程式旁）、
    `slow-menu-extensions.tsv`（讓右鍵選單變慢的第三方擴充功能，7 天後失效重新量，見 `shellext.rs`）。
  - 索引服務：執行檔安裝時複製到 `%ProgramFiles%\zing-explore\zing-index.exe`（以 LocalSystem 執行的程式不能放在一般使用者
    可修改的位置）；快照（`<磁碟區序號>.zidx`）與 `index.log` 放在服務帳號的 `%LOCALAPPDATA%`，即
    `C:\Windows\System32\config\systemprofile\AppData\Local\zing-explore`（不用 `%ProgramData%`：一般使用者能先建立同名資料夾）。
  - 不要把 `zing.exe` 放在使用者無法寫入的位置（如 Program Files）；開發時從 `target\release` 執行，設定會在
    `cargo clean` 時一起被刪除。

## 已確認的決策

- 語言 Rust；腳本 Lua。
- 預設佈局：單窗格 + 樹狀列；佈局可自訂（樹狀列可移動、關閉、換位置；窗格可增加）。
- 主題系統：可匯入匯出，主題檔只含資料、不含腳本。
- 介面風格：預設「緊湊」（高密度、直角、網址列顯示完整路徑、選取列為實心強調色），另內建「Fluent」與「折衷」
  兩種風格的主題（`zing_core::theme::Style`），各有深色與淺色。強調色是圖示的黃色 `#f2c230`。
  風格的差異只透過主題資料（`metrics`、`color`、`chrome`）表達，繪製程式碼不判斷風格名稱。
- 自用為主，但會開源。
