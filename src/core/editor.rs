//! High-level pure editor facade over buffer, cursor, selection, and undo.

use super::{
    buffer::TextBuffer,
    movement,
    position::Position,
    selection::Selection,
    undo::{EditGroup, EditKind, EditOp, MergeInfo, UndoStack},
};

/// Fixed indent width for `indent`/`outdent` (TASK-260711-19 scope decision:
/// no per-buffer/tab-vs-space config yet — that is future work).
const INDENT_WIDTH: isize = 4;
const INDENT_STR: &str = "    ";

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

    /// Returns the selected text, or the current logical line plus LF when no
    /// selection is active (VS Code-style line copy).
    pub fn copy_text(&self) -> Option<String> {
        if self.buffer.line_count() == 0 {
            return None;
        }
        if let Some(selection) = self.selection
            && !selection.is_empty()
        {
            let (start, end) = selection.range();
            return Some(self.buffer.text_range(start, end));
        }

        let line = self
            .cursor
            .line
            .min(self.buffer.line_count().saturating_sub(1));
        Some(format!(
            "{}\n",
            self.buffer.line(line).expect("line index is clamped")
        ))
    }

    /// Cuts the selected text, or the current logical line when no selection is
    /// active. The deletion is recorded as one undo group.
    pub fn cut(&mut self) -> Option<String> {
        let copied = self.copy_text()?;
        if self
            .selection
            .is_some_and(|selection| !selection.is_empty())
        {
            self.delete_selection_as_group();
        } else {
            self.delete_line();
        }
        Some(copied)
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

    /// Deletes from the current cursor position back to the start of the line.
    pub fn delete_to_line_start(&mut self) {
        if self.delete_selection_as_group() {
            return;
        }
        let start = movement::line_start(&self.buffer, self.cursor);
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

    /// Moves the cursor line or selected line block up by one logical line.
    pub fn move_lines_up(&mut self) {
        let Some((start_line, end_line)) = self.selected_line_block() else {
            return;
        };
        if start_line == 0 {
            return;
        }

        let before_cursor = self.cursor;
        let before_selection = self.selection;
        let old = self.lines_inclusive(start_line - 1, end_line);
        let mut new = self.lines_inclusive(start_line, end_line);
        new.push(old[0].clone());
        self.buffer
            .replace_lines(start_line - 1, end_line + 1, new.clone());

        self.cursor = shift_line(self.cursor, -1);
        self.selection = self.selection.map(|selection| {
            Selection::new(
                shift_line(selection.anchor, -1),
                shift_line(selection.head, -1),
            )
        });
        self.update_preferred();
        self.record_line_replace_group(LineReplaceUndo {
            start: start_line - 1,
            old,
            new,
            cursor_before: before_cursor,
            selection_before: before_selection,
            cursor_after: self.cursor,
            selection_after: self.selection,
        });
    }

    /// Moves the cursor line or selected line block down by one logical line.
    pub fn move_lines_down(&mut self) {
        let Some((start_line, end_line)) = self.selected_line_block() else {
            return;
        };
        if end_line + 1 >= self.buffer.line_count() {
            return;
        }

        let before_cursor = self.cursor;
        let before_selection = self.selection;
        let old = self.lines_inclusive(start_line, end_line + 1);
        let mut new = Vec::with_capacity(old.len());
        new.push(old.last().expect("range has following line").clone());
        new.extend(old[..old.len() - 1].iter().cloned());
        self.buffer
            .replace_lines(start_line, end_line + 2, new.clone());

        self.cursor = shift_line(self.cursor, 1);
        self.selection = self.selection.map(|selection| {
            Selection::new(
                shift_line(selection.anchor, 1),
                shift_line(selection.head, 1),
            )
        });
        self.update_preferred();
        self.record_line_replace_group(LineReplaceUndo {
            start: start_line,
            old,
            new,
            cursor_before: before_cursor,
            selection_before: before_selection,
            cursor_after: self.cursor,
            selection_after: self.selection,
        });
    }

    /// Indents the current line, or every line touched by the selection, by
    /// [`INDENT_WIDTH`] spaces. Mirrors `move_lines_up`/`move_lines_down`:
    /// the whole block is one undo group (TASK-260711-19).
    pub fn indent(&mut self) {
        self.apply_line_indent(|line| (format!("{INDENT_STR}{line}"), INDENT_WIDTH));
    }

    /// Outdents (removes one indent level from) the current line, or every
    /// line touched by the selection. Each line loses a single leading tab,
    /// or up to [`INDENT_WIDTH`] leading spaces — whichever is actually
    /// present — so a block with mixed tab/space indentation degrades one
    /// level per line instead of requiring uniform indentation
    /// (TASK-260711-19).
    pub fn outdent(&mut self) {
        self.apply_line_indent(|line| {
            if let Some(rest) = line.strip_prefix('\t') {
                (rest.to_string(), -1)
            } else {
                let removable = line
                    .chars()
                    .take(INDENT_WIDTH as usize)
                    .take_while(|character| *character == ' ')
                    .count();
                (
                    line.chars().skip(removable).collect(),
                    -(removable as isize),
                )
            }
        });
    }

    /// Shared engine for `indent`/`outdent`: rewrites every line in the
    /// current line block (selection, or just the cursor's line when there
    /// is none) via `transform`, which returns each line's new content and
    /// the signed grapheme delta that content change applies at column 0 of
    /// that same line. The whole block change is recorded as a single
    /// `ReplaceLines` undo group, identical in shape to
    /// `record_line_replace_group` used by the line-move commands, so indent
    /// and outdent get the same "one keypress, one undo" behavior for free.
    fn apply_line_indent(&mut self, transform: impl Fn(&str) -> (String, isize)) {
        let Some((start_line, end_line)) = self.selected_line_block() else {
            return;
        };

        let old = self.lines_inclusive(start_line, end_line);
        let mut new = Vec::with_capacity(old.len());
        let mut deltas = Vec::with_capacity(old.len());
        for line in &old {
            let (new_line, delta) = transform(line);
            new.push(new_line);
            deltas.push(delta);
        }
        if new == old {
            // Nothing changed (e.g. outdenting already-flush lines) — no-op,
            // not an empty undo group.
            return;
        }

        let before_cursor = self.cursor;
        let before_selection = self.selection;
        self.buffer
            .replace_lines(start_line, end_line + 1, new.clone());

        let shift = |pos: Position| {
            if pos.line < start_line || pos.line > end_line {
                return pos;
            }
            let delta = deltas[pos.line - start_line];
            Position::new(pos.line, pos.grapheme.saturating_add_signed(delta))
        };
        self.cursor = self.buffer.clamp_position(shift(before_cursor));
        self.selection = before_selection.map(|selection| {
            Selection::new(
                self.buffer.clamp_position(shift(selection.anchor)),
                self.buffer.clamp_position(shift(selection.head)),
            )
        });
        self.update_preferred();

        self.record_line_replace_group(LineReplaceUndo {
            start: start_line,
            old,
            new,
            cursor_before: before_cursor,
            selection_before: before_selection,
            cursor_after: self.cursor,
            selection_after: self.selection,
        });
    }

    /// Inserts an empty line after the current line and moves the cursor there.
    pub fn insert_line_after(&mut self) {
        self.selection = None;
        let before = self.cursor;
        let line = self
            .cursor
            .line
            .min(self.buffer.line_count().saturating_sub(1));
        let insert_at = Position::new(line, self.buffer.grapheme_count(line));
        let after_insert = self.buffer.insert(insert_at, "\n");
        self.cursor = Position::new(line + 1, 0);
        self.update_preferred();
        self.record_group(
            vec![EditOp::Insert {
                pos: insert_at,
                text: "\n".to_string(),
            }],
            vec![EditOp::Delete {
                start: insert_at,
                end: after_insert,
            }],
            before,
            self.cursor,
            EditKind::Other,
            insert_at,
            after_insert,
            true,
        );
    }

    /// Inserts an empty line before the current line and moves the cursor there.
    pub fn insert_line_before(&mut self) {
        self.selection = None;
        let before = self.cursor;
        let line = self
            .cursor
            .line
            .min(self.buffer.line_count().saturating_sub(1));
        let insert_at = Position::new(line, 0);
        let after_insert = self.buffer.insert(insert_at, "\n");
        self.cursor = Position::new(line, 0);
        self.update_preferred();
        self.record_group(
            vec![EditOp::Insert {
                pos: insert_at,
                text: "\n".to_string(),
            }],
            vec![EditOp::Delete {
                start: insert_at,
                end: after_insert,
            }],
            before,
            self.cursor,
            EditKind::Other,
            insert_at,
            after_insert,
            true,
        );
    }

    /// Selects the full buffer.
    pub fn select_all(&mut self) {
        let start = Position::new(0, 0);
        let end = movement::buffer_end(&self.buffer, self.cursor);
        self.cursor = end;
        self.preferred_grapheme = end.grapheme;
        self.selection = Some(Selection::new(start, end));
    }

    /// Moves the cursor to a (clamped) buffer position and clears any
    /// selection — the mouse-click primitive (ADR-0008).
    pub fn set_cursor_position(&mut self, position: Position) {
        self.cursor = self.buffer.clamp_position(position);
        self.update_preferred();
        self.selection = None;
    }

    /// Selects a grapheme range and places the cursor at the range end.
    pub fn select_range(&mut self, start: Position, end: Position) {
        let start = self.buffer.clamp_position(start);
        let end = self.buffer.clamp_position(end);
        self.cursor = end;
        self.update_preferred();
        self.selection = Some(Selection::new(start, end));
    }

    /// Replaces one or more ranges as a single undoable edit group.
    ///
    /// Ranges are applied from the end of the buffer to the front so earlier
    /// replacements cannot invalidate later original positions. The undo record
    /// stores whole logical lines, not per-range inverse offsets, because mixed
    /// replacement lengths would otherwise require translating positions in the
    /// post-edit buffer.
    pub fn replace_ranges(&mut self, replacements: &[(Position, Position, &str)]) {
        if replacements.is_empty() {
            return;
        }

        let cursor_before = self.cursor;
        let selection_before = self.selection;
        let old = self.buffer.lines_snapshot();
        let mut ordered = replacements.to_vec();
        ordered.sort_by_key(|(start, end, _)| (*start, *end));

        let mut cursor_after = cursor_before;
        for (start, end, replacement) in ordered.iter().rev() {
            let start = self.buffer.clamp_position(*start);
            let end = self.buffer.clamp_position(*end);
            self.buffer.delete_range(start, end);
            cursor_after = self.buffer.insert(start, replacement);
        }

        let new = self.buffer.lines_snapshot();
        if old == new {
            return;
        }

        self.cursor = self.buffer.clamp_position(cursor_after);
        self.selection = None;
        self.update_preferred();
        let group = EditGroup::new(
            vec![EditOp::ReplaceLines {
                start: 0,
                old: old.clone(),
                new: new.clone(),
            }],
            vec![EditOp::ReplaceLines {
                start: 0,
                old: new,
                new: old,
            }],
            cursor_before,
            self.cursor,
            MergeInfo {
                kind: EditKind::Other,
                start: cursor_before,
                end: self.cursor,
                text_has_newline: true,
            },
        )
        .with_selection(selection_before, None);
        self.undo.record(group);
    }

    /// Forces a boundary before the next undoable edit.
    pub fn commit_group(&mut self) {
        self.undo.commit_group();
    }

    /// Undoes one group and restores the group's starting cursor.
    pub fn undo(&mut self) -> bool {
        let Some((cursor, selection)) = self.undo.undo(&mut self.buffer) else {
            return false;
        };
        self.selection = selection;
        self.set_cursor(cursor, true);
        true
    }

    /// Redoes one group and restores the group's ending cursor.
    pub fn redo(&mut self) -> bool {
        let Some((cursor, selection)) = self.undo.redo(&mut self.buffer) else {
            return false;
        };
        self.selection = selection;
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

    fn selected_line_block(&self) -> Option<(usize, usize)> {
        let line_count = self.buffer.line_count();
        if line_count == 0 {
            return None;
        }
        if let Some(selection) = self.selection
            && !selection.is_empty()
        {
            let (start, end) = selection.range();
            let end_line = if end.grapheme == 0 && end.line > start.line {
                end.line - 1
            } else {
                end.line
            };
            return Some((start.line.min(line_count - 1), end_line.min(line_count - 1)));
        }
        Some((
            self.cursor.line.min(line_count - 1),
            self.cursor.line.min(line_count - 1),
        ))
    }

    fn lines_inclusive(&self, start: usize, end: usize) -> Vec<String> {
        (start..=end)
            .map(|line| {
                self.buffer
                    .line(line)
                    .expect("line block range is valid")
                    .to_string()
            })
            .collect()
    }

    fn record_line_replace_group(&mut self, change: LineReplaceUndo) {
        let group = EditGroup::new(
            vec![EditOp::ReplaceLines {
                start: change.start,
                old: change.old.clone(),
                new: change.new.clone(),
            }],
            vec![EditOp::ReplaceLines {
                start: change.start,
                old: change.new,
                new: change.old,
            }],
            change.cursor_before,
            change.cursor_after,
            MergeInfo {
                kind: EditKind::Other,
                start: change.cursor_before,
                end: change.cursor_after,
                text_has_newline: true,
            },
        )
        .with_selection(change.selection_before, change.selection_after);
        self.undo.record(group);
    }
}

struct LineReplaceUndo {
    start: usize,
    old: Vec<String>,
    new: Vec<String>,
    cursor_before: Position,
    selection_before: Option<Selection>,
    cursor_after: Position,
    selection_after: Option<Selection>,
}

fn is_horizontal(motion: Motion) -> bool {
    !matches!(
        motion,
        Motion::Up | Motion::Down | Motion::PageUp { .. } | Motion::PageDown { .. }
    )
}

fn shift_line(pos: Position, delta: isize) -> Position {
    Position::new(pos.line.saturating_add_signed(delta), pos.grapheme)
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

    #[test]
    fn line_actions_are_table_driven() {
        enum Case {
            MoveDownMiddle,
            MoveUpMiddle,
            MoveSelectionBlockDown,
            MoveAtEdgesNoop,
            MoveAcrossFinalLine,
            InsertLineAfter,
            InsertLineBefore,
            DeleteToLineStart,
            DeleteToLineStartAtStartNoop,
        }

        let cases = [
            ("move down middle", Case::MoveDownMiddle),
            ("move up middle", Case::MoveUpMiddle),
            ("move selected block down", Case::MoveSelectionBlockDown),
            ("move at edges noop", Case::MoveAtEdgesNoop),
            ("move across final line", Case::MoveAcrossFinalLine),
            ("insert line after", Case::InsertLineAfter),
            ("insert line before", Case::InsertLineBefore),
            ("delete to line start", Case::DeleteToLineStart),
            (
                "delete to line start at start noop",
                Case::DeleteToLineStartAtStartNoop,
            ),
        ];

        for (name, case) in cases {
            match case {
                Case::MoveDownMiddle => {
                    let mut editor = editor("a\nb\nc");
                    editor.cursor = Position::new(1, 1);
                    editor.move_lines_down();
                    assert_eq!(text(&editor), "a\nc\nb", "case {name}");
                    assert_eq!(editor.cursor, Position::new(2, 1), "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "a\nb\nc", "case {name}");
                    assert_eq!(editor.cursor, Position::new(1, 1), "case {name}");
                    assert!(editor.redo(), "case {name}");
                    assert_eq!(text(&editor), "a\nc\nb", "case {name}");
                }
                Case::MoveUpMiddle => {
                    let mut editor = editor("a\nb\nc");
                    editor.cursor = Position::new(1, 0);
                    editor.move_lines_up();
                    assert_eq!(text(&editor), "b\na\nc", "case {name}");
                    assert_eq!(editor.cursor, Position::new(0, 0), "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "a\nb\nc", "case {name}");
                    assert_eq!(editor.cursor, Position::new(1, 0), "case {name}");
                }
                Case::MoveSelectionBlockDown => {
                    let mut editor = editor("a\nb\nc\nd");
                    editor.selection =
                        Some(Selection::new(Position::new(1, 0), Position::new(2, 1)));
                    editor.cursor = Position::new(2, 1);
                    editor.move_lines_down();
                    assert_eq!(text(&editor), "a\nd\nb\nc", "case {name}");
                    assert_eq!(
                        editor.selection.unwrap(),
                        Selection::new(Position::new(2, 0), Position::new(3, 1)),
                        "case {name}"
                    );
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "a\nb\nc\nd", "case {name}");
                    assert_eq!(
                        editor.selection.unwrap(),
                        Selection::new(Position::new(1, 0), Position::new(2, 1)),
                        "case {name}"
                    );
                }
                Case::MoveAtEdgesNoop => {
                    let mut editor = editor("a\nb");
                    editor.cursor = Position::new(0, 0);
                    editor.move_lines_up();
                    assert_eq!(text(&editor), "a\nb", "case {name}");
                    assert!(!editor.undo(), "case {name}");
                    editor.cursor = Position::new(1, 0);
                    editor.move_lines_down();
                    assert_eq!(text(&editor), "a\nb", "case {name}");
                    assert!(!editor.undo(), "case {name}");
                }
                Case::MoveAcrossFinalLine => {
                    let mut editor = editor("a\nb\nc");
                    editor.cursor = Position::new(2, 0);
                    editor.move_lines_up();
                    assert_eq!(text(&editor), "a\nc\nb", "case {name}");
                    assert_eq!(editor.buffer.line_count(), 3, "case {name}");
                    editor.cursor = Position::new(1, 0);
                    editor.move_lines_down();
                    assert_eq!(text(&editor), "a\nb\nc", "case {name}");
                    assert_eq!(editor.buffer.line_count(), 3, "case {name}");
                }
                Case::InsertLineAfter => {
                    let mut editor = editor("a\nb");
                    editor.cursor = Position::new(0, 1);
                    editor.insert_line_after();
                    assert_eq!(text(&editor), "a\n\nb", "case {name}");
                    assert_eq!(editor.cursor, Position::new(1, 0), "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "a\nb", "case {name}");
                    assert_eq!(editor.cursor, Position::new(0, 1), "case {name}");
                }
                Case::InsertLineBefore => {
                    let mut editor = editor("a\nb");
                    editor.cursor = Position::new(1, 1);
                    editor.insert_line_before();
                    assert_eq!(text(&editor), "a\n\nb", "case {name}");
                    assert_eq!(editor.cursor, Position::new(1, 0), "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "a\nb", "case {name}");
                    assert_eq!(editor.cursor, Position::new(1, 1), "case {name}");
                }
                Case::DeleteToLineStart => {
                    let mut editor = editor("abc");
                    editor.cursor = Position::new(0, 2);
                    editor.delete_to_line_start();
                    assert_eq!(text(&editor), "c", "case {name}");
                    assert_eq!(editor.cursor, Position::new(0, 0), "case {name}");
                    assert!(editor.undo(), "case {name}");
                    assert_eq!(text(&editor), "abc", "case {name}");
                    assert_eq!(editor.cursor, Position::new(0, 2), "case {name}");
                }
                Case::DeleteToLineStartAtStartNoop => {
                    let mut editor = editor("abc");
                    editor.cursor = Position::new(0, 0);
                    editor.delete_to_line_start();
                    assert_eq!(text(&editor), "abc", "case {name}");
                    assert!(!editor.undo(), "case {name}");
                }
            }
        }
    }

    /// TASK-260711-19 testcases: `indent`/`outdent` table-driven over
    /// multi-line selections, an unindented no-op, and mixed tab/space
    /// content, each asserting the whole block undoes in a single call.
    #[test]
    fn indent_and_outdent_are_table_driven() {
        enum Case {
            IndentMultiLineSelection,
            OutdentNoLeadingWhitespaceIsNoop,
            OutdentMixedTabsAndSpaces,
        }

        let cases = [
            (
                "indent multi-line selection",
                Case::IndentMultiLineSelection,
            ),
            (
                "outdent no leading whitespace is a no-op",
                Case::OutdentNoLeadingWhitespaceIsNoop,
            ),
            (
                "outdent mixed tabs and spaces",
                Case::OutdentMixedTabsAndSpaces,
            ),
        ];

        for (name, case) in cases {
            match case {
                Case::IndentMultiLineSelection => {
                    let mut editor = editor("foo\nbar\nbaz");
                    let selection_before = Selection::new(Position::new(0, 0), Position::new(1, 3));
                    editor.selection = Some(selection_before);
                    editor.cursor = Position::new(1, 3);

                    editor.indent();

                    assert_eq!(text(&editor), "    foo\n    bar\nbaz", "case {name}");
                    assert_eq!(editor.cursor, Position::new(1, 7), "case {name}");
                    assert_eq!(
                        editor.selection,
                        Some(Selection::new(Position::new(0, 4), Position::new(1, 7))),
                        "case {name}"
                    );

                    assert!(editor.undo(), "case {name}: one undo call must revert");
                    assert_eq!(text(&editor), "foo\nbar\nbaz", "case {name}");
                    assert_eq!(editor.cursor, Position::new(1, 3), "case {name}");
                    assert_eq!(editor.selection, Some(selection_before), "case {name}");
                }
                Case::OutdentNoLeadingWhitespaceIsNoop => {
                    let mut editor = editor("foo\nbar");
                    editor.cursor = Position::new(0, 1);

                    editor.outdent();

                    assert_eq!(text(&editor), "foo\nbar", "case {name}");
                    assert!(
                        !editor.undo(),
                        "case {name}: a no-op edit must not push an undo group"
                    );
                }
                Case::OutdentMixedTabsAndSpaces => {
                    let mut editor = editor("\tfoo\n  bar\n    baz");
                    let selection_before = Selection::new(Position::new(0, 0), Position::new(2, 7));
                    editor.selection = Some(selection_before);
                    editor.cursor = Position::new(2, 7);

                    editor.outdent();

                    // One leading tab, two of four leading spaces, and a full
                    // four leading spaces are each removed as a single level
                    // (whichever is actually present on that line).
                    assert_eq!(text(&editor), "foo\nbar\nbaz", "case {name}");
                    assert_eq!(editor.cursor, Position::new(2, 3), "case {name}");
                    assert_eq!(
                        editor.selection,
                        Some(Selection::new(Position::new(0, 0), Position::new(2, 3))),
                        "case {name}"
                    );

                    assert!(editor.undo(), "case {name}: one undo call must revert");
                    assert_eq!(text(&editor), "\tfoo\n  bar\n    baz", "case {name}");
                    assert_eq!(editor.cursor, Position::new(2, 7), "case {name}");
                    assert_eq!(editor.selection, Some(selection_before), "case {name}");
                }
            }
        }
    }

    #[test]
    fn copy_text_selection_line_and_empty_buffer_cases() {
        let mut selected = editor("abc\ndef");
        selected.select_range(Position::new(0, 1), Position::new(1, 2));
        assert_eq!(selected.copy_text(), Some("bc\nde".to_string()));

        let mut line = editor("abc\ndef");
        line.cursor = Position::new(1, 1);
        assert_eq!(line.copy_text(), Some("def\n".to_string()));

        let empty = editor("");
        assert_eq!(empty.copy_text(), Some("\n".to_string()));
    }

    #[test]
    fn cut_selection_and_line_delete_are_single_undo_groups() {
        let mut selected = editor("abc\ndef");
        selected.select_range(Position::new(0, 1), Position::new(1, 2));
        assert_eq!(selected.cut(), Some("bc\nde".to_string()));
        assert_eq!(text(&selected), "af");
        assert!(selected.undo());
        assert_eq!(text(&selected), "abc\ndef");

        let mut line = editor("abc\ndef\nghi");
        line.cursor = Position::new(1, 1);
        assert_eq!(line.cut(), Some("def\n".to_string()));
        assert_eq!(text(&line), "abc\nghi");
        assert!(line.undo());
        assert_eq!(text(&line), "abc\ndef\nghi");
    }

    #[test]
    fn replace_ranges_applies_back_to_front_and_undos_once() {
        let mut editor = editor("foo foo\nfoo");
        editor.cursor = Position::new(1, 3);
        editor.selection = Some(Selection::new(Position::new(0, 0), Position::new(0, 3)));

        editor.replace_ranges(&[
            (Position::new(0, 0), Position::new(0, 3), "bar"),
            (Position::new(0, 4), Position::new(0, 7), "bazzz"),
            (Position::new(1, 0), Position::new(1, 3), "qux"),
        ]);

        assert_eq!(text(&editor), "bar bazzz\nqux");
        assert!(editor.selection.is_none());
        assert!(editor.undo(), "all replacements are one undo group");
        assert_eq!(text(&editor), "foo foo\nfoo");
        assert_eq!(editor.cursor, Position::new(1, 3));
        assert_eq!(
            editor.selection,
            Some(Selection::new(Position::new(0, 0), Position::new(0, 3)))
        );
    }
}
