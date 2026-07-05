//! ANSI serialization for [`Screen`](crate::ui::Screen).
//!
//! The renderer diffs at row granularity. That is intentionally coarser than a
//! cell diff, but keeps output predictable and easy to test while avoiding full
//! screen redraws over SSH.

use std::io::{self, Write};

use super::{Cell, Screen, Style};

const CSI: &str = "\x1b[";

pub fn render_full(next: &Screen, out: &mut impl Write) -> io::Result<()> {
    write!(out, "{CSI}2J")?;
    for y in 0..next.height() {
        render_row(next, y, out)?;
    }
    render_cursor(next, out)
}

pub fn render_diff(prev: &Screen, next: &Screen, out: &mut impl Write) -> io::Result<()> {
    if prev.size() != next.size() {
        for y in 0..next.height() {
            render_row(next, y, out)?;
        }
        return render_cursor(next, out);
    }

    for y in 0..next.height() {
        if prev.row(y) != next.row(y) {
            render_row(next, y, out)?;
        }
    }
    render_cursor(next, out)
}

fn render_row(screen: &Screen, y: u16, out: &mut impl Write) -> io::Result<()> {
    write!(out, "{CSI}{};1H", y + 1)?;

    let row = screen.row(y).expect("row index is bounded by caller");
    let mut active_style = Style::default();
    write_sgr_if_needed(Style::default(), &mut active_style, out)?;

    let mut x = 0;
    while x < row.len() {
        let cell = &row[x];
        if cell.is_continuation() {
            x += 1;
            continue;
        }

        write_sgr_if_needed(cell.style, &mut active_style, out)?;
        write_cell(cell, out)?;
        x += usize::from(cell.width.max(1));
    }

    write!(out, "{CSI}0m{CSI}K")
}

fn write_cell(cell: &Cell, out: &mut impl Write) -> io::Result<()> {
    if cell.symbol.is_empty() {
        out.write_all(b" ")
    } else {
        out.write_all(cell.symbol.as_bytes())
    }
}

fn write_sgr_if_needed(
    next: Style,
    active_style: &mut Style,
    out: &mut impl Write,
) -> io::Result<()> {
    if next == *active_style {
        return Ok(());
    }

    if *active_style != Style::default() {
        write!(out, "{CSI}0m")?;
    }

    match (next.reverse, next.dim) {
        (false, false) => {}
        (true, false) => write!(out, "{CSI}7m")?,
        (false, true) => write!(out, "{CSI}2m")?,
        (true, true) => write!(out, "{CSI}7m{CSI}2m")?,
    }
    *active_style = next;
    Ok(())
}

fn render_cursor(screen: &Screen, out: &mut impl Write) -> io::Result<()> {
    if let Some((x, y)) = screen.cursor {
        write!(out, "{CSI}{};{}H{CSI}?25h", y + 1, x + 1)
    } else {
        write!(out, "{CSI}?25l")
    }
}

#[cfg(test)]
mod tests {
    use super::{render_diff, render_full};
    use crate::ui::{Screen, Style};

    #[test]
    fn render_full_clears_screen_and_renders_all_rows() {
        let mut screen = Screen::new(3, 2);
        screen.put_str(0, 0, "abc", Style::default());
        screen.put_str(0, 1, "def", Style::default());
        let out = render_to_string(|buffer| render_full(&screen, buffer));

        assert!(out.contains("\x1b[2J"));
        assert!(out.contains("\x1b[1;1Habc"));
        assert!(out.contains("\x1b[2;1Hdef"));
    }

    #[test]
    fn render_diff_redraws_only_changed_rows() {
        let mut prev = Screen::new(3, 3);
        prev.put_str(0, 0, "aaa", Style::default());
        prev.put_str(0, 1, "bbb", Style::default());
        prev.put_str(0, 2, "ccc", Style::default());
        let mut next = prev.clone();
        next.put_str(0, 1, "BxB", Style::default());

        let out = render_to_string(|buffer| render_diff(&prev, &next, buffer));

        assert!(!out.contains("\x1b[1;1H"));
        assert!(out.contains("\x1b[2;1H"));
        assert!(!out.contains("\x1b[3;1H"));
    }

    #[test]
    fn unchanged_render_diff_only_emits_cursor_control() {
        let screen = Screen::new(2, 1);
        let out = render_to_string(|buffer| render_diff(&screen, &screen, buffer));

        assert_eq!(out, "\x1b[?25l");
    }

    #[test]
    fn reverse_style_run_emits_sgr_once_per_run() {
        let mut screen = Screen::new(4, 1);
        screen.put_str(
            0,
            0,
            "ab",
            Style {
                reverse: true,
                dim: false,
            },
        );
        screen.put_str(2, 0, "cd", Style::default());

        let out = render_to_string(|buffer| render_full(&screen, buffer));

        assert_eq!(out.matches("\x1b[7m").count(), 1);
        assert_eq!(out.matches("\x1b[0m").count(), 2);
        assert!(out.contains("\x1b[7mab\x1b[0mcd\x1b[0m\x1b[K"));
    }

    #[test]
    fn switching_between_non_default_styles_resets_previous_attributes() {
        let mut screen = Screen::new(2, 1);
        screen.put_str(
            0,
            0,
            "a",
            Style {
                reverse: false,
                dim: true,
            },
        );
        screen.put_str(
            1,
            0,
            "b",
            Style {
                reverse: true,
                dim: false,
            },
        );

        let out = render_to_string(|buffer| render_full(&screen, buffer));

        assert!(out.contains("\x1b[2ma\x1b[0m\x1b[7mb\x1b[0m\x1b[K"));
    }

    #[test]
    fn cursor_some_moves_and_shows_cursor() {
        let mut screen = Screen::new(2, 2);
        screen.set_cursor(1, 0);

        let out = render_to_string(|buffer| render_diff(&screen, &screen, buffer));

        assert_eq!(out, "\x1b[1;2H\x1b[?25h");
    }

    #[test]
    fn cursor_none_hides_cursor() {
        let screen = Screen::new(2, 2);
        let out = render_to_string(|buffer| render_diff(&screen, &screen, buffer));

        assert_eq!(out, "\x1b[?25l");
    }

    #[test]
    fn size_change_redraws_all_next_rows() {
        let prev = Screen::new(2, 1);
        let next = Screen::new(2, 2);

        let out = render_to_string(|buffer| render_diff(&prev, &next, buffer));

        assert!(out.contains("\x1b[1;1H"));
        assert!(out.contains("\x1b[2;1H"));
    }

    fn render_to_string(render: impl FnOnce(&mut Vec<u8>) -> std::io::Result<()>) -> String {
        let mut buffer = Vec::new();
        render(&mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }
}
