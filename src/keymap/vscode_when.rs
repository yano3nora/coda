//! VS Code `when` clause conversion for the keymap importer.
//!
//! Keep this deliberately smaller than VS Code's expression language. Guessing
//! complex predicates would silently enable wrong shortcuts, which is worse than
//! reporting a lost binding.

use std::fmt;

/// Error returned when a VS Code `when` clause cannot be represented by the MVP
/// [`crate::keymap::ContextPredicate`] language.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UnsupportedCondition(pub String);

/// Converts a VS Code `when` expression into the internal predicate syntax.
///
/// Supported grammar: `identifier` terms joined by `&&`, each optionally
/// prefixed with `!`. Everything else must be rejected and reported.
pub fn convert_vscode_when(value: &str) -> Result<String, UnsupportedCondition> {
    let value = value.trim();
    if value.is_empty() {
        return Err(UnsupportedCondition("empty condition".to_string()));
    }
    if value.contains("||")
        || value.contains('(')
        || value.contains(')')
        || value.contains("==")
        || value.contains("!=")
        || value.contains('=')
        || value.contains('<')
        || value.contains('>')
    {
        return Err(UnsupportedCondition(format!(
            "unsupported VS Code when syntax: {value}"
        )));
    }

    let mut converted = Vec::new();
    for raw_term in value.split("&&") {
        let raw_term = raw_term.trim();
        if raw_term.is_empty() || raw_term.contains(char::is_whitespace) {
            return Err(UnsupportedCondition(format!(
                "unsupported VS Code when syntax: {value}"
            )));
        }

        let (negated, name) = raw_term
            .strip_prefix('!')
            .map_or((false, raw_term), |name| (true, name));
        if name.is_empty() || name.starts_with('!') {
            return Err(UnsupportedCondition(format!(
                "unsupported VS Code when syntax: {value}"
            )));
        }

        let mapped = map_identifier(name).ok_or_else(|| {
            UnsupportedCondition(format!("unsupported VS Code when identifier: {name}"))
        })?;
        converted.push(if negated {
            format!("!{mapped}")
        } else {
            mapped.to_string()
        });
    }

    Ok(converted.join(" && "))
}

fn map_identifier(name: &str) -> Option<&'static str> {
    match name {
        "editorFocus" => Some("editorFocus"),
        "editorTextFocus" => Some("textInputFocus"),
        "editorHasMultipleSelections" => Some("hasMultipleSelections"),
        "editorReadonly" => Some("isReadonly"),
        "suggestWidgetVisible" => Some("suggestVisible"),
        "inQuickOpen" => Some("quickOpenVisible"),
        "listFocus" => Some("listFocus"),
        _ => None,
    }
}

impl fmt::Display for UnsupportedCondition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for UnsupportedCondition {}

#[cfg(test)]
mod tests {
    use super::convert_vscode_when;

    #[test]
    fn converts_supported_identifiers_and_negation() {
        assert_eq!(
            convert_vscode_when("editorTextFocus && !editorReadonly"),
            Ok("textInputFocus && !isReadonly".to_string())
        );
    }

    #[test]
    fn rejects_unsupported_syntax_and_identifiers() {
        assert!(convert_vscode_when("resourceLangId == markdown").is_err());
        assert!(convert_vscode_when("editorFocus || listFocus").is_err());
        assert!(convert_vscode_when("unknownFocus").is_err());
    }
}
