//! Built-in bindings for rescue access and OS-standard text editing.
//!
//! The default layer (`Source::Default`) implements the *host OS* text
//! editing convention rather than one arbitrary convention for every
//! platform. macOS terminals (e.g. Ghostty) translate `Cmd+Left/Right` and
//! `Opt+Left/Right` into the emacs-style keys (`Ctrl+A/E`, `Meta+B/F`) that
//! macOS guarantees across all native text fields, so the default table must
//! resolve those keys the same way macOS does, or terminal-translated input
//! silently misbehaves with zero terminal configuration involved. See
//! ADR-0011 for the full rationale.
//!
//! Bindings are split into a `COMMON` table (identical across platforms) and
//! per-platform tables (`MACOS`, `OTHER`) that are merged by
//! [`bindings_for`]. `bindings()` resolves the current platform at compile
//! time via `cfg!(target_os = "macos")`.

use crate::keymap::{Binding, EditorAction, Source, parse_key_sequence};

type RawBinding = (&'static str, EditorAction, Option<&'static str>, Source);

/// Host OS this binary is compiled for. Only affects which entries are
/// merged into the default binding table (ADR-0011); it does not affect
/// user/imported layers.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Platform {
    MacOs,
    Other,
}

impl Platform {
    /// Resolves the current platform at compile time. `Windows` is folded
    /// into `Other` for now (its convention already matches `OTHER`); a
    /// dedicated variant can be added later without touching `COMMON`.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Other
        }
    }
}

/// Bindings shared by every platform (cursor/selection/clipboard/etc. that
/// do not depend on host OS text-editing convention).
const COMMON: &[RawBinding] = &[
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
        "cmd+left",
        EditorAction::CursorLineStart,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "cmd+right",
        EditorAction::CursorLineEnd,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "cmd+up",
        EditorAction::CursorBufferStart,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "cmd+down",
        EditorAction::CursorBufferEnd,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "alt+left",
        EditorAction::CursorWordLeft,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "alt+right",
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
        "cmd+shift+left",
        EditorAction::SelectionLineStart,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "cmd+shift+right",
        EditorAction::SelectionLineEnd,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "cmd+shift+up",
        EditorAction::SelectionBufferStart,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "cmd+shift+down",
        EditorAction::SelectionBufferEnd,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "alt+shift+left",
        EditorAction::SelectionWordLeft,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "alt+shift+right",
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
        "alt+backspace",
        EditorAction::EditDeleteWordLeft,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "cmd+backspace",
        EditorAction::EditDeleteToLineStart,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "alt+up",
        EditorAction::EditMoveLinesUp,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "alt+down",
        EditorAction::EditMoveLinesDown,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "cmd+enter",
        EditorAction::EditInsertLineAfter,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "cmd+shift+enter",
        EditorAction::EditInsertLineBefore,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "ctrl+c",
        EditorAction::EditCopy,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "ctrl+x",
        EditorAction::EditCut,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "ctrl+v",
        EditorAction::EditPaste,
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
    ("ctrl+s", EditorAction::FileSave, None, Source::Default),
    ("ctrl+tab", EditorAction::BufferNext, None, Source::Default),
    (
        "ctrl+shift+tab",
        EditorAction::BufferPrevious,
        None,
        Source::Default,
    ),
    ("ctrl+w", EditorAction::BufferClose, None, Source::Default),
    ("cmd+w", EditorAction::BufferClose, None, Source::Default),
    ("cmd+f", EditorAction::SearchOpen, None, Source::Default),
    (
        "cmd+alt+f",
        EditorAction::ReplaceOpen,
        None,
        Source::Default,
    ),
    ("cmd+g", EditorAction::SearchNext, None, Source::Default),
    (
        "cmd+shift+g",
        EditorAction::SearchPrevious,
        None,
        Source::Default,
    ),
    ("f3", EditorAction::SearchNext, None, Source::Default),
    (
        "shift+f3",
        EditorAction::SearchPrevious,
        None,
        Source::Default,
    ),
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

/// macOS-only default bindings: the emacs-style keys macOS guarantees in
/// every native text field (`Ctrl+A/E/N/P/F/B`, `Meta+B/F`) plus `Cmd+A` for
/// select-all. `ctrl+a` intentionally resolves to `CursorLineStart` here
/// (not select-all) because that is the macOS convention terminals
/// translate `Cmd+Left` into (ADR-0011).
const MACOS: &[RawBinding] = &[
    (
        "ctrl+a",
        EditorAction::CursorLineStart,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+e",
        EditorAction::CursorLineEnd,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+n",
        EditorAction::CursorDown,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+p",
        EditorAction::CursorUp,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+f",
        EditorAction::CursorRight,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+b",
        EditorAction::CursorLeft,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "ctrl+d",
        EditorAction::EditDelete,
        Some("textInputFocus"),
        Source::Default,
    ),
    (
        "alt+b",
        EditorAction::CursorWordLeft,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "alt+f",
        EditorAction::CursorWordRight,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "alt+shift+b",
        EditorAction::SelectionWordLeft,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "alt+shift+f",
        EditorAction::SelectionWordRight,
        Some("editorFocus"),
        Source::Default,
    ),
    (
        "cmd+a",
        EditorAction::SelectionAll,
        Some("textInputFocus"),
        Source::Default,
    ),
];

/// Non-macOS default bindings (Windows/Linux convention): `Ctrl+A` is
/// select-all, as it always has been.
const OTHER: &[RawBinding] = &[(
    "ctrl+a",
    EditorAction::SelectionAll,
    Some("textInputFocus"),
    Source::Default,
)];

fn table_for(platform: Platform) -> &'static [RawBinding] {
    match platform {
        Platform::MacOs => MACOS,
        Platform::Other => OTHER,
    }
}

fn to_bindings(entries: &[RawBinding]) -> impl Iterator<Item = Binding> + '_ {
    entries.iter().map(|(key, action, when, source)| {
        Binding::new(
            parse_key_sequence(key).expect("default binding keys are valid"),
            *action,
            when.map(|value| value.parse().expect("default when clause is valid")),
            *source,
        )
    })
}

/// Builds the default binding table for a given platform: `COMMON` plus the
/// platform-specific OS-convention table (ADR-0011).
pub fn bindings_for(platform: Platform) -> Vec<Binding> {
    to_bindings(COMMON)
        .chain(to_bindings(table_for(platform)))
        .collect()
}

/// Builds the default binding table for the platform this binary was
/// compiled for.
pub fn bindings() -> Vec<Binding> {
    bindings_for(Platform::current())
}

#[cfg(test)]
mod tests {
    use super::{Platform, bindings, bindings_for};
    use crate::keymap::{EditorAction, EditorContext, ResolveResult, Resolver};

    #[test]
    fn all_default_bindings_load_into_resolver() {
        let resolver = Resolver::new(bindings());
        assert!(matches!(
            resolver.resolve(&["ctrl+s".parse().unwrap()], &EditorContext::default()),
            ResolveResult::Matched(_)
        ));
    }

    #[test]
    fn os_standard_default_binding_resolves_through_resolver() {
        let resolver = Resolver::new(bindings());
        assert_eq!(
            resolver.resolve(&["cmd+left".parse().unwrap()], &EditorContext::default()),
            ResolveResult::Matched(EditorAction::CursorLineStart)
        );
    }

    #[test]
    fn clipboard_default_bindings_parse_and_resolve() {
        let resolver = Resolver::new(bindings());
        let context = EditorContext {
            text_input_focus: true,
            ..EditorContext::default()
        };
        let cases = [
            ("ctrl+c", EditorAction::EditCopy),
            ("ctrl+x", EditorAction::EditCut),
            ("ctrl+v", EditorAction::EditPaste),
        ];

        for (key, action) in cases {
            assert_eq!(
                resolver.resolve(&[key.parse().unwrap()], &context),
                ResolveResult::Matched(action),
                "{key}"
            );
        }
    }

    #[test]
    fn search_default_bindings_parse_and_resolve() {
        let resolver = Resolver::new(bindings());
        let cases = [
            ("cmd+f", EditorAction::SearchOpen),
            ("cmd+alt+f", EditorAction::ReplaceOpen),
            ("cmd+g", EditorAction::SearchNext),
            ("cmd+shift+g", EditorAction::SearchPrevious),
            ("f3", EditorAction::SearchNext),
            ("shift+f3", EditorAction::SearchPrevious),
        ];

        for (key, action) in cases {
            assert_eq!(
                resolver.resolve(&[key.parse().unwrap()], &EditorContext::default()),
                ResolveResult::Matched(action),
                "{key}"
            );
        }
    }

    #[test]
    fn buffer_default_bindings_parse_and_resolve() {
        let resolver = Resolver::new(bindings());
        let cases = [
            ("ctrl+tab", EditorAction::BufferNext),
            ("ctrl+shift+tab", EditorAction::BufferPrevious),
            ("ctrl+w", EditorAction::BufferClose),
            ("cmd+w", EditorAction::BufferClose),
        ];

        for (key, action) in cases {
            assert_eq!(
                resolver.resolve(&[key.parse().unwrap()], &EditorContext::default()),
                ResolveResult::Matched(action),
                "{key}"
            );
        }
    }

    #[test]
    fn macos_os_convention_bindings_resolve_through_resolver() {
        let resolver = Resolver::new(bindings_for(Platform::MacOs));
        let cursor_context = EditorContext::default();
        let text_context = EditorContext {
            text_input_focus: true,
            ..EditorContext::default()
        };

        let cursor_cases = [
            ("ctrl+a", EditorAction::CursorLineStart),
            ("ctrl+e", EditorAction::CursorLineEnd),
            ("ctrl+n", EditorAction::CursorDown),
            ("ctrl+p", EditorAction::CursorUp),
            ("ctrl+f", EditorAction::CursorRight),
            ("ctrl+b", EditorAction::CursorLeft),
            ("alt+b", EditorAction::CursorWordLeft),
            ("alt+f", EditorAction::CursorWordRight),
            ("alt+shift+f", EditorAction::SelectionWordRight),
        ];
        for (key, action) in cursor_cases {
            assert_eq!(
                resolver.resolve(&[key.parse().unwrap()], &cursor_context),
                ResolveResult::Matched(action),
                "{key}"
            );
        }

        let text_cases = [
            ("ctrl+d", EditorAction::EditDelete),
            ("cmd+a", EditorAction::SelectionAll),
        ];
        for (key, action) in text_cases {
            assert_eq!(
                resolver.resolve(&[key.parse().unwrap()], &text_context),
                ResolveResult::Matched(action),
                "{key}"
            );
        }
    }

    #[test]
    fn other_platform_keeps_ctrl_a_as_select_all_and_lacks_macos_keys() {
        let resolver = Resolver::new(bindings_for(Platform::Other));
        let text_context = EditorContext {
            text_input_focus: true,
            ..EditorContext::default()
        };

        assert_eq!(
            resolver.resolve(&["ctrl+a".parse().unwrap()], &text_context),
            ResolveResult::Matched(EditorAction::SelectionAll)
        );

        let macos_only_keys = ["ctrl+e", "ctrl+n", "ctrl+p", "ctrl+f", "ctrl+b", "alt+b"];
        for key in macos_only_keys {
            assert_eq!(
                resolver.resolve(&[key.parse().unwrap()], &EditorContext::default()),
                ResolveResult::NoMatch,
                "{key}"
            );
        }
    }

    #[test]
    fn common_bindings_resolve_on_both_platforms() {
        for platform in [Platform::MacOs, Platform::Other] {
            let resolver = Resolver::new(bindings_for(platform));
            assert_eq!(
                resolver.resolve(&["cmd+left".parse().unwrap()], &EditorContext::default()),
                ResolveResult::Matched(EditorAction::CursorLineStart),
                "{platform:?}"
            );
            assert_eq!(
                resolver.resolve(
                    &["cmd+shift+right".parse().unwrap()],
                    &EditorContext::default()
                ),
                ResolveResult::Matched(EditorAction::SelectionLineEnd),
                "{platform:?}"
            );
        }
    }
}
