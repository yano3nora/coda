//! Command palette state and filtering.

use crate::{
    input::KeyEvent,
    keymap::{Binding, EditorAction},
    ui::{Screen, Style},
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PaletteItem {
    pub action: EditorAction,
    pub binding: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CommandPalette {
    pub visible: bool,
    pub query: String,
    pub selected: usize,
}

impl CommandPalette {
    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.query.clear();
        self.selected = 0;
    }

    pub fn push_char(&mut self, character: char) {
        self.query.push(character);
        self.selected = 0;
    }

    pub fn push_text(&mut self, text: &str) {
        self.query.push_str(text);
        self.selected = 0;
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.selected = 0;
    }

    pub fn move_selection(&mut self, delta: isize, item_count: usize) {
        if item_count == 0 {
            self.selected = 0;
            return;
        }
        let current = self.selected.min(item_count - 1) as isize;
        self.selected = (current + delta).rem_euclid(item_count as isize) as usize;
    }

    pub fn selected_action(&self, items: &[PaletteItem]) -> Option<EditorAction> {
        items.get(self.selected).map(|item| item.action)
    }
}

pub fn filter_actions(query: &str, bindings: &[Binding]) -> Vec<PaletteItem> {
    let needle = query.to_ascii_lowercase();
    EditorAction::ALL
        .iter()
        .copied()
        .filter(|action| action.as_str().to_ascii_lowercase().contains(&needle))
        .map(|action| PaletteItem {
            action,
            binding: best_binding_for(action, bindings)
                .map(|binding| format_key_sequence(&binding.keys)),
        })
        .collect()
}

/// Returns the first visible item index so `selected` stays on screen.
///
/// Pure so the scroll window rule is unit-testable apart from drawing.
pub fn scroll_offset(selected: usize, item_count: usize, max_rows: usize) -> usize {
    if max_rows == 0 || item_count <= max_rows {
        return 0;
    }
    let max_offset = item_count - max_rows;
    selected.saturating_sub(max_rows - 1).min(max_offset)
}

pub fn draw_palette(screen: &mut Screen, palette: &CommandPalette, items: &[PaletteItem]) {
    if !palette.visible || screen.height() < 6 || screen.width() < 12 {
        return;
    }
    // Boxed modal: ╭ title ╮ / query / items / ╰ count ╯. The interior is
    // blanked so editor text underneath cannot bleed through between rows.
    let box_x = 2;
    let box_width = screen.width().saturating_sub(4);
    let inner_width = usize::from(box_width.saturating_sub(4));
    let max_items = usize::from(screen.height().saturating_sub(6)).clamp(1, 10);
    let shown = items.len().min(max_items);
    let box_top = 1;
    let box_height = (shown as u16) + 3; // top border + query + items + bottom border

    let reverse = Style {
        reverse: true,
        dim: false,
        fg: None,
    };
    let dim = Style {
        reverse: false,
        dim: true,
        fg: None,
    };
    let normal = Style::default();

    for row in 0..box_height {
        let y = box_top + row;
        let line = if row == 0 {
            frame_line("╭", "─", "╮", " Command Palette ", usize::from(box_width))
        } else if row == box_height - 1 {
            let count = format!(" {}/{} ", shown, items.len());
            frame_line("╰", "─", "╯", &count, usize::from(box_width))
        } else {
            format!("│{}│", " ".repeat(usize::from(box_width) - 2))
        };
        screen.put_str(box_x, y, &line, dim);
    }

    screen.put_str(
        box_x + 2,
        box_top + 1,
        &clip_to_width(&format!("> {}", palette.query), inner_width),
        normal,
    );

    let offset = scroll_offset(palette.selected, items.len(), max_items);
    for (row, item) in items.iter().skip(offset).take(max_items).enumerate() {
        let label = match &item.binding {
            Some(binding) => format!("{:<32}{}", item.action.as_str(), binding),
            None => item.action.as_str().to_string(),
        };
        let clipped = clip_to_width(&label, inner_width);
        let is_selected = offset + row == palette.selected;
        let style = if is_selected { reverse } else { normal };
        // Pad the selected row to full width so the highlight forms a bar.
        let padded = format!("{:<width$}", clipped, width = inner_width);
        screen.put_str(box_x + 2, box_top + 2 + row as u16, &padded, style);
    }
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

fn best_binding_for(action: EditorAction, bindings: &[Binding]) -> Option<&Binding> {
    bindings
        .iter()
        .enumerate()
        .filter(|(_, binding)| binding.action == action)
        .max_by_key(|(index, binding)| (binding.source.priority(), binding.term_count(), *index))
        .map(|(_, binding)| binding)
}

fn format_key_sequence(keys: &[KeyEvent]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn clip_to_width(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::{CommandPalette, filter_actions, scroll_offset};

    #[test]
    fn clear_query_removes_input_and_resets_selection() {
        let mut palette = CommandPalette {
            visible: true,
            query: "save".to_string(),
            selected: 3,
        };
        palette.clear_query();
        assert!(palette.query.is_empty());
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn scroll_offset_keeps_selection_visible() {
        let cases = [
            ("fits entirely", 5, 6, 10, 0),
            ("top of long list", 0, 30, 8, 0),
            ("selection at window edge", 7, 30, 8, 0),
            ("selection scrolls window", 12, 30, 8, 5),
            ("selection at end pins to max offset", 29, 30, 8, 22),
        ];
        for (name, selected, count, rows, expected) in cases {
            assert_eq!(scroll_offset(selected, count, rows), expected, "{name}");
        }
    }

    #[test]
    fn palette_filter_matches_case_insensitive_substrings() {
        let lower = filter_actions("sav", &[])
            .into_iter()
            .map(|item| item.action.as_str())
            .collect::<Vec<_>>();
        assert!(lower.contains(&"file.save"));
        assert!(lower.contains(&"file.saveAs"));

        let upper = filter_actions("SAV", &[])
            .into_iter()
            .map(|item| item.action.as_str())
            .collect::<Vec<_>>();
        assert_eq!(lower, upper);
    }
}
