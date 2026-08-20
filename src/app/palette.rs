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
    /// The shown binding cannot arrive in this terminal (quirk-intercepted
    /// chord, or the binding was disabled by `keymap verify`). The action
    /// itself still runs from the palette; only the key column is dimmed.
    pub undelivered: bool,
    /// A verify-disabled binding that lost to a live one on the same action
    /// (e.g. an imported Cmd chord removed while the default still works).
    /// Shown as a dim note so the user's muscle-memory key never vanishes
    /// silently.
    pub lost: Option<String>,
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

/// `disabled_bindings` are the bindings `keymap verify` measured as
/// undeliverable and removed from the resolver. `blocked_chords` are every
/// quirk-intercepted trigger (checked against each chord of a sequence);
/// `harmless_single_chords` exempts single-chord bindings whose interception
/// still produces the bound behavior (e.g. Cmd+V arriving as a paste). All
/// surface as dim + `✗ undelivered` so a missing key never vanishes silently
/// (SPEC-0003).
pub fn filter_actions(
    query: &str,
    bindings: &[Binding],
    disabled_bindings: &[Binding],
    blocked_chords: &[KeyEvent],
    harmless_single_chords: &[KeyEvent],
) -> Vec<PaletteItem> {
    let needle = query.to_ascii_lowercase();
    EditorAction::ALL
        .iter()
        .copied()
        .filter(|action| action.as_str().to_ascii_lowercase().contains(&needle))
        .map(|action| {
            // A live binding wins the key column; a verify-disabled one then
            // rides along as the `lost` note. With no live binding the
            // disabled one takes the column itself (as a dimmed explanation,
            // not a working shortcut).
            let live = best_binding_for(action, bindings);
            let disabled = best_binding_for(action, disabled_bindings);
            let (binding, undelivered, lost) = match live {
                Some(live) => {
                    let blocked = live.keys.iter().any(|key| blocked_chords.contains(key))
                        && !(live.keys.len() == 1
                            && harmless_single_chords.contains(&live.keys[0]));
                    (Some(live), blocked, disabled)
                }
                None => (disabled, disabled.is_some(), None),
            };
            PaletteItem {
                action,
                binding: binding.map(|binding| format_key_sequence(&binding.keys)),
                undelivered,
                lost: lost.map(|binding| format_key_sequence(&binding.keys)),
            }
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
        // `head` draws in the row style; `tail` (an undeliverable key column,
        // or the lost-binding note) is dimmed. The action name itself never
        // dims — it still runs from the palette; it is the key that this
        // terminal will not deliver.
        let (head, tail) = match (&item.binding, &item.lost) {
            (Some(binding), _) if item.undelivered => (
                format!("{:<32}", item.action.as_str()),
                format!("{binding}  ✗ undelivered"),
            ),
            (Some(binding), Some(lost)) => (
                format!("{:<32}{binding}", item.action.as_str()),
                format!("  ✗ {lost} undelivered"),
            ),
            (Some(binding), None) => (
                format!("{:<32}{binding}", item.action.as_str()),
                String::new(),
            ),
            (None, _) => (item.action.as_str().to_string(), String::new()),
        };
        let clipped = clip_to_width(&format!("{head}{tail}"), inner_width);
        let is_selected = offset + row == palette.selected;
        let style = if is_selected { reverse } else { normal };
        // Pad the selected row to full width so the highlight forms a bar
        // (selection keeps the full reverse bar for visibility, so the dim
        // overdraw below skips selected rows).
        let padded = format!("{:<width$}", clipped, width = inner_width);
        let y = box_top + 2 + row as u16;
        screen.put_str(box_x + 2, y, &padded, style);
        let dim_from = head.chars().count();
        if !tail.is_empty() && !is_selected && dim_from < inner_width {
            let dim_tail: String = padded.chars().skip(dim_from).collect();
            screen.put_str(box_x + 2 + dim_from as u16, y, &dim_tail, dim);
        }
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
        let lower = filter_actions("sav", &[], &[], &[], &[])
            .into_iter()
            .map(|item| item.action.as_str())
            .collect::<Vec<_>>();
        assert!(lower.contains(&"file.save"));
        assert!(lower.contains(&"file.saveAs"));

        let upper = filter_actions("SAV", &[], &[], &[], &[])
            .into_iter()
            .map(|item| item.action.as_str())
            .collect::<Vec<_>>();
        assert_eq!(lower, upper);
    }

    #[test]
    fn palette_marks_undelivered_bindings_from_both_sources() {
        use crate::keymap::{Binding, EditorAction, Source, parse_key_sequence};

        let binding = |keys: &str, action| Binding {
            keys: parse_key_sequence(keys).unwrap(),
            action,
            when: None,
            source: Source::User,
        };
        // alt chords keep the expected labels platform-independent (Super
        // renders as "Cmd" on macOS and "Super" elsewhere).
        let live = [
            binding("ctrl+s", EditorAction::FileSave),
            binding("alt+z", EditorAction::EditUndo),
            binding("ctrl+k alt+x", EditorAction::EditRedo),
            binding("alt+v", EditorAction::EditPaste),
        ];
        let disabled = [
            binding("alt+c", EditorAction::EditCopy),
            binding("alt+s", EditorAction::FileSave),
        ];
        let blocked = parse_key_sequence("alt+z alt+x alt+v").unwrap();
        let harmless = parse_key_sequence("alt+v").unwrap();

        let items = filter_actions("", &live, &disabled, &blocked, &harmless);
        let find = |name: &str| items.iter().find(|i| i.action.as_str() == name).unwrap();

        let save = find("file.save");
        assert_eq!(save.binding.as_deref(), Some("Ctrl+S"));
        assert!(!save.undelivered, "deliverable binding stays normal");
        assert_eq!(
            save.lost.as_deref(),
            Some("Alt+S"),
            "a verify-disabled key rides along even when a live one exists"
        );

        let undo = find("edit.undo");
        assert!(undo.undelivered, "quirk-intercepted chord is marked");

        let redo = find("edit.redo");
        assert!(
            redo.undelivered,
            "a blocked chord inside a sequence marks the whole binding"
        );

        let paste = find("edit.paste");
        assert!(
            !paste.undelivered,
            "harmless single-chord interception is exempt"
        );

        let copy = find("edit.copy");
        assert_eq!(
            copy.binding.as_deref(),
            Some("Alt+C"),
            "verify-disabled binding still shows instead of vanishing"
        );
        assert!(copy.undelivered);

        let quit = find("app.quit");
        assert_eq!(quit.binding, None);
        assert!(!quit.undelivered, "unbound action carries no mark");
    }
}
