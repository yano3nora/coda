//! In-memory styled terminal screen buffer.
//!
//! `Screen` is deliberately UI-local: callers provide already-expanded text
//! (for example, tabs should be expanded before calling `put_str`). This module
//! still sanitizes control characters to a single space so invalid input cannot
//! leak terminal control bytes into the renderer.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Minimal cell attributes for MVP rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub reverse: bool,
    pub dim: bool,
    /// Foreground RGB color. Background intentionally stays terminal-owned.
    pub fg: Option<(u8, u8, u8)>,
}

/// One terminal grid cell.
///
/// A printable grapheme starts at a cell with `width` 1 or 2. The second cell of
/// a wide grapheme is represented as a continuation cell: empty `symbol` and
/// `width == 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub symbol: String,
    pub width: u8,
    pub style: Style,
}

impl Cell {
    pub fn blank() -> Self {
        Self {
            symbol: " ".to_string(),
            width: 1,
            style: Style::default(),
        }
    }

    pub fn is_continuation(&self) -> bool {
        self.width == 0
    }
}

/// Styled terminal grid with an optional cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
    pub cursor: Option<(u16, u16)>,
}

impl Screen {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::blank(); usize::from(width) * usize::from(height)],
            cursor: None,
        }
    }

    /// Resizes the screen and clears every cell.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.cells = vec![Cell::blank(); usize::from(width) * usize::from(height)];
        self.cursor = None;
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn row(&self, y: u16) -> Option<&[Cell]> {
        if y >= self.height {
            return None;
        }
        let start = usize::from(y) * usize::from(self.width);
        let end = start + usize::from(self.width);
        Some(&self.cells[start..end])
    }

    pub fn cell(&self, x: u16, y: u16) -> Option<&Cell> {
        self.index(x, y).map(|index| &self.cells[index])
    }

    /// Writes text at `(x, y)` and returns the next x position.
    ///
    /// Graphemes are clipped at the right edge. A wide grapheme is not written
    /// when only one column remains, because allowing it would corrupt the next
    /// terminal row on real terminals.
    pub fn put_str(&mut self, x: u16, y: u16, text: &str, style: Style) -> usize {
        if y >= self.height || x >= self.width {
            return usize::from(x);
        }

        let mut next_x = x;
        for grapheme in text.graphemes(true) {
            let symbol = sanitize_grapheme(grapheme);
            let width = display_width(&symbol);
            if width == 0 {
                continue;
            }
            if usize::from(next_x) + width > usize::from(self.width) {
                break;
            }

            self.put_grapheme(next_x, y, &symbol, width as u8, style);
            next_x += width as u16;
        }

        usize::from(next_x)
    }

    pub fn set_cursor(&mut self, x: u16, y: u16) {
        self.cursor = Some((x, y));
    }

    pub fn hide_cursor(&mut self) {
        self.cursor = None;
    }

    fn put_grapheme(&mut self, x: u16, y: u16, symbol: &str, width: u8, style: Style) {
        self.clear_cell_for_write(x, y);
        if width == 2 && x + 1 < self.width {
            self.clear_cell_for_write(x + 1, y);
        }

        let index = self.index(x, y).expect("coordinates checked before write");
        self.cells[index] = Cell {
            symbol: symbol.to_string(),
            width,
            style,
        };

        if width == 2 {
            let continuation = self.index(x + 1, y).expect("wide cell fits before write");
            self.cells[continuation] = Cell {
                symbol: String::new(),
                width: 0,
                style,
            };
        }
    }

    fn clear_cell_for_write(&mut self, x: u16, y: u16) {
        let Some(index) = self.index(x, y) else {
            return;
        };

        if self.cells[index].is_continuation()
            && x > 0
            && let Some(previous) = self.index(x - 1, y)
            && self.cells[previous].width == 2
        {
            self.cells[previous] = Cell::blank();
        }

        if self.cells[index].width == 2
            && x + 1 < self.width
            && let Some(next) = self.index(x + 1, y)
        {
            self.cells[next] = Cell::blank();
        }

        self.cells[index] = Cell::blank();
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(usize::from(y) * usize::from(self.width) + usize::from(x))
    }
}

fn sanitize_grapheme(grapheme: &str) -> String {
    if grapheme.chars().any(char::is_control) {
        " ".to_string()
    } else {
        grapheme.to_string()
    }
}

fn display_width(symbol: &str) -> usize {
    UnicodeWidthStr::width(symbol)
}

#[cfg(test)]
mod tests {
    use super::{Screen, Style};

    #[test]
    fn put_str_places_ascii_japanese_and_zwj_emoji_by_display_width() {
        let mut screen = Screen::new(12, 1);
        let next = screen.put_str(0, 0, "aあ👨‍👩‍👧‍👦", Style::default());

        assert_eq!(screen.cell(0, 0).unwrap().symbol, "a");
        assert_eq!(screen.cell(0, 0).unwrap().width, 1);
        assert_eq!(screen.cell(1, 0).unwrap().symbol, "あ");
        assert_eq!(screen.cell(1, 0).unwrap().width, 2);
        assert!(screen.cell(2, 0).unwrap().is_continuation());
        assert_eq!(screen.cell(3, 0).unwrap().symbol, "👨‍👩‍👧‍👦");
        assert!(screen.cell(3, 0).unwrap().width >= 1);
        assert_eq!(next, 3 + usize::from(screen.cell(3, 0).unwrap().width));
    }

    #[test]
    fn put_str_clips_at_right_edge() {
        let mut screen = Screen::new(4, 1);
        let next = screen.put_str(0, 0, "あいう", Style::default());

        assert_eq!(next, 4);
        assert_eq!(screen.cell(0, 0).unwrap().symbol, "あ");
        assert!(screen.cell(1, 0).unwrap().is_continuation());
        assert_eq!(screen.cell(2, 0).unwrap().symbol, "い");
        assert!(screen.cell(3, 0).unwrap().is_continuation());
    }

    #[test]
    fn wide_grapheme_is_not_written_when_only_one_column_remains() {
        let mut screen = Screen::new(3, 1);
        screen.put_str(0, 0, "ab", Style::default());
        let next = screen.put_str(2, 0, "あ", Style::default());

        assert_eq!(next, 2);
        assert_eq!(screen.cell(2, 0).unwrap().symbol, " ");
    }

    #[test]
    fn control_characters_are_replaced_with_spaces() {
        let mut screen = Screen::new(4, 1);
        screen.put_str(0, 0, "a\tb", Style::default());

        assert_eq!(screen.cell(0, 0).unwrap().symbol, "a");
        assert_eq!(screen.cell(1, 0).unwrap().symbol, " ");
        assert_eq!(screen.cell(2, 0).unwrap().symbol, "b");
    }

    #[test]
    fn resize_clears_all_cells_and_cursor() {
        let mut screen = Screen::new(2, 1);
        screen.put_str(0, 0, "あ", Style::default());
        screen.set_cursor(1, 0);

        screen.resize(3, 2);

        assert_eq!(screen.size(), (3, 2));
        assert_eq!(screen.cursor, None);
        for y in 0..2 {
            for x in 0..3 {
                let cell = screen.cell(x, y).unwrap();
                assert_eq!(cell.symbol, " ");
                assert_eq!(cell.width, 1);
            }
        }
    }
}
