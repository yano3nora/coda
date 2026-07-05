//! Linear undo/redo stack based on inverse edit operations.

use super::{buffer::TextBuffer, position::Position};

/// A primitive buffer mutation used for replaying undo and redo.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum EditOp {
    /// Insert `text` at `pos`.
    Insert { pos: Position, text: String },
    /// Delete the half-open range `start..end`.
    Delete { start: Position, end: Position },
}

/// The merge class for adjacent edits.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum EditKind {
    Insert,
    Backspace,
    Other,
}

/// Metadata used to decide whether adjacent groups can be merged.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct MergeInfo {
    pub kind: EditKind,
    pub start: Position,
    pub end: Position,
    pub text_has_newline: bool,
}

/// One undoable group, possibly containing multiple primitive operations.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EditGroup {
    pub forward: Vec<EditOp>,
    pub inverse: Vec<EditOp>,
    pub cursor_before: Position,
    pub cursor_after: Position,
    pub merge: MergeInfo,
}

impl EditGroup {
    /// Creates a new undoable group.
    pub fn new(
        forward: Vec<EditOp>,
        inverse: Vec<EditOp>,
        cursor_before: Position,
        cursor_after: Position,
        merge: MergeInfo,
    ) -> Self {
        Self {
            forward,
            inverse,
            cursor_before,
            cursor_after,
            merge,
        }
    }
}

/// Undo/redo history. Redo is linear and is cleared by any new edit.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct UndoStack {
    undo: Vec<EditGroup>,
    redo: Vec<EditGroup>,
    force_boundary: bool,
}

impl UndoStack {
    /// Records an edit group and merges it with the previous group when allowed.
    pub fn record(&mut self, group: EditGroup) {
        self.redo.clear();

        if !self.force_boundary
            && let Some(previous) = self.undo.last_mut()
            && can_merge(previous, &group)
        {
            previous.forward.extend(group.forward);
            previous.inverse.extend(group.inverse);
            previous.cursor_after = group.cursor_after;
            previous.merge.start = previous.merge.start.min(group.merge.start);
            previous.merge.end = previous.merge.end.max(group.merge.end);
            previous.merge.text_has_newline |= group.merge.text_has_newline;
            return;
        }

        self.undo.push(group);
        self.force_boundary = false;
    }

    /// Forces the next recorded edit to start a new group.
    pub fn commit_group(&mut self) {
        self.force_boundary = true;
    }

    /// Applies the latest inverse group and returns the restored cursor.
    pub fn undo(&mut self, buffer: &mut TextBuffer) -> Option<Position> {
        let group = self.undo.pop()?;
        for op in group.inverse.iter().rev() {
            apply(buffer, op);
        }
        let cursor = group.cursor_before;
        self.redo.push(group);
        self.force_boundary = true;
        Some(cursor)
    }

    /// Re-applies the latest undone group and returns the redone cursor.
    pub fn redo(&mut self, buffer: &mut TextBuffer) -> Option<Position> {
        let group = self.redo.pop()?;
        for op in &group.forward {
            apply(buffer, op);
        }
        let cursor = group.cursor_after;
        self.undo.push(group);
        self.force_boundary = true;
        Some(cursor)
    }
}

fn can_merge(previous: &EditGroup, next: &EditGroup) -> bool {
    match (previous.merge.kind, next.merge.kind) {
        (EditKind::Insert, EditKind::Insert) => {
            !previous.merge.text_has_newline
                && !next.merge.text_has_newline
                && previous.merge.end == next.merge.start
        }
        (EditKind::Backspace, EditKind::Backspace) => previous.merge.start == next.merge.end,
        _ => false,
    }
}

fn apply(buffer: &mut TextBuffer, op: &EditOp) {
    match op {
        EditOp::Insert { pos, text } => {
            buffer.insert(*pos, text);
        }
        EditOp::Delete { start, end } => {
            buffer.delete_range(*start, *end);
        }
    }
}
