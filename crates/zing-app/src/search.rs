//! 全磁碟搜尋（Ctrl+F）：沿用命令面板的輸入框、清單與按鍵，結果向 NTFS 索引服務查詢。
//!
//! - 輸入停頓 `DEBOUNCE_MS` 後在工作池查詢（服務端約數十毫秒），以 generation 丟棄過時的結果。
//! - 查詢語法（路徑、篩選、排序）見 `zing_core::namequery`；無法解讀的詞先在這裡提示，不送出查詢。
//! - 服務只回傳呼叫者能列出的資料夾中的項目（見 zing-index 的 server.rs）；隱藏項目依「顯示隱藏檔案」過濾。
//! - Enter：前往所在的資料夾並選取它；Ctrl+Enter：開啟（資料夾就進入）。
//! - 隔離模式（效能、冒煙測試）不使用索引；冒煙測試以 `TEST_RESULTS` 提供結果。

use zing_core::format;
use zing_core::indexproto::{self, QueryError, Request, Response, SearchItem};
use zing_core::path;

use crate::app::App;
use crate::gfx::{Font, Rect};
use crate::palette::{MAX_ROWS, Palette, PaletteLayout};
use crate::services::{self, Msg};
use crate::view::Hit;
use crate::window;

/// 輸入停頓多久才查詢
const DEBOUNCE_MS: u32 = 120;
/// 一次最多取回的結果
const LIMIT: u32 = 500;
/// 輸入框右側顯示排序與結果數的寬度
const COUNT_W: f32 = 150.0;

/// 冒煙測試：有值時不查詢服務，改從這些項目中以名稱比對
#[cfg(feature = "smoke")]
pub static TEST_RESULTS: std::sync::Mutex<Option<Vec<SearchItem>>> = std::sync::Mutex::new(None);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    Pending,
    Done,
    /// 服務沒有安裝或沒有執行
    NoService,
    /// 查詢中有無法解讀的詞（例如 `size:abc`）
    Invalid(String),
    Error(String),
}

#[derive(Default)]
pub struct Results {
    pub items: Vec<SearchItem>,
    /// 還有更多（服務端達到數量或時間上限）
    pub more: bool,
    pub state: State,
    pub generation: u64,
}

/// 查詢語法的提示（空白時、無法解讀時顯示）
const SYNTAX: &str = "ext:jpg;png  size:>10mb  dm:today / 2024-05 / 7d  file: folder:  !排除  路徑\\名稱  sort:size / date / name / path / ext";
/// 排序按鍵的提示
const SORT_KEYS: &str = "Ctrl+1–5 依名稱、路徑、副檔名、大小、日期排序（再按一次反向），Ctrl+0 取消排序";

fn query(q: &str) -> Result<Response, QueryError> {
    #[cfg(feature = "smoke")]
    if let Some(items) = TEST_RESULTS.lock().unwrap().clone() {
        return Ok(Response::Found { items: test_search(items, q), more: false });
    }
    indexproto::query(&Request::Search { query: q.into(), limit: LIMIT })
}

/// 冒煙測試：在程序內以同樣的語法比對與排序（服務端的排序規則見 zing-index 的 server.rs）
#[cfg(feature = "smoke")]
fn test_search(items: Vec<SearchItem>, q: &str) -> Vec<SearchItem> {
    use zing_core::namequery::{Meta, NameQuery, SortKey, fold, natural_cmp};
    let nq = NameQuery::parse(q);
    let unix = |ft: u64| if ft == 0 { 0 } else { (ft / 10_000_000).saturating_sub(11_644_473_600) as u32 };
    let mut v: Vec<SearchItem> = items
        .into_iter()
        .filter(|it| {
            let m = Meta { dir: it.dir, size: it.size, mtime: unix(it.modified) };
            nq.matches(path::file_name(&it.path), &it.path, m)
        })
        .collect();
    if let Some(s) = nq.sort {
        v.sort_by(|a, b| {
            let o = match s.key {
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Date => a.modified.cmp(&b.modified),
                SortKey::Path => natural_cmp(&fold(a.path.as_bytes()), &fold(b.path.as_bytes())),
                _ => natural_cmp(&fold(path::file_name(&a.path).as_bytes()), &fold(path::file_name(&b.path).as_bytes())),
            };
            if s.desc { o.reverse() } else { o }
        });
    }
    v
}

impl App {
    pub fn open_search(&mut self) {
        if self.palette.as_ref().is_some_and(|p| p.search.is_some()) {
            return;
        }
        self.close_palette();
        self.palette = Some(Palette {
            entries: Vec::new(),
            query: String::new(),
            matches: Vec::new(),
            sel: 0,
            top: 0,
            search: Some(Results::default()),
        });
        if let Some(lay) = self.palette_layout() {
            // 右側留給結果數
            let mut r = lay.input.inset(10.0, 0.0);
            r.r -= COUNT_W;
            window::begin_palette_edit(self, r);
        }
        self.invalidate();
    }

    /// 搜尋面板的排序按鍵：改寫後的查詢（不是搜尋面板時為 None）。`key` 為 None 表示取消排序
    pub fn search_sort_text(&self, key: Option<zing_core::namequery::SortKey>) -> Option<String> {
        let p = self.palette.as_ref().filter(|p| p.search.is_some())?;
        Some(zing_core::namequery::set_sort(&p.query, key))
    }

    /// 輸入框內容改變：稍候再查詢（連續輸入只查最後一次）
    pub fn search_query_changed(&mut self) {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), crate::app::TIMER_SEARCH, DEBOUNCE_MS, None);
        }
    }

    pub fn on_search_timer(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), crate::app::TIMER_SEARCH);
        }
        let isolated = self.isolated;
        let Some(p) = self.palette.as_mut() else { return };
        let Some(r) = p.search.as_mut() else { return };
        r.generation += 1;
        let q = p.query.trim().to_string();
        let parsed = zing_core::namequery::NameQuery::parse(&q);
        if q.is_empty() || !parsed.errors.is_empty() {
            r.items.clear();
            r.more = false;
            r.state = if q.is_empty() { State::Idle } else { State::Invalid(parsed.errors.join("  ")) };
            p.matches.clear();
            self.invalidate();
            return;
        }
        #[cfg(feature = "smoke")]
        let isolated = isolated && TEST_RESULTS.lock().unwrap().is_none();
        if isolated {
            r.state = State::NoService;
            self.invalidate();
            return;
        }
        r.state = State::Pending;
        let (generation, n) = (r.generation, self.notifier.clone());
        self.pool.spawn(move || n.send(Msg::SearchResults { generation, result: query(&q) }));
        self.invalidate();
    }

    pub fn on_search_results(&mut self, generation: u64, result: Result<Response, QueryError>) {
        let show_hidden = self.show_hidden;
        let Some(p) = self.palette.as_mut() else { return };
        let Some(r) = p.search.as_mut().filter(|r| r.generation == generation) else { return };
        match result {
            Ok(Response::Found { items, more }) => {
                r.items = items.into_iter().filter(|it| show_hidden || !it.hidden).collect();
                r.more = more;
                r.state = State::Done;
            }
            // 舊版服務不認得搜尋請求
            Ok(Response::Error(e)) if e == "無法解析請求" => {
                r.state = State::Error("索引服務是舊版，不支援搜尋：在命令面板重新執行「啟用 NTFS 索引服務」更新".into())
            }
            Ok(Response::Error(e)) => r.state = State::Error(e),
            Ok(_) => r.state = State::Error("索引服務的回應不符".into()),
            Err(QueryError::NotRunning) => r.state = State::NoService,
            Err(QueryError::Failed(e)) => r.state = State::Error(e),
        }
        if r.state != State::Done {
            r.items.clear();
        }
        p.matches = (0..r.items.len()).collect();
        p.sel = 0;
        p.top = 0;
        self.invalidate();
    }

    /// Enter：前往所在的資料夾並選取；Ctrl+Enter：開啟
    pub fn open_search_item(&mut self, item: SearchItem, open: bool) {
        let p = self.active_pane;
        if open {
            if item.dir {
                self.navigate(p, item.path, true);
            } else {
                let dir = path::parent(&item.path);
                services::launch(&self.notifier, item.path, None, dir, None, services::Show::Normal);
            }
            return;
        }
        match path::parent(&item.path) {
            Some(dir) => {
                let name = path::file_name(&item.path).to_string();
                self.navigate(p, dir, true);
                self.tab_mut().pending_select = Some((name, false));
            }
            None => self.navigate(p, item.path, true),
        }
    }

    pub fn paint_search(&self, p: &Palette, lay: &PaletteLayout) {
        let Some(r) = &p.search else { return };
        let c = &self.theme.color;
        let g = &self.gfx;
        // 輸入框右側：排序與結果數
        if r.state == State::Done && !p.query.trim().is_empty() {
            let mut n = if r.more { format!("{}+ 個", r.items.len()) } else { format!("{} 個", r.items.len()) };
            if let Some(s) = zing_core::namequery::NameQuery::parse(&p.query).sort {
                n = format!("{} {}　{n}", s.key.label(), if s.desc { "↓" } else { "↑" });
            }
            let ir = lay.input;
            g.text(&n, Font::SmallRight, Rect::new(ir.l, ir.t, ir.r - 10.0, ir.b), c.text_secondary);
        }
        let row0 = lay.rows[0];
        if p.matches.is_empty() {
            let text = match &r.state {
                State::Idle => format!(
                    "輸入名稱搜尋所有 NTFS 磁碟（空白分隔多個詞，可用 * ?；Enter 前往所在位置、Ctrl+Enter 開啟）　{SORT_KEYS}　{SYNTAX}"
                ),
                State::Invalid(words) => format!("無法解讀：{words}　　{SYNTAX}"),
                State::Pending => "搜尋中…".to_string(),
                State::Done => "找不到符合的項目".to_string(),
                State::NoService => "全磁碟搜尋需要 NTFS 索引服務：在命令面板執行「啟用 NTFS 索引服務」".to_string(),
                State::Error(e) => format!("搜尋失敗：{e}"),
            };
            g.text(&text, Font::Text, Rect::new(row0.l + 10.0, row0.t, row0.r - 10.0, row0.b), c.text_secondary);
            return;
        }
        for (i, row) in lay.rows.iter().enumerate() {
            let mi = p.top + i;
            let Some(&k) = p.matches.get(mi) else { break };
            let it = &r.items[k];
            if mi == p.sel {
                g.fill_round(*row, self.theme.metrics.radius, c.selection);
            } else if self.hover == Hit::PaletteRow(mi) {
                g.fill_round(*row, self.theme.metrics.radius, c.hover);
            }
            let (fg, dim) = if mi == p.sel {
                (c.selection_text, c.selection_text.with_alpha(170))
            } else {
                (c.text, c.text_secondary)
            };
            let mut info = if it.dir { format!("資料夾 {}", format::size_short(it.size)) } else { format::size_short(it.size) };
            if it.modified != 0 {
                info = format!("{}　{info}", format::filetime_local(it.modified));
            }
            g.text(&info, Font::SmallRight, Rect::new(row.l, row.t, row.r - 10.0, row.b), dim);
            let right = row.r - 10.0 - g.measure(&info, Font::Small) - 16.0;
            let name = path::file_name(&it.path);
            let name_r = Rect::new(row.l + 10.0, row.t, right, row.b);
            g.text(name, Font::Text, name_r, fg);
            // 所在的資料夾以次要顏色接在名稱後面
            let x = name_r.l + g.measure(name, Font::Text) + 12.0;
            if x + 40.0 < right {
                let dir = path::parent(&it.path).unwrap_or_default();
                g.text(&dir, Font::Small, Rect::new(x, row.t, right, row.b), dim);
            }
        }
        if p.matches.len() > MAX_ROWS {
            let card = lay.card;
            let track = Rect::new(card.r - 5.0, lay.rows[0].t, card.r - 2.0, lay.rows[lay.rows.len() - 1].b);
            let h = track.h() * MAX_ROWS as f32 / p.matches.len() as f32;
            let y = track.t + track.h() * p.top as f32 / p.matches.len() as f32;
            g.fill_round(Rect::new(track.l, y, track.r, y + h), 1.5, c.text_secondary.with_alpha(110));
        }
    }
}
