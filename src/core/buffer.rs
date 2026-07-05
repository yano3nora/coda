//! Pure text buffer model.
//!
//! The MVP buffer is intentionally line-based (`Vec<String>`) per ADR-0009.
//! Public editing APIs accept and return grapheme-indexed [`Position`] values;
//! byte offsets stay private so Unicode clusters are never split by callers.

use std::str;

use unicode_segmentation::UnicodeSegmentation;

use super::position::Position;

/// File-wide line ending style used when serializing the buffer.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum LineEnding {
    /// `\n`
    #[default]
    Lf,
    /// `\r\n`
    CrLf,
}

impl LineEnding {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

/// Metadata reported by [`TextBuffer::from_bytes`].
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct LoadInfo {
    /// True when both LF and CRLF were present in the loaded bytes.
    pub mixed_line_endings: bool,
}

/// Errors that prevent loading bytes into a text buffer.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LoadError {
    /// Input was not valid UTF-8. Lossy conversion is forbidden by ADR-0009.
    InvalidUtf8,
}

/// Editable text plus file round-trip metadata.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TextBuffer {
    lines: Vec<String>,
    line_ending: LineEnding,
    trailing_newline: bool,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            line_ending: LineEnding::Lf,
            // New (not-loaded) buffers default to POSIX-friendly output: files
            // written without a final newline make shells show a "%" marker and
            // pollute diffs. Loaded files keep whatever from_bytes detected.
            trailing_newline: true,
        }
    }
}

impl TextBuffer {
    /// Builds a buffer from UTF-8 bytes while preserving EOL metadata.
    pub fn from_bytes(bytes: &[u8]) -> Result<(Self, LoadInfo), LoadError> {
        let text = str::from_utf8(bytes).map_err(|_| LoadError::InvalidUtf8)?;
        let (lines, lf_count, crlf_count) = split_file_lines(text);
        let line_ending = if crlf_count > lf_count {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };

        Ok((
            Self {
                lines,
                line_ending,
                trailing_newline: text.ends_with('\n'),
            },
            LoadInfo {
                mixed_line_endings: lf_count > 0 && crlf_count > 0,
            },
        ))
    }

    /// Serializes the buffer using the loaded line ending style and final-EOL flag.
    pub fn to_bytes(&self) -> Vec<u8> {
        let eol = self.line_ending.as_str();
        let mut text = self.lines.join(eol);
        if self.trailing_newline {
            text.push_str(eol);
        }
        text.into_bytes()
    }

    /// Inserts text at `pos` and returns the cursor position after the inserted text.
    ///
    /// Newlines inside `text` split lines. Both LF and CRLF input are accepted here,
    /// but the buffer stores only line contents; file EOL style remains metadata.
    pub fn insert(&mut self, pos: Position, text: &str) -> Position {
        if text.is_empty() {
            return self.clamp_position(pos);
        }

        let pos = self.clamp_position(pos);
        let byte = grapheme_to_byte(
            self.line(pos.line).expect("clamped line exists"),
            pos.grapheme,
        );
        let insert_lines = split_insert_text(text);

        if insert_lines.len() == 1 {
            self.lines[pos.line].insert_str(byte, insert_lines[0]);
            return Position::new(pos.line, pos.grapheme + grapheme_count(insert_lines[0]));
        }

        let current = &self.lines[pos.line];
        let before = current[..byte].to_string();
        let after = current[byte..].to_string();

        let mut replacement = Vec::with_capacity(insert_lines.len());
        replacement.push(format!("{}{}", before, insert_lines[0]));
        replacement.extend(
            insert_lines[1..insert_lines.len() - 1]
                .iter()
                .map(|line| (*line).to_string()),
        );
        let last_inserted = insert_lines.last().expect("len checked above");
        replacement.push(format!("{}{}", last_inserted, after));

        let inserted_line_count = insert_lines.len() - 1;
        let cursor = Position::new(
            pos.line + inserted_line_count,
            grapheme_count(last_inserted),
        );
        self.lines.splice(pos.line..=pos.line, replacement);
        cursor
    }

    /// Deletes a grapheme range and returns the removed text for undo replay.
    ///
    /// Ranges are normalized after clamping, so callers may pass reversed or
    /// out-of-bounds positions without risking panics.
    pub fn delete_range(&mut self, start: Position, end: Position) -> String {
        let (start, end) = self.normalized_range(start, end);
        if start == end {
            return String::new();
        }

        if start.line == end.line {
            let line = &mut self.lines[start.line];
            let start_byte = grapheme_to_byte(line, start.grapheme);
            let end_byte = grapheme_to_byte(line, end.grapheme);
            return line.drain(start_byte..end_byte).collect();
        }

        let start_byte = grapheme_to_byte(&self.lines[start.line], start.grapheme);
        let end_byte = grapheme_to_byte(&self.lines[end.line], end.grapheme);

        let mut deleted = String::new();
        deleted.push_str(&self.lines[start.line][start_byte..]);
        for line_index in start.line + 1..end.line {
            deleted.push('\n');
            deleted.push_str(&self.lines[line_index]);
        }
        deleted.push('\n');
        deleted.push_str(&self.lines[end.line][..end_byte]);

        let merged = format!(
            "{}{}",
            &self.lines[start.line][..start_byte],
            &self.lines[end.line][end_byte..]
        );
        self.lines.splice(start.line..=end.line, [merged]);
        self.ensure_cursor_line();
        deleted
    }

    /// Returns a line without its EOL marker.
    pub fn line(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(String::as_str)
    }

    /// Number of logical lines. Always at least one.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Counts Unicode grapheme clusters in a line, or returns zero for an invalid line.
    pub fn grapheme_count(&self, line_index: usize) -> usize {
        self.line(line_index).map(grapheme_count).unwrap_or(0)
    }

    /// Replaces a contiguous logical line range and returns the removed lines.
    ///
    /// This intentionally preserves file-level EOL metadata (`line_ending` and
    /// `trailing_newline`). Line-moving edits need to reorder logical lines
    /// without accidentally changing whether the file serializes with a final
    /// newline.
    pub(crate) fn replace_lines(
        &mut self,
        start: usize,
        end_exclusive: usize,
        replacement: Vec<String>,
    ) -> Vec<String> {
        let start = start.min(self.lines.len());
        let end_exclusive = end_exclusive.min(self.lines.len()).max(start);
        let removed = self.lines[start..end_exclusive].to_vec();
        self.lines.splice(start..end_exclusive, replacement);
        self.ensure_cursor_line();
        removed
    }

    /// Returns a clone of the logical lines for coarse-grained undo groups.
    ///
    /// This is intentionally crate-private: editing APIs should still expose
    /// grapheme positions, but full-buffer replace needs a stable snapshot so
    /// undo can restore the exact pre-edit content in one step.
    pub(crate) fn lines_snapshot(&self) -> Vec<String> {
        self.lines.clone()
    }

    /// Clamps a position to an existing line and that line's grapheme end.
    pub fn clamp_position(&self, pos: Position) -> Position {
        let line = pos.line.min(self.lines.len().saturating_sub(1));
        let grapheme = pos.grapheme.min(self.grapheme_count(line));
        Position::new(line, grapheme)
    }

    /// Line ending selected at load time or defaulted for new/empty buffers.
    pub const fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Whether serialization should append a final line ending.
    pub const fn trailing_newline(&self) -> bool {
        self.trailing_newline
    }

    fn normalized_range(&self, start: Position, end: Position) -> (Position, Position) {
        let start = self.clamp_position(start);
        let end = self.clamp_position(end);
        if start <= end {
            (start, end)
        } else {
            (end, start)
        }
    }

    fn ensure_cursor_line(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
    }
}

fn split_file_lines(text: &str) -> (Vec<String>, usize, usize) {
    if text.is_empty() {
        return (vec![String::new()], 0, 0);
    }

    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    let mut lf_count = 0;
    let mut crlf_count = 0;

    while offset < bytes.len() {
        if bytes[offset] == b'\n' {
            let line_end = if offset > start && bytes[offset - 1] == b'\r' {
                crlf_count += 1;
                offset - 1
            } else {
                lf_count += 1;
                offset
            };
            lines.push(text[start..line_end].to_string());
            start = offset + 1;
        }
        offset += 1;
    }

    if start < text.len() {
        lines.push(text[start..].to_string());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    (lines, lf_count, crlf_count)
}

fn split_insert_text(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut offset = 0;

    while offset < bytes.len() {
        if bytes[offset] == b'\n' {
            let line_end = if offset > start && bytes[offset - 1] == b'\r' {
                offset - 1
            } else {
                offset
            };
            parts.push(&text[start..line_end]);
            start = offset + 1;
        }
        offset += 1;
    }
    parts.push(&text[start..]);
    parts
}

fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

fn grapheme_to_byte(text: &str, grapheme_index: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(byte_offset, _)| byte_offset)
        .nth(grapheme_index)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::{LineEnding, LoadError, TextBuffer};
    use crate::core::position::Position;

    #[test]
    fn round_trip_preserves_loaded_bytes() {
        let cases: &[(&str, &[u8])] = &[
            ("lf with trailing newline", b"alpha\nbeta\n"),
            ("lf without trailing newline", b"alpha\nbeta"),
            ("crlf with trailing newline", b"alpha\r\nbeta\r\n"),
            ("empty file", b""),
            ("unicode graphemes", "日本語\n👨‍👩‍👧‍👦 café\n".as_bytes()),
        ];

        for (name, input) in cases {
            let (buffer, _info) = TextBuffer::from_bytes(input).unwrap_or_else(|error| {
                panic!("case {name:?} should load successfully, got {error:?}")
            });
            assert_eq!(buffer.to_bytes(), *input, "case {name}");
        }
    }

    #[test]
    fn load_reports_mixed_line_endings_and_rejects_invalid_utf8() {
        enum Expected {
            Loaded {
                line_ending: LineEnding,
                mixed_line_endings: bool,
            },
            Error(LoadError),
        }

        let cases: &[(&str, &[u8], Expected)] = &[
            (
                "crlf majority with lf minority",
                b"a\r\nb\r\nc\n",
                Expected::Loaded {
                    line_ending: LineEnding::CrLf,
                    mixed_line_endings: true,
                },
            ),
            (
                "invalid utf8",
                &[0xff, 0xfe, b'a'],
                Expected::Error(LoadError::InvalidUtf8),
            ),
        ];

        for (name, input, expected) in cases {
            match expected {
                Expected::Loaded {
                    line_ending,
                    mixed_line_endings,
                } => {
                    let (buffer, info) = TextBuffer::from_bytes(input).unwrap_or_else(|error| {
                        panic!("case {name:?} should load successfully, got {error:?}")
                    });
                    assert_eq!(buffer.line_ending(), *line_ending, "case {name}");
                    assert_eq!(info.mixed_line_endings, *mixed_line_endings, "case {name}");
                }
                Expected::Error(error) => {
                    assert_eq!(TextBuffer::from_bytes(input), Err(*error), "case {name}");
                }
            }
        }
    }

    #[test]
    fn editing_operations_are_grapheme_based_and_round_trippable() {
        enum EditCase {
            Insert {
                initial: &'static [u8],
                pos: Position,
                text: &'static str,
                expected_lines: Vec<&'static str>,
                expected_cursor: Position,
            },
            Delete {
                initial: &'static [u8],
                start: Position,
                end: Position,
                expected_deleted: &'static str,
                expected_lines: Vec<&'static str>,
            },
            DeleteThenInsertUndo {
                initial: &'static [u8],
                start: Position,
                end: Position,
            },
            Clamp {
                initial: &'static [u8],
                pos: Position,
                expected: Position,
            },
        }

        let cases = vec![
            (
                "ascii insert",
                EditCase::Insert {
                    initial: b"abc",
                    pos: Position::new(0, 1),
                    text: "XY",
                    expected_lines: vec!["aXYbc"],
                    expected_cursor: Position::new(0, 3),
                },
            ),
            (
                "ascii delete",
                EditCase::Delete {
                    initial: b"aXYbc",
                    start: Position::new(0, 1),
                    end: Position::new(0, 3),
                    expected_deleted: "XY",
                    expected_lines: vec!["abc"],
                },
            ),
            (
                "insert between unicode graphemes",
                EditCase::Insert {
                    initial: "あa👍".as_bytes(),
                    pos: Position::new(0, 1),
                    text: "Z",
                    expected_lines: vec!["あZa👍"],
                    expected_cursor: Position::new(0, 2),
                },
            ),
            (
                "delete zwj emoji as one grapheme",
                EditCase::Delete {
                    initial: "a👨‍👩‍👧‍👦b".as_bytes(),
                    start: Position::new(0, 1),
                    end: Position::new(0, 2),
                    expected_deleted: "👨‍👩‍👧‍👦",
                    expected_lines: vec!["ab"],
                },
            ),
            (
                "insert text containing newlines splits lines",
                EditCase::Insert {
                    initial: b"abcd",
                    pos: Position::new(0, 2),
                    text: "X\nY\nZ",
                    expected_lines: vec!["abX", "Y", "Zcd"],
                    expected_cursor: Position::new(2, 1),
                },
            ),
            (
                "delete across lines joins remainder",
                EditCase::Delete {
                    initial: b"alpha\nbeta\ngamma",
                    start: Position::new(0, 2),
                    end: Position::new(2, 2),
                    expected_deleted: "pha\nbeta\nga",
                    expected_lines: vec!["almma"],
                },
            ),
            (
                "delete then insert restores original logical text",
                EditCase::DeleteThenInsertUndo {
                    initial: b"alpha\nbeta\ngamma",
                    start: Position::new(0, 2),
                    end: Position::new(2, 2),
                },
            ),
            (
                "clamp position past line end and last line",
                EditCase::Clamp {
                    initial: "a\nあ👍".as_bytes(),
                    pos: Position::new(99, 99),
                    expected: Position::new(1, 2),
                },
            ),
        ];

        for (name, case) in cases {
            match case {
                EditCase::Insert {
                    initial,
                    pos,
                    text,
                    expected_lines,
                    expected_cursor,
                } => {
                    let (mut buffer, _info) = TextBuffer::from_bytes(initial).unwrap();
                    let cursor = buffer.insert(pos, text);
                    assert_eq!(cursor, expected_cursor, "case {name}");
                    assert_lines(&buffer, &expected_lines, name);
                }
                EditCase::Delete {
                    initial,
                    start,
                    end,
                    expected_deleted,
                    expected_lines,
                } => {
                    let (mut buffer, _info) = TextBuffer::from_bytes(initial).unwrap();
                    let deleted = buffer.delete_range(start, end);
                    assert_eq!(deleted, expected_deleted, "case {name}");
                    assert_lines(&buffer, &expected_lines, name);
                }
                EditCase::DeleteThenInsertUndo {
                    initial,
                    start,
                    end,
                } => {
                    let (mut buffer, _info) = TextBuffer::from_bytes(initial).unwrap();
                    let before = buffer.clone();
                    let deleted = buffer.delete_range(start, end);
                    let cursor = buffer.insert(start, &deleted);
                    assert_eq!(buffer, before, "case {name}");
                    assert_eq!(cursor, end, "case {name}");
                }
                EditCase::Clamp {
                    initial,
                    pos,
                    expected,
                } => {
                    let (buffer, _info) = TextBuffer::from_bytes(initial).unwrap();
                    assert_eq!(buffer.clamp_position(pos), expected, "case {name}");
                }
            }
        }
    }

    fn assert_lines(buffer: &TextBuffer, expected: &[&str], case_name: &str) {
        let actual = (0..buffer.line_count())
            .map(|index| buffer.line(index).expect("line should exist"))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "case {case_name}");
    }
}
