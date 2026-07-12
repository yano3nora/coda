//! Minimal `when` predicate parser and evaluator.
//!
//! MVP deliberately supports only `term && term` with optional `!`. More complex
//! VS Code expressions must become import-report errors instead of being guessed.

use std::{fmt, str::FromStr};

use super::context::EditorContext;

/// A conjunction of boolean context terms.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ContextPredicate {
    terms: Vec<Term>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct Term {
    name: String,
    negated: bool,
}

/// Parse errors for context predicates.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ParsePredicateError {
    Empty,
    UnsupportedSyntax(String),
    UnknownContext(String),
}

impl ContextPredicate {
    pub fn eval(&self, ctx: &EditorContext) -> bool {
        self.terms.iter().all(|term| {
            let value = ctx
                .get(&term.name)
                .expect("predicate parser validates context names");
            if term.negated { !value } else { value }
        })
    }

    pub fn term_count(&self) -> usize {
        self.terms.len()
    }

    /// Returns the name of the first *positive* (non-negated) term whose
    /// name appears in `keys`, or `None` if there is no such term.
    ///
    /// Used by the VS Code importer (`context::RESERVED_FALSE_KEYS`) to
    /// decide whether a binding's `when` clause can never evaluate true: a
    /// positive term on a permanently-false key anywhere in the conjunction
    /// makes the whole predicate always-false (`a && false == false`
    /// regardless of the other terms), but a *negated* term on that key
    /// (`!suggestVisible`) is always-true and must not count — precision
    /// matters here, since flagging a negated term would wrongly mark a
    /// perfectly reachable binding as dead.
    pub fn positive_term_matching<'a>(&'a self, keys: &[&str]) -> Option<&'a str> {
        self.terms
            .iter()
            .find(|term| !term.negated && keys.contains(&term.name.as_str()))
            .map(|term| term.name.as_str())
    }
}

impl FromStr for ContextPredicate {
    type Err = ParsePredicateError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ParsePredicateError::Empty);
        }
        if value.contains("||") || value.contains('(') || value.contains(')') {
            return Err(ParsePredicateError::UnsupportedSyntax(value.to_string()));
        }

        let mut terms = Vec::new();
        for raw_term in value.split("&&") {
            let raw_term = raw_term.trim();
            if raw_term.is_empty() || raw_term.contains(char::is_whitespace) {
                return Err(ParsePredicateError::UnsupportedSyntax(value.to_string()));
            }

            let (negated, name) = raw_term
                .strip_prefix('!')
                .map_or((false, raw_term), |name| (true, name));
            if name.is_empty() || name.starts_with('!') {
                return Err(ParsePredicateError::UnsupportedSyntax(value.to_string()));
            }
            if EditorContext::default().get(name).is_none() {
                return Err(ParsePredicateError::UnknownContext(name.to_string()));
            }
            terms.push(Term {
                name: name.to_string(),
                negated,
            });
        }

        Ok(Self { terms })
    }
}

impl fmt::Display for ContextPredicate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, term) in self.terms.iter().enumerate() {
            if index > 0 {
                formatter.write_str(" && ")?;
            }
            if term.negated {
                formatter.write_str("!")?;
            }
            formatter.write_str(&term.name)?;
        }
        Ok(())
    }
}

impl fmt::Display for ParsePredicateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("empty context predicate"),
            Self::UnsupportedSyntax(value) => {
                write!(formatter, "unsupported context predicate: {value}")
            }
            Self::UnknownContext(name) => write!(formatter, "unknown context identifier: {name}"),
        }
    }
}

impl std::error::Error for ParsePredicateError {}

#[cfg(test)]
mod tests {
    use super::{ContextPredicate, ParsePredicateError};
    use crate::keymap::EditorContext;

    #[test]
    fn parses_and_evaluates_supported_predicates() {
        struct Case {
            input: &'static str,
            ctx: EditorContext,
            expected: bool,
            terms: usize,
        }

        let readonly = EditorContext {
            is_readonly: true,
            ..EditorContext::default()
        };
        let cases = [
            Case {
                input: "editorFocus",
                ctx: EditorContext::default(),
                expected: true,
                terms: 1,
            },
            Case {
                input: "!isReadonly",
                ctx: EditorContext::default(),
                expected: true,
                terms: 1,
            },
            Case {
                input: "editorFocus && !isReadonly",
                ctx: EditorContext::default(),
                expected: true,
                terms: 2,
            },
            Case {
                input: "editorFocus && !isReadonly",
                ctx: readonly,
                expected: false,
                terms: 2,
            },
        ];

        for case in cases {
            let predicate = case.input.parse::<ContextPredicate>().unwrap();
            assert_eq!(predicate.eval(&case.ctx), case.expected, "{}", case.input);
            assert_eq!(predicate.term_count(), case.terms, "{}", case.input);
        }
    }

    #[test]
    fn rejects_unknown_context_identifier() {
        assert_eq!(
            "resourceLangId".parse::<ContextPredicate>(),
            Err(ParsePredicateError::UnknownContext(
                "resourceLangId".to_string()
            ))
        );
    }

    /// Table-driven per the importer's "inactive context" requirement: a
    /// positive term on a reserved key anywhere in the conjunction blocks
    /// the predicate, a negated term never does, and an unrelated key never
    /// matches at all.
    #[test]
    fn positive_term_matching_finds_only_non_negated_reserved_terms() {
        let reserved = ["suggestVisible", "quickOpenVisible"];
        let cases: &[(&str, Option<&str>)] = &[
            ("suggestVisible", Some("suggestVisible")),
            ("!suggestVisible", None),
            ("editorFocus && suggestVisible", Some("suggestVisible")),
            ("suggestVisible && editorFocus", Some("suggestVisible")),
            ("editorFocus && !suggestVisible", None),
            ("editorFocus", None),
            ("quickOpenVisible", Some("quickOpenVisible")),
        ];

        for (input, expected) in cases {
            let predicate = input.parse::<ContextPredicate>().unwrap();
            assert_eq!(
                predicate.positive_term_matching(&reserved),
                *expected,
                "{input}"
            );
        }
    }
}
