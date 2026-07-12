//! Editor buffer viewport renderer.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    core::{editor::EditorCore, position::Position},
    highlight::HighlightSpan,
    ui::{Screen, Style},
};

/// Marker drawn where a line is cut off by the viewport edge (wrap off).
const TRUNCATION_MARKER: &str = "…";

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct EditorView {
    pub top_line: usize,
    pub left_col: usize,
    /// Wrap mode only: index of the first visible visual row *inside*
    /// `top_line`, so a logical line taller than the viewport can still be
    /// scrolled through. Always 0 while wrap is off.
    pub top_segment: usize,
}

pub struct StatusLine<'a> {
    pub filename: &'a str,
    pub modified: bool,
    pub message: &'a str,
    pub pending: &'a str,
}

impl EditorView {
    /// Width of the line-number gutter, including one trailing space.
    ///
    /// Sized to the whole buffer (not the viewport) so the text area does not
    /// shift while scrolling through the file.
    fn gutter_width(editor: &EditorCore) -> usize {
        let digits = editor.buffer.line_count().max(1).to_string().len();
        digits.max(3) + 1
    }

    pub fn draw(
        &mut self,
        editor: &EditorCore,
        screen: &mut Screen,
        highlights: &[Vec<HighlightSpan>],
        status: StatusLine<'_>,
        origin_y: u16,
        wrap: bool,
    ) {
        let gutter = Self::gutter_width(editor);
        let editor_rows = screen.height().saturating_sub(origin_y + 1) as usize;
        let editor_cols = (screen.width() as usize).saturating_sub(gutter);
        if wrap {
            // Wrap mode has no horizontal scroll; visual rows absorb the width.
            self.left_col = 0;
            self.ensure_cursor_visible_wrapped(editor, editor_rows, editor_cols);
            self.draw_wrapped_rows(editor, screen, highlights, origin_y, gutter, editor_rows);
        } else {
            self.top_segment = 0;
            self.ensure_cursor_visible(editor, editor_rows, editor_cols);
            self.draw_unwrapped_rows(editor, screen, highlights, origin_y, gutter, editor_rows);
        }

        if screen.height() > 0 {
            let line = editor.cursor.line + 1;
            let col = editor.cursor.grapheme + 1;
            let dirty = if status.modified { " [+]" } else { "" };
            let text = format!(
                "{}{} | Ln {},Col {} | {} | {}",
                status.filename, dirty, line, col, status.message, status.pending
            );
            screen.put_str(
                0,
                screen.height() - 1,
                &text,
                Style {
                    reverse: true,
                    dim: false,
                    fg: None,
                },
            );
        }

        let cursor = if wrap {
            self.cursor_screen_position_wrapped(editor, editor_cols)
        } else {
            self.cursor_screen_position(editor)
        };
        if let Some((x, y)) = cursor
            && x < screen.width()
            && origin_y + y < screen.height().saturating_sub(1)
        {
            screen.set_cursor(x, origin_y + y);
        }
    }

    fn draw_unwrapped_rows(
        &self,
        editor: &EditorCore,
        screen: &mut Screen,
        highlights: &[Vec<HighlightSpan>],
        origin_y: u16,
        gutter: usize,
        editor_rows: usize,
    ) {
        let editor_cols = (screen.width() as usize).saturating_sub(gutter);
        for row in 0..editor_rows {
            let line_index = self.top_line + row;
            let Some(line) = editor.buffer.line(line_index) else {
                continue;
            };
            let y = origin_y + row as u16;
            draw_gutter_number(screen, y, gutter, line_index);
            draw_line(
                screen,
                line,
                line_index,
                y,
                gutter as u16,
                self.left_col,
                editor.selection.map(|selection| selection.range()),
                highlights.get(row).map(Vec::as_slice).unwrap_or(&[]),
            );
            draw_truncation_markers(screen, line, y, gutter, self.left_col, editor_cols);
        }
    }

    fn draw_wrapped_rows(
        &self,
        editor: &EditorCore,
        screen: &mut Screen,
        highlights: &[Vec<HighlightSpan>],
        origin_y: u16,
        gutter: usize,
        editor_rows: usize,
    ) {
        let editor_cols = (screen.width() as usize).saturating_sub(gutter);
        let selection = editor.selection.map(|selection| selection.range());
        let mut row = 0usize;
        let mut line_index = self.top_line;
        let mut segment_index = self.top_segment;
        while row < editor_rows {
            let Some(line) = editor.buffer.line(line_index) else {
                break;
            };
            let segments = wrap_segments(line, editor_cols);
            while segment_index < segments.len() && row < editor_rows {
                let y = origin_y + row as u16;
                if segment_index == 0 {
                    draw_gutter_number(screen, y, gutter, line_index);
                }
                let (start, end) = segments[segment_index];
                draw_segment(
                    screen,
                    line,
                    line_index,
                    y,
                    gutter as u16,
                    start,
                    end,
                    selection,
                    highlights
                        .get(line_index - self.top_line)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                );
                row += 1;
                segment_index += 1;
            }
            line_index += 1;
            segment_index = 0;
        }
    }

    fn ensure_cursor_visible(&mut self, editor: &EditorCore, rows: usize, cols: usize) {
        if rows == 0 || cols == 0 {
            return;
        }
        if editor.cursor.line < self.top_line {
            self.top_line = editor.cursor.line;
        } else if editor.cursor.line >= self.top_line + rows {
            self.top_line = editor.cursor.line + 1 - rows;
        }

        let display_col = display_col_for_grapheme(
            editor.buffer.line(editor.cursor.line).unwrap_or_default(),
            editor.cursor.grapheme,
        );
        if display_col < self.left_col {
            self.left_col = display_col;
        } else if display_col >= self.left_col + cols {
            self.left_col = display_col + 1 - cols;
        }
    }

    /// Wrap-mode viewport follow, in visual-row units: the cursor's visual
    /// row must land inside `[top, top + rows)` where `top` is the
    /// `(top_line, top_segment)` pair.
    fn ensure_cursor_visible_wrapped(&mut self, editor: &EditorCore, rows: usize, cols: usize) {
        if rows == 0 || cols == 0 {
            return;
        }
        let cursor_line = editor.cursor.line;
        let line = editor.buffer.line(cursor_line).unwrap_or_default();
        let (cursor_segment, _) = cursor_visual_position(line, editor.cursor.grapheme, cols);

        if (cursor_line, cursor_segment) < (self.top_line, self.top_segment) {
            self.top_line = cursor_line;
            self.top_segment = cursor_segment;
            return;
        }

        // Count visual rows from the current top toward the cursor; the walk
        // is capped at `rows`, so a far-away cursor costs O(rows) not O(file).
        let mut distance = 0usize;
        let mut line_index = self.top_line;
        let mut segment_index = self.top_segment;
        while (line_index, segment_index) < (cursor_line, cursor_segment) && distance < rows {
            let segment_count =
                wrap_segments(editor.buffer.line(line_index).unwrap_or_default(), cols).len();
            segment_index += 1;
            if segment_index >= segment_count {
                line_index += 1;
                segment_index = 0;
            }
            distance += 1;
        }
        if distance < rows {
            return; // already visible
        }

        // Cursor is below the viewport: walk *backward* from the cursor by
        // rows - 1 visual rows so the cursor ends up on the last visible row.
        let mut remaining = rows - 1;
        let mut line_index = cursor_line;
        let mut segment_index = cursor_segment;
        while remaining > 0 {
            if segment_index > 0 {
                let step = segment_index.min(remaining);
                segment_index -= step;
                remaining -= step;
            } else if line_index == 0 {
                break;
            } else {
                line_index -= 1;
                segment_index =
                    wrap_segments(editor.buffer.line(line_index).unwrap_or_default(), cols).len()
                        - 1;
                remaining -= 1;
            }
        }
        self.top_line = line_index;
        self.top_segment = segment_index;
    }

    fn cursor_screen_position(&self, editor: &EditorCore) -> Option<(u16, u16)> {
        let line = editor.buffer.line(editor.cursor.line)?;
        let col = display_col_for_grapheme(line, editor.cursor.grapheme);
        Some((
            (Self::gutter_width(editor) + col.saturating_sub(self.left_col)) as u16,
            editor.cursor.line.saturating_sub(self.top_line) as u16,
        ))
    }

    fn cursor_screen_position_wrapped(
        &self,
        editor: &EditorCore,
        cols: usize,
    ) -> Option<(u16, u16)> {
        let line = editor.buffer.line(editor.cursor.line)?;
        let (cursor_segment, x) = cursor_visual_position(line, editor.cursor.grapheme, cols);
        if (editor.cursor.line, cursor_segment) < (self.top_line, self.top_segment) {
            return None;
        }

        let mut row = 0usize;
        let mut line_index = self.top_line;
        let mut segment_index = self.top_segment;
        while (line_index, segment_index) < (editor.cursor.line, cursor_segment) {
            let segment_count =
                wrap_segments(editor.buffer.line(line_index).unwrap_or_default(), cols).len();
            segment_index += 1;
            if segment_index >= segment_count {
                line_index += 1;
                segment_index = 0;
            }
            row += 1;
        }
        Some((
            (Self::gutter_width(editor) + x) as u16,
            row.try_into().ok()?,
        ))
    }
}

/// Display width of one grapheme in the editor area. Tabs render as a fixed
/// 4-cell block; zero-width graphemes still occupy one cell so the cursor can
/// sit on them.
fn grapheme_display_width(grapheme: &str) -> usize {
    if grapheme == "\t" {
        4
    } else {
        UnicodeWidthStr::width(grapheme).max(1)
    }
}

/// Splits a logical line into wrap segments of at most `cols` display width.
/// Returns grapheme-index ranges `(start, end)`; every line yields at least
/// one segment (an empty line yields `(0, 0)`) so each line owns a visual row.
/// A single grapheme wider than `cols` gets its own (overflowing) segment
/// rather than being dropped.
fn wrap_segments(line: &str, cols: usize) -> Vec<(usize, usize)> {
    let cols = cols.max(1);
    let mut segments = Vec::new();
    let mut start = 0;
    let mut width = 0;
    let mut grapheme_count = 0;
    for (index, grapheme) in line.graphemes(true).enumerate() {
        let grapheme_width = grapheme_display_width(grapheme);
        if width + grapheme_width > cols && index > start {
            segments.push((start, index));
            start = index;
            width = 0;
        }
        width += grapheme_width;
        grapheme_count = index + 1;
    }
    segments.push((start, grapheme_count));
    segments
}

/// Cursor position in wrap mode: which segment the cursor grapheme falls in,
/// and its x offset (display cells) within that segment. A cursor sitting at
/// the very end of the line belongs to the last segment.
fn cursor_visual_position(line: &str, cursor_grapheme: usize, cols: usize) -> (usize, usize) {
    let segments = wrap_segments(line, cols);
    let segment_index = segments
        .iter()
        .position(|&(start, end)| cursor_grapheme >= start && cursor_grapheme < end)
        .unwrap_or(segments.len() - 1);
    let (start, _) = segments[segment_index];
    let x = line
        .graphemes(true)
        .skip(start)
        .take(cursor_grapheme.saturating_sub(start))
        .map(grapheme_display_width)
        .sum();
    (segment_index, x)
}

fn draw_gutter_number(screen: &mut Screen, y: u16, gutter: usize, line_index: usize) {
    screen.put_str(
        0,
        y,
        &format!("{:>width$} ", line_index + 1, width = gutter - 1),
        Style {
            reverse: false,
            dim: true,
            fg: None,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_line(
    screen: &mut Screen,
    line: &str,
    line_index: usize,
    row: u16,
    origin_x: u16,
    left_col: usize,
    selection: Option<(Position, Position)>,
    highlights: &[HighlightSpan],
) {
    let mut display_col = 0;
    for (grapheme_index, grapheme) in line.graphemes(true).enumerate() {
        let expanded = if grapheme == "\t" { "    " } else { grapheme };
        let width = UnicodeWidthStr::width(expanded).max(1);
        let next_col = display_col + width;
        if next_col > left_col {
            let style =
                style_for_grapheme(selection, highlights, line_index, grapheme_index, grapheme);
            let x = origin_x + display_col.saturating_sub(left_col) as u16;
            screen.put_str(x, row, expanded, style);
        }
        display_col = next_col;
        if origin_x as usize + display_col.saturating_sub(left_col) >= usize::from(screen.width()) {
            break;
        }
    }
}

/// Draws one wrap segment (`start..end` graphemes of `line`) at the gutter
/// edge. Selection and highlight styling use the grapheme's *logical* index
/// within the line, so both stay correct across wrapped rows.
#[allow(clippy::too_many_arguments)]
fn draw_segment(
    screen: &mut Screen,
    line: &str,
    line_index: usize,
    row: u16,
    origin_x: u16,
    start: usize,
    end: usize,
    selection: Option<(Position, Position)>,
    highlights: &[HighlightSpan],
) {
    let mut x = usize::from(origin_x);
    for (grapheme_index, grapheme) in line
        .graphemes(true)
        .enumerate()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let expanded = if grapheme == "\t" { "    " } else { grapheme };
        let style = style_for_grapheme(selection, highlights, line_index, grapheme_index, grapheme);
        screen.put_str(x as u16, row, expanded, style);
        x += grapheme_display_width(grapheme);
        if x >= usize::from(screen.width()) {
            break;
        }
    }
}

/// Wrap-off truncation markers: `…` on the right edge when the line continues
/// past the viewport, and on the left text edge while horizontally scrolled.
/// Drawn only when content is actually cut so an exactly-fitting line stays
/// marker-free.
fn draw_truncation_markers(
    screen: &mut Screen,
    line: &str,
    y: u16,
    gutter: usize,
    left_col: usize,
    editor_cols: usize,
) {
    let marker_style = Style {
        reverse: false,
        dim: true,
        fg: None,
    };
    let line_width: usize = line.graphemes(true).map(grapheme_display_width).sum();
    if line_width > left_col + editor_cols && screen.width() > 0 {
        screen.put_str(screen.width() - 1, y, TRUNCATION_MARKER, marker_style);
    }
    if left_col > 0 && line_width > 0 {
        screen.put_str(gutter as u16, y, TRUNCATION_MARKER, marker_style);
    }
}

fn style_for_grapheme(
    selection: Option<(Position, Position)>,
    highlights: &[HighlightSpan],
    line_index: usize,
    grapheme_index: usize,
    grapheme: &str,
) -> Style {
    if is_selected(selection, line_index, grapheme_index) {
        Style {
            reverse: true,
            dim: false,
            fg: None,
        }
    } else if grapheme == "\t" {
        Style::default()
    } else {
        Style {
            reverse: false,
            dim: false,
            fg: color_for_grapheme(highlights, grapheme_index),
        }
    }
}

fn is_selected(selection: Option<(Position, Position)>, line: usize, grapheme: usize) -> bool {
    let Some((start, end)) = selection else {
        return false;
    };
    Position::new(line, grapheme) >= start && Position::new(line, grapheme) < end
}

fn display_col_for_grapheme(line: &str, target: usize) -> usize {
    line.graphemes(true)
        .take(target)
        .map(grapheme_display_width)
        .sum()
}

fn color_for_grapheme(highlights: &[HighlightSpan], grapheme_index: usize) -> Option<(u8, u8, u8)> {
    highlights
        .iter()
        .find(|(range, _)| range.contains(&grapheme_index))
        .map(|(_, rgb)| *rgb)
}

#[cfg(test)]
mod tests {
    use super::{EditorView, StatusLine, cursor_visual_position, wrap_segments};
    use crate::{
        core::{buffer::TextBuffer, editor::EditorCore, position::Position},
        ui::Screen,
    };

    /// TASK-260711-18 testcase: wrap segmentation over ASCII, fullwidth,
    /// emoji, and tab graphemes, table-driven.
    #[test]
    fn wrap_segments_split_by_display_width() {
        // (line, cols, expected grapheme ranges)
        type Case = (&'static str, usize, &'static [(usize, usize)]);
        let cases: &[Case] = &[
            // empty line still owns one visual row
            ("", 4, &[(0, 0)]),
            // exact fit produces no extra segment
            ("abcd", 4, &[(0, 4)]),
            ("abcde", 4, &[(0, 4), (4, 5)]),
            ("abcdefghij", 4, &[(0, 4), (4, 8), (8, 10)]),
            // fullwidth: 2 cells each, so only 2 fit into 4 cells
            ("あいうえお", 4, &[(0, 2), (2, 4), (4, 5)]),
            // mixed: "aあ" = 3 cells, adding い (2) would overflow 4
            ("aあいb", 4, &[(0, 2), (2, 4)]),
            // emoji count as their display width (2 cells)
            ("🙂🙂🙂", 4, &[(0, 2), (2, 3)]),
            // tab expands to 4 cells and forces the next grapheme over
            ("\tab", 4, &[(0, 1), (1, 3)]),
            // single grapheme wider than cols still gets a segment
            ("あ", 1, &[(0, 1)]),
        ];
        for (line, cols, expected) in cases {
            assert_eq!(
                wrap_segments(line, *cols),
                *expected,
                "line={line:?} cols={cols}"
            );
        }
    }

    #[test]
    fn cursor_visual_position_maps_grapheme_to_segment_and_x() {
        // "abcdefghij" wrapped at 4: segments (0,4) (4,8) (8,10)
        let cases: &[(usize, (usize, usize))] = &[
            (0, (0, 0)),
            (3, (0, 3)),
            (4, (1, 0)), // boundary grapheme starts the next visual row
            (7, (1, 3)),
            (8, (2, 0)),
            (10, (2, 2)), // end of line sticks to the last segment
        ];
        for (grapheme, expected) in cases {
            assert_eq!(
                cursor_visual_position("abcdefghij", *grapheme, 4),
                *expected,
                "grapheme={grapheme}"
            );
        }
    }

    fn view_with(text: &str, cursor: Position) -> (EditorCore, EditorView) {
        let (buffer, _) = TextBuffer::from_bytes(text.as_bytes()).unwrap();
        let mut editor = EditorCore::new(buffer);
        editor.cursor = cursor;
        (editor, EditorView::default())
    }

    fn draw(editor: &EditorCore, view: &mut EditorView, screen: &mut Screen, wrap: bool) {
        view.draw(
            editor,
            screen,
            &[],
            StatusLine {
                filename: "test",
                modified: false,
                message: "",
                pending: "",
            },
            0,
            wrap,
        );
    }

    fn row_text(screen: &Screen, y: u16) -> String {
        screen
            .row(y)
            .unwrap()
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// TASK-260711-18 testcase: wrap on folds a long line onto continuation
    /// rows (blank gutter), and the cursor lands on the right visual row.
    #[test]
    fn wrap_on_draws_continuation_rows_and_positions_cursor() {
        // screen 10 wide, gutter "  1 " = 4 cells -> 6 text cells per row
        let (editor, mut view) = view_with("abcdefghij\n", Position::new(0, 8));
        let mut screen = Screen::new(10, 4);

        draw(&editor, &mut view, &mut screen, true);

        assert_eq!(row_text(&screen, 0), "  1 abcdef");
        assert_eq!(row_text(&screen, 1), "    ghij");
        // cursor grapheme 8 -> segment 1, x=2 -> screen (gutter 4 + 2, row 1)
        assert_eq!(screen.cursor, Some((6, 1)));
    }

    /// TASK-260711-18 testcase: wrap on follows the cursor by visual rows —
    /// a cursor deep inside one long logical line scrolls `top_segment`.
    #[test]
    fn wrap_on_scrolls_by_visual_rows_within_one_logical_line() {
        // 6 text cells per row; 30 chars -> 5 segments; viewport shows 3 rows
        let (editor, mut view) =
            view_with(&format!("{}\n", "abcdef".repeat(5)), Position::new(0, 29));
        let mut screen = Screen::new(10, 4);

        draw(&editor, &mut view, &mut screen, true);

        // cursor segment = 4; top must have advanced to segment 2 of line 0
        assert_eq!(view.top_line, 0);
        assert_eq!(view.top_segment, 2);
        assert_eq!(view.left_col, 0, "wrap mode never scrolls horizontally");
        assert_eq!(screen.cursor, Some((4 + 5, 2)));
    }

    /// TASK-260711-18 testcase: wrap off keeps horizontal scrolling and the
    /// cursor math unchanged.
    #[test]
    fn wrap_off_scrolls_horizontally_to_follow_cursor() {
        let (editor, mut view) = view_with("abcdefghijklmno\n", Position::new(0, 14));
        let mut screen = Screen::new(10, 3);

        draw(&editor, &mut view, &mut screen, false);

        assert!(view.left_col > 0, "cursor past viewport must scroll");
        assert_eq!(view.top_segment, 0, "wrap-off never uses top_segment");
    }

    /// TASK-260711-18 testcase: the truncation marker appears only when the
    /// line is actually cut at that edge.
    #[test]
    fn truncation_marker_only_when_line_is_cut() {
        // 6 text cells per row (10 - gutter 4)
        let (editor, mut view) = view_with("abcdefgh\nxy\n", Position::new(1, 0));
        let mut screen = Screen::new(10, 4);

        draw(&editor, &mut view, &mut screen, false);

        let long_row = row_text(&screen, 0);
        let short_row = row_text(&screen, 1);
        assert!(
            long_row.ends_with("…"),
            "cut line must end with the marker: {long_row:?}"
        );
        assert!(
            !short_row.contains("…"),
            "fitting line must not show a marker: {short_row:?}"
        );
    }

    /// TASK-260711-18 testcase: while horizontally scrolled, the left edge
    /// shows a marker over the first text column.
    #[test]
    fn truncation_marker_on_left_edge_while_scrolled() {
        let (editor, mut view) = view_with("abcdefghijklmno\n", Position::new(0, 14));
        let mut screen = Screen::new(10, 3);

        draw(&editor, &mut view, &mut screen, false);

        let row = row_text(&screen, 0);
        assert!(view.left_col > 0);
        // gutter is 4 cells; the first text cell shows the left marker
        assert_eq!(screen.cell(4, 0).unwrap().symbol, "…");
        assert!(row.starts_with("  1 …"), "{row:?}");
    }
}
