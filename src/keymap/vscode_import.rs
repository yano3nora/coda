//! VS Code `keybindings.json` importer (SPEC-0004).

use serde_json::Value;

use crate::input::{Key, KeyEvent, KeyboardCapabilities};

use super::{
    Binding, ContextPredicate, EditorAction, ImportReport, ReportEntry, Source,
    action_for_vscode_command, parse_key_sequence, user_bindings::strip_jsonc_comments,
    vscode_when::convert_vscode_when,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VsCodeImport {
    pub bindings: Vec<Binding>,
    pub report: ImportReport,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum VsCodeImportError {
    InvalidJson(String),
    RootNotArray,
}

pub fn import_vscode_keybindings(
    text: &str,
    capabilities: &KeyboardCapabilities,
) -> Result<VsCodeImport, VsCodeImportError> {
    let stripped = strip_jsonc_comments(text);
    let value: Value = serde_json::from_str(&stripped)
        .map_err(|error| VsCodeImportError::InvalidJson(error.to_string()))?;
    let entries = value.as_array().ok_or(VsCodeImportError::RootNotArray)?;

    let mut bindings = Vec::new();
    let mut report = ImportReport::default();

    for entry in entries {
        classify_entry(entry, &mut bindings, &mut report, capabilities);
    }

    debug_assert_eq!(entries.len(), report.total_classified());
    Ok(VsCodeImport { bindings, report })
}

pub fn render_generated_bindings(bindings: &[Binding]) -> String {
    let values = bindings
        .iter()
        .map(|binding| {
            let mut object = serde_json::Map::new();
            object.insert(
                "key".to_string(),
                Value::String(format_key_for_config(&binding.keys)),
            );
            object.insert(
                "command".to_string(),
                Value::String(binding.action.as_str().to_string()),
            );
            if let Some(when) = &binding.when {
                object.insert("when".to_string(), Value::String(when.to_string()));
            }
            Value::Object(object)
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&values).expect("generated bindings are serializable") + "\n"
}

pub fn format_key_for_config(keys: &[KeyEvent]) -> String {
    keys.iter()
        .map(format_chord_for_config)
        .collect::<Vec<_>>()
        .join(" ")
}

fn classify_entry(
    entry: &Value,
    bindings: &mut Vec<Binding>,
    report: &mut ImportReport,
    capabilities: &KeyboardCapabilities,
) {
    let key = string_field(entry, "key").map(ToOwned::to_owned);
    let command = string_field(entry, "command").map(ToOwned::to_owned);
    let when = string_field(entry, "when").map(ToOwned::to_owned);

    let Some(command_text) = command.as_deref() else {
        report
            .unsupported_commands
            .push(ReportEntry::new(key, command, when, "missing command"));
        return;
    };

    // VS Code の "command": "" は default binding の打ち消し (unbind) 記法。
    // coda 側に打ち消す対象の default はないため、機能未実装ではなく
    // 意図的な無効化として ignored に分類する (-command と同族の扱い)。
    if command_text.is_empty() {
        report.ignored.push(ReportEntry::new(
            key,
            command,
            when,
            "empty command unbinds a VS Code default; not applicable in MVP",
        ));
        return;
    }

    if command_text.starts_with('-') {
        report.unsupported_commands.push(ReportEntry::new(
            key,
            command,
            when,
            "negative binding is not supported in MVP",
        ));
        return;
    }

    let Some(action) = action_for_vscode_command(command_text) else {
        if is_ignored_command(command_text) {
            report
                .ignored
                .push(ReportEntry::new(key, command, when, "outside editor scope"));
        } else {
            report.unsupported_commands.push(ReportEntry::new(
                key,
                command,
                when,
                "feature not implemented",
            ));
        }
        return;
    };

    let Some(key_text) = key.as_deref() else {
        report
            .invalid_keys
            .push(ReportEntry::new(key, command, when, "missing key"));
        return;
    };
    let parsed_keys = match parse_key_sequence(key_text) {
        Ok(keys) => keys,
        Err(error) => {
            report
                .invalid_keys
                .push(ReportEntry::new(key, command, when, error.to_string()));
            return;
        }
    };

    let parsed_when = match when.as_deref() {
        Some(when_text) => match convert_vscode_when(when_text).and_then(|converted| {
            converted
                .parse::<ContextPredicate>()
                .map_err(|e| e.to_string().into())
        }) {
            Ok(predicate) => Some(predicate),
            Err(error) => {
                report.unsupported_conditions.push(ReportEntry::new(
                    key,
                    command,
                    when,
                    error.to_string(),
                ));
                return;
            }
        },
        None => None,
    };

    // Chord-level deliverability check (SPEC-0003 / TASK-260712-16): a
    // sequence can't be "half" pressed, so any single undeliverable chord
    // disables the whole binding. Checked after key/when parsing succeeded
    // (an entry that already failed those never reaches here) and before
    // conflict detection, so a capability-disabled entry is never treated as
    // a "previous" binding a later import could overwrite.
    if let Some(reason) = sequence_disable_reason(&parsed_keys, capabilities) {
        report
            .disabled_by_terminal_capability
            .push(ReportEntry::new(key, command, when, reason));
        return;
    }

    if let Some(existing_index) = bindings
        .iter()
        .position(|binding| binding.keys == parsed_keys && binding.when == parsed_when)
    {
        let previous = bindings.remove(existing_index);
        remove_imported_report_entry(report, &previous);
        report.conflicts.push(ReportEntry::new(
            Some(format_key_for_config(&previous.keys)),
            Some(previous.action.as_str().to_string()),
            previous.when.as_ref().map(ToString::to_string),
            "overwritten by later VS Code binding",
        ));
    }

    let binding = Binding::new(parsed_keys, action, parsed_when, Source::Imported);
    report.imported.push(ReportEntry::new(
        Some(format_key_for_config(&binding.keys)),
        Some(action.as_str().to_string()),
        binding.when.as_ref().map(ToString::to_string),
        imported_reason(action),
    ));
    bindings.push(binding);
}

fn remove_imported_report_entry(report: &mut ImportReport, previous: &Binding) {
    if let Some(index) = report.imported.iter().position(|entry| {
        entry.key.as_deref() == Some(format_key_for_config(&previous.keys).as_str())
            && entry.command.as_deref() == Some(previous.action.as_str())
            && entry.when == previous.when.as_ref().map(ToString::to_string)
    }) {
        report.imported.remove(index);
    }
}

/// Finds the first chord in a key sequence this terminal cannot deliver, per
/// the four SPEC-0003 rules (checked in the order TASK-260712-16 lists
/// them). `None` means every chord in the sequence is deliverable.
fn sequence_disable_reason(
    keys: &[KeyEvent],
    capabilities: &KeyboardCapabilities,
) -> Option<String> {
    keys.iter()
        .find_map(|key| capability_disable_reason(key, capabilities))
}

/// Explains why a single chord is undeliverable on `capabilities`, or `None`
/// if it is fine. Arrow / Home / End / etc. special keys are intentionally
/// excluded from the Ctrl+Shift rule: legacy CSI sequences (`CSI 1;6C` etc.)
/// carry a numeric modifier field that already distinguishes Ctrl from
/// Ctrl+Shift for those keys, unlike plain characters which rely on kitty
/// CSI u — so `Ctrl+Shift+Right` is never disabled by terminal capability
/// (TASK-260712-16 note). The `Key::Char` guard on that rule is what
/// achieves the exclusion; no extra special-casing is needed.
fn capability_disable_reason(
    key: &KeyEvent,
    capabilities: &KeyboardCapabilities,
) -> Option<String> {
    if key.modifiers.contains_super() && !capabilities.supports_modified_keys {
        return Some("terminal cannot deliver Cmd/Super".to_string());
    }

    if let Key::Char(character) = &key.key
        && key.modifiers.contains_ctrl()
        && key.modifiers.contains_shift()
        && !capabilities.supports_shift_ctrl_distinction
    {
        let letter = character.to_ascii_uppercase();
        return Some(format!(
            "terminal cannot distinguish Ctrl+Shift+{letter} from Ctrl+{letter}"
        ));
    }

    if key.key == Key::Enter {
        if key.modifiers.contains_shift() && !capabilities.supports_shift_enter {
            return Some("terminal cannot receive Shift+Enter".to_string());
        }
        if key.modifiers.contains_ctrl() && !capabilities.supports_ctrl_enter {
            return Some("terminal cannot receive Ctrl+Enter".to_string());
        }
    }

    None
}

fn imported_reason(_action: EditorAction) -> &'static str {
    // TODO(SPEC-0004): mark suggest/quick-open predicates as inactive once the
    // report entry carries enough context to distinguish those imports.
    "imported"
}

fn is_ignored_command(command: &str) -> bool {
    if command.starts_with("editor.") || command.starts_with("cursor") {
        return false;
    }
    command.starts_with("workbench.")
        || command.starts_with("extension.")
        || command.split_once('.').is_some()
}

fn string_field<'a>(entry: &'a Value, name: &str) -> Option<&'a str> {
    entry.as_object()?.get(name)?.as_str()
}

fn format_chord_for_config(key: &KeyEvent) -> String {
    let mut parts = Vec::new();
    if key.modifiers.contains_ctrl() {
        parts.push("ctrl".to_string());
    }
    if key.modifiers.contains_alt() {
        parts.push("alt".to_string());
    }
    if key.modifiers.contains_shift() {
        parts.push("shift".to_string());
    }
    if key.modifiers.contains_super() {
        parts.push("cmd".to_string());
    }
    parts.push(format_key_name(&key.key));
    parts.join("+")
}

fn format_key_name(key: &Key) -> String {
    match key {
        Key::Char(' ') => "space".to_string(),
        Key::Char(character) => character.to_ascii_lowercase().to_string(),
        Key::Enter => "enter".to_string(),
        Key::Esc => "escape".to_string(),
        Key::Tab => "tab".to_string(),
        Key::Backspace => "backspace".to_string(),
        Key::Up => "up".to_string(),
        Key::Down => "down".to_string(),
        Key::Left => "left".to_string(),
        Key::Right => "right".to_string(),
        Key::Home => "home".to_string(),
        Key::End => "end".to_string(),
        Key::PageUp => "pageup".to_string(),
        Key::PageDown => "pagedown".to_string(),
        Key::Delete => "delete".to_string(),
        Key::F(number) => format!("f{number}"),
        Key::Unknown(bytes) => format!("unknown({})", crate::input::escape_bytes(bytes)),
    }
}

impl From<String> for super::vscode_when::UnsupportedCondition {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{format_key_for_config, import_vscode_keybindings};
    use crate::{
        input::KeyboardCapabilities,
        keymap::{EditorAction, Source, parse_key_sequence},
    };

    #[test]
    fn imports_and_classifies_every_fixture_entry() {
        let fixture = r#"[
            { "key": "ctrl+j", "command": "cursorDown", "when": "editorFocus" },
            { "key": "ctrl+shift+j", "command": "cursorDownSelect", "when": "editorFocus" },
            { "key": "cmd+t", "command": "workbench.action.terminal.new" },
            { "key": "cmd+e", "command": "extension.foo" },
            { "key": "cmd+p", "command": "projectManager.list" },
            { "key": "f2", "command": "editor.action.rename" },
            { "key": "ctrl+j", "command": "-cursorDown" },
            { "key": "ctrl+m", "command": "cursorDown", "when": "resourceLangId == markdown" },
            { "key": "ctrl+t", "command": "cursorDown", "when": "editorTextFocus && !editorReadonly" },
            { "key": "ctrl+[IntlBackslash]", "command": "cursorDown" },
            { "key": "ctrl+x", "command": "cursorDown", "when": "editorFocus" },
            { "key": "ctrl+x", "command": "cursorUp", "when": "editorFocus" }
        ]"#;

        let imported = import_vscode_keybindings(fixture, &KeyboardCapabilities::modern()).unwrap();
        let summary = imported.report.summary();

        assert_eq!(summary.imported, 4);
        assert_eq!(summary.ignored, 3);
        assert_eq!(summary.unsupported_commands, 2);
        assert_eq!(summary.unsupported_conditions, 1);
        assert_eq!(summary.invalid_keys, 1);
        assert_eq!(summary.conflicts, 1);
        assert_eq!(summary.disabled_by_terminal_capability, 0);
        assert_eq!(imported.report.total_classified(), 12);

        assert!(imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::CursorDown
                && binding.source == Source::Imported
                && binding
                    .when
                    .as_ref()
                    .is_some_and(|when| when.to_string() == "editorFocus")
        }));
        assert!(imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::SelectionDown
                && binding
                    .when
                    .as_ref()
                    .is_some_and(|when| when.to_string() == "editorFocus")
        }));
        assert!(imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::CursorDown
                && binding
                    .when
                    .as_ref()
                    .is_some_and(|when| when.to_string() == "textInputFocus && !isReadonly")
        }));
        assert!(imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::CursorUp
                && format_key_for_config(&binding.keys) == "ctrl+x"
        }));
        assert!(!imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::CursorDown
                && format_key_for_config(&binding.keys) == "ctrl+x"
        }));

        assert_eq!(imported.report.ignored[0].reason, "outside editor scope");
        assert_eq!(
            imported.report.unsupported_commands[0].reason,
            "feature not implemented"
        );
        assert_eq!(
            imported.report.unsupported_commands[1].reason,
            "negative binding is not supported in MVP"
        );
        assert!(
            imported.report.unsupported_conditions[0]
                .reason
                .contains("unsupported VS Code when syntax")
        );
    }

    #[test]
    fn empty_command_is_classified_as_ignored_unbind_not_unsupported() {
        let fixture = r#"[
            { "key": "cmd+shift+i", "command": "" },
            { "key": "cmd+ctrl+i", "command": "" }
        ]"#;

        let imported = import_vscode_keybindings(fixture, &KeyboardCapabilities::modern()).unwrap();
        let summary = imported.report.summary();

        assert_eq!(summary.ignored, 2);
        assert_eq!(summary.unsupported_commands, 0);
        assert!(
            imported
                .report
                .ignored
                .iter()
                .all(|entry| entry.reason.contains("unbind")),
            "{:?}",
            imported.report.ignored
        );
    }

    #[test]
    fn report_render_contains_counts_and_full_entry_listing() {
        let imported = import_vscode_keybindings(
            r#"[
                { "key": "ctrl+j", "command": "cursorDown", "when": "editorFocus" },
                { "key": "cmd+t", "command": "workbench.action.terminal.new" },
                { "key": "f2", "command": "editor.action.rename" },
                { "key": "ctrl+m", "command": "cursorDown", "when": "resourceLangId == markdown" },
                { "key": "ctrl+[IntlBackslash]", "command": "cursorDown" }
            ]"#,
            &KeyboardCapabilities::modern(),
        )
        .unwrap();

        let report = imported.report.render_text();
        for expected in [
            "VS Code keybinding import completed.",
            "Imported: 1",
            "Ignored: 1",
            "Unsupported commands: 1",
            "Unsupported conditions: 1",
            "Invalid keys: 1",
            "Conflicts: 0",
            "Disabled by terminal capability: 0",
            "Imported (1):",
            "- ctrl+j -> cursor.down [editorFocus] [imported]",
            "Ignored (1):",
            "- cmd+t -> workbench.action.terminal.new [outside editor scope]",
            "Unsupported commands (1):",
            "- f2 -> editor.action.rename [feature not implemented]",
            "Unsupported conditions (1):",
            "- ctrl+m -> cursorDown [resourceLangId == markdown]",
            "Invalid keys (1):",
            "- ctrl+[IntlBackslash] -> cursorDown",
        ] {
            assert!(
                report.contains(expected),
                "missing `{expected}` in\n{report}"
            );
        }
        // "Examples:" sampling is gone in favor of full per-bucket listings
        // (TASK-260712-17); zero-count buckets (Conflicts, Disabled) must
        // not get a heading either.
        assert!(!report.contains("Examples:"), "{report}");
        assert!(!report.contains("Conflicts (0)"), "{report}");
        assert!(
            !report.contains("Disabled by terminal capability (0)"),
            "{report}"
        );
    }

    /// A multi-entry bucket must list every entry, not just the first one
    /// (TASK-260712-17 testcase: 複数 entry を含む bucket の全 entry が出力に現れる).
    #[test]
    fn report_render_lists_every_entry_in_a_multi_entry_bucket() {
        let imported = import_vscode_keybindings(
            r#"[
                { "key": "cmd+t", "command": "workbench.action.terminal.new" },
                { "key": "cmd+p", "command": "workbench.action.quickOpen" },
                { "key": "cmd+shift+e", "command": "workbench.view.explorer" }
            ]"#,
            &KeyboardCapabilities::modern(),
        )
        .unwrap();

        assert_eq!(imported.report.ignored.len(), 3);
        let report = imported.report.render_text();
        assert!(report.contains("Ignored (3):"), "{report}");
        for expected in [
            "- cmd+t -> workbench.action.terminal.new [outside editor scope]",
            "- cmd+p -> workbench.action.quickOpen [outside editor scope]",
            "- cmd+shift+e -> workbench.view.explorer [outside editor scope]",
        ] {
            assert!(
                report.contains(expected),
                "missing `{expected}` in\n{report}"
            );
        }
    }

    /// Table-driven per TASK-260712-16 testcases: a legacy terminal disables
    /// exactly the chords SPEC-0003's four rules say it must, with the
    /// documented fixed reason string, and never disables an arrow-key
    /// chord (`ctrl+shift+right`) because legacy CSI already carries that
    /// modifier information.
    #[test]
    fn legacy_capability_disables_undeliverable_chords_table_driven() {
        let cases: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "super chord",
                "cmd+s",
                "workbench.action.files.save",
                Some("terminal cannot deliver Cmd/Super"),
            ),
            (
                "ctrl+shift character",
                "ctrl+shift+j",
                "cursorDownSelect",
                Some("terminal cannot distinguish Ctrl+Shift+J from Ctrl+J"),
            ),
            (
                "shift+enter",
                "shift+enter",
                "cursorDown",
                Some("terminal cannot receive Shift+Enter"),
            ),
            (
                "ctrl+enter",
                "ctrl+enter",
                "cursorUp",
                Some("terminal cannot receive Ctrl+Enter"),
            ),
            (
                "ctrl+shift+right stays deliverable (legacy CSI carries the modifier)",
                "ctrl+shift+right",
                "cursorRightSelect",
                None,
            ),
        ];

        for (name, key, command, expected_reason) in cases {
            let fixture = format!(r#"[{{ "key": "{key}", "command": "{command}" }}]"#);
            let imported =
                import_vscode_keybindings(&fixture, &KeyboardCapabilities::legacy()).unwrap();
            let summary = imported.report.summary();

            match expected_reason {
                Some(reason) => {
                    assert_eq!(summary.disabled_by_terminal_capability, 1, "{name}");
                    assert_eq!(summary.imported, 0, "{name}");
                    assert_eq!(
                        imported.report.disabled_by_terminal_capability[0].reason, *reason,
                        "{name}"
                    );
                    assert!(
                        imported.bindings.is_empty(),
                        "{name}: a disabled chord must not become a binding"
                    );
                }
                None => {
                    assert_eq!(summary.disabled_by_terminal_capability, 0, "{name}");
                    assert_eq!(summary.imported, 1, "{name}");
                }
            }
        }
    }

    #[test]
    fn modern_capability_imports_every_chord_legacy_would_disable() {
        let fixture = r#"[
            { "key": "cmd+s", "command": "workbench.action.files.save" },
            { "key": "ctrl+shift+j", "command": "cursorDownSelect" },
            { "key": "shift+enter", "command": "cursorDown" },
            { "key": "ctrl+enter", "command": "cursorUp" },
            { "key": "ctrl+shift+right", "command": "cursorRightSelect" }
        ]"#;

        let imported = import_vscode_keybindings(fixture, &KeyboardCapabilities::modern()).unwrap();
        let summary = imported.report.summary();

        assert_eq!(summary.disabled_by_terminal_capability, 0);
        assert_eq!(summary.imported, 5);
    }

    #[test]
    fn format_key_for_config_round_trips_through_parser() {
        for input in ["ctrl+shift+j", "cmd+s", "ctrl+k ctrl+s", "f1", "ctrl+space"] {
            let parsed = parse_key_sequence(input).unwrap();
            let formatted = format_key_for_config(&parsed);
            assert_eq!(parse_key_sequence(&formatted), Ok(parsed), "{input}");
            assert_eq!(formatted, input);
        }
    }
}
