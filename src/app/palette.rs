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

    pub fn backspace(&mut self) {
        self.query.pop();
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

pub fn draw_palette(screen: &mut Screen, palette: &CommandPalette, items: &[PaletteItem]) {
    if !palette.visible || screen.height() < 3 || screen.width() < 10 {
        return;
    }
    let width = screen.width().saturating_sub(4);
    let max_items = usize::from(screen.height().saturating_sub(4)).min(8);
    let x = 2;
    let y = 1;
    let reverse = Style {
        reverse: true,
        dim: false,
    };
    let normal = Style::default();

    screen.put_str(x, y, &format!("> {}", palette.query), reverse);
    for row in 0..max_items {
        let Some(item) = items.get(row) else {
            break;
        };
        let label = match &item.binding {
            Some(binding) => format!("{}    {}", item.action.as_str(), binding),
            None => item.action.as_str().to_string(),
        };
        let clipped = clip_to_width(&label, usize::from(width));
        screen.put_str(
            x,
            y + 1 + row as u16,
            &clipped,
            if row == palette.selected {
                reverse
            } else {
                normal
            },
        );
    }
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
    use super::filter_actions;

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
