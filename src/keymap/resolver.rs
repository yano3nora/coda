//! Stateless keybinding resolver.

use crate::input::KeyEvent;

use super::{Binding, EditorAction, EditorContext};

/// Result of resolving the current pending key sequence.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ResolveResult {
    /// A complete binding was selected and no longer sequence can still match.
    Matched(EditorAction),
    /// Current keys are a prefix of longer bindings. `exact` is what timeout may fire.
    Pending {
        exact: Option<EditorAction>,
        candidates: Vec<(Vec<KeyEvent>, EditorAction)>,
    },
    NoMatch,
}

/// Pure resolver. It owns binding order but no pending runtime state.
#[derive(Debug, Clone)]
pub struct Resolver {
    bindings: Vec<Binding>,
}

impl Resolver {
    pub fn new(bindings: Vec<Binding>) -> Self {
        Self { bindings }
    }

    pub fn resolve(&self, pending: &[KeyEvent], ctx: &EditorContext) -> ResolveResult {
        if pending.is_empty() {
            return ResolveResult::NoMatch;
        }

        let mut exact = Vec::new();
        let mut prefix = Vec::new();
        for (index, binding) in self.bindings.iter().enumerate() {
            if !binding_matches_context(binding, ctx) || binding.keys.len() < pending.len() {
                continue;
            }
            if !binding.keys.starts_with(pending) {
                continue;
            }

            if binding.keys.len() == pending.len() {
                exact.push((index, binding));
            } else {
                prefix.push(binding);
            }
        }

        let exact_action = best_match(&exact).map(|binding| binding.action);
        if !prefix.is_empty() {
            return ResolveResult::Pending {
                exact: exact_action,
                candidates: prefix
                    .into_iter()
                    .map(|binding| (binding.keys.clone(), binding.action))
                    .collect(),
            };
        }

        exact_action.map_or(ResolveResult::NoMatch, ResolveResult::Matched)
    }
}

fn binding_matches_context(binding: &Binding, ctx: &EditorContext) -> bool {
    binding.when.as_ref().is_none_or(|when| when.eval(ctx))
}

fn best_match<'a>(matches: &[(usize, &'a Binding)]) -> Option<&'a Binding> {
    matches
        .iter()
        .max_by_key(|(index, binding)| (binding.source.priority(), binding.term_count(), *index))
        .map(|(_, binding)| *binding)
}

#[cfg(test)]
mod tests {
    use super::{ResolveResult, Resolver};
    use crate::{
        input::KeyEvent,
        keymap::{
            Binding, EditorAction, EditorContext, Source, parse_key_chord, parse_key_sequence,
        },
    };

    #[test]
    fn resolves_by_source_specificity_and_definition_order() {
        struct Case {
            name: &'static str,
            bindings: Vec<Binding>,
            pending: KeyEvent,
            ctx: EditorContext,
            expected: ResolveResult,
        }

        let cases = [
            Case {
                name: "user beats default",
                bindings: vec![
                    binding("ctrl+j", EditorAction::CursorDown, None, Source::Default),
                    binding("ctrl+j", EditorAction::CursorUp, None, Source::User),
                ],
                pending: key("ctrl+j"),
                ctx: EditorContext::default(),
                expected: ResolveResult::Matched(EditorAction::CursorUp),
            },
            Case {
                name: "rescue beats imported",
                bindings: vec![
                    binding(
                        "ctrl+space",
                        EditorAction::SearchOpen,
                        None,
                        Source::Imported,
                    ),
                    binding(
                        "ctrl+space",
                        EditorAction::PaletteOpen,
                        None,
                        Source::Rescue,
                    ),
                ],
                pending: key("ctrl+space"),
                ctx: EditorContext::default(),
                expected: ResolveResult::Matched(EditorAction::PaletteOpen),
            },
            Case {
                name: "more specific when wins within source",
                bindings: vec![
                    binding("ctrl+j", EditorAction::CursorDown, None, Source::Default),
                    binding(
                        "ctrl+j",
                        EditorAction::SearchNext,
                        Some("editorFocus"),
                        Source::Default,
                    ),
                ],
                pending: key("ctrl+j"),
                ctx: EditorContext::default(),
                expected: ResolveResult::Matched(EditorAction::SearchNext),
            },
            Case {
                name: "later definition wins exact tie",
                bindings: vec![
                    binding("ctrl+j", EditorAction::CursorDown, None, Source::User),
                    binding("ctrl+j", EditorAction::CursorUp, None, Source::User),
                ],
                pending: key("ctrl+j"),
                ctx: EditorContext::default(),
                expected: ResolveResult::Matched(EditorAction::CursorUp),
            },
            Case {
                name: "false when is excluded",
                bindings: vec![binding(
                    "ctrl+j",
                    EditorAction::SearchNext,
                    Some("searchVisible"),
                    Source::User,
                )],
                pending: key("ctrl+j"),
                ctx: EditorContext::default(),
                expected: ResolveResult::NoMatch,
            },
            Case {
                name: "same key changes with context",
                bindings: vec![
                    binding(
                        "ctrl+j",
                        EditorAction::CursorDown,
                        Some("editorFocus"),
                        Source::User,
                    ),
                    binding(
                        "ctrl+j",
                        EditorAction::SearchNext,
                        Some("searchVisible"),
                        Source::User,
                    ),
                ],
                pending: key("ctrl+j"),
                ctx: EditorContext {
                    search_visible: true,
                    ..EditorContext::default()
                },
                expected: ResolveResult::Matched(EditorAction::SearchNext),
            },
        ];

        for case in cases {
            let resolver = Resolver::new(case.bindings);
            assert_eq!(
                resolver.resolve(&[case.pending], &case.ctx),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn resolves_sequences_and_unrelated_keys() {
        struct Case {
            name: &'static str,
            bindings: Vec<Binding>,
            pending: Vec<KeyEvent>,
            expected: ResolveResult,
        }

        let sequence = keys("ctrl+x ctrl+s");
        let cases = [
            Case {
                name: "prefix waits for sequence",
                bindings: vec![binding(
                    "ctrl+x ctrl+s",
                    EditorAction::FileSave,
                    None,
                    Source::User,
                )],
                pending: vec![key("ctrl+x")],
                expected: ResolveResult::Pending {
                    exact: None,
                    candidates: vec![(sequence.clone(), EditorAction::FileSave)],
                },
            },
            Case {
                name: "full sequence matches",
                bindings: vec![binding(
                    "ctrl+x ctrl+s",
                    EditorAction::FileSave,
                    None,
                    Source::User,
                )],
                pending: sequence.clone(),
                expected: ResolveResult::Matched(EditorAction::FileSave),
            },
            Case {
                name: "prefix with exact action returns pending exact",
                bindings: vec![
                    binding("ctrl+x", EditorAction::BufferClose, None, Source::User),
                    binding("ctrl+x ctrl+s", EditorAction::FileSave, None, Source::User),
                ],
                pending: vec![key("ctrl+x")],
                expected: ResolveResult::Pending {
                    exact: Some(EditorAction::BufferClose),
                    candidates: vec![(sequence, EditorAction::FileSave)],
                },
            },
            Case {
                name: "unrelated key has no match",
                bindings: vec![binding(
                    "ctrl+x",
                    EditorAction::BufferClose,
                    None,
                    Source::User,
                )],
                pending: vec![key("ctrl+j")],
                expected: ResolveResult::NoMatch,
            },
        ];

        for case in cases {
            let resolver = Resolver::new(case.bindings);
            assert_eq!(
                resolver.resolve(&case.pending, &EditorContext::default()),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    fn binding(keys: &str, action: EditorAction, when: Option<&str>, source: Source) -> Binding {
        Binding::new(
            parse_key_sequence(keys).unwrap(),
            action,
            when.map(|value| value.parse().unwrap()),
            source,
        )
    }

    fn key(value: &str) -> KeyEvent {
        parse_key_chord(value).unwrap()
    }

    fn keys(value: &str) -> Vec<KeyEvent> {
        parse_key_sequence(value).unwrap()
    }
}
