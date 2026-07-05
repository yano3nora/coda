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
}
