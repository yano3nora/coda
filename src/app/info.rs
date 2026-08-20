//! Startup info panel: config-breakage warnings and environment facts
//! (TASK-260820, extended by TASK-260820-environment-info-panel).
//!
//! The one-line status bar joins messages with `"; "` and truncates at the
//! right edge, so anything the user must *act on* could scroll out of view
//! unread (that is how the trailing comma incident stayed invisible). Panel
//! policy — the dividing line is "can the user act on it?", not where the
//! problem originates:
//!
//! - **Config section** (`AppConfig::warnings`: bindings.json / config.toml /
//!   state-file load issues, HOME unset): shown on every startup until fixed
//!   — nagging is correct while the user's own configuration is broken.
//! - **Environment section** (terminal interception report, or the honest
//!   "cannot query this terminal" notice): shown until *acknowledged*. A
//!   user who decides to keep their terminal's bindings must be able to say
//!   "I know" once; the panel then stays silent until the facts change
//!   (Ghostty update, edited terminal config, different terminal).
//! - **Any key or paste dismisses and is swallowed** (vim's hit-enter prompt
//!   convention) — except `F1`, which opens the palette on top per the
//!   always-available rescue rule. Dismissing acknowledges the environment
//!   section; `info.show` reopens the full panel at any time.
//! - **Input priority mirrors draw order** and **an undrawable panel never
//!   blocks input** — see `blocks_input`.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::inspector::frame_line;
use crate::ui::{Screen, Style};

#[derive(Debug, Clone, Default)]
pub struct InfoOverlay {
    pub visible: bool,
    config_lines: Vec<String>,
    env_lines: Vec<String>,
    env_acknowledged: bool,
    /// First visible wrapped row. Up/Down/PageUp/PageDown adjust it so every
    /// row can actually be read before the panel is dismissed — dismissal
    /// acknowledges the environment section, and acknowledging lines the
    /// user could never scroll to would be dishonest.
    scroll: usize,
}

impl InfoOverlay {
    /// Stores both sections and decides startup visibility: config problems
    /// always open the panel; environment facts only until acknowledged.
    /// Called once at startup, before the first draw.
    pub fn set_startup(
        &mut self,
        config_lines: Vec<String>,
        env_lines: Vec<String>,
        env_acknowledged: bool,
    ) {
        self.visible = !config_lines.is_empty() || (!env_lines.is_empty() && !env_acknowledged);
        self.config_lines = config_lines;
        self.env_lines = env_lines;
        self.env_acknowledged = env_acknowledged;
        self.scroll = 0;
    }

    /// Reopens the panel (`info.show`), acknowledged or not. Returns `false`
    /// when there is nothing to show, so the caller can report that instead.
    pub fn reopen(&mut self) -> bool {
        self.visible = !self.config_lines.is_empty() || !self.env_lines.is_empty();
        self.scroll = 0;
        self.visible
    }

    /// Scrolls by `delta` wrapped rows, clamped to the content. Needs the
    /// screen size because the wrap width and row budget are per-screen.
    pub fn scroll_by(&mut self, delta: isize, screen_width: u16, screen_height: u16) {
        if !can_draw(screen_width, screen_height) {
            return;
        }
        let (inner_width, max_rows) = geometry(screen_width, screen_height);
        let total = wrapped_rows(&self.lines(), inner_width).len();
        let max_scroll = total.saturating_sub(max_rows);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max_scroll);
    }

    /// Rows the panel shows per screenful — the PageUp/PageDown step.
    pub fn page_rows(&self, screen_width: u16, screen_height: u16) -> isize {
        if !can_draw(screen_width, screen_height) {
            return 0;
        }
        geometry(screen_width, screen_height).1 as isize
    }

    /// Closes the panel. Returns the environment lines exactly once — on the
    /// first dismissal of a not-yet-acknowledged environment section — so the
    /// caller can persist the acknowledgement; the overlay itself never does
    /// IO.
    pub fn dismiss(&mut self) -> Option<Vec<String>> {
        self.visible = false;
        if self.env_acknowledged || self.env_lines.is_empty() {
            return None;
        }
        self.env_acknowledged = true;
        Some(self.env_lines.clone())
    }

    /// Whether the panel should consume the next key/paste/mouse event.
    /// Deliberately identical to the draw guard: an overlay that is not on
    /// screen must never swallow input.
    pub fn blocks_input(&self, screen_width: u16, screen_height: u16) -> bool {
        self.visible && can_draw(screen_width, screen_height)
    }

    /// Config section first (it needs fixing), then the environment section,
    /// separated by a blank row when both are present.
    fn lines(&self) -> Vec<&str> {
        let mut lines: Vec<&str> = self.config_lines.iter().map(String::as_str).collect();
        if !lines.is_empty() && !self.env_lines.is_empty() {
            lines.push("");
        }
        lines.extend(self.env_lines.iter().map(String::as_str));
        lines
    }
}

fn can_draw(screen_width: u16, screen_height: u16) -> bool {
    screen_height >= 6 && screen_width >= 12
}

/// Wrap width and row budget for a drawable screen: box is inset 2 columns
/// each side with 2 columns of inner padding, and the body leaves the tab
/// bar plus at least one editor row visible above the panel.
fn geometry(screen_width: u16, screen_height: u16) -> (usize, usize) {
    let box_width = screen_width.saturating_sub(4);
    let inner_width = usize::from(box_width.saturating_sub(4)).max(1);
    let max_rows = usize::from(screen_height.saturating_sub(5)).clamp(1, 20);
    (inner_width, max_rows)
}

/// Bottom-anchored boxed panel, sitting directly above the status bar so it
/// reads as an expansion of it. Draws nothing when hidden or when the screen
/// is too small (`blocks_input` is false in exactly the same cases).
pub fn draw_info(screen: &mut Screen, overlay: &InfoOverlay) {
    if !overlay.visible || !can_draw(screen.width(), screen.height()) {
        return;
    }

    let box_x = 2;
    let box_width = screen.width().saturating_sub(4);
    let (inner_width, max_rows) = geometry(screen.width(), screen.height());

    let all_rows = wrapped_rows(&overlay.lines(), inner_width);
    let overflowing = all_rows.len() > max_rows;
    let body = visible_rows(all_rows, overlay.scroll, max_rows);
    let box_height = body.len() as u16 + 2; // top border + body rows + bottom border
    // Directly above the status bar (the screen's last row).
    let box_top = screen.height().saturating_sub(1 + box_height);

    let dim = Style {
        reverse: false,
        dim: true,
        fg: None,
    };
    let normal = Style::default();

    let footer = if overflowing {
        " ↑/↓ scroll — any other key dismisses (info.show reopens) "
    } else {
        " press any key to dismiss — info.show reopens "
    };
    for row in 0..box_height {
        let y = box_top + row;
        let line = if row == 0 {
            frame_line("╭", "─", "╮", " coda info ", usize::from(box_width))
        } else if row == box_height - 1 {
            frame_line("╰", "─", "╯", footer, usize::from(box_width))
        } else {
            format!("│{}│", " ".repeat(usize::from(box_width).saturating_sub(2)))
        };
        screen.put_str(box_x, y, &line, dim);
    }

    for (row, line) in body.iter().enumerate() {
        let clipped = wrap_row(line, inner_width).swap_remove(0);
        screen.put_str(box_x + 2, box_top + 1 + row as u16, &clipped, normal);
    }
}

/// Lines split on embedded newlines (toml parse errors carry a multi-line
/// caret excerpt) and wrapped to `width` display columns — an unsplit `\n`
/// (display width 0, rendered as a 1-col sanitized space) would silently
/// push text past the box border.
fn wrapped_rows(lines: &[&str], width: usize) -> Vec<String> {
    lines
        .iter()
        .flat_map(|line| line.split('\n'))
        .flat_map(|line| wrap_row(line, width))
        .collect()
}

/// The `max_rows` window at `scroll`, with explicit edge markers — hidden
/// content must never be silent (the status bar's silent right-edge cut is
/// the exact failure this panel replaces), and scrolling must be able to
/// reach every row. On a 1–2 row budget the markers would occupy the whole
/// window and hide content forever, so the raw window wins there.
fn visible_rows(all: Vec<String>, scroll: usize, max_rows: usize) -> Vec<String> {
    let total = all.len();
    if total <= max_rows {
        return all;
    }
    let scroll = scroll.min(total - max_rows);
    let mut shown = all[scroll..scroll + max_rows].to_vec();
    if max_rows >= 3 {
        if scroll > 0 {
            shown[0] = format!("… {} line(s) above", scroll + 1);
        }
        let below = total - (scroll + max_rows);
        if below > 0 {
            let last = shown.len() - 1;
            shown[last] = format!("… {} more line(s)", below + 1);
        }
    }
    shown
}

/// Grapheme-safe wrap by display width. Breaks anywhere rather than at word
/// boundaries: the text is dominated by paths and config fragments with no
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
    use super::{InfoOverlay, draw_info, visible_rows, wrap_row, wrapped_rows};
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

    fn borrowed(lines: &[String]) -> Vec<&str> {
        lines.iter().map(String::as_str).collect()
    }

    #[test]
    fn panel_lists_both_sections_above_the_status_bar() {
        let mut overlay = InfoOverlay::default();
        overlay.set_startup(
            vec!["bindings.json: invalid bindings.json: trailing comma".to_string()],
            vec!["Ghostty intercepts 1 binding(s):".to_string()],
            false,
        );
        assert!(overlay.visible, "startup problems open the panel");

        let mut screen = Screen::new(80, 24);
        draw_info(&mut screen, &overlay);

        // 3 body rows (config, blank separator, env) + 2 borders → box top at
        // row 18, bottom border on row 22 (status bar = 23, untouched).
        assert!(row_text(&screen, 18).contains("coda info"));
        assert!(row_text(&screen, 19).contains("trailing comma"));
        assert!(
            row_text(&screen, 20).chars().all(|c| c == ' ' || c == '│'),
            "blank separator row between the sections"
        );
        assert!(row_text(&screen, 21).contains("Ghostty intercepts"));
        assert!(row_text(&screen, 22).contains("press any key to dismiss"));
        assert_eq!(row_text(&screen, 23).trim(), "", "status bar row untouched");
    }

    #[test]
    fn config_problems_always_open_even_when_environment_is_acknowledged() {
        let mut overlay = InfoOverlay::default();
        overlay.set_startup(
            vec!["config.toml: broken".to_string()],
            vec!["terminal: cannot query".to_string()],
            true,
        );
        assert!(overlay.visible, "config problems nag until fixed");
    }

    #[test]
    fn acknowledged_environment_alone_stays_silent_but_info_show_reopens() {
        let mut overlay = InfoOverlay::default();
        overlay.set_startup(Vec::new(), vec!["terminal: cannot query".to_string()], true);
        assert!(!overlay.visible, "acknowledged facts do not nag");

        assert!(overlay.reopen(), "info.show shows the full panel anyway");
        assert_eq!(
            overlay.dismiss(),
            None,
            "re-dismissing an acknowledged section persists nothing"
        );
    }

    #[test]
    fn first_dismissal_yields_the_environment_lines_exactly_once() {
        let mut overlay = InfoOverlay::default();
        let env = vec!["Ghostty intercepts 2 binding(s):".to_string()];
        overlay.set_startup(Vec::new(), env.clone(), false);
        assert!(overlay.visible);

        assert_eq!(overlay.dismiss(), Some(env), "first dismissal acknowledges");
        assert!(!overlay.visible);
        assert!(overlay.reopen());
        assert_eq!(overlay.dismiss(), None, "second dismissal is a no-op");
    }

    #[test]
    fn empty_sections_keep_the_panel_hidden_and_reopen_reports_it() {
        let mut overlay = InfoOverlay::default();
        overlay.set_startup(Vec::new(), Vec::new(), false);
        assert!(!overlay.visible);
        assert!(!overlay.reopen(), "nothing to show");

        let mut screen = Screen::new(80, 24);
        draw_info(&mut screen, &overlay);
        assert_eq!(row_text(&screen, 22).trim(), "");
    }

    #[test]
    fn long_lines_wrap_instead_of_truncating() {
        let mut overlay = InfoOverlay::default();
        let long = format!("bindings.json: {}", "x".repeat(100));
        overlay.set_startup(vec![long], Vec::new(), false);

        let mut screen = Screen::new(80, 24);
        draw_info(&mut screen, &overlay);

        // 80-wide screen → box 76 wide → 72 inner columns: 115 chars wrap to
        // two body rows (20/21), bottom border lands on row 22.
        assert!(row_text(&screen, 20).contains("bindings.json:"));
        assert!(row_text(&screen, 21).contains("xxx"));
        assert!(row_text(&screen, 22).contains("press any key"));
    }

    #[test]
    fn fullwidth_text_stays_inside_the_box_border() {
        let mut overlay = InfoOverlay::default();
        overlay.set_startup(vec![format!("設定{}", "全".repeat(60))], Vec::new(), false);

        let mut screen = Screen::new(40, 24);
        draw_info(&mut screen, &overlay);

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
        let rows = visible_rows(wrapped_rows(&borrowed(&lines), 72), 0, 20);

        assert_eq!(rows.len(), 20);
        assert_eq!(rows[18], "warning 18");
        assert_eq!(rows[19], "… 11 more line(s)");
    }

    #[test]
    fn scrolling_reaches_every_row_with_edge_markers() {
        let lines: Vec<String> = (0..30).map(|i| format!("warning {i}")).collect();
        let all = wrapped_rows(&borrowed(&lines), 72);

        // Scrolled into the middle: both edges are marked and the window
        // moved (scroll=5 shows rows 5..25, minus the two marker slots).
        let mid = visible_rows(all.clone(), 5, 20);
        assert_eq!(mid[0], "… 6 line(s) above");
        assert_eq!(mid[1], "warning 6");
        assert_eq!(mid[19], "… 6 more line(s)");

        // Fully scrolled (clamped even when asked for more): the true last
        // row is readable — acknowledgement must never cover unreachable
        // lines.
        let end = visible_rows(all, 999, 20);
        assert_eq!(end[19], "warning 29", "last row readable at max scroll");
        assert_eq!(end[0], "… 11 line(s) above");
    }

    #[test]
    fn tiny_row_budget_skips_markers_so_content_stays_reachable() {
        let lines: Vec<String> = (0..4).map(|i| format!("w{i}")).collect();
        let all = wrapped_rows(&borrowed(&lines), 72);

        // max_rows=1: markers would occupy the whole window, so raw rows win.
        assert_eq!(visible_rows(all.clone(), 0, 1), vec!["w0"]);
        assert_eq!(visible_rows(all, 3, 1), vec!["w3"]);
    }

    #[test]
    fn multiline_warnings_split_before_wrapping() {
        // toml parse errors carry a multi-line caret excerpt; each source
        // line must become its own row instead of leaking `\n` into one.
        let lines = vec!["config.toml: expected value\n  |\n3 | wrap =\n  |        ^".to_string()];
        let rows = wrapped_rows(&borrowed(&lines), 72);

        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], "config.toml: expected value");
        assert_eq!(rows[2], "3 | wrap =");
    }

    #[test]
    fn narrowest_drawable_screen_keeps_borders_intact() {
        let mut overlay = InfoOverlay::default();
        // Enough rows to overflow max_rows=1 on a 6-row screen, forcing the
        // "… N more" marker into a 4-column body (12-wide screen, box 8).
        overlay.set_startup(
            (0..5).map(|i| format!("warning {i}")).collect(),
            Vec::new(),
            false,
        );

        let mut screen = Screen::new(12, 6);
        draw_info(&mut screen, &overlay);

        // Geometry on 12x6: box x 2..=9, rows 2..=4 (status bar = row 5),
        // single body row 3 with 4 inner columns at x 4..=7 (max_rows=1 →
        // raw window, no markers; scrolling reaches the rest).
        let body_row = row_text(&screen, 3);
        assert!(
            body_row.contains("warn"),
            "first wrapped row fits the 4-column body: {body_row:?}"
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
        // 10 fullwidth chars = 20 columns → wraps at 8 columns into 3 rows.
        let rows = wrap_row(&"あ".repeat(10), 8);
        assert_eq!(rows, vec!["ああああ", "ああああ", "ああ"]);
        assert_eq!(rows.concat(), "あ".repeat(10), "no grapheme dropped");

        assert_eq!(wrap_row("", 8), vec![""], "empty line keeps one row");
    }

    #[test]
    fn undrawable_screen_neither_draws_nor_blocks_input() {
        let mut overlay = InfoOverlay::default();
        overlay.set_startup(vec!["broken".to_string()], Vec::new(), false);

        assert!(overlay.blocks_input(80, 24));
        assert!(
            !overlay.blocks_input(80, 5),
            "an invisible panel must not swallow keys"
        );
        assert!(!overlay.blocks_input(11, 24));

        let mut tiny = Screen::new(80, 5);
        draw_info(&mut tiny, &overlay); // must not panic or underflow
    }
}
