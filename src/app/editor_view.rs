//! Editor buffer viewport renderer.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    core::{editor::EditorCore, position::Position},
    highlight::HighlightSpan,
    ui::{Screen, Style},
};

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct EditorView {
    pub top_line: usize,
    pub left_col: usize,
}

pub struct StatusLine<'a> {
    pub filename: &'a str,
    pub modified: bool,
    pub message: &'a str,
    pub pending: &'a str,
}

impl EditorView {
    pub fn draw(
        &mut self,
        editor: &EditorCore,
        screen: &mut Screen,
        highlights: &[Vec<HighlightSpan>],
        status: StatusLine<'_>,
    ) {
        let editor_rows = screen.height().saturating_sub(1) as usize;
        let editor_cols = screen.width() as usize;
        self.ensure_cursor_visible(editor, editor_rows, editor_cols);

        for row in 0..editor_rows {
            let line_index = self.top_line + row;
            let Some(line) = editor.buffer.line(line_index) else {
                continue;
            };
            draw_line(
                screen,
                line,
                line_index,
                row as u16,
                self.left_col,
                editor.selection.map(|selection| selection.range()),
                highlights.get(row).map(Vec::as_slice).unwrap_or(&[]),
            );
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

        if let Some((x, y)) = self.cursor_screen_position(editor)
            && x < screen.width()
            && y < screen.height().saturating_sub(1)
        {
            screen.set_cursor(x, y);
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

    fn cursor_screen_position(&self, editor: &EditorCore) -> Option<(u16, u16)> {
        let line = editor.buffer.line(editor.cursor.line)?;
        let col = display_col_for_grapheme(line, editor.cursor.grapheme);
        Some((
            col.saturating_sub(self.left_col) as u16,
            editor.cursor.line.saturating_sub(self.top_line) as u16,
        ))
    }
}

fn draw_line(
    screen: &mut Screen,
    line: &str,
    line_index: usize,
    row: u16,
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
            let style = if is_selected(selection, line_index, grapheme_index) {
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
            };
            let x = display_col.saturating_sub(left_col) as u16;
            screen.put_str(x, row, expanded, style);
        }
        display_col = next_col;
        if display_col.saturating_sub(left_col) >= usize::from(screen.width()) {
            break;
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
    line.graphemes(true).take(target).fold(0, |col, grapheme| {
        col + if grapheme == "\t" {
            4
        } else {
            UnicodeWidthStr::width(grapheme).max(1)
        }
    })
}

fn color_for_grapheme(highlights: &[HighlightSpan], grapheme_index: usize) -> Option<(u8, u8, u8)> {
    highlights
        .iter()
        .find(|(range, _)| range.contains(&grapheme_index))
        .map(|(_, rgb)| *rgb)
}
