//! 縮圖檢視的格狀版面：每格上方是縮圖、下方最多兩行檔名。
//!
//! 座標以清單區域左上角為原點、未扣捲動量（內容座標）。欄數依可用寬度決定，
//! 剩餘寬度平均分給每一格，讓整列貼齊兩側。格子之間與清單邊緣留空白（不屬於任何項目，
//! 點了取消選取、按右鍵是資料夾本身的選單），項目再多也隨時有地方可以點。

/// 格與清單邊緣的間距
const MARGIN: f32 = 8.0;
/// 相鄰兩格之間的空白
const SPACING: f32 = 10.0;
/// 縮圖框四周（左右各 12、上方 6）
const PAD_X: f32 = 12.0;
const PAD_TOP: f32 = 6.0;
/// 縮圖與檔名之間、檔名下方
const GAP: f32 = 4.0;
const PAD_BOTTOM: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub cols: usize,
    pub cell_w: f32,
    pub cell_h: f32,
    /// 縮圖框邊長
    pub thumb: f32,
    /// 一行檔名的高度
    pub line_h: f32,
}

impl Grid {
    /// `width`：清單內容寬度（不含捲軸）；`thumb`：縮圖邊長；`line_h`：一行文字高度
    pub fn new(width: f32, thumb: f32, line_h: f32) -> Self {
        let min_w = thumb + PAD_X * 2.0 + SPACING;
        let avail = (width - MARGIN * 2.0).max(min_w);
        let cols = ((avail / min_w).floor() as usize).max(1);
        Grid { cols, cell_w: avail / cols as f32, cell_h: PAD_TOP + thumb + GAP + line_h * 2.0 + PAD_BOTTOM + SPACING, thumb, line_h }
    }

    pub fn rows(&self, n: usize) -> usize {
        n.div_ceil(self.cols)
    }

    pub fn content_h(&self, n: usize) -> f32 {
        self.rows(n) as f32 * self.cell_h + MARGIN * 2.0
    }

    /// 第 `i` 項的格子（不含與相鄰格子之間的空白）
    pub fn cell(&self, i: usize) -> Cell {
        let (r, c) = (i / self.cols, i % self.cols);
        let half = SPACING / 2.0;
        Cell {
            x: MARGIN + c as f32 * self.cell_w + half,
            y: MARGIN + r as f32 * self.cell_h + half,
            w: self.cell_w - SPACING,
            h: self.cell_h - SPACING,
        }
    }

    /// 格子內的縮圖框
    pub fn thumb_box(&self, c: Cell) -> Cell {
        Cell { x: c.x + (c.w - self.thumb) / 2.0, y: c.y + PAD_TOP, w: self.thumb, h: self.thumb }
    }

    /// 格子內的檔名區（兩行）
    pub fn caption(&self, c: Cell) -> Cell {
        Cell { x: c.x + 4.0, y: c.y + PAD_TOP + self.thumb + GAP, w: c.w - 8.0, h: self.line_h * 2.0 }
    }

    /// 內容座標上的項目；格子之間的空白與最後一列之後回傳 None
    pub fn index_at(&self, x: f32, y: f32, n: usize) -> Option<usize> {
        if x < MARGIN || y < MARGIN {
            return None;
        }
        let c = ((x - MARGIN) / self.cell_w).floor() as usize;
        let r = ((y - MARGIN) / self.cell_h).floor() as usize;
        if c >= self.cols {
            return None;
        }
        let i = r * self.cols + c;
        let cell = self.cell(i);
        let inside = x >= cell.x && x < cell.x + cell.w && y >= cell.y && y < cell.y + cell.h;
        (inside && i < n).then_some(i)
    }

    /// 可見範圍內的項目（含部分可見的列）
    pub fn visible(&self, scroll: f32, height: f32, n: usize) -> std::ops::Range<usize> {
        let first_row = ((scroll - MARGIN) / self.cell_h).floor().max(0.0) as usize;
        let last_row = ((scroll + height - MARGIN) / self.cell_h).ceil().max(0.0) as usize;
        (first_row * self.cols).min(n)..(last_row * self.cols).min(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fill_width() {
        let g = Grid::new(1000.0, 96.0, 16.0);
        // 每格至少 96 + 24 + 10 = 130，(1000 - 16) / 130 = 7 欄，剩餘平均分配
        assert_eq!(g.cols, 7);
        assert!((g.cell_w - 984.0 / 7.0).abs() < 0.01);
        assert_eq!(g.cell_h, 6.0 + 96.0 + 4.0 + 32.0 + 8.0 + 10.0);
        // 太窄時至少一欄
        assert_eq!(Grid::new(50.0, 256.0, 16.0).cols, 1);
    }

    #[test]
    fn cells_and_hit_test() {
        let g = Grid::new(1000.0, 96.0, 16.0);
        let c = g.cell(8); // 第 2 列第 2 欄
        assert_eq!((c.x, c.y), (8.0 + g.cell_w + 5.0, 8.0 + g.cell_h + 5.0));
        assert_eq!(g.index_at(c.x + 1.0, c.y + 1.0, 20), Some(8));
        assert_eq!(g.index_at(c.x + 1.0, c.y + 1.0, 8), None); // 超過項目數
        assert_eq!(g.index_at(4.0, 50.0, 20), None); // 邊緣的空白
        assert_eq!(g.index_at(c.x - 2.0, c.y + 20.0, 20), None); // 格子之間的空白
        assert_eq!(g.index_at(c.x + 20.0, c.y - 2.0, 20), None);
        let t = g.thumb_box(c);
        assert!((t.x - (c.x + (c.w - 96.0) / 2.0)).abs() < 0.01 && t.w == 96.0);
        assert_eq!(g.caption(c).y, c.y + 6.0 + 96.0 + 4.0);
    }

    #[test]
    fn rows_and_visible_range() {
        let g = Grid::new(1000.0, 96.0, 16.0);
        assert_eq!(g.rows(17), 3);
        assert_eq!(g.content_h(17), 3.0 * g.cell_h + 16.0);
        assert_eq!(g.visible(0.0, g.cell_h * 1.5, 100), 0..14);
        // 捲到第 3 列中間：第 3、4 列可見（每列 7 格）
        assert_eq!(g.visible(8.0 + g.cell_h * 2.5, g.cell_h, 100), 14..28);
        assert_eq!(g.visible(0.0, 10_000.0, 5), 0..5);
    }
}
