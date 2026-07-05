//! Cursor and selection positions in the text buffer.
//!
//! `Position` deliberately uses grapheme indexes rather than byte offsets. Byte
//! offsets are an implementation detail of `TextBuffer`; exposing them would let
//! callers split a Unicode grapheme cluster and corrupt text.

/// A zero-based location in a [`TextBuffer`](crate::core::buffer::TextBuffer).
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Position {
    pub line: usize,
    pub grapheme: usize,
}

impl Position {
    /// Creates a new zero-based buffer position.
    pub const fn new(line: usize, grapheme: usize) -> Self {
        Self { line, grapheme }
    }
}
