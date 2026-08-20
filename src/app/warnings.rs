//! Startup warning panel for config-breakage warnings (TASK-260820).
//!
//! The one-line status bar joins every warning with `"; "` and truncates at
//! the right edge, so a broken `bindings.json` — a warning the user must
//! *act on* — could scroll out of view unread (that is how the trailing
//! comma incident stayed invisible). Panel policy:
//!
//! - **Config-loss warnings only** (`AppConfig::warnings`: bindings.json /
//!   config.toml / disabled-chords load issues, HOME unset). Environmental
//!   warnings (Ghostty intercepts, capability fallback) repeat on every
//!   launch and would turn the panel into a nag screen, so they stay in the
//!   status bar with `inspector.open` as their detail view. The dividing
//!   line: "does fixing the user's own configuration make it go away?"
//! - **Any key or paste dismisses and is swallowed** (vim's hit-enter prompt
//!   convention) — except `F1`, which opens the palette on top per the
//!   always-available rescue rule. `warnings.show` reopens the panel later.
//! - **Input priority mirrors draw order**: the panel is drawn above the
//!   prompt/inspector overlays and below the palette, so it also takes keys
//!   after the palette and before those overlays (`EventLoop::handle_key`).
//! - **A panel that cannot be drawn must not block**: on screens too small
//!   to render the box, keys pass through (`blocks_input`), otherwise the
//!   first keystroke would vanish into an invisible overlay.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::inspector::frame_line;
use crate::ui::{Screen, Style};

#[derive(Debug, Clone, Default)]
pub struct WarningsOverlay {
    pub visible: bool,
    lines: Vec<String>,
}

impl WarningsOverlay {
    /// Stores the config warnings and shows the panel when there are any.
    /// Called once at startup, before the first draw.
    pub fn set_startup_warnings(&mut self, lines: Vec<String>) {
        self.visible = !lines.is_empty();
        self.lines = lines;
    }

    /// Reopens the panel (`warnings.show`). Returns `false` when there is
    /// nothing to show, so the caller can report that instead.
    pub fn reopen(&mut self) -> bool {
        self.visible = !self.lines.is_empty();
        self.visible
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Whether the panel should consume the next key/paste/mouse event.
    /// Deliberately identical to the draw guard: an overlay that is not on
    /// screen must never swallow input.
    pub fn blocks_input(&self, screen_width: u16, screen_height: u16) -> bool {
        self.visible && can_draw(screen_width, screen_height)
    }
}

fn can_draw(screen_width: u16, screen_height: u16) -> bool {
    screen_height >= 6 && screen_width >= 12
}

/// Bottom-anchored boxed panel, sitting directly above the status bar so it
/// reads as an expansion of it. Draws nothing when hidden or when the screen
/// is too small (`blocks_input` is false in exactly the same cases).
pub fn draw_warnings(screen: &mut Screen, overlay: &WarningsOverlay) {
    if !overlay.visible || !can_draw(screen.width(), screen.height()) {
        return;
    }

    let box_x = 2;
    let box_width = screen.width().saturating_sub(4);
    let inner_width = usize::from(box_width.saturating_sub(4)).max(1);
    // Leave the tab bar and at least one editor row visible above the panel.
    let max_rows = usize::from(screen.height().saturating_sub(5)).clamp(1, 20);

    let body = body_rows(&overlay.lines, inner_width, max_rows);
    let box_height = body.len() as u16 + 2; // top border + body rows + bottom border
    // Directly above the status bar (the screen's last row).
    let box_top = screen.height().saturating_sub(1 + box_height);

    let dim = Style {
        reverse: false,
        dim: true,
        fg: None,
    };
    let normal = Style::default();

    let title = format!(" {} config warning(s) ", overlay.lines.len());
    for row in 0..box_height {
        let y = box_top + row;
        let line = if row == 0 {
            frame_line("╭", "─", "╮", &title, usize::from(box_width))
        } else if row == box_height - 1 {
            frame_line(
                "╰",
                "─",
                "╯",
                " press any key to dismiss — warnings.show reopens ",
                usize::from(box_width),
            )
        } else {
            format!("│{}│", " ".repeat(usize::from(box_width).saturating_sub(2)))
        };
        screen.put_str(box_x, y, &line, dim);
    }

    for (row, line) in body.iter().enumerate() {
        screen.put_str(box_x + 2, box_top + 1 + row as u16, line, normal);
    }
}

/// Warning lines wrapped to `width` display columns. When the wrapped rows
/// exceed `max_rows`, the last row becomes an explicit `… N more line(s)`
/// marker — truncation must never be silent (the status bar's silent
/// right-edge cut is the exact failure this panel replaces).
fn body_rows(lines: &[String], width: usize, max_rows: usize) -> Vec<String> {
    let mut rows: Vec<String> = lines
        .iter()
        // toml parse errors are multi-line (message + caret excerpt): split
        // them first, or their `\n` (display width 0, rendered as a 1-col
        // sanitized space) would silently push text past the box border.
        .flat_map(|line| line.split('\n'))
        .flat_map(|line| wrap_row(line, width))
        .collect();
    if rows.len() > max_rows {
        let hidden = rows.len() - (max_rows - 1);
        rows.truncate(max_rows - 1);
        // The marker itself must obey `width` — on the narrowest drawable
        // screens the full text would break the border it reports for.
        let marker = format!("… {hidden} more line(s)");
        rows.push(wrap_row(&marker, width).swap_remove(0));
    }
    rows
}

/// Grapheme-safe wrap by display width. Breaks anywhere rather than at word
/// boundaries: warning text is dominated by paths and JSON fragments with no
/// guaranteed spaces, and a dropped character is worse than an ugly break.
fn wrap_row(line: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for grapheme in line.graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if current_width + grapheme_width > width && !current.is_empty() {
            rows.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push_str(grapheme);
        current_width += grapheme_width;
    }
    rows.push(current);
    rows
}

#[cfg(test)]
mod tests {
    use super::{WarningsOverlay, body_rows, draw_warnings, wrap_row};
    use crate::ui::Screen;

    fn row_text(screen: &Screen, y: u16) -> String {
        (0..screen.width())
            .map(|x| {
                screen
                    .cell(x, y)
                    .map_or(" ".to_string(), |cell| cell.symbol.clone())
            })
            .collect()
    }

    #[test]
    fn panel_lists_warnings_above_the_status_bar() {
        let mut overlay = WarningsOverlay::default();
        overlay.set_startup_warnings(vec![
            "bindings.json: invalid bindings.json: trailing comma".to_string(),
            "config.toml: expected value; using default settings".to_string(),
        ]);
        assert!(overlay.visible, "startup warnings open the panel");

        let mut screen = Screen::new(80, 24);
        draw_warnings(&mut screen, &overlay);

        // 2 body rows + 2 borders, bottom border on row 22 (status bar = 23).
        assert!(row_text(&screen, 19).contains("2 config warning(s)"));
        assert!(row_text(&screen, 20).contains("trailing comma"));
        assert!(row_text(&screen, 21).contains("config.toml"));
        assert!(row_text(&screen, 22).contains("press any key to dismiss"));
        assert_eq!(row_text(&screen, 23).trim(), "", "status bar row untouched");
    }

    #[test]
    fn long_warnings_wrap_instead_of_truncating() {
        let mut overlay = WarningsOverlay::default();
        let long = format!("bindings.json: {}", "x".repeat(100));
        overlay.set_startup_warnings(vec![long]);

        let mut screen = Screen::new(80, 24);
        draw_warnings(&mut screen, &overlay);

        // 80-wide screen → box 76 wide → 72 inner columns: 115 chars wrap to
        // two body rows (20/21), bottom border lands on row 22.
        assert!(row_text(&screen, 20).contains("bindings.json:"));
        assert!(row_text(&screen, 21).contains("xxx"));
        assert!(row_text(&screen, 22).contains("press any key"));
    }

    #[test]
    fn fullwidth_text_stays_inside_the_box_border() {
        let mut overlay = WarningsOverlay::default();
        overlay.set_startup_warnings(vec![format!("設定{}", "全".repeat(60))]);

        let mut screen = Screen::new(40, 24);
        draw_warnings(&mut screen, &overlay);

        // Box is 36 wide starting at x=2: the right border must survive on
        // every body row (display-width wrapping, not char-count clipping).
        for y in 20..=21 {
            let row = row_text(&screen, y);
            assert_eq!(
                row.trim_end().chars().last(),
                Some('│'),
                "right border intact on row {y}: {row:?}"
            );
        }
    }

    #[test]
    fn overflowing_rows_end_in_an_explicit_more_marker() {
        let lines: Vec<String> = (0..30).map(|i| format!("warning {i}")).collect();
        let rows = body_rows(&lines, 72, 20);

        assert_eq!(rows.len(), 20);
        assert_eq!(rows[18], "warning 18");
        assert_eq!(rows[19], "… 11 more line(s)");
    }

    #[test]
    fn multiline_warnings_split_before_wrapping() {
        // toml parse errors carry a multi-line caret excerpt; each source
        // line must become its own row instead of leaking `\n` into one.
        let lines = vec!["config.toml: expected value\n  |\n3 | wrap =\n  |        ^".to_string()];
        let rows = body_rows(&lines, 72, 20);

        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], "config.toml: expected value");
        assert_eq!(rows[2], "3 | wrap =");
    }

    #[test]
    fn narrowest_drawable_screen_keeps_borders_intact() {
        let mut overlay = WarningsOverlay::default();
        // Enough rows to overflow max_rows=1 on a 6-row screen, forcing the
        // "… N more" marker into a 4-column body (12-wide screen, box 8).
        overlay.set_startup_warnings((0..5).map(|i| format!("warning {i}")).collect());

        let mut screen = Screen::new(12, 6);
        draw_warnings(&mut screen, &overlay);

        // Geometry on 12x6: box x 2..=9, rows 2..=4 (status bar = row 5),
        // single body row 3 with 4 inner columns at x 4..=7.
        let body_row = row_text(&screen, 3);
        assert!(
            body_row.contains('…'),
            "shortened marker fits the 4-column body: {body_row:?}"
        );
        assert_eq!(
            screen.cell(9, 3).unwrap().symbol,
            "│",
            "right border intact next to the marker"
        );
        assert_eq!(
            screen.cell(10, 3).unwrap().symbol,
            " ",
            "nothing drawn past the border"
        );
    }

    #[test]
    fn wrap_row_splits_by_display_width_and_keeps_every_grapheme() {
        // 10 fullwidth chars = 20 columns → wraps at 8 columns into 4 rows.
        let rows = wrap_row(&"あ".repeat(10), 8);
        assert_eq!(rows, vec!["ああああ", "ああああ", "ああ"]);
        assert_eq!(rows.concat(), "あ".repeat(10), "no grapheme dropped");

        assert_eq!(wrap_row("", 8), vec![""], "empty line keeps one row");
    }

    #[test]
    fn no_warnings_keeps_the_panel_hidden_and_reopen_reports_it() {
        let mut overlay = WarningsOverlay::default();
        overlay.set_startup_warnings(Vec::new());
        assert!(!overlay.visible);
        assert!(!overlay.reopen(), "nothing to show");

        let mut screen = Screen::new(80, 24);
        draw_warnings(&mut screen, &overlay);
        assert_eq!(row_text(&screen, 22).trim(), "");
    }

    #[test]
    fn undrawable_screen_neither_draws_nor_blocks_input() {
        let mut overlay = WarningsOverlay::default();
        overlay.set_startup_warnings(vec!["broken".to_string()]);

        assert!(overlay.blocks_input(80, 24));
        assert!(
            !overlay.blocks_input(80, 5),
            "an invisible panel must not swallow keys"
        );
        assert!(!overlay.blocks_input(11, 24));

        let mut tiny = Screen::new(80, 5);
        draw_warnings(&mut tiny, &overlay); // must not panic or underflow

        overlay.close();
        assert!(!overlay.blocks_input(80, 24));
        assert!(overlay.reopen(), "warnings are retained after dismissal");
    }
}
