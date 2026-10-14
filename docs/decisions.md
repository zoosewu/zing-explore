# 決策紀錄

技術取捨的原因，供日後查詢。新的在上面。

## 2026-10-14　Lua 腳本：Lua 5.4（mlua 內建）、函數式但不用 functor／monad、release 維持 panic = "abort"

**決定**：
- 用 `mlua` 0.11 搭配內建編譯的 Lua 5.4（`vendored`），開放完整的標準函式庫；腳本在專用執行緒執行，命令 30 秒、縮圖規則 0.3 秒的時間上限。
- prelude 提供惰性求值（`seq`）、不可變（`freeze`）、組合（`compose`／`pipe`／`partial`、`|` 管線），但不提供 functor／monad。
- release 維持 `panic = "abort"`，改由程式規則保證 Lua 的錯誤不穿過 Rust 的堆疊框架。

**原因**：
- 執行檔增加約 0.79 MB（2.60 → 3.39 MB），建置時間多約 40 秒；LuaJIT 或外部 DLL 都會讓可攜式的單一執行檔變複雜。
- 10 萬筆資料實測：迴圈 8 ms、map/filter/reduce 11 ms、惰性串流 12 ms、Maybe monad 65 ms。前三者差距小，monad 慢 5–8 倍，
  而且在 Lua 裡寫起來不自然，所以只保留函數式設計中成本低的部分。
- Windows 上 Lua 的錯誤是用 SEH 展開的 longjmp；`panic = "abort"` 時 Rust 函式都標為不可展開，錯誤只要穿過 Rust 框架
  （例如 mlua 的 `set_hook` 回傳錯誤、Rust 函式的參數型別不符）程序就會直接結束（release 冒煙測試實際發生過，0xC0000409）。
  改成 `panic = "unwind"` 可以根治，但執行檔再多 0.86 MB（+25%）。

**代價與處理**：
- Rust 提供給 Lua 的函式參數一律收 `Value`、不回傳 `Err`；時間上限用 prelude 裡的 Lua 鉤子（`debug.sethook`，並包裝
  `coroutine.create`／`wrap` 讓協程也受限制）；Rust 端讀寫全域變數只用 `raw_get`／`raw_set`。
- 冒煙測試涵蓋：錯誤、無窮迴圈、協程裡的無窮迴圈、型別錯誤的參數、全域表設了會報錯的 metatable，並以 release 執行。
- 腳本可以自己呼叫 `debug.sethook` 拿掉時間上限；腳本與設定檔同等可信，不另外防範。

## 2026-10-14　預覽處理常式以「沒有擁有者」的頂層視窗承載

**決定**：Windows 預覽處理常式（IPreviewHandler）在專用的 STA 執行緒上，用一個沒有父視窗、也沒有擁有者的
頂層彈出視窗（WS_POPUP、WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW）承載，疊在預覽窗格的內容區。

**原因**：第三方處理常式可能卡住。跨執行緒的父子視窗**或擁有者／被擁有視窗**都會讓系統連結兩個執行緒的輸入佇列，
處理常式的執行緒卡住時會連帶卡住 UI（原本以為擁有者視窗可以避免，查證後不行）。獨立的頂層視窗不會連結。

**代價與處理**：
- 視窗的 Z 順序要自己維護：主視窗啟用時以 SWP_ASYNCWINDOWPOS 把它放回上層；命令面板、批次重新命名面板開啟或視窗最小化時暫時隱藏。
- UI 只用不會等待的方式操作它（PostThreadMessage 傳請求、SWP_ASYNCWINDOWPOS、ShowWindowAsync）。
- 看門狗 3 秒：逾時就放棄該執行緒（之後換新的），改顯示縮圖；被放棄的執行緒恢復後自行關閉視窗並結束。
- 優先以 CLSCTX_LOCAL_SERVER 建立（prevhost.exe），處理常式當掉不影響本程式。
- 冒煙測試的截圖（PrintWindow）不含這個視窗。

## 2026-10-14　批次重新命名改用完整的 `regex`

**決定**：批次重新命名的規則運算式使用 `regex`（預設功能），不用 `regex-lite`。

**原因**：`regex-lite` 不支援 Unicode 感知的比對，對中文檔名影響明顯（實測）：

| 寫法 | `regex-lite` | `regex` |
| --- | --- | --- |
| `\w` 比對「中」 | 不符合（只認 ASCII） | 符合 |
| `\d` 比對全形「３」 | 不符合 | 符合 |
| `\s` 比對全形空白 | 不符合 | 符合 |
| `(?i)` 讓 ä 等於 Ä | 不符合（只對英文字母） | 符合 |
| `\p{Han}` 等 Unicode 類別 | 不支援 | 支援 |

兩者都不支援後向參照與環視；其他語法（群組、`$1`、`${名稱}`、量詞、錨點）相同。

**代價**：執行檔增加約 1.1 MB（實測：`regex-lite` 約 52 KB；`regex` 只開 `unicode-perl`、`unicode-case` 多 881 KB；
預設功能多 1,118 KB），由 1.505 MB 變為 2.597 MB，仍在 10 MB 門檻內；`perf/baseline.json` 的 `exe_mb` 隨之更新。
只有使用批次重新命名的規則運算式時才會用到，啟動時間不受影響（53 ms，在容差內）。

**考慮過的替代方案**：維持 `regex-lite` 並在面板提示改寫法（中文用 `[一-鿿]`、全形數字用 `[0-9０-９]`），
或 `regex` 只開部分 Unicode 功能；使用者選擇完整的 `regex`。
