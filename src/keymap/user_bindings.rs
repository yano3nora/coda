//! Loader for user-authored `bindings.json` files.
//!
//! This module is intentionally pure: it accepts text and returns normalized
//! keymap data plus per-entry issues. Filesystem discovery and warning display
//! belong to the app layer so a broken config file never prevents startup.

use std::{fmt, str::FromStr};

use serde_json::Value;

use super::{Binding, ContextPredicate, EditorAction, Source, parse_key_sequence};

/// Successful load result for a user `bindings.json` document.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UserBindingsLoad {
    pub bindings: Vec<Binding>,
    pub issues: Vec<BindingIssue>,
}

/// Whole-file failures. Entry-level failures are reported as [`BindingIssue`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UserBindingsError {
    InvalidJson(String),
    RootNotArray,
}

/// A recoverable issue in one binding entry.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BindingIssue {
    /// Zero-based position in the root JSON array. This deliberately points to
    /// the original entry order so users can find the damaged binding quickly.
    pub index: usize,
    /// Best-effort `key` field value. Missing/non-string keys cannot be echoed.
    pub key: Option<String>,
    pub reason: BindingIssueReason,
}

/// Why a binding entry could not be converted.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BindingIssueReason {
    InvalidKey(String),
    UnknownCommand(String),
    InvalidWhen(String),
    MissingField(&'static str),
}

/// Loads user bindings from a JSONC string.
///
/// Unknown fields are ignored for VS Code compatibility. Invalid entries are
/// skipped, but valid siblings keep their original definition order.
pub fn load_user_bindings(text: &str) -> Result<UserBindingsLoad, UserBindingsError> {
    load_bindings_with_source(text, Source::User)
}

/// Loads bindings from JSONC text and stamps every valid binding with `source`.
pub fn load_bindings_with_source(
    text: &str,
    source: Source,
) -> Result<UserBindingsLoad, UserBindingsError> {
    let stripped = strip_jsonc_comments(text);
    let value: Value = serde_json::from_str(&stripped)
        .map_err(|error| UserBindingsError::InvalidJson(error.to_string()))?;
    let entries = value.as_array().ok_or(UserBindingsError::RootNotArray)?;

    let mut bindings = Vec::new();
    let mut issues = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        let key = string_field(entry, "key");
        let command = string_field(entry, "command");
        let when = string_field(entry, "when");
        let before_issue_count = issues.len();

        let Some(key_text) = key else {
            issues.push(issue(index, key, BindingIssueReason::MissingField("key")));
            continue;
        };
        let Some(command_text) = command else {
            issues.push(issue(
                index,
                Some(key_text),
                BindingIssueReason::MissingField("command"),
            ));
            continue;
        };

        let parsed_keys = match parse_key_sequence(key_text) {
            Ok(keys) => keys,
            Err(error) => {
                issues.push(issue(
                    index,
                    Some(key_text),
                    BindingIssueReason::InvalidKey(error.to_string()),
                ));
                continue;
            }
        };

        let action = match EditorAction::from_str(command_text) {
            Ok(action) => action,
            Err(_) => {
                issues.push(issue(
                    index,
                    Some(key_text),
                    BindingIssueReason::UnknownCommand(command_text.to_string()),
                ));
                continue;
            }
        };

        let when = match when {
            Some(when_text) => match when_text.parse::<ContextPredicate>() {
                Ok(predicate) => Some(predicate),
                Err(error) => {
                    issues.push(issue(
                        index,
                        Some(key_text),
                        BindingIssueReason::InvalidWhen(error.to_string()),
                    ));
                    continue;
                }
            },
            None => None,
        };

        debug_assert_eq!(issues.len(), before_issue_count);
        bindings.push(Binding::new(parsed_keys, action, when, source));
    }

    Ok(UserBindingsLoad { bindings, issues })
}

fn issue(index: usize, key: Option<&str>, reason: BindingIssueReason) -> BindingIssue {
    BindingIssue {
        index,
        key: key.map(ToOwned::to_owned),
        reason,
    }
}

fn string_field<'a>(entry: &'a Value, name: &str) -> Option<&'a str> {
    entry.as_object()?.get(name)?.as_str()
}

/// Removes JSONC comments while preserving string literals and whitespace shape.
pub(crate) fn strip_jsonc_comments(text: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        String,
        Escape,
        LineComment,
        BlockComment,
    }

    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut state = State::Normal;

    while let Some(character) = chars.next() {
        match state {
            State::Normal => match character {
                '"' => {
                    output.push(character);
                    state = State::String;
                }
                '/' if chars.peek() == Some(&'/') => {
                    chars.next();
                    output.push(' ');
                    output.push(' ');
                    state = State::LineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    output.push(' ');
                    output.push(' ');
                    state = State::BlockComment;
                }
                _ => output.push(character),
            },
            State::String => {
                output.push(character);
                match character {
                    '\\' => state = State::Escape,
                    '"' => state = State::Normal,
                    _ => {}
                }
            }
            State::Escape => {
                output.push(character);
                state = State::String;
            }
            State::LineComment => {
                if character == '\n' {
                    output.push('\n');
                    state = State::Normal;
                } else {
                    output.push(' ');
                }
            }
            State::BlockComment => {
                if character == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    output.push(' ');
                    output.push(' ');
                    state = State::Normal;
                } else if character == '\n' {
                    output.push('\n');
                } else {
                    output.push(' ');
                }
            }
        }
    }

    output
}

impl fmt::Display for UserBindingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(formatter, "invalid bindings.json: {error}"),
            Self::RootNotArray => formatter.write_str("bindings.json root must be an array"),
        }
    }
}

impl std::error::Error for UserBindingsError {}

impl fmt::Display for BindingIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.key {
            Some(key) => write!(
                formatter,
                "binding[{}] key `{key}`: {}",
                self.index, self.reason
            ),
            None => write!(formatter, "binding[{}]: {}", self.index, self.reason),
        }
    }
}

impl fmt::Display for BindingIssueReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey(detail) => write!(formatter, "invalid key ({detail})"),
            Self::UnknownCommand(command) => {
                write!(formatter, "unknown command `{command}`")?;
                // Users habitually paste VS Code entries into bindings.json.
                // Point them at the internal name; keep it short because this
                // often renders in the one-line status bar.
                if let Some(action) = super::action_for_vscode_command(command) {
                    write!(formatter, " — use `{action}`")?;
                }
                Ok(())
            }
            Self::InvalidWhen(detail) => write!(formatter, "invalid when clause ({detail})"),
            Self::MissingField(name) => write!(formatter, "missing required field `{name}`"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BindingIssueReason, UserBindingsError, load_user_bindings, strip_jsonc_comments};
    use crate::{
        input::{Key, KeyEvent, Modifiers},
        keymap::{EditorAction, Source},
    };

    #[test]
    fn unknown_vscode_command_display_suggests_internal_name() {
        let loaded =
            load_user_bindings(r#"[{ "key": "cmd+j", "command": "cursorDown" }]"#).unwrap();
        let message = loaded.issues[0].to_string();
        assert!(
            message.contains("use `cursor.down`"),
            "suggestion missing from: {message}"
        );
    }

    #[test]
    fn loads_valid_entries_in_definition_order_as_user_source() {
        let loaded = load_user_bindings(
            r#"[
                { "key": "ctrl+j", "command": "cursor.down", "when": "editorFocus" },
                { "key": "ctrl+k", "command": "cursor.up" }
            ]"#,
        )
        .unwrap();

        assert!(loaded.issues.is_empty());
        assert_eq!(loaded.bindings.len(), 2);
        assert_eq!(loaded.bindings[0].source, Source::User);
        assert_eq!(loaded.bindings[0].action, EditorAction::CursorDown);
        assert_eq!(loaded.bindings[0].keys, vec![key('j', Modifiers::ctrl())]);
        assert!(loaded.bindings[0].when.is_some());
        assert_eq!(loaded.bindings[1].source, Source::User);
        assert_eq!(loaded.bindings[1].action, EditorAction::CursorUp);
        assert_eq!(loaded.bindings[1].keys, vec![key('k', Modifiers::ctrl())]);
        assert!(loaded.bindings[1].when.is_none());
    }

    #[test]
    fn accepts_jsonc_line_and_block_comments() {
        let loaded = load_user_bindings(
            r#"[
                // Move down like the user's GUI editor.
                { "key": "ctrl+j", "command": "cursor.down" },
                /* And the reverse direction. */
                { "key": "ctrl+k", "command": "cursor.up" }
            ]"#,
        )
        .unwrap();

        assert!(loaded.issues.is_empty());
        assert_eq!(loaded.bindings.len(), 2);
    }

    #[test]
    fn does_not_treat_double_slash_inside_strings_as_comment() {
        let loaded = load_user_bindings(
            r#"[
                { "key": "ctrl+j", "command": "cursor.down", "when": "editorFocus // not-a-comment" },
                { "key": "ctrl+k", "command": "cursor.up" }
            ]"#,
        )
        .unwrap();

        assert_eq!(loaded.bindings.len(), 1);
        assert_eq!(loaded.issues.len(), 1);
        assert_eq!(loaded.issues[0].index, 0);
        assert!(matches!(
            loaded.issues[0].reason,
            BindingIssueReason::InvalidWhen(_)
        ));
    }

    #[test]
    fn reports_invalid_key_for_only_the_bad_entry() {
        let loaded = load_user_bindings(
            r#"[
                { "key": "ctrl+banana", "command": "cursor.down" },
                { "key": "ctrl+k", "command": "cursor.up" }
            ]"#,
        )
        .unwrap();

        assert_eq!(loaded.bindings.len(), 1);
        assert_eq!(loaded.bindings[0].action, EditorAction::CursorUp);
        assert_eq!(loaded.issues.len(), 1);
        assert_eq!(loaded.issues[0].index, 0);
        assert!(matches!(
            loaded.issues[0].reason,
            BindingIssueReason::InvalidKey(_)
        ));
    }

    #[test]
    fn reports_unknown_command() {
        let loaded =
            load_user_bindings(r#"[{ "key": "ctrl+r", "command": "editor.action.rename" }]"#)
                .unwrap();

        assert!(loaded.bindings.is_empty());
        assert_eq!(
            loaded.issues[0].reason,
            BindingIssueReason::UnknownCommand("editor.action.rename".to_string())
        );
    }

    #[test]
    fn reports_invalid_when() {
        let loaded = load_user_bindings(
            r#"[{ "key": "ctrl+m", "command": "cursor.down", "when": "resourceLangId == markdown" }]"#,
        )
        .unwrap();

        assert!(loaded.bindings.is_empty());
        assert!(matches!(
            loaded.issues[0].reason,
            BindingIssueReason::InvalidWhen(_)
        ));
    }

    #[test]
    fn reports_missing_key() {
        let loaded = load_user_bindings(r#"[{ "command": "cursor.down" }]"#).unwrap();

        assert!(loaded.bindings.is_empty());
        assert_eq!(
            loaded.issues[0].reason,
            BindingIssueReason::MissingField("key")
        );
    }

    #[test]
    fn empty_array_succeeds_without_bindings_or_issues() {
        let loaded = load_user_bindings("[]").unwrap();

        assert!(loaded.bindings.is_empty());
        assert!(loaded.issues.is_empty());
    }

    #[test]
    fn root_object_and_broken_json_are_whole_file_errors() {
        assert_eq!(
            load_user_bindings("{}"),
            Err(UserBindingsError::RootNotArray)
        );
        assert!(matches!(
            load_user_bindings("["),
            Err(UserBindingsError::InvalidJson(_))
        ));
    }

    #[test]
    fn issue_index_points_to_original_entry_position() {
        let loaded = load_user_bindings(
            r#"[
                { "key": "ctrl+j", "command": "cursor.down" },
                { "key": "ctrl+banana", "command": "cursor.up" },
                { "key": "ctrl+k", "command": "editor.action.rename" }
            ]"#,
        )
        .unwrap();

        assert_eq!(loaded.bindings.len(), 1);
        assert_eq!(loaded.issues.len(), 2);
        assert_eq!(loaded.issues[0].index, 1);
        assert_eq!(loaded.issues[1].index, 2);
    }

    #[test]
    fn jsonc_stripper_preserves_comment_like_text_in_strings() {
        let stripped =
            strip_jsonc_comments(r#"{ "when": "editorFocus // text", "key": "ctrl+j" } // tail"#);

        assert!(stripped.contains(r#"editorFocus // text"#));
        assert!(!stripped.contains("tail"));
    }

    fn key(character: char, modifiers: Modifiers) -> KeyEvent {
        KeyEvent::new(Key::Char(character), modifiers)
    }
}
