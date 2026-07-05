//! Minimal built-in bindings for MVP editor operation.

use crate::keymap::{Binding, EditorAction, Source, parse_key_sequence};

const DEFAULTS: &[(&str, EditorAction, Option<&str>, Source)] = &[
    (
        "up",
        EditorAction::CursorUp,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "down",
        EditorAction::CursorDown,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "left",
        EditorAction::CursorLeft,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "right",
        EditorAction::CursorRight,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "home",
        EditorAction::CursorLineStart,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "end",
        EditorAction::CursorLineEnd,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "pageup",
        EditorAction::CursorPageUp,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "pagedown",
        EditorAction::CursorPageDown,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+left",
        EditorAction::CursorWordLeft,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+right",
        EditorAction::CursorWordRight,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "shift+up",
        EditorAction::SelectionUp,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "shift+down",
        EditorAction::SelectionDown,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "shift+left",
        EditorAction::SelectionLeft,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "shift+right",
        EditorAction::SelectionRight,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "shift+home",
        EditorAction::CursorLineStart,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "shift+end",
        EditorAction::CursorLineEnd,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "shift+pageup",
        EditorAction::SelectionUp,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "shift+pagedown",
        EditorAction::SelectionDown,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+shift+left",
        EditorAction::SelectionWordLeft,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+shift+right",
        EditorAction::SelectionWordRight,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "backspace",
        EditorAction::EditBackspace,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "delete",
        EditorAction::EditDelete,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "ctrl+backspace",
        EditorAction::EditDeleteWordLeft,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "ctrl+z",
        EditorAction::EditUndo,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "ctrl+shift+z",
        EditorAction::EditRedo,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "ctrl+a",
        EditorAction::SelectionAll,
        Some("textInputFocus"),
        Source::Default,
    ),
    ("ctrl+s", EditorAction::FileSave, None, Source::Default),
    ("ctrl+q", EditorAction::AppQuit, None, Source::Default),
    (
        "ctrl+space",
        EditorAction::PaletteOpen,
        None,
        Source::Rescue,
    ),
    (
        "escape",
        EditorAction::PaletteOpen,
        Some("commandPaletteVisible"),
        Source::Rescue,
    ),
];

pub fn bindings() -> Vec<Binding> {
    DEFAULTS
        .iter()
        .map(|(key, action, when, source)| {
            Binding::new(
                parse_key_sequence(key).expect("default binding keys are valid"),
                *action,
                when.map(|value| value.parse().expect("default when clause is valid")),
                *source,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::bindings;
    use crate::keymap::{EditorContext, ResolveResult, Resolver};

    #[test]
    fn all_default_bindings_load_into_resolver() {
        let resolver = Resolver::new(bindings());
        assert!(matches!(
            resolver.resolve(&["ctrl+s".parse().unwrap()], &EditorContext::default()),
            ResolveResult::Matched(_)
        ));
    }
}
