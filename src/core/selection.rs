//! Selection ranges represented by stable grapheme-indexed positions.

use super::position::Position;

/// A single contiguous selection.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Selection {
    /// Fixed end of an extending selection.
    pub anchor: Position,
    /// Moving end of an extending selection.
    pub head: Position,
}

impl Selection {
    /// Creates a selection from an anchor and a head.
    pub const fn new(anchor: Position, head: Position) -> Self {
        Self { anchor, head }
    }

    /// Returns true when the anchor and head point at the same buffer location.
    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    /// Returns the selection endpoints in buffer order.
    pub fn range(self) -> (Position, Position) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}
