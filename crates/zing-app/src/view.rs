//! 版面配置、繪製與點擊判定。
//!
//! 每次繪製與滑鼠事件都重新計算版面（成本很低），只繪製可見的列；
//! 檔名直接從字串池以 UTF-16 交給 DirectWrite，不做轉換。

use zing_core::config::TreeSide;
use zing_core::format;
use zing_core::path;
use zing_core::sort::SortKey;
use zing_core::folderview::ViewMode;
use zing_core::grid::Grid;
use zing_core::theme::AddressStyle;

use crate::app::{App, Focus};
use crate::gfx::{Font, Rect, glyph};
use crate::icons::IconKey;
use crate::model::NodeKind;

pub const SCROLLBAR_W: f32 = 10.0;
/// 預覽窗格最多顯示的屬性列數
const PREVIEW_PROPS: usize = 12;
pub const NAV_BTN: f32 = 32.0;
const SPLITTER_W: f32 = 5.0;
const COL_MIN: f32 = 48.0;

pub const COLUMN_TITLES: [&str; 4] = [tr_key!("名稱"), tr_key!("修改日期"), tr_key!("類型"), tr_key!("大小")];
pub const COLUMN_KEYS: [SortKey; 4] = [SortKey::Name, SortKey::Modified, SortKey::Type, SortKey::Size];

#[derive(Clone, Debug, Default)]
pub struct PaneLayout {
    pub area: Rect,
    pub strip: Rect,
    pub tabs: Vec<Rect>,
    pub tab_close: Vec<Rect>,
    pub tab_new: Rect,
    pub header: Rect,
    /// 各欄的左右邊界：名稱、修改日期、類型、大小
    pub cols: [(f32, f32); 4],
    pub list: Rect,
    pub scrollbar: Rect,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub toolbar: Rect,
    pub status: Rect,
    pub nav: [Rect; 4],
    /// 工具列最右側的設定按鈕（固定，不受設定檔的按鈕影響）
    pub settings_btn: Rect,
    pub address: Rect,
    pub crumbs: Vec<(Rect, String)>,
    pub crumb_overflow: bool,
    pub filter_chip: Option<Rect>,
    /// 篩選標籤可用位置的右緣（寬度 0；還沒有篩選時也有），輸入法組字視窗以此對齊
    pub filter_slot: Rect,
    pub buttons: Vec<Rect>,
    pub tree: Option<Rect>,
    pub tree_splitter: Option<Rect>,
    /// 右側面板（窗格區域最右側；設定頁、主題編輯、自訂命令列、快速鍵）
    pub side_panel: Option<Rect>,
    /// 命令列（工具列下方）
    pub cmdbar: Option<crate::cmdbar::BarLayout>,
    /// 預覽窗格（窗格區域右側）
    pub preview: Option<Rect>,
    pub preview_splitter: Option<Rect>,
    pub panes: Vec<PaneLayout>,
    /// 檔案作業面板（狀態列上方）
    pub jobs: Option<Rect>,
    /// 狀態列上的檔案作業進度（面板收起時）
    pub jobs_indicator: Option<Rect>,
    /// 狀態列上的「有新版本」（update.rs）
    pub update_indicator: Option<Rect>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    None,
    Nav(usize),
    Crumb(String),
    Address,
    FilterChip,
    Button(usize),
    TreeRow(usize),
    TreeArrow(usize),
    TreeEmpty,
    TreeSplitter,
    /// 批次重新命名面板：選項、輸入框、執行、取消、面板內其他位置
    BatchChip(usize),
    BatchField(usize),
    BatchRun,
    BatchCancel,
    Batch,
    Preview,
    PreviewSplitter,
    /// 主題編輯面板（索引是 `ThemeEditor::rows` 的列）：整列、色碼或樣式文字、色票（列, 第幾個）、還原、
    /// 數字的 −／＋、滑桿、刪除檔案顏色、底部按鈕、進階連結、面板其他位置
    ThemeRow(usize),
    ThemeText(usize),
    ThemeCell(usize, usize),
    ThemeReset(usize),
    ThemeStep(usize, bool),
    ThemeSlider(usize),
    ThemeDelete(usize),
    ThemeButton(usize),
    ThemeLink(usize),
    ThemePanel,
    /// 設定頁：列（`SettingsPage::rows` 的索引）、標題列（回到分類清單）、底部按鈕、面板其他位置
    SettingsRow(usize),
    SettingsBack,
    /// 工具列的設定按鈕
    SettingsGear,
    /// 命令列：第幾個顯示中的按鈕（`BarLayout::buttons` 的索引）、命令列的空白處
    CmdButton(usize),
    CmdBar,
    /// 下拉選單：（第幾層, 第幾項）、選單內的其他位置
    MenuItem(usize, usize),
    Menu,
    /// 自訂命令列面板（cmdbaredit.rs）：列、列中的按鈕（列, 第幾個）、底部按鈕、面板其他位置
    BarEditRow(usize),
    BarEditPart(usize, usize),
    BarEditButton(usize),
    BarEditPanel,
    /// 自訂命令列的搜尋框（true：清除鈕）
    BarEditSearch(bool),
    /// 快速鍵編輯面板（keyspanel.rs）：列、列中的按鍵或按鈕（列, 第幾個）、底部按鈕、面板其他位置
    KeysRow(usize),
    KeysPart(usize, usize),
    KeysButton(usize),
    KeysPanel,
    SettingsButton(usize),
    SettingsPanel,
    Tab(usize, usize),
    TabClose(usize, usize),
    TabNew(usize),
    TabStrip(usize),
    Header(usize, usize),
    HeaderEdge(usize, usize),
    Row(usize, usize),
    ListEmpty(usize),
    Scrollbar(usize),
    /// 命令面板的第幾個符合項目
    PaletteRow(usize),
    /// 命令面板內其他位置
    Palette,
    JobPause(crate::jobs::JobId),
    JobCancel(crate::jobs::JobId),
    JobsClear,
    JobsClose,
    /// 檔案作業面板內其他位置
    JobsPanel,
    /// 狀態列上的檔案作業進度
    JobsIndicator,
    /// 狀態列上的「有新版本」
    UpdateIndicator,
}

impl App {
    pub fn row_h(&self) -> f32 {
        self.theme.metrics.row_height
    }

    /// 縮圖檢視時的格狀版面；清單檢視為 None
    pub fn grid(&self, p: usize, list: Rect) -> Option<Grid> {
        let t = self.panes.get(p)?.tab();
        (t.mode == ViewMode::Thumbnails).then(|| Grid::new(list.w() - SCROLLBAR_W, t.thumb as f32, self.caption_line_h()))
    }

    /// 縮圖下方一行檔名的高度
    pub fn caption_line_h(&self) -> f32 {
        (self.theme.font.size * 1.4).ceil()
    }

    /// 第 `i` 項在視窗中的範圍（清單為整列、縮圖為整格；依上次的版面）
    pub fn item_rect(&self, p: usize, i: usize) -> Option<Rect> {
        let pl = self.layout.panes.get(p)?;
        let scroll = self.panes[p].tab().scroll;
        Some(match self.grid(p, pl.list) {
            Some(g) => {
                let c = g.cell(i);
                let (l, t) = (pl.list.l + c.x, pl.list.t + c.y - scroll);
                Rect::new(l, t, l + c.w, t + c.h)
            }
            None => {
                let y = pl.list.t + i as f32 * self.row_h() - scroll;
                Rect::new(pl.list.l, y, pl.list.r - SCROLLBAR_W, y + self.row_h())
            }
        })
    }

    pub fn compute_layout(&self) -> Layout {
        let (w, h) = self.gfx.size();
        let m = &self.theme.metrics;
        let mut lay = Layout {
            toolbar: Rect::new(0.0, 0.0, w, m.toolbar_height),
            status: Rect::new(0.0, h - m.status_height, w, h),
            ..Default::default()
        };

        // 工具列：導覽按鈕、網址列（麵包屑）、篩選標籤、自訂按鈕
        let tb = lay.toolbar;
        let cy = (tb.t + tb.b) / 2.0;
        // 工具列內元件的半高：隨主題的工具列高度縮放（緊湊風格較矮）
        let hh = ((m.toolbar_height - 6.0) / 2.0).clamp(10.0, 16.0);
        let mut x = 6.0;
        for i in 0..4 {
            lay.nav[i] = Rect::new(x, cy - hh, x + NAV_BTN, cy + hh);
            x += NAV_BTN + 2.0;
        }
        let mut right = w - 6.0;
        lay.settings_btn = Rect::new(right - NAV_BTN, cy - hh, right, cy + hh);
        right -= NAV_BTN + 4.0;
        let tab = self.tab();
        lay.filter_slot = Rect::new(right, cy - hh + 2.0, right, cy + hh - 2.0);
        if !tab.filter.is_empty() {
            // 左側圖示 26 + 右側關閉鈕 22 + 間距（與 ime_anchor 相同）
            let cw = (self.gfx.measure(&tr!("篩選：{filter}", filter = tab.filter), Font::Small) + 54.0).min(280.0);
            lay.filter_chip = Some(Rect::new(right - cw, cy - hh + 2.0, right, cy + hh - 2.0));
            right -= cw + 6.0;
        }
        lay.address = Rect::new(x + 6.0, cy - hh + 1.0, right.max(x + 120.0), cy + hh - 1.0);
        if self.theme.chrome.address == AddressStyle::Breadcrumb {
            self.layout_crumbs(&mut lay);
        }

        // 檔案作業面板與狀態列上的進度
        let jobs_h = self.jobs_panel_height();
        if jobs_h > 0.0 {
            lay.jobs = Some(Rect::new(0.0, lay.status.t - jobs_h, w, lay.status.t));
        }
        if let Some(text) = self.jobs_indicator_text() {
            let tw = self.gfx.measure(&text, Font::Small) + 16.0;
            lay.jobs_indicator = Some(Rect::new(w - 8.0 - tw, lay.status.t, w - 4.0, lay.status.b));
        }
        if let Some(text) = self.update_indicator_text() {
            let r = lay.jobs_indicator.map_or(w - 4.0, |j| j.l - 4.0);
            let tw = self.gfx.measure(&text, Font::Small) + 16.0;
            lay.update_indicator = Some(Rect::new(r - tw, lay.status.t, r, lay.status.b));
        }

        // 主體：樹狀列 + 窗格
        // 命令列（工具列下方）
        let bar_h = self.cmdbar_height();
        if bar_h > 0.0 {
            lay.cmdbar = Some(self.layout_cmdbar(Rect::new(0.0, tb.b, w, tb.b + bar_h)));
        }
        let body = Rect::new(0.0, tb.b + bar_h, w, lay.status.t - jobs_h);
        let mut panes_area = body;
        let tree_visible = self.tree_visible && self.tree_side != TreeSide::Hidden;
        if tree_visible {
            let tw = self.tree_width.clamp(120.0, (w * 0.6).max(120.0));
            if self.tree_side == TreeSide::Right {
                lay.tree = Some(Rect::new(w - tw, body.t, w, body.b));
                lay.tree_splitter = Some(Rect::new(w - tw - SPLITTER_W / 2.0, body.t, w - tw + SPLITTER_W / 2.0, body.b));
                panes_area.r = w - tw - 1.0;
            } else {
                lay.tree = Some(Rect::new(0.0, body.t, tw, body.b));
                lay.tree_splitter = Some(Rect::new(tw - SPLITTER_W / 2.0, body.t, tw + SPLITTER_W / 2.0, body.b));
                panes_area.l = tw + 1.0;
            }
        }
        if let Some(w) = self.side_width(panes_area.w()) {
            let l = panes_area.r - w;
            lay.side_panel = Some(Rect::new(l, body.t, panes_area.r, body.b));
            panes_area.r = l - 1.0;
        }
        if self.preview.visible {
            let pv = self.preview.width.clamp(200.0, (panes_area.w() * 0.6).max(200.0));
            let l = panes_area.r - pv;
            lay.preview = Some(Rect::new(l, body.t, panes_area.r, body.b));
            lay.preview_splitter = Some(Rect::new(l - SPLITTER_W / 2.0, body.t, l + SPLITTER_W / 2.0, body.b));
            panes_area.r = l - 1.0;
        }
        let n = self.panes.len().max(1) as f32;
        let pw = (panes_area.w() - (n - 1.0)) / n;
        for (i, pane) in self.panes.iter().enumerate() {
            let l = panes_area.l + i as f32 * (pw + 1.0);
            let area = Rect::new(l, panes_area.t, l + pw, panes_area.b);
            lay.panes.push(self.layout_pane(area, pane));
        }
        lay
    }

    fn layout_crumbs(&self, lay: &mut Layout) {
        let segs = path::segments(&self.tab().path);
        let inner_l = lay.address.l + 6.0;
        let inner_r = lay.address.r - 6.0;
        let sep = 16.0;
        let widths: Vec<f32> = segs.iter().map(|(n, _)| self.gfx.measure(n, Font::Text) + 12.0).collect();
        // 放不下時從前面開始省略，至少保留最後一段
        let mut first = 0;
        let total = |from: usize| widths[from..].iter().sum::<f32>() + sep * (widths.len() - from) as f32;
        while first + 1 < segs.len() && total(first) > inner_r - inner_l - 24.0 {
            first += 1;
        }
        lay.crumb_overflow = first > 0;
        let mut x = inner_l + if first > 0 { 24.0 } else { 0.0 };
        for (k, (_, full)) in segs.iter().enumerate().skip(first) {
            let r = Rect::new(x, lay.address.t + 3.0, (x + widths[k]).min(inner_r), lay.address.b - 3.0);
            lay.crumbs.push((r, full.clone()));
            x += widths[k] + sep;
        }
    }

    fn layout_pane(&self, area: Rect, pane: &crate::model::Pane) -> PaneLayout {
        let m = &self.theme.metrics;
        let strip = Rect::new(area.l, area.t, area.r, area.t + m.tab_height);
        let mut pl = PaneLayout { area, strip, ..Default::default() };
        let avail = strip.w() - 36.0;
        let mut widths: Vec<f32> = pane
            .tabs
            .iter()
            .map(|t| (self.gfx.measure(&t.title(), Font::Text) + 52.0).clamp(90.0, 220.0))
            .collect();
        let sum: f32 = widths.iter().sum();
        if sum > avail && sum > 0.0 {
            let k = avail / sum;
            widths.iter_mut().for_each(|w| *w = (*w * k).max(48.0));
        }
        let mut x = strip.l + 4.0;
        for w in widths {
            let r = Rect::new(x, strip.t + 4.0, x + w, strip.b);
            pl.tab_close.push(Rect::new(r.r - 26.0, r.t + 5.0, r.r - 6.0, r.b - 5.0));
            pl.tabs.push(r);
            x += w + 2.0;
        }
        pl.tab_new = Rect::new(x + 2.0, strip.t + 6.0, x + 30.0, strip.b - 2.0);

        let rh = self.row_h();
        // 縮圖檢視沒有欄位標題（依 Ctrl+1–4 排序）
        let header_h = if pane.tab().mode == ViewMode::Thumbnails { 0.0 } else { rh };
        pl.header = Rect::new(area.l, strip.b, area.r, strip.b + header_h);
        pl.list = Rect::new(area.l, pl.header.b, area.r, area.b);
        pl.scrollbar = Rect::new(area.r - SCROLLBAR_W, pl.list.t, area.r, pl.list.b);
        let content_r = area.r - SCROLLBAR_W;
        let [cm, ct, cs] = fit_columns(content_r - area.l, self.cols);
        let size_l = content_r - cs;
        let type_l = size_l - ct;
        let name_r = type_l - cm;
        pl.cols = [(area.l, name_r), (name_r, type_l), (type_l, size_l), (size_l, content_r)];
        pl
    }

    pub fn max_scroll(&self, pane: usize, list_h: f32) -> f32 {
        let t = self.panes[pane].tab();
        if let Some(g) = self.layout.panes.get(pane).and_then(|pl| self.grid(pane, pl.list)) {
            return (g.content_h(t.view.len()) - list_h).max(0.0);
        }
        (t.view.len() as f32 * self.row_h() - list_h).max(0.0)
    }

    pub fn tree_max_scroll(&self, tree_h: f32) -> f32 {
        (self.tree.nodes.len() as f32 * self.row_h() + 8.0 - tree_h).max(0.0)
    }

    // ------------------------------------------------------------ 點擊判定

    pub fn hit_test(&self, lay: &Layout, x: f32, y: f32) -> Hit {
        if self.menu.is_some() {
            return self.menu_hit(x, y).unwrap_or(Hit::None);
        }
        if let Some(h) = self.batch_hit(x, y) {
            return h;
        }
        if let Some(h) = self.palette_hit(x, y) {
            return h;
        }
        if let Some(h) = self.jobs_hit(lay, x, y) {
            return h;
        }
        if lay.update_indicator.is_some_and(|r| r.contains(x, y)) {
            return Hit::UpdateIndicator;
        }
        if lay.toolbar.contains(x, y) {
            if let Some(i) = lay.nav.iter().position(|r| r.contains(x, y)) {
                return Hit::Nav(i);
            }
            if let Some(i) = lay.buttons.iter().position(|r| r.contains(x, y)) {
                return Hit::Button(i);
            }
            if lay.settings_btn.contains(x, y) {
                return Hit::SettingsGear;
            }
            if lay.filter_chip.is_some_and(|r| r.contains(x, y)) {
                return Hit::FilterChip;
            }
            if let Some((_, p)) = lay.crumbs.iter().find(|(r, _)| r.contains(x, y)) {
                return Hit::Crumb(p.clone());
            }
            if lay.address.contains(x, y) {
                return Hit::Address;
            }
            return Hit::None;
        }
        if lay.tree_splitter.is_some_and(|r| r.contains(x, y)) {
            return Hit::TreeSplitter;
        }
        if let Some(sp) = lay.side_panel.filter(|r| r.contains(x, y)) {
            return self.side_hit(sp, x, y);
        }
        if let Some(bl) = lay.cmdbar.as_ref().filter(|b| b.bar.contains(x, y)) {
            return bl
                .buttons
                .iter()
                .position(|(i, r, _)| r.contains(x, y) && self.config.cmdbar.get(*i).is_some_and(|it| !it.is_separator()))
                .map_or(Hit::CmdBar, Hit::CmdButton);
        }
        if lay.preview_splitter.is_some_and(|r| r.contains(x, y)) {
            return Hit::PreviewSplitter;
        }
        if lay.preview.is_some_and(|r| r.contains(x, y)) {
            return Hit::Preview;
        }
        if let Some(tr) = lay.tree.filter(|r| r.contains(x, y)) {
            let rh = self.row_h();
            let i = ((y - tr.t - 4.0 + self.tree.scroll) / rh).floor();
            if i >= 0.0 && (i as usize) < self.tree.nodes.len() {
                let i = i as usize;
                let n = &self.tree.nodes[i];
                let arrow_l = tr.l + 6.0 + n.depth as f32 * 14.0;
                if x >= arrow_l && x < arrow_l + 18.0 {
                    return Hit::TreeArrow(i);
                }
                return Hit::TreeRow(i);
            }
            return Hit::TreeEmpty;
        }
        for (p, pl) in lay.panes.iter().enumerate() {
            if !pl.area.contains(x, y) {
                continue;
            }
            if pl.strip.contains(x, y) {
                for (t, r) in pl.tabs.iter().enumerate() {
                    if r.contains(x, y) {
                        if pl.tab_close[t].contains(x, y) && self.panes[p].tabs.len() > 1 {
                            return Hit::TabClose(p, t);
                        }
                        return Hit::Tab(p, t);
                    }
                }
                if pl.tab_new.contains(x, y) {
                    return Hit::TabNew(p);
                }
                return Hit::TabStrip(p);
            }
            if pl.header.contains(x, y) {
                for c in 0..4 {
                    let (l, r) = pl.cols[c];
                    if c < 3 && (x - r).abs() <= 4.0 {
                        return Hit::HeaderEdge(p, c);
                    }
                    if x >= l && x < r {
                        return Hit::Header(p, c);
                    }
                }
                return Hit::None;
            }
            if pl.scrollbar.contains(x, y) && self.max_scroll(p, pl.list.h()) > 0.0 {
                return Hit::Scrollbar(p);
            }
            if pl.list.contains(x, y) {
                let t = self.panes[p].tab();
                if let Some(g) = self.grid(p, pl.list) {
                    return match g.index_at(x - pl.list.l, y - pl.list.t + t.scroll, t.view.len()) {
                        Some(i) => Hit::Row(p, i),
                        None => Hit::ListEmpty(p),
                    };
                }
                let i = ((y - pl.list.t + t.scroll) / self.row_h()).floor();
                if i >= 0.0 && (i as usize) < t.view.len() {
                    return Hit::Row(p, i as usize);
                }
                return Hit::ListEmpty(p);
            }
        }
        Hit::None
    }

    // ------------------------------------------------------------ 繪製

    /// 重新命名框的外框（輸入框本身是子視窗，畫在框線裡）
    fn paint_rename_frame(&self) {
        let Some(f) = self.edit.as_ref().and_then(|e| e.frame) else { return };
        let (c, g) = (&self.theme.color, &self.gfx);
        g.fill_round(f, self.theme.metrics.radius, c.background);
        g.stroke_round(f, self.theme.metrics.radius, c.accent, 1.0);
    }

    pub fn paint(&mut self) {
        let lay = self.compute_layout();
        let c = self.theme.color.clone();
        if self.icons.take_evicted() {
            self.gfx.clear_icons();
        }
        if !self.gfx.begin(c.background) {
            return;
        }
        crate::perf::mark("begin_draw");
        self.paint_toolbar(&lay);
        self.paint_cmdbar(&lay);
        if let Some(tr) = lay.tree {
            self.paint_tree(tr);
        }
        for p in 0..lay.panes.len() {
            self.paint_pane(p, &lay.panes[p]);
            if p > 0 {
                let x = lay.panes[p].area.l - 1.0;
                self.gfx.vline(x, lay.panes[p].area.t, lay.panes[p].area.b, c.border);
            }
        }
        if let Some(tr) = lay.tree {
            let x = if self.tree_side == TreeSide::Right { tr.l - 1.0 } else { tr.r };
            self.gfx.vline(x, tr.t, tr.b, c.border);
        }
        if let Some(sp) = lay.side_panel {
            self.paint_side(sp);
        }
        if let Some(pr) = lay.preview {
            self.paint_preview(pr);
            self.gfx.vline(pr.l - 1.0, pr.t, pr.b, c.border);
        }
        // 縮圖：釋放快取丟掉的點陣圖；這一幀沒要求、還在排隊的工作取消
        let evicted = self.thumbs.take_evicted();
        if !evicted.is_empty() {
            self.gfx.drop_thumbs(&evicted);
        }
        self.thumbs.end_frame();
        self.paint_rename_frame();
        self.paint_jobs(&lay);
        self.paint_status(&lay);
        self.paint_palette();
        self.paint_batch();
        self.paint_ask();
        self.paint_menu();
        self.paint_tip();
        self.gfx.end();
        self.layout = lay;
        self.update_title();
    }

    fn paint_toolbar(&self, lay: &Layout) {
        let c = &self.theme.color;
        let g = &self.gfx;
        g.fill(lay.toolbar, c.surface);
        g.hline(0.0, lay.toolbar.r, lay.toolbar.b - 1.0, c.border);
        let tab = self.tab();
        let enabled = [!tab.back.is_empty(), !tab.forward.is_empty(), !tab.path.is_empty(), true];
        let icons = [glyph::BACK, glyph::FORWARD, glyph::UP, glyph::REFRESH];
        for i in 0..4 {
            if self.hover == Hit::Nav(i) && enabled[i] {
                g.fill_round(lay.nav[i], self.theme.metrics.radius, c.hover);
            }
            let col = if enabled[i] { c.text } else { c.text_secondary.with_alpha(110) };
            g.text(icons[i], Font::Glyph, lay.nav[i], col);
        }

        // 網址列
        g.fill_round(lay.address, self.theme.metrics.radius, c.background);
        g.stroke_round(lay.address, self.theme.metrics.radius, c.border, 1.0);
        if self.theme.chrome.address == AddressStyle::Path {
            // 完整路徑；點一下進入編輯（與麵包屑的空白處相同）
            let a = lay.address;
            let r = Rect::new(a.l + 8.0, a.t, a.r - 8.0, a.b);
            if tab.path.is_empty() {
                // 「此電腦」用一般字型：等寬字型沒有中文字形，啟動的第一幀會多一次字型後備查詢（約 1 ms）
                g.text(tr!("此電腦"), Font::Text, r, c.text_secondary);
            } else {
                g.text(&tab.path, Font::Mono, r, c.text);
            }
        }
        if lay.crumb_overflow {
            g.text("…", Font::Text, Rect::new(lay.address.l + 8.0, lay.address.t, lay.address.l + 28.0, lay.address.b), c.text_secondary);
        }
        let segs = path::segments(&tab.path);
        let first = segs.len() - lay.crumbs.len();
        for (k, (r, p)) in lay.crumbs.iter().enumerate() {
            if self.hover == Hit::Crumb(p.clone()) {
                g.fill_round(*r, 4.0, c.hover);
            }
            let name = &segs[first + k].0;
            let last = k + 1 == lay.crumbs.len();
            g.text(name, if last { Font::Bold } else { Font::Text }, Rect::new(r.l + 6.0, r.t, r.r, r.b), c.text);
            if !last {
                g.text(glyph::CHEVRON_RIGHT, Font::GlyphSmall, Rect::new(r.r, r.t, r.r + 16.0, r.b), c.text_secondary);
            }
        }

        if let Some(chip) = lay.filter_chip {
            g.fill_round(chip, 14.0, c.accent.with_alpha(40));
            g.text(glyph::FILTER, Font::GlyphSmall, Rect::new(chip.l + 6.0, chip.t, chip.l + 24.0, chip.b), c.accent);
            g.text(&tr!("篩選：{filter}", filter = tab.filter), Font::Small, Rect::new(chip.l + 26.0, chip.t, chip.r - 22.0, chip.b), c.text);
            g.text(glyph::CLOSE, Font::GlyphSmall, Rect::new(chip.r - 22.0, chip.t, chip.r - 4.0, chip.b), c.text_secondary);
        }
        // 設定按鈕：設定頁開著時以強調色顯示
        if self.hover == Hit::SettingsGear {
            g.fill_round(lay.settings_btn, self.theme.metrics.radius, c.hover);
        }
        let gear = if self.settings_page().is_some() { c.accent } else { c.text };
        g.text(glyph::SETTINGS, Font::Glyph, lay.settings_btn, gear);
        for (i, r) in lay.buttons.iter().enumerate() {
            g.fill_round(*r, self.theme.metrics.radius, if self.hover == Hit::Button(i) { c.hover } else { c.surface });
            g.stroke_round(*r, self.theme.metrics.radius, c.border, 1.0);
            g.text(&zing_core::i18n::tr_str(&self.config.buttons[i].label), Font::Small, Rect::new(r.l + 10.0, r.t, r.r - 6.0, r.b), c.text);
        }
    }

    /// 輸入法組字視窗的位置（DIP）：篩選標籤中文字的結尾，還沒有篩選時為標籤將出現的位置。
    /// 回傳 (文字結尾的 x, 標籤的範圍)
    pub fn ime_anchor(&self) -> (f32, Rect) {
        let slot = self.layout.filter_slot;
        let tw = self.gfx.measure(&tr!("篩選：{filter}", filter = self.tab().filter), Font::Small);
        let cw = (tw + 54.0).min(280.0);
        let chip = Rect::new(slot.r - cw, slot.t, slot.r, slot.b);
        ((chip.l + 26.0 + tw).min(chip.r - 22.0), chip)
    }

    fn paint_tree(&self, tr: Rect) {
        let c = &self.theme.color;
        let g = &self.gfx;
        g.fill(tr, c.surface);
        g.push_clip(tr);
        let rh = self.row_h();
        let current = self.tree.find(&self.tab().path);
        let first = (self.tree.scroll / rh).floor().max(0.0) as usize;
        let count = (tr.h() / rh).ceil() as usize + 1;
        for i in first..(first + count).min(self.tree.nodes.len()) {
            let n = &self.tree.nodes[i];
            let y = tr.t + 4.0 + i as f32 * rh - self.tree.scroll;
            let row = Rect::new(tr.l + 4.0, y, tr.r - 4.0, y + rh);
            let is_current = current == Some(i) && n.kind != NodeKind::Favorite;
            let x = tr.l + 6.0 + n.depth as f32 * 14.0;
            if is_current {
                let sel = if self.theme.chrome.selection_after_icon { Rect::new(x + 40.0, row.t, row.r, row.b) } else { row };
                g.fill_round(sel, self.theme.metrics.radius, if self.focus == Focus::Tree { c.selection } else { c.selection_inactive });
            } else if self.hover == Hit::TreeRow(i) || self.hover == Hit::TreeArrow(i) {
                g.fill_round(row, self.theme.metrics.radius, c.hover);
            }
            if self.focus == Focus::Tree && self.tree.focus == i {
                let ring = if is_current { c.selection_text } else { c.accent };
                g.stroke_round(row, self.theme.metrics.radius, ring, 1.0);
            }
            let header = matches!(n.kind, NodeKind::FavRoot | NodeKind::Pc);
            if n.arrow() {
                let arrow = if n.expanded { glyph::CHEVRON_DOWN_SMALL } else { glyph::CHEVRON_RIGHT_SMALL };
                g.text(arrow, Font::GlyphSmall, Rect::new(x, y, x + 18.0, y + rh), c.text_secondary);
            }
            let (icon, icol) = match n.kind {
                NodeKind::FavRoot => (glyph::STAR, c.accent),
                NodeKind::Pc => (glyph::PC, c.text_secondary),
                NodeKind::Drive => (glyph::DRIVE, c.text_secondary),
                _ => (glyph::FOLDER, c.folder),
            };
            let key = match n.kind {
                NodeKind::Drive => Some(IconKey::Path(n.path.clone())),
                NodeKind::Folder | NodeKind::Favorite => Some(IconKey::Folder),
                _ => None,
            };
            let icon_r = Rect::new(x + 18.0, y, x + 40.0, y + rh);
            let drawn = key.and_then(|k| self.icons.get(k)).is_some_and(|id| g.icon(id, &self.icons, icon_r, 1.0));
            if !drawn {
                g.text(icon, Font::Glyph, icon_r, icol);
            }
            let font = if header { Font::Bold } else { Font::Text };
            let label = if n.loading { format!("{}…", n.name) } else { n.name.clone() };
            let text_col = if is_current && self.focus == Focus::Tree { c.selection_text } else { c.text };
            g.text(&label, font, Rect::new(x + 44.0, y, row.r - 4.0, y + rh), text_col);
        }
        g.pop_clip();
    }

    fn paint_pane(&self, p: usize, pl: &PaneLayout) {
        let c = &self.theme.color;
        let g = &self.gfx;
        let pane = &self.panes[p];
        let tab = pane.tab();
        let active_pane = p == self.active_pane;
        let rh = self.row_h();
        let radius = self.theme.metrics.radius;

        // 分頁列
        g.fill(pl.strip, c.surface);
        for (t, r) in pl.tabs.iter().enumerate() {
            let active = t == pane.active;
            if active {
                g.fill_round(Rect::new(r.l, r.t, r.r, r.b + radius), radius, c.background);
                let multi = self.panes.len() > 1;
                if self.theme.chrome.tab_accent || (active_pane && multi) {
                    // 多窗格時只有作用中的窗格用強調色
                    let line = if active_pane || !multi { c.accent } else { c.border };
                    g.fill(Rect::new(r.l + radius, r.t, r.r - radius, r.t + 2.0), line);
                }
            } else if self.hover == Hit::Tab(p, t) || self.hover == Hit::TabClose(p, t) {
                g.fill_round(r.inset(0.0, 2.0), radius, c.hover);
            }
            let title = pane.tabs[t].title();
            let text_r = if pane.tabs.len() > 1 { pl.tab_close[t].l } else { r.r - 8.0 };
            g.text(glyph::FOLDER, Font::Glyph, Rect::new(r.l + 6.0, r.t, r.l + 28.0, r.b), c.folder);
            g.text(&title, Font::Text, Rect::new(r.l + 30.0, r.t, text_r, r.b), if active { c.text } else { c.text_secondary });
            if pane.tabs.len() > 1 && (active || self.hover == Hit::Tab(p, t) || self.hover == Hit::TabClose(p, t)) {
                if self.hover == Hit::TabClose(p, t) {
                    g.fill_round(pl.tab_close[t], 4.0, c.hover);
                }
                g.text(glyph::CLOSE, Font::GlyphSmall, pl.tab_close[t], c.text_secondary);
            }
        }
        if self.hover == Hit::TabNew(p) {
            g.fill_round(pl.tab_new, radius, c.hover);
        }
        g.text(glyph::ADD, Font::GlyphSmall, pl.tab_new, c.text_secondary);

        // 欄位標題（縮圖檢視沒有）
        let has_header = pl.header.h() > 0.0;
        if has_header {
        g.fill(pl.header, c.background);
        for col in 0..4 {
            let (l, r) = pl.cols[col];
            if r - l < 8.0 {
                continue;
            }
            if self.hover == Hit::Header(p, col) {
                g.fill(Rect::new(l, pl.header.t, r, pl.header.b), c.hover);
            }
            let font = if col == 3 { Font::SmallRight } else { Font::Small };
            let sorted_here = tab.spec.key == COLUMN_KEYS[col];
            let text_r = r - 22.0;
            g.text(zing_core::i18n::tr(COLUMN_TITLES[col]), font, Rect::new(l + 12.0, pl.header.t, text_r, pl.header.b), c.text_secondary);
            if sorted_here {
                let arrow = if tab.spec.ascending { glyph::SORT_UP } else { glyph::SORT_DOWN };
                g.text(arrow, Font::GlyphSmall, Rect::new(r - 20.0, pl.header.t, r - 4.0, pl.header.b), c.text_secondary);
            }
            if col < 3 {
                g.vline(r, pl.header.t + 6.0, pl.header.b - 6.0, c.border);
            }
        }
        g.hline(pl.area.l, pl.area.r, pl.header.b - 1.0, c.border);
        }

        // 清單：只繪製可見的列
        let list = pl.list;
        g.push_clip(list);
        let first = (tab.scroll / rh).floor().max(0.0) as usize;
        let visible = (list.h() / rh).ceil() as usize + 1;
        let list_focused = active_pane && self.focus == Focus::List;
        let drop_row = self.drop_hover.filter(|(dp, _)| *dp == p).and_then(|(_, r)| r);
        let size_key = crate::sizes::key(&tab.path);
        let grid = self.grid(p, list);
        if let Some(gr) = &grid {
            self.paint_grid(p, list, gr, list_focused, drop_row);
        }
        for i in first..(first + visible).min(tab.view.len()) {
            if grid.is_some() {
                break;
            }
            let e = tab.view[i] as usize;
            let entry = &tab.listing.entries[e];
            let y = list.t + i as f32 * rh - tab.scroll;
            let row = Rect::new(list.l + 4.0, y, list.r - SCROLLBAR_W, y + rh);
            let selected = tab.is_selected(e);
            // 焦點所在的選取列：文字一律用選取文字色（緊湊風格的實心強調色上用深色字）
            let on_selection = selected && list_focused && drop_row != Some(i);
            if drop_row == Some(i) {
                g.fill_round(row, radius, c.accent.with_alpha(70));
            } else if selected {
                let (nl, _) = pl.cols[0];
                let sel = if self.theme.chrome.selection_after_icon { Rect::new(nl + 34.0, row.t, row.r, row.b) } else { row };
                g.fill_round(sel, radius, if list_focused { c.selection } else { c.selection_inactive });
            } else if self.hover == Hit::Row(p, i) {
                g.fill_round(row, radius, c.hover);
            }
            if list_focused && i == tab.focus {
                // 實心選取色上的焦點框用選取文字色才看得見；選取底色不含圖示時框也只框住文字部分
                let ring_r = if on_selection && self.theme.chrome.selection_after_icon {
                    Rect::new(pl.cols[0].0 + 34.0, row.t, row.r, row.b)
                } else {
                    row
                };
                g.stroke_round(ring_r, radius, if on_selection { c.selection_text } else { c.accent }, 1.0);
            }
            let name = tab.listing.name(e);
            let dir = entry.is_dir();
            let mut text_col = if entry.is_hidden() { c.text_secondary } else { c.text };
            if !dir
                && let Some((_, col)) = self.rules.iter().find(|(pat, _)| pat.matches(name)) {
                    text_col = *col;
                }
            let secondary = if on_selection { c.selection_text } else { c.text_secondary };
            if on_selection {
                text_col = c.selection_text;
            }
            let (nl, nr) = pl.cols[0];
            // 系統圖示；尚未載入時先畫字型圖示
            let icon_r = Rect::new(nl + 8.0, y, nl + 32.0, y + rh);
            let opacity = if entry.is_hidden() { 0.5 } else { 1.0 };
            let key = IconKey::for_entry(&tab.path, &tab.listing, e);
            if !self.icons.get(key).is_some_and(|id| g.icon(id, &self.icons, icon_r, opacity)) {
                g.text(if dir { glyph::FOLDER } else { glyph::FILE }, Font::Glyph, icon_r, if dir { c.folder } else { c.text_secondary });
            }
            g.text_w(name, Font::Text, Rect::new(nl + 36.0, y, nr - 6.0, y + rh), text_col);
            let (ml, mr) = pl.cols[1];
            if mr - ml > 20.0 {
                g.text(&format::filetime_local(entry.modified), Font::Text, Rect::new(ml + 12.0, y, mr - 6.0, y + rh), secondary);
            }
            let (tl, tr) = pl.cols[2];
            if tr - tl > 20.0 {
                let label = if tab.path.is_empty() { tr!("磁碟機").to_string() } else { format::type_label(dir, tab.listing.ext(e)) };
                g.text(&label, Font::Text, Rect::new(tl + 12.0, y, tr - 6.0, y + rh), secondary);
            }
            let (sl, sr) = pl.cols[3];
            if sr - sl > 20.0 {
                let r = Rect::new(sl + 6.0, y, sr - 12.0, y + rh);
                if !dir {
                    g.text(&format::size_kb(entry.size), Font::TextRight, r, secondary);
                } else if let Some((text, computing)) = self.folder_size_text(&size_key, &tab.listing.name_string(e)) {
                    g.text(&text, Font::TextRight, r, if computing && !on_selection { c.accent } else { secondary });
                }
            }
        }
        // 空狀態
        if tab.view.is_empty() {
            let msg = if let Some(e) = &tab.error {
                Some((e.clone(), c.error))
            } else if tab.loading {
                Some((tr!("正在載入…").to_string(), c.text_secondary))
            } else if !tab.filter.is_empty() {
                Some((tr!("沒有符合「{filter}」的項目（Esc 清除篩選）", filter = tab.filter), c.text_secondary))
            } else {
                Some((tr!("這個資料夾是空的").to_string(), c.text_secondary))
            };
            if let Some((m, col)) = msg {
                g.text(&m, Font::Text, Rect::new(list.l + 20.0, list.t + 8.0, list.r - 20.0, list.t + 8.0 + rh), col);
            }
        }
        if drop_row.is_none() && self.drop_hover.is_some_and(|(dp, r)| dp == p && r.is_none()) {
            g.stroke_round(list.inset(2.0, 2.0), radius, c.accent, 2.0);
        }
        g.pop_clip();

        // 捲軸
        let max = self.max_scroll(p, list.h());
        if max > 0.0 {
            let track = pl.scrollbar.inset(2.0, 2.0);
            let thumb_h = (track.h() * list.h() / (list.h() + max)).max(24.0);
            let thumb_t = track.t + (track.h() - thumb_h) * (tab.scroll / max);
            let wide = matches!(self.hover, Hit::Scrollbar(hp) if hp == p) || matches!(self.press, Some(crate::app::Press::Scroll { pane, .. }) if pane == p);
            let tw = if wide { 6.0 } else { 3.0 };
            let thumb = Rect::new(track.r - tw, thumb_t, track.r, thumb_t + thumb_h);
            g.fill_round(thumb, tw / 2.0, c.text_secondary.with_alpha(if wide { 200 } else { 120 }));
        }
    }

    /// 縮圖檢視：只畫可見的格子，縮圖未就緒時先畫圖示
    fn paint_grid(&self, p: usize, list: Rect, gr: &Grid, list_focused: bool, drop_row: Option<usize>) {
        let c = &self.theme.color;
        let g = &self.gfx;
        let tab = self.panes[p].tab();
        let radius = self.theme.metrics.radius;
        let px = crate::thumbs::bucket(gr.thumb * g.scale());
        let at = |x: f32, y: f32, w: f32, h: f32| {
            let (l, t) = (list.l + x, list.t + y - tab.scroll);
            Rect::new(l, t, l + w, t + h)
        };
        for i in gr.visible(tab.scroll, list.h(), tab.view.len()) {
            let e = tab.view[i] as usize;
            let entry = &tab.listing.entries[e];
            let cell = gr.cell(i);
            let r = at(cell.x, cell.y, cell.w, cell.h).inset(2.0, 2.0);
            let selected = tab.is_selected(e);
            let on_selection = selected && list_focused && drop_row != Some(i);
            if drop_row == Some(i) {
                g.fill_round(r, radius, c.accent.with_alpha(70));
            } else if selected {
                g.fill_round(r, radius, if list_focused { c.selection } else { c.selection_inactive });
            } else if self.hover == Hit::Row(p, i) {
                g.fill_round(r, radius, c.hover);
            }
            if list_focused && i == tab.focus {
                g.stroke_round(r, radius, if on_selection { c.selection_text } else { c.accent }, 1.0);
            }
            let tb = gr.thumb_box(cell);
            let boxr = at(tb.x, tb.y, tb.w, tb.h);
            let dir = entry.is_dir();
            let opacity = if entry.is_hidden() { 0.5 } else { 1.0 };
            let full = if tab.path.is_empty() { tab.listing.name_string(e) } else { tab.full_path(e) };
            let drawn = self.thumbs.get(&full, dir, px).is_some_and(|id| g.thumb(id, &self.thumbs, boxr, opacity));
            if !drawn {
                let key = IconKey::for_entry(&tab.path, &tab.listing, e);
                if !self.icons.get(key).is_some_and(|id| g.icon(id, &self.icons, boxr, opacity)) {
                    g.text(if dir { glyph::FOLDER } else { glyph::FILE }, Font::Glyph, boxr, if dir { c.folder } else { c.text_secondary });
                }
            }
            let cap = gr.caption(cell);
            let capr = at(cap.x, cap.y, cap.w, cap.h);
            let col = if on_selection { c.selection_text } else if entry.is_hidden() { c.text_secondary } else { c.text };
            g.push_clip(capr);
            g.text_w(tab.listing.name(e), Font::Caption, capr, col);
            g.pop_clip();
        }
    }

    /// 預覽窗格內各區塊：(名稱列, 內容, 屬性區)
    pub fn preview_rects(&self, r: Rect) -> (Rect, Rect, Rect) {
        let rh = self.row_h();
        let inner = r.inset(12.0, 10.0);
        let rows = self.preview.loaded.as_ref().map_or(0, |l| {
            let folder = matches!(l.body, crate::preview::Body::Folder { items: Some(_), .. }) as usize;
            l.props.len().min(PREVIEW_PROPS) + folder
        });
        let props_h = rows as f32 * rh;
        let name = Rect::new(inner.l, inner.t, inner.r, inner.t + rh);
        let props = Rect::new(inner.l, inner.b - props_h, inner.r, inner.b);
        let content = Rect::new(inner.l, name.b + 8.0, inner.r, (props.t - if rows > 0 { 10.0 } else { 0.0 }).max(name.b + 8.0));
        (name, content, props)
    }

    fn paint_preview(&self, r: Rect) {
        let c = &self.theme.color;
        let g = &self.gfx;
        g.fill(r, c.surface);
        let (name_r, content, props_r) = self.preview_rects(r);
        let rh = self.row_h();
        let Some(ld) = self.preview.loaded.as_ref().filter(|l| l.path == self.preview.target) else {
            let msg = if self.preview.target.is_empty() { tr!("選取檔案以預覽") } else { tr!("載入中…") };
            g.text(msg, Font::Text, name_r, c.text_secondary);
            return;
        };
        let name = path::file_name(&ld.path);
        g.text(if name.is_empty() { &ld.path } else { name }, Font::Bold, name_r, c.text);
        // 預覽處理常式的視窗蓋在內容區上：載入中顯示提示，完成後不畫
        match self.preview.host_state {
            crate::preview::HostState::Waiting => {
                g.text(tr!("載入預覽…"), Font::Text, Rect::new(content.l, content.t, content.r, content.t + rh), c.text_secondary);
            }
            crate::preview::HostState::Shown => {}
            _ => self.paint_preview_body(ld, content),
        }
        self.paint_preview_props(ld, props_r);
    }

    fn paint_preview_body(&self, ld: &crate::preview::Loaded, content: Rect) {
        use crate::preview::Body;
        let c = &self.theme.color;
        let g = &self.gfx;
        let rh = self.row_h();
        match &ld.body {
            Body::Image(Some(p)) | Body::Folder { image: Some(p), .. } => {
                g.preview_image(self.preview.image_id, p, content);
            }
            Body::Image(None) | Body::Folder { image: None, .. } => {
                g.text(tr!("沒有可用的預覽"), Font::Text, Rect::new(content.l, content.t, content.r, content.t + rh), c.text_secondary);
            }
            Body::Text { lines, truncated } => {
                let lh = self.caption_line_h();
                g.push_clip(content);
                let first = (self.preview.scroll / lh).floor().max(0.0) as usize;
                for (k, line) in lines.iter().enumerate().skip(first) {
                    let y = content.t + k as f32 * lh - self.preview.scroll;
                    if y > content.b {
                        break;
                    }
                    g.text(line, Font::Mono, Rect::new(content.l, y, content.r + 2000.0, y + lh), c.text);
                }
                if *truncated && first + ((content.h() / lh) as usize) >= lines.len() {
                    let y = content.t + lines.len() as f32 * lh - self.preview.scroll;
                    g.text(tr!("（只顯示檔案開頭）"), Font::Small, Rect::new(content.l, y, content.r, y + lh), c.text_secondary);
                }
                g.pop_clip();
            }
        }
    }

    fn paint_preview_props(&self, ld: &crate::preview::Loaded, props_r: Rect) {
        use crate::preview::Body;
        let c = &self.theme.color;
        let g = &self.gfx;
        let rh = self.row_h();
        // 屬性：名稱與值兩欄
        let label_w = (props_r.w() * 0.4).min(130.0);
        let mut y = props_r.t;
        let mut row = |label: &str, value: &str| {
            g.text(label, Font::Small, Rect::new(props_r.l, y, props_r.l + label_w, y + rh), c.text_secondary);
            g.text(value, Font::Small, Rect::new(props_r.l + label_w + 8.0, y, props_r.r, y + rh), c.text);
            y += rh;
        };
        if let Body::Folder { items: Some(n), .. } = &ld.body {
            row(tr!("項目"), &tr!("{n} 個", n));
        }
        for (l, v) in ld.props.iter().take(PREVIEW_PROPS) {
            row(l, v);
        }
    }

    fn paint_status(&self, lay: &Layout) {
        let c = &self.theme.color;
        let g = &self.gfx;
        let s = lay.status;
        g.fill(s, c.surface);
        g.hline(0.0, s.r, s.t, c.border);
        let tab = self.tab();
        let (n_sel, bytes) = tab.selection_count();
        let mut left = tr!("{n} 個項目", n = tab.view.len());
        if tab.view.len() != tab.listing.len() {
            left.push_str(&tr!("（共 {n}）", n = tab.listing.len()));
        }
        if n_sel > 0 {
            left.push_str(&tr!("　已選取 {n_sel} 個", n_sel));
            if bytes > 0 {
                left.push_str(&tr!("（{size}）", size = format::size_short(bytes)));
            }
        }
        if tab.loading {
            left.push_str(tr!("　載入中…"));
        }
        if self.sizes.pending > 0 {
            left.push_str(&tr!("　計算資料夾大小（剩 {n} 個）", n = self.sizes.pending));
        }
        g.text(&left, Font::Small, Rect::new(12.0, s.t, s.r * 0.5, s.b), c.text_secondary);
        // 「有新版本」一直顯示；其右側（檔案作業進度）照舊，左側的訊息與排序資訊讓出位置
        let mut right_edge = s.r - 12.0;
        if let (Some(r), Some(text)) = (lay.update_indicator, self.update_indicator_text()) {
            if self.hover == Hit::UpdateIndicator {
                g.fill_round(r.inset(0.0, 3.0), self.theme.metrics.radius, c.hover);
            }
            g.text(&text, Font::Small, Rect::new(r.l + 8.0, r.t, r.r, r.b), c.accent);
            right_edge = r.l - 8.0;
        }
        if let (Some(r), Some(text)) = (lay.jobs_indicator, self.jobs_indicator_text()) {
            if self.hover == Hit::JobsIndicator {
                g.fill_round(r.inset(0.0, 3.0), self.theme.metrics.radius, c.hover);
            }
            g.text(&text, Font::Small, Rect::new(r.l + 8.0, r.t, r.r, r.b), c.accent);
        } else if let Some((msg, err, _)) = &self.status {
            g.text(msg, Font::SmallRight, Rect::new(s.r * 0.35, s.t, right_edge, s.b), if *err { c.error } else { c.text });
        } else {
            let mut right = Vec::new();
            if self.show_hidden {
                right.push(tr!("顯示隱藏檔").to_string());
            }
            right.push(tr!(
                "排序：{col}{arrow}",
                col = zing_core::i18n::tr(COLUMN_TITLES[COLUMN_KEYS.iter().position(|k| *k == tab.spec.key).unwrap_or(0)]),
                arrow = if tab.spec.ascending { "↑" } else { "↓" }
            ));
            g.text(&right.join(tr!("　")), Font::SmallRight, Rect::new(s.r * 0.5, s.t, right_edge, s.b), c.text_secondary);
        }
    }
}

pub fn clamp_col(w: f32) -> f32 {
    w.clamp(COL_MIN, 600.0)
}

/// 名稱欄至少保留的寬度
const NAME_MIN: f32 = 180.0;

/// 窗格太窄時整欄收起（不縮小欄寬，日期、大小不會被截斷）：先收起類型，再收起大小，最後是修改日期。
/// `widths` 為修改日期、類型、大小的寬度，`avail` 為清單的內容寬度；收起的欄位寬度為 0，多出的空間給名稱欄
fn fit_columns(avail: f32, widths: [f32; 3]) -> [f32; 3] {
    let mut w = widths;
    let room = (avail - NAME_MIN).max(0.0);
    for i in [1, 2, 0] {
        if w.iter().sum::<f32>() <= room {
            break;
        }
        w[i] = 0.0;
    }
    w
}

#[cfg(test)]
mod column_tests {
    use super::*;

    #[test]
    fn narrow_panes_drop_whole_columns() {
        let cols = [150.0, 120.0, 90.0];
        assert_eq!(fit_columns(1000.0, cols), cols);
        // 依序收起類型、大小、修改日期
        assert_eq!(fit_columns(180.0 + 300.0, cols), [150.0, 0.0, 90.0]);
        assert_eq!(fit_columns(180.0 + 200.0, cols), [150.0, 0.0, 0.0]);
        assert_eq!(fit_columns(180.0 + 100.0, cols), [0.0, 0.0, 0.0]);
        assert_eq!(fit_columns(50.0, cols), [0.0, 0.0, 0.0]);
    }
}
