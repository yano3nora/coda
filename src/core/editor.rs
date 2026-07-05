//! High-level pure editor facade over buffer, cursor, selection, and undo.

use super::{
    buffer::TextBuffer,
    movement,
    position::Position,
    selection::Selection,
    undo::{EditGroup, EditKind, EditOp, MergeInfo, UndoStack},
};

/// Cursor movement commands exposed by [`EditorCore::move_cursor`].
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    BufferStart,
    BufferEnd,
    PageUp { rows: usize },
    PageDown { rows: usize },
    WordLeft,
    WordRight,
}

/// Pure editor state facade for future app/keymap layers.
#[derive(Debug, Clone)]
pub struct EditorCore {
    pub buffer: TextBuffer,
    pub cursor: Position,
    pub preferred_grapheme: usize,
    pub selection: Option<Selection>,
    pub undo: UndoStack,
}

impl Default for EditorCore {
    fn default() -> Self {
        Self::new(TextBuffer::default())
    }
}

impl EditorCore {
    /// Creates an editor around an existing buffer.
    pub fn new(buffer: TextBuffer) -> Self {
        Self {
            buffer,
            cursor: Position::new(0, 0),
            preferred_grapheme: 0,
            selection: None,
            undo: UndoStack::default(),
        }
    }

    /// Moves the cursor. With `extend`, keeps or creates a selection anchor.
    pub fn move_cursor(&mut self, motion: Motion, extend: bool) {
        self.cursor = self.buffer.clamp_position(self.cursor);

        if !extend && let Some(selection) = self.selection.take() {
            match motion {
                Motion::Left => {
                    self.set_cursor(selection.range().0, true);
                    return;
                }
                Motion::Right => {
                    self.set_cursor(selection.range().1, true);
                    return;
                }
                _ => {}
            }
        }

        let anchor = extend.then(|| {
            self.selection
                .map_or(self.cursor, |selection| selection.anchor)
        });
        let next = self.motion_target(motion);
        self.set_cursor(next, is_horizontal(motion));

        if let Some(anchor) = anchor {
            self.selection = Some(Selection::new(anchor, self.cursor));
        } else {
            self.selection = None;
        }
    }

    /// Inserts text, replacing the active selection atomically when present.
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        if let Some((start, end)) = self.take_selection_range() {
            let before = self.cursor;
            let deleted = self.buffer.delete_range(start, end);
            let after = self.buffer.insert(start, text);
            self.cursor = after;
            self.update_preferred();
            self.record_group(
                vec![
                    EditOp::Delete { start, end },
                    EditOp::Insert {
                        pos: start,
                        text: text.to_string(),
                    },
                ],
                vec![
                    EditOp::Insert {
                        pos: start,
                        text: deleted,
                    },
                    EditOp::Delete { start, end: after },
                ],
                before,
                after,
                EditKind::Other,
                start,
                after,
                true,
            );
            return;
        }

        let before = self.cursor;
        let after = self.buffer.insert(before, text);
        self.cursor = after;
        self.update_preferred();
        self.record_group(
            vec![EditOp::Insert {
                pos: before,
                text: text.to_string(),
            }],
            vec![EditOp::Delete {
                start: before,
                end: after,
            }],
            before,
            after,
            EditKind::Insert,
            before,
            after,
            text.contains('\n'),
        );
    }

    /// Deletes the selection, previous grapheme, or joins with the previous line.
    pub fn backspace(&mut self) {
        if self.delete_selection_as_group() {
            return;
        }
        let start = movement::left(&self.buffer, self.cursor);
        if start == self.cursor {
            return;
        }
        self.delete_range_recorded(start, self.cursor, start, EditKind::Backspace);
    }

    /// Deletes the selection, next grapheme, or joins with the next line.
    pub fn delete_forward(&mut self) {
        if self.delete_selection_as_group() {
            return;
        }
        let end = movement::right(&self.buffer, self.cursor);
        if end == self.cursor {
            return;
        }
        self.delete_range_recorded(self.cursor, end, self.cursor, EditKind::Other);
    }

    /// Deletes from the previous word boundary to the cursor.
    pub fn delete_word_left(&mut self) {
        if self.delete_selection_as_group() {
            return;
        }
        let start = movement::word_left(&self.buffer, self.cursor);
        if start != self.cursor {
            self.delete_range_recorded(start, self.cursor, start, EditKind::Other);
        }
    }

    /// Deletes from the cursor to the next word boundary.
    pub fn delete_word_right(&mut self) {
        if self.delete_selection_as_group() {
            return;
        }
        let end = movement::word_right(&self.buffer, self.cursor);
        if end != self.cursor {
            self.delete_range_recorded(self.cursor, end, self.cursor, EditKind::Other);
        }
    }

    /// Deletes the current line. A single-line buffer keeps one empty line;
    /// deleting the last line of a multi-line buffer removes the line together
    /// with its preceding newline (VS Code behavior).
    pub fn delete_line(&mut self) {
        self.selection = None;
        let line = self
            .cursor
            .line
            .min(self.buffer.line_count().saturating_sub(1));
        let (start, end, cursor_after) = if self.buffer.line_count() == 1 {
            let start = Position::new(line, 0);
            (
                start,
                Position::new(line, self.buffer.grapheme_count(line)),
                start,
            )
        } else if line + 1 < self.buffer.line_count() {
            let start = Position::new(line, 0);
            (start, Position::new(line + 1, 0), start)
        } else {
            (
                Position::new(line - 1, self.buffer.grapheme_count(line - 1)),
                Position::new(line, self.buffer.grapheme_count(line)),
                Position::new(line - 1, 0),
            )
        };
        self.delete_range_recorded(start, end, cursor_after, EditKind::Other);
    }

    /// Selects the full buffer.
    pub fn select_all(&mut self) {
        let start = Position::new(0, 0);
        let end = movement::buffer_end(&self.buffer, self.cursor);
        self.cursor = end;
        self.preferred_grapheme = end.grapheme;
        self.selection = Some(Selection::new(start, end));
    }

    /// Forces a boundary before the next undoable edit.
    pub fn commit_group(&mut self) {
        self.undo.commit_group();
    }

    /// Undoes one group and restores the group's starting cursor.
    pub fn undo(&mut self) -> bool {
        let Some(cursor) = self.undo.undo(&mut self.buffer) else {
            return false;
        };
        self.selection = None;
        self.set_cursor(cursor, true);
        true
    }

    /// Redoes one group and restores the group's ending cursor.
    pub fn redo(&mut self) -> bool {
        let Some(cursor) = self.undo.redo(&mut self.buffer) else {
            return false;
        };
        self.selection = None;
        self.set_cursor(cursor, true);
        true
    }

    fn motion_target(&self, motion: Motion) -> Position {
        match motion {
            Motion::Left => movement::left(&self.buffer, self.cursor),
            Motion::Right => movement::right(&self.buffer, self.cursor),
            Motion::Up => movement::up(&self.buffer, self.cursor, self.preferred_grapheme),
            Motion::Down => movement::down(&self.buffer, self.cursor, self.preferred_grapheme),
            Motion::LineStart => movement::line_start(&self.buffer, self.cursor),
            Motion::LineEnd => movement::line_end(&self.buffer, self.cursor),
            Motion::BufferStart => movement::buffer_start(&self.buffer, self.cursor),
            Motion::BufferEnd => movement::buffer_end(&self.buffer, self.cursor),
            Motion::PageUp { rows } => {
                movement::page_up(&self.buffer, self.cursor, self.preferred_grapheme, rows)
            }
            Motion::PageDown { rows } => {
                movement::page_down(&self.buffer, self.cursor, self.preferred_grapheme, rows)
            }
            Motion::WordLeft => movement::word_left(&self.buffer, self.cursor),
            Motion::WordRight => movement::word_right(&self.buffer, self.cursor),
        }
    }

    fn delete_selection_as_group(&mut self) -> bool {
        if let Some((start, end)) = self.take_selection_range() {
            self.delete_range_recorded(start, end, start, EditKind::Other);
            true
        } else {
            false
        }
    }

    fn take_selection_range(&mut self) -> Option<(Position, Position)> {
        let selection = self.selection.take()?;
        (!selection.is_empty()).then_some(selection.range())
    }

    fn delete_range_recorded(
        &mut self,
        start: Position,
        end: Position,
        cursor_after: Position,
        kind: EditKind,
    ) {
        let before = self.cursor;
        let deleted = self.buffer.delete_range(start, end);
        if deleted.is_empty() {
            return;
        }
        self.cursor = self.buffer.clamp_position(cursor_after);
        self.update_preferred();
        self.record_group(
            vec![EditOp::Delete { start, end }],
            vec![EditOp::Insert {
                pos: start,
                text: deleted,
            }],
            before,
            self.cursor,
            kind,
            start,
            end,
            false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_group(
        &mut self,
        forward: Vec<EditOp>,
        inverse: Vec<EditOp>,
        cursor_before: Position,
        cursor_after: Position,
        kind: EditKind,
        merge_start: Position,
        merge_end: Position,
        merge_text_has_newline: bool,
    ) {
        self.undo.record(EditGroup::new(
            forward,
            inverse,
            cursor_before,
            cursor_after,
            MergeInfo {
                kind,
                start: merge_start,
                end: merge_end,
                text_has_newline: merge_text_has_newline,
            },
        ));
    }

    fn set_cursor(&mut self, pos: Position, update_preferred: bool) {
        self.cursor = self.buffer.clamp_position(pos);
        if update_preferred {
            self.update_preferred();
        }
    }

    fn update_preferred(&mut self) {
        self.preferred_grapheme = self.cursor.grapheme;
    }
}

fn is_horizontal(motion: Motion) -> bool {
    !matches!(
        motion,
        Motion::Up | Motion::Down | Motion::PageUp { .. } | Motion::PageDown { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> EditorCore {
        EditorCore::new(TextBuffer::from_bytes(text.as_bytes()).unwrap().0)
    }

    fn text(editor: &EditorCore) -> String {
        String::from_utf8(editor.buffer.to_bytes()).unwrap()
    }

    #[test]
    fn extending_selection_keeps_anchor_and_normalizes_range() {
        let mut cases = [
            (
                "forward",
                Position::new(0, 1),
                Motion::Right,
                (Position::new(0, 1), Position::new(0, 2)),
            ),
            (
                "reverse",
                Position::new(0, 1),
                Motion::Left,
                (Position::new(0, 0), Position::new(0, 1)),
            ),
        ];

        for (name, cursor, motion, expected_range) in &mut cases {
            let mut editor = editor("abc");
            editor.cursor = *cursor;
            editor.move_cursor(*motion, true);
            assert_eq!(editor.selection.unwrap().anchor, *cursor, "case {name}");
            assert_eq!(
                editor.selection.unwrap().range(),
                *expected_range,
                "case {name}"
            );
        }
    }

    #[test]
    fn non_extending_left_right_collapse_selection_to_range_edges() {
        let cases = [
            ("left collapses to start", Motion::Left, Position::new(0, 1)),
            ("right collapses to end", Motion::Right, Position::new(0, 3)),
        ];

        for (name, motion, expected) in cases {
            let mut editor = editor("abcd");
            editor.selection = Some(Selection::new(Position::new(0, 3), Position::new(0, 1)));
            editor.cursor = Position::new(0, 1);
            editor.move_cursor(motion, false);
            assert_eq!(editor.cursor, expected, "case {name}");
            assert!(editor.selection.is_none(), "case {name}");
        }
    }

    #[test]
    fn editing_and_undo_are_table_driven() {
        enum Case {
            ConsecutiveInsertOneUndo,
            NewlineBreaksGroup,
            ConsecutiveBackspaceOneUndo,
            SelectionReplacementAtomic,
            UndoRedoRoundTrip,
            NewEditClearsRedo,
            BackspaceAtLineStartRoundTrip,
            DeleteLineKeepsFinalEmptyLine,
            DeleteLastLineRemovesItEntirely,
            DeleteWordLeft,
        }

        let cases = [
            ("consecutive insert groups", Case::ConsecutiveInsertOneUndo),
            ("newline breaks group", Case::NewlineBreaksGroup),
            (
                "consecutive backspace groups",
                Case::ConsecutiveBackspaceOneUndo,
            ),
            (
                "selection replacement atomic",
                Case::SelectionReplacementAtomic,
            ),
            ("undo redo round trip", Case::UndoRedoRoundTrip),
            ("new edit clears redo", Case::NewEditClearsRedo),
            (
                "backspace line join undo",
                Case::BackspaceAtLineStartRoundTrip,
            ),
            (
                "delete_line final empty",
                Case::DeleteLineKeepsFinalEmptyLine,
            ),
            (
                "delete_line last line removed",
                Case::DeleteLastLineRemovesItEntirely,
            ),
            ("delete_word_left foo bar", Case::DeleteWordLeft),
        ];

        for (name, case) in cases {
            match case {
                Case::ConsecutiveInsertOneUndo => {
                    let mut editor = editor("");
                    editor.insert_text("a");
                    editor.insert_text("b");
                    editor.insert_text("c");
                    assert_eq!(text(&editor), "abc", "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "", "case {name}");
                    assert_eq!(editor.cursor, Position::new(0, 0), "case {name}");
                }
                Case::NewlineBreaksGroup => {
                    let mut editor = editor("");
                    editor.insert_text("a");
                    editor.insert_text("\n");
                    editor.insert_text("b");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "a\n", "case {name}");
                }
                Case::ConsecutiveBackspaceOneUndo => {
                    let mut editor = editor("abc");
                    editor.cursor = Position::new(0, 3);
                    editor.backspace();
                    editor.backspace();
                    editor.backspace();
                    assert_eq!(text(&editor), "", "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "abc", "case {name}");
                    assert_eq!(editor.cursor, Position::new(0, 3), "case {name}");
                }
                Case::SelectionReplacementAtomic => {
                    let mut editor = editor("abc");
                    editor.selection =
                        Some(Selection::new(Position::new(0, 1), Position::new(0, 2)));
                    editor.cursor = Position::new(0, 2);
                    editor.insert_text("X");
                    assert_eq!(text(&editor), "aXc", "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "abc", "case {name}");
                    assert_eq!(editor.cursor, Position::new(0, 2), "case {name}");
                }
                Case::UndoRedoRoundTrip => {
                    let mut editor = editor("a");
                    editor.cursor = Position::new(0, 1);
                    editor.insert_text("b");
                    let after = (text(&editor), editor.cursor);
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(
                        (text(&editor), editor.cursor),
                        ("a".to_string(), Position::new(0, 1)),
                        "case {name}"
                    );
                    assert!(editor.redo(), "case {name}");
                    assert_eq!((text(&editor), editor.cursor), after, "case {name}");
                }
                Case::NewEditClearsRedo => {
                    let mut editor = editor("");
                    editor.insert_text("a");
                    assert!(editor.undo(), "case {name}");
                    editor.insert_text("b");
                    assert!(!editor.redo(), "case {name}");
                    assert_eq!(text(&editor), "b", "case {name}");
                }
                Case::BackspaceAtLineStartRoundTrip => {
                    let mut editor = editor("a\nb");
                    editor.cursor = Position::new(1, 0);
                    editor.backspace();
                    assert_eq!(text(&editor), "ab", "case {name}");
                    assert_eq!(editor.cursor, Position::new(0, 1), "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "a\nb", "case {name}");
                    assert_eq!(editor.cursor, Position::new(1, 0), "case {name}");
                }
                Case::DeleteLineKeepsFinalEmptyLine => {
                    let mut editor = editor("abc");
                    editor.cursor = Position::new(0, 1);
                    editor.delete_line();
                    assert_eq!(editor.buffer.line_count(), 1, "case {name}");
                    assert_eq!(text(&editor), "", "case {name}");
                }
                Case::DeleteLastLineRemovesItEntirely => {
                    let mut editor = editor("a\nb\nc");
                    editor.cursor = Position::new(2, 1);
                    editor.delete_line();
                    assert_eq!(text(&editor), "a\nb", "case {name}");
                    assert_eq!(editor.cursor, Position::new(1, 0), "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "a\nb\nc", "case {name}");
                }
                Case::DeleteWordLeft => {
                    let mut editor = editor("foo bar");
                    editor.cursor = Position::new(0, 7);
                    editor.delete_word_left();
                    assert_eq!(text(&editor), "foo ", "case {name}");
                    assert_eq!(editor.cursor, Position::new(0, 4), "case {name}");
                }
            }
        }
    }
}
