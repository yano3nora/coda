//! Pure cursor movement helpers for [`TextBuffer`](super::buffer::TextBuffer).

use unicode_segmentation::UnicodeSegmentation;

use super::{buffer::TextBuffer, position::Position};

/// Moves one grapheme left, crossing to the previous line when needed.
pub fn left(buffer: &TextBuffer, pos: Position) -> Position {
    let pos = buffer.clamp_position(pos);
    if pos.grapheme > 0 {
        Position::new(pos.line, pos.grapheme - 1)
    } else if pos.line > 0 {
        Position::new(pos.line - 1, buffer.grapheme_count(pos.line - 1))
    } else {
        pos
    }
}

/// Moves one grapheme right, crossing to the next line when needed.
pub fn right(buffer: &TextBuffer, pos: Position) -> Position {
    let pos = buffer.clamp_position(pos);
    if pos.grapheme < buffer.grapheme_count(pos.line) {
        Position::new(pos.line, pos.grapheme + 1)
    } else if pos.line + 1 < buffer.line_count() {
        Position::new(pos.line + 1, 0)
    } else {
        pos
    }
}

/// Moves one line up, clamping to `preferred_grapheme` on the destination line.
pub fn up(buffer: &TextBuffer, pos: Position, preferred_grapheme: usize) -> Position {
    vertical(buffer, pos, pos.line.saturating_sub(1), preferred_grapheme)
}

/// Moves one line down, clamping to `preferred_grapheme` on the destination line.
pub fn down(buffer: &TextBuffer, pos: Position, preferred_grapheme: usize) -> Position {
    let pos = buffer.clamp_position(pos);
    vertical(
        buffer,
        pos,
        (pos.line + 1).min(buffer.line_count().saturating_sub(1)),
        preferred_grapheme,
    )
}

/// Moves to the start of the current line.
pub fn line_start(buffer: &TextBuffer, pos: Position) -> Position {
    let pos = buffer.clamp_position(pos);
    Position::new(pos.line, 0)
}

/// Moves to the end of the current line.
pub fn line_end(buffer: &TextBuffer, pos: Position) -> Position {
    let pos = buffer.clamp_position(pos);
    Position::new(pos.line, buffer.grapheme_count(pos.line))
}

/// Moves to the start of the buffer.
pub fn buffer_start(_buffer: &TextBuffer, _pos: Position) -> Position {
    Position::new(0, 0)
}

/// Moves to the end of the buffer.
pub fn buffer_end(buffer: &TextBuffer, _pos: Position) -> Position {
    let line = buffer.line_count().saturating_sub(1);
    Position::new(line, buffer.grapheme_count(line))
}

/// Moves up by `rows` lines, clamping to the preferred grapheme column.
pub fn page_up(
    buffer: &TextBuffer,
    pos: Position,
    preferred_grapheme: usize,
    rows: usize,
) -> Position {
    let pos = buffer.clamp_position(pos);
    vertical(
        buffer,
        pos,
        pos.line.saturating_sub(rows),
        preferred_grapheme,
    )
}

/// Moves down by `rows` lines, clamping to the preferred grapheme column.
pub fn page_down(
    buffer: &TextBuffer,
    pos: Position,
    preferred_grapheme: usize,
    rows: usize,
) -> Position {
    let pos = buffer.clamp_position(pos);
    vertical(
        buffer,
        pos,
        pos.line
            .saturating_add(rows)
            .min(buffer.line_count().saturating_sub(1)),
        preferred_grapheme,
    )
}

/// Moves to the start of the previous word-like piece, crossing line starts.
pub fn word_left(buffer: &TextBuffer, pos: Position) -> Position {
    let pos = buffer.clamp_position(pos);
    if pos.grapheme == 0 {
        return left(buffer, pos);
    }

    let line = buffer.line(pos.line).expect("clamped line exists");
    let byte = grapheme_to_byte(line, pos.grapheme);

    // The last word piece STARTING before the cursor is the target, so that a
    // cursor in the middle of a word jumps to that word's start (VS Code
    // behavior), not past it to the previous word.
    let mut target = None;
    for (start, end) in word_ranges(line) {
        if start >= byte {
            break;
        }
        if line[start..end].trim().is_empty() {
            continue;
        }
        target = Some(start);
    }

    match target {
        Some(start) => Position::new(pos.line, byte_to_grapheme(line, start)),
        None => Position::new(pos.line, 0),
    }
}

/// Moves to the end of the next word-like piece, crossing line ends.
pub fn word_right(buffer: &TextBuffer, pos: Position) -> Position {
    let pos = buffer.clamp_position(pos);
    if pos.grapheme == buffer.grapheme_count(pos.line) {
        return right(buffer, pos);
    }

    let line = buffer.line(pos.line).expect("clamped line exists");
    let byte = grapheme_to_byte(line, pos.grapheme);
    for (start, end) in word_ranges(line) {
        if end <= byte || line[start..end].trim().is_empty() {
            continue;
        }
        return Position::new(pos.line, byte_to_grapheme(line, end));
    }

    Position::new(pos.line, buffer.grapheme_count(pos.line))
}

fn vertical(
    buffer: &TextBuffer,
    pos: Position,
    target_line: usize,
    preferred_grapheme: usize,
) -> Position {
    let _pos = buffer.clamp_position(pos);
    let line = target_line.min(buffer.line_count().saturating_sub(1));
    Position::new(line, preferred_grapheme.min(buffer.grapheme_count(line)))
}

fn word_ranges(line: &str) -> Vec<(usize, usize)> {
    let mut ranges = line
        .unicode_word_indices()
        .map(|(start, word)| (start, start + word.len()))
        .collect::<Vec<_>>();
    if ranges.is_empty() && !line.is_empty() {
        ranges.push((0, line.len()));
    }
    ranges
}

fn grapheme_to_byte(text: &str, grapheme_index: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(byte_offset, _)| byte_offset)
        .nth(grapheme_index)
        .unwrap_or(text.len())
}

fn byte_to_grapheme(text: &str, byte_index: usize) -> usize {
    text.grapheme_indices(true)
        .take_while(|(byte_offset, _)| *byte_offset < byte_index)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> TextBuffer {
        TextBuffer::from_bytes(text.as_bytes()).unwrap().0
    }

    #[test]
    fn horizontal_edges_cross_lines_and_clamp_at_buffer_edges() {
        enum Case {
            Left(Position, Position),
            Right(Position, Position),
        }
        let buffer = buffer("ab\nc");
        let cases = [
            (
                "left at buffer start",
                Case::Left(Position::new(0, 0), Position::new(0, 0)),
            ),
            (
                "left crosses line",
                Case::Left(Position::new(1, 0), Position::new(0, 2)),
            ),
            (
                "right crosses line",
                Case::Right(Position::new(0, 2), Position::new(1, 0)),
            ),
            (
                "right at buffer end",
                Case::Right(Position::new(1, 1), Position::new(1, 1)),
            ),
        ];

        for (name, case) in cases {
            let actual = match case {
                Case::Left(pos, _) => left(&buffer, pos),
                Case::Right(pos, _) => right(&buffer, pos),
            };
            let expected = match case {
                Case::Left(_, expected) | Case::Right(_, expected) => expected,
            };
            assert_eq!(actual, expected, "case {name}");
        }
    }

    #[test]
    fn vertical_movement_preserves_preferred_grapheme_across_short_lines() {
        let buffer = buffer("abcdef\nx\nabcdef");
        let cases = [
            (
                "down clamps to short line",
                Position::new(0, 5),
                Position::new(1, 1),
            ),
            (
                "down restores on long line",
                Position::new(1, 1),
                Position::new(2, 5),
            ),
            (
                "up restores on long line",
                Position::new(1, 1),
                Position::new(0, 5),
            ),
        ];

        for (name, input, expected) in cases {
            let actual = if name.starts_with("up") {
                up(&buffer, input, 5)
            } else {
                down(&buffer, input, 5)
            };
            assert_eq!(actual, expected, "case {name}");
        }
    }

    #[test]
    fn word_right_moves_to_word_ends_and_across_line_end() {
        let buffer = buffer("foo  bar\nbaz");
        let cases = [
            ("first word end", Position::new(0, 0), Position::new(0, 3)),
            ("second word end", Position::new(0, 3), Position::new(0, 8)),
            (
                "line end to next line start",
                Position::new(0, 8),
                Position::new(1, 0),
            ),
        ];

        for (name, input, expected) in cases {
            assert_eq!(word_right(&buffer, input), expected, "case {name}");
        }
    }

    #[test]
    fn word_left_moves_to_word_starts_and_across_line_start() {
        let buffer = buffer("foo\nfoo  bar");
        let cases = [
            (
                "second word start",
                Position::new(1, 8),
                Position::new(1, 5),
            ),
            (
                "mid-word to its own start, not the previous word",
                Position::new(1, 7),
                Position::new(1, 5),
            ),
            ("first word start", Position::new(1, 5), Position::new(1, 0)),
            (
                "line start to previous line end",
                Position::new(1, 0),
                Position::new(0, 3),
            ),
        ];

        for (name, input, expected) in cases {
            assert_eq!(word_left(&buffer, input), expected, "case {name}");
        }
    }

    #[test]
    fn japanese_word_right_is_monotonic_and_does_not_panic() {
        let buffer = buffer("これはtestです");
        let mut pos = Position::new(0, 0);
        let mut seen = vec![pos];

        for _ in 0..10 {
            let next = word_right(&buffer, pos);
            assert!(next >= pos, "word_right must be monotonic");
            seen.push(next);
            if next == pos {
                break;
            }
            pos = next;
        }

        assert_eq!(
            seen.last().copied(),
            Some(Position::new(0, buffer.grapheme_count(0)))
        );
    }
}
