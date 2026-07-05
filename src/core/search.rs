//! Pure buffer search helpers.
//!
//! Search works in grapheme-indexed positions so callers never split Unicode
//! clusters. Matching is intentionally literal and line-local for the MVP;
//! regex and multi-line queries are outside SPEC-0001 scope.

use unicode_segmentation::UnicodeSegmentation;

use super::{buffer::TextBuffer, position::Position};

/// Finds all non-overlapping literal matches in `buffer`.
///
/// Returned ranges are half-open grapheme positions. Queries containing a line
/// break return no matches because MVP search is line-local.
pub fn find_matches(
    buffer: &TextBuffer,
    query: &str,
    case_sensitive: bool,
) -> Vec<(Position, Position)> {
    if query.is_empty() || query.contains('\n') || query.contains('\r') {
        return Vec::new();
    }

    let query_graphemes = normalized_graphemes(query, case_sensitive);
    if query_graphemes.is_empty() {
        return Vec::new();
    }
    let query_len = query_graphemes.len();
    let mut matches = Vec::new();

    for line_index in 0..buffer.line_count() {
        let Some(line) = buffer.line(line_index) else {
            continue;
        };
        let line_graphemes = normalized_graphemes(line, case_sensitive);
        if line_graphemes.len() < query_len {
            continue;
        }

        // Skip past each match so results never overlap: overlapping ranges
        // would corrupt replace-all, which applies them back to front.
        let mut start = 0;
        while start + query_len <= line_graphemes.len() {
            if line_graphemes[start..start + query_len] == query_graphemes[..] {
                matches.push((
                    Position::new(line_index, start),
                    Position::new(line_index, start + query_len),
                ));
                start += query_len;
            } else {
                start += 1;
            }
        }
    }

    matches
}

/// Selects the first match whose start is at or after `cursor`, wrapping to 0.
pub fn next_match_from(matches: &[(Position, Position)], cursor: Position) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    matches
        .iter()
        .position(|(start, _)| *start >= cursor)
        .or(Some(0))
}

/// Selects the last match whose start is before `cursor`, wrapping to the end.
pub fn previous_match_from(matches: &[(Position, Position)], cursor: Position) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    matches
        .iter()
        .rposition(|(start, _)| *start < cursor)
        .or(Some(matches.len() - 1))
}

fn normalized_graphemes(text: &str, case_sensitive: bool) -> Vec<String> {
    text.graphemes(true)
        .map(|grapheme| {
            if case_sensitive {
                grapheme.to_string()
            } else {
                grapheme.to_lowercase()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn overlapping_candidates_yield_non_overlapping_matches() {
        let (buffer, _) = super::super::buffer::TextBuffer::from_bytes(b"aaa").unwrap();
        let matches = super::find_matches(&buffer, "aa", true);
        assert_eq!(
            matches,
            vec![(super::Position::new(0, 0), super::Position::new(0, 2))]
        );
    }

    use super::*;

    fn buffer(text: &str) -> TextBuffer {
        TextBuffer::from_bytes(text.as_bytes()).unwrap().0
    }

    #[test]
    fn finds_multiple_matches_across_lines_in_grapheme_positions() {
        let matches = find_matches(&buffer("one two one\none"), "one", true);
        assert_eq!(
            matches,
            vec![
                (Position::new(0, 0), Position::new(0, 3)),
                (Position::new(0, 8), Position::new(0, 11)),
                (Position::new(1, 0), Position::new(1, 3)),
            ]
        );
    }

    #[test]
    fn case_sensitivity_changes_match_count() {
        let buffer = buffer("Foo foo");
        assert_eq!(find_matches(&buffer, "foo", true).len(), 1);
        assert_eq!(find_matches(&buffer, "foo", false).len(), 2);
    }

    #[test]
    fn unicode_grapheme_positions_do_not_split_clusters() {
        let buffer = buffer("あ👍あ\nが が");
        assert_eq!(
            find_matches(&buffer, "👍", true),
            vec![(Position::new(0, 1), Position::new(0, 2))]
        );
        assert!(
            find_matches(&buffer, "か", true).is_empty(),
            "a decomposed voiced kana grapheme must not be split"
        );
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let matches = find_matches(&buffer("a b a"), "a", true);
        assert_eq!(next_match_from(&matches, Position::new(0, 1)), Some(1));
        assert_eq!(next_match_from(&matches, Position::new(0, 5)), Some(0));
        assert_eq!(previous_match_from(&matches, Position::new(0, 4)), Some(0));
        assert_eq!(previous_match_from(&matches, Position::new(0, 0)), Some(1));
    }
}
