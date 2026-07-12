//! Which-key overlay: candidate continuations for a pending key sequence
//! (backlog P1, SPEC-0005 `:which-key`).
//!
//! The overlay has no state of its own — the event loop re-resolves the
//! pending prefix on every draw and feeds `ResolveResult::Pending` straight
//! in, so the panel can never drift from what the resolver would actually do.

use crate::{
    input::KeyEvent,
    keymap::EditorAction,
    ui::{Screen, Style},
};

/// Hard cap on candidate rows, independent of screen height: past this the
/// list stops being a reminder and the palette (F1) is the better tool.
const MAX_CANDIDATE_ROWS: usize = 10;

/// Builds the overlay body: one row per candidate continuation, preceded by
/// the exact-match row that a sequence timeout would fire. Candidates are
/// shown by their *remaining* keys (the typed prefix is in the title), left
/// aligned so the action column lines up.
pub(crate) fn which_key_lines(
    pending: &[KeyEvent],
    candidates: &[(Vec<KeyEvent>, EditorAction)],
    exact: Option<EditorAction>,
) -> Vec<String> {
    let mut entries: Vec<(String, String)> = candidates
        .iter()
        .map(|(keys, action)| (format_keys(&keys[pending.len()..]), action.to_string()))
        .collect();
    entries.sort();

    let mut lines = Vec::new();
    if let Some(action) = exact {
        lines.push(format!("(wait) {action}"));
    }

    let shown = entries.len().min(MAX_CANDIDATE_ROWS);
    let key_width = entries[..shown]
        .iter()
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(0);
    for (keys, action) in &entries[..shown] {
        lines.push(format!("{keys:key_width$}  {action}"));
    }
    if entries.len() > shown {
        lines.push(format!("… {} more", entries.len() - shown));
    }
    lines
}

/// Draws the which-key panel anchored just above the status line, following
/// the inspector's boxed-overlay conventions. `pending_label` is the typed
/// prefix (also shown in the status bar) used as the box title.
pub(crate) fn draw_which_key(screen: &mut Screen, pending_label: &str, lines: &[String]) {
    if lines.is_empty() || screen.height() < 6 || screen.width() < 12 {
        return;
    }

    let box_x = 2;
    let box_width = screen.width().saturating_sub(4);
    let inner_width = usize::from(box_width.saturating_sub(4));
    // Keep the tab bar (row 0) and at least one editor row visible.
    let max_rows = usize::from(screen.height().saturating_sub(5)).clamp(1, MAX_CANDIDATE_ROWS + 2);

    let shown = lines.len().min(max_rows);
    let box_height = shown as u16 + 2; // top border + body rows + bottom border
    // Directly above the status line (last row).
    let box_top = screen.height().saturating_sub(1 + box_height);

    let dim = Style {
        reverse: false,
        dim: true,
        fg: None,
    };
    let normal = Style::default();

    let title = format!(" {pending_label} … ");
    for row in 0..box_height {
        let y = box_top + row;
        let line = if row == 0 {
            frame_line("╭", "─", "╮", &title, usize::from(box_width))
        } else if row == box_height - 1 {
            frame_line("╰", "─", "╯", "", usize::from(box_width))
        } else {
            format!("│{}│", " ".repeat(usize::from(box_width).saturating_sub(2)))
        };
        screen.put_str(box_x, y, &line, dim);
    }

    for (row, line) in lines.iter().take(shown).enumerate() {
        let clipped = clip_to_width(line, inner_width);
        screen.put_str(box_x + 2, box_top + 1 + row as u16, &clipped, normal);
    }
}

fn format_keys(keys: &[KeyEvent]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn frame_line(left: &str, fill: &str, right: &str, title: &str, width: usize) -> String {
    let inner = width.saturating_sub(2);
    let title = clip_to_width(title, inner);
    let title_len = title.chars().count();
    format!(
        "{left}{title}{}{right}",
        fill.repeat(inner.saturating_sub(title_len))
    )
}

fn clip_to_width(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::{MAX_CANDIDATE_ROWS, which_key_lines};
    use crate::keymap::{EditorAction, parse_key_sequence};

    type Candidate = (Vec<crate::input::KeyEvent>, EditorAction);

    fn candidate(sequence: &str, action: EditorAction) -> Candidate {
        (parse_key_sequence(sequence).unwrap(), action)
    }

    type Case = (
        &'static str,
        Vec<Candidate>,
        Option<EditorAction>,
        Vec<&'static str>,
    );

    #[test]
    fn which_key_lines_table_driven() {
        let pending = parse_key_sequence("ctrl+k").unwrap();
        let cases: &[Case] = &[
            (
                "single candidate, no exact",
                vec![candidate("ctrl+k ctrl+u", EditorAction::CursorUp)],
                None,
                vec!["Ctrl+U  cursor.up"],
            ),
            (
                "exact match leads with the timeout row",
                vec![candidate("ctrl+k ctrl+u", EditorAction::CursorUp)],
                Some(EditorAction::CursorDown),
                vec!["(wait) cursor.down", "Ctrl+U  cursor.up"],
            ),
            (
                "candidates sort by continuation and align the action column",
                vec![
                    candidate("ctrl+k shift+enter", EditorAction::CursorLineEnd),
                    candidate("ctrl+k ctrl+u", EditorAction::CursorUp),
                ],
                None,
                vec!["Ctrl+U       cursor.up", "Shift+Enter  cursor.lineEnd"],
            ),
        ];

        for (name, candidates, exact, expected) in cases {
            assert_eq!(
                which_key_lines(&pending, candidates, *exact),
                *expected,
                "{name}"
            );
        }
    }

    #[test]
    fn which_key_lines_caps_rows_and_reports_overflow() {
        let pending = parse_key_sequence("ctrl+k").unwrap();
        let candidates: Vec<_> = ('a'..='z')
            .take(MAX_CANDIDATE_ROWS + 3)
            .map(|c| candidate(&format!("ctrl+k ctrl+{c}"), EditorAction::CursorUp))
            .collect();

        let lines = which_key_lines(&pending, &candidates, None);

        assert_eq!(lines.len(), MAX_CANDIDATE_ROWS + 1);
        assert_eq!(lines.last().unwrap(), "… 3 more");
    }
}
