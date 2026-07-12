//! VS Code `keybindings.json` importer (SPEC-0004).

use serde_json::Value;

use crate::input::{Key, KeyEvent, KeyboardCapabilities, Modifiers};

use super::{
    Binding, ContextPredicate, EditorAction, ImportReport, ReportEntry, Source,
    action_for_vscode_command, context::RESERVED_FALSE_KEYS, parse_key_sequence,
    user_bindings::strip_jsonc_comments, vscode_when::convert_vscode_when,
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

/// How the importer handles Cmd/Super chords (ADR-0007 §3). Chords without
/// Super are never affected by any strategy.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum CmdStrategy {
    /// Import `cmd+*` as-is (a Super chord). Recommended only on terminals
    /// known to deliver Super; `keymap verify` (ADR-0007) is the real source
    /// of truth. This is the default because Super *is* deliverable on
    /// modern terminals (Ghostty 1.3.1 measurement, ADR-0007) and downgrading
    /// unconditionally would throw away muscle memory for users it works for.
    #[default]
    Keep,
    /// Replace every Super chord with the equivalent Ctrl chord (dropping
    /// Super, keeping any other modifiers already present) before importing
    /// — mirrors VS Code's own macOS/Windows keymap convention.
    Ctrl,
    /// Import both: the original Super binding (as `Keep`) and a
    /// Ctrl-converted variant, each independently classified in the report.
    Both,
}

pub fn import_vscode_keybindings(
    text: &str,
    capabilities: &KeyboardCapabilities,
    cmd_strategy: CmdStrategy,
) -> Result<VsCodeImport, VsCodeImportError> {
    let stripped = strip_jsonc_comments(text);
    let value: Value = serde_json::from_str(&stripped)
        .map_err(|error| VsCodeImportError::InvalidJson(error.to_string()))?;
    let entries = value.as_array().ok_or(VsCodeImportError::RootNotArray)?;

    let mut bindings = Vec::new();
    let mut report = ImportReport::default();
    // `--cmd=both` can classify a single VS Code entry into two report
    // entries (the original Super binding plus a synthesized Ctrl variant),
    // so the "every entry lands somewhere" invariant below must account for
    // those extras separately from `entries.len()`.
    let mut synthesized = 0usize;

    for entry in entries {
        synthesized += classify_entry(
            entry,
            &mut bindings,
            &mut report,
            capabilities,
            cmd_strategy,
        );
    }

    debug_assert_eq!(entries.len() + synthesized, report.total_classified());
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

/// Classifies one VS Code entry, mutating `bindings`/`report`, and returns
/// how many *extra* report entries beyond the usual one it produced — always
/// 0 except for `CmdStrategy::Both` on a Super chord, which additionally
/// synthesizes a Ctrl variant (see `import_vscode_keybindings`'s invariant).
fn classify_entry(
    entry: &Value,
    bindings: &mut Vec<Binding>,
    report: &mut ImportReport,
    capabilities: &KeyboardCapabilities,
    cmd_strategy: CmdStrategy,
) -> usize {
    let key = string_field(entry, "key").map(ToOwned::to_owned);
    let command = string_field(entry, "command").map(ToOwned::to_owned);
    let when = string_field(entry, "when").map(ToOwned::to_owned);

    let Some(command_text) = command.as_deref() else {
        report
            .unsupported_commands
            .push(ReportEntry::new(key, command, when, "missing command"));
        return 0;
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
        return 0;
    }

    if command_text.starts_with('-') {
        report.unsupported_commands.push(ReportEntry::new(
            key,
            command,
            when,
            "negative binding is not supported in MVP",
        ));
        return 0;
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
        return 0;
    };

    let Some(key_text) = key.as_deref() else {
        report
            .invalid_keys
            .push(ReportEntry::new(key, command, when, "missing key"));
        return 0;
    };
    let parsed_keys = match parse_key_sequence(key_text) {
        Ok(keys) => keys,
        Err(error) => {
            report
                .invalid_keys
                .push(ReportEntry::new(key, command, when, error.to_string()));
            return 0;
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
                return 0;
            }
        },
        None => None,
    };

    // Reservation belongs to the original VS Code gesture. Check before
    // `--cmd=ctrl|both`; remapping Cmd+Q must not make an inherently
    // non-portable source binding look successfully imported.
    if sequence_contains_os_reserved_key(&parsed_keys) {
        report.unsupported_commands.push(ReportEntry::new(
            Some(format_key_for_config(&parsed_keys)),
            Some(action.as_str().to_string()),
            parsed_when.as_ref().map(ToString::to_string),
            "OS/terminal reserved",
        ));
        return 0;
    }

    // ADR-0007 §3: `--cmd=ctrl`/`--cmd=both` only ever touch chords that
    // actually carry Super — a sequence with no Super chord at all is
    // classified exactly once, identically to `Keep`.
    match cmd_strategy {
        CmdStrategy::Keep => {
            finalize_binding(
                bindings,
                report,
                capabilities,
                parsed_keys,
                action,
                parsed_when,
                false,
            );
            0
        }
        CmdStrategy::Ctrl => {
            let has_super = sequence_has_super(&parsed_keys);
            let keys = if has_super {
                convert_super_chords_to_ctrl(&parsed_keys)
            } else {
                parsed_keys
            };
            finalize_binding(
                bindings,
                report,
                capabilities,
                keys,
                action,
                parsed_when,
                has_super,
            );
            0
        }
        CmdStrategy::Both => {
            let has_super = sequence_has_super(&parsed_keys);
            // The original Super binding is registered exactly like `Keep`.
            finalize_binding(
                bindings,
                report,
                capabilities,
                parsed_keys.clone(),
                action,
                parsed_when.clone(),
                false,
            );
            if !has_super {
                return 0;
            }
            // Plus a synthesized Ctrl variant — this is the one extra report
            // entry `import_vscode_keybindings`'s invariant accounts for.
            let converted_keys = convert_super_chords_to_ctrl(&parsed_keys);
            finalize_binding(
                bindings,
                report,
                capabilities,
                converted_keys,
                action,
                parsed_when,
                true,
            );
            1
        }
    }
}

/// Runs the capability / conflict / inactive-context / imported
/// classification pipeline shared by every `CmdStrategy` variant, for one
/// already-parsed `(keys, action, when)` combination. `converted` marks a
/// chord set that had its Super modifier remapped to Ctrl (ADR-0007 §3):
/// it only changes the reason text appended to whichever bucket this variant
/// lands in, never the classification logic itself.
fn finalize_binding(
    bindings: &mut Vec<Binding>,
    report: &mut ImportReport,
    capabilities: &KeyboardCapabilities,
    keys: Vec<KeyEvent>,
    action: EditorAction,
    when: Option<ContextPredicate>,
    converted: bool,
) {
    // Chord-level deliverability check (SPEC-0003 / TASK-260712-16): a
    // sequence can't be "half" pressed, so any single undeliverable chord
    // disables the whole binding. Checked before conflict detection, so a
    // capability-disabled entry is never treated as a "previous" binding a
    // later import could overwrite.
    if let Some(reason) = sequence_disable_reason(&keys, capabilities) {
        let reason = if reason == SUPER_UNDELIVERABLE_REASON {
            format!("{reason} — retry import with --cmd=ctrl to remap")
        } else {
            reason
        };
        let reason = if converted {
            append_cmd_remap_note(reason)
        } else {
            reason
        };
        report
            .disabled_by_terminal_capability
            .push(ReportEntry::new(
                Some(format_key_for_config(&keys)),
                Some(action.as_str().to_string()),
                when.as_ref().map(ToString::to_string),
                reason,
            ));
        return;
    }

    if let Some(existing_index) = bindings
        .iter()
        .position(|binding| binding.keys == keys && binding.when == when)
    {
        let previous = bindings.remove(existing_index);
        remove_previous_report_entry(report, &previous);
        report.conflicts.push(ReportEntry::new(
            Some(format_key_for_config(&previous.keys)),
            Some(previous.action.as_str().to_string()),
            previous.when.as_ref().map(ToString::to_string),
            "overwritten by later VS Code binding",
        ));
    }

    // SPEC-0002/0004: `suggestVisible`/`quickOpenVisible` are reserved for
    // future UI and never true in the MVP runtime context, so a `when` that
    // positively requires one can never fire. The binding is still written
    // to the generated output below (it becomes live automatically if a
    // future version starts setting the flag) — only the report bucket
    // differs from a plain `imported` (product rule: never silently break).
    let inactive_key = when
        .as_ref()
        .and_then(|predicate| predicate.positive_term_matching(RESERVED_FALSE_KEYS))
        .map(str::to_string);

    let binding = Binding::new(keys, action, when, Source::Imported);
    let key_column = Some(format_key_for_config(&binding.keys));
    let command_column = Some(action.as_str().to_string());
    let when_column = binding.when.as_ref().map(ToString::to_string);

    if let Some(blocked_key) = inactive_key {
        let reason = format!(
            "when references '{blocked_key}', which coda never activates (reserved for future UI)"
        );
        let reason = if converted {
            append_cmd_remap_note(reason)
        } else {
            reason
        };
        report.inactive_contexts.push(ReportEntry::new(
            key_column,
            command_column,
            when_column,
            reason,
        ));
    } else {
        let reason = if converted {
            append_cmd_remap_note(imported_reason(action).to_string())
        } else {
            imported_reason(action).to_string()
        };
        report.imported.push(ReportEntry::new(
            key_column,
            command_column,
            when_column,
            reason,
        ));
    }
    bindings.push(binding);
}

/// Universally non-portable combinations explicitly named by ADR-0007 §4.
/// Keep this list deliberately small: terminal-specific reservations belong
/// to quirks/verify, not to an ever-growing guessed database.
fn sequence_contains_os_reserved_key(keys: &[KeyEvent]) -> bool {
    keys.iter().any(|key| {
        key.modifiers.contains_super()
            && !key.modifiers.contains_ctrl()
            && !key.modifiers.contains_alt()
            && !key.modifiers.contains_shift()
            && matches!(key.key, Key::Char('q') | Key::Tab)
    })
}

/// A previously-classified binding can be sitting in either `imported` or
/// `inactive_contexts` (Feature 2 splits what used to be one bucket) —
/// checked in that order since `imported` is the far more common case.
fn remove_previous_report_entry(report: &mut ImportReport, previous: &Binding) {
    let key = format_key_for_config(&previous.keys);
    let matches = |entry: &ReportEntry| {
        entry.key.as_deref() == Some(key.as_str())
            && entry.command.as_deref() == Some(previous.action.as_str())
            && entry.when == previous.when.as_ref().map(ToString::to_string)
    };
    if let Some(index) = report.imported.iter().position(&matches) {
        report.imported.remove(index);
        return;
    }
    if let Some(index) = report.inactive_contexts.iter().position(&matches) {
        report.inactive_contexts.remove(index);
    }
}

fn sequence_has_super(keys: &[KeyEvent]) -> bool {
    keys.iter().any(|key| key.modifiers.contains_super())
}

/// ADR-0007 §3 `--cmd=ctrl`/`--cmd=both`: remaps every chord that carries
/// Super to Ctrl instead (dropping Super, keeping any Alt/Shift already
/// present); chords without Super pass through unchanged. `cmd+ctrl+s`
/// collapses to `ctrl+s` (Super dropped, Ctrl already present) rather than
/// becoming a nonsensical doubled modifier.
fn convert_super_chords_to_ctrl(keys: &[KeyEvent]) -> Vec<KeyEvent> {
    keys.iter().map(convert_chord_super_to_ctrl).collect()
}

fn convert_chord_super_to_ctrl(key: &KeyEvent) -> KeyEvent {
    if !key.modifiers.contains_super() {
        return key.clone();
    }
    let mut modifiers = Modifiers::none().with_ctrl();
    if key.modifiers.contains_alt() {
        modifiers = modifiers.with_alt();
    }
    if key.modifiers.contains_shift() {
        modifiers = modifiers.with_shift();
    }
    KeyEvent::new(key.key.clone(), modifiers)
}

/// Fixed reason string `capability_disable_reason` returns for the Super
/// case — checked by identity in `finalize_binding` to decide whether to
/// append the `--cmd=ctrl` retry suggestion, so keep the two in sync.
const SUPER_UNDELIVERABLE_REASON: &str = "terminal cannot deliver Cmd/Super";

fn append_cmd_remap_note(reason: impl Into<String>) -> String {
    format!("{} — cmd remapped to ctrl (--cmd=ctrl)", reason.into())
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
        return Some(SUPER_UNDELIVERABLE_REASON.to_string());
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
    // Suggest/quick-open `when` predicates that can never be true are routed
    // to `report.inactive_contexts` instead of reaching this function at all
    // (see `finalize_binding`'s `inactive_key` check) — this always means a
    // binding that both parsed and can actually fire.
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
    use super::{CmdStrategy, format_key_for_config, import_vscode_keybindings};
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

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Keep)
                .unwrap();
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

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Keep)
                .unwrap();
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
            CmdStrategy::Keep,
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
            CmdStrategy::Keep,
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
    /// modifier information. The Super case additionally covers ADR-0007
    /// §3's `--cmd=ctrl` retry suggestion (appended for `CmdStrategy::Keep`
    /// whenever the disable reason is specifically Super non-delivery).
    #[test]
    fn legacy_capability_disables_undeliverable_chords_table_driven() {
        let cases: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "super chord",
                "cmd+s",
                "workbench.action.files.save",
                Some("terminal cannot deliver Cmd/Super — retry import with --cmd=ctrl to remap"),
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
            let imported = import_vscode_keybindings(
                &fixture,
                &KeyboardCapabilities::legacy(),
                CmdStrategy::Keep,
            )
            .unwrap();
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

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Keep)
                .unwrap();
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

    // --- CmdStrategy (ADR-0007 §3) ---

    /// `--cmd=ctrl`: a Super chord is imported with Super replaced by Ctrl,
    /// and the report's key column and reason reflect the conversion.
    #[test]
    fn ctrl_strategy_converts_cmd_chord_to_ctrl() {
        let fixture = r#"[{ "key": "cmd+s", "command": "workbench.action.files.save" }]"#;

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Ctrl)
                .unwrap();

        assert_eq!(
            imported.report.summary().imported,
            1,
            "{:?}",
            imported.report
        );
        assert_eq!(imported.report.imported[0].key.as_deref(), Some("ctrl+s"));
        assert!(
            imported.report.imported[0]
                .reason
                .contains("cmd remapped to ctrl (--cmd=ctrl)"),
            "{:?}",
            imported.report.imported[0]
        );
        assert!(imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::FileSave
                && format_key_for_config(&binding.keys) == "ctrl+s"
        }));
    }

    /// A `cmd+s` converted to `ctrl+s` under `--cmd=ctrl` goes through the
    /// same conflict detection as any other binding: an explicit `ctrl+s`
    /// imported earlier is overwritten (lands in `conflicts`) and the later,
    /// converted binding wins (lands in `imported`).
    #[test]
    fn ctrl_strategy_conversion_collides_with_explicit_ctrl_binding() {
        let fixture = r#"[
            { "key": "ctrl+s", "command": "cursorDown" },
            { "key": "cmd+s", "command": "workbench.action.files.save" }
        ]"#;

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Ctrl)
                .unwrap();
        let summary = imported.report.summary();

        assert_eq!(summary.conflicts, 1, "{:?}", imported.report);
        assert_eq!(summary.imported, 1, "{:?}", imported.report);
        assert_eq!(
            imported.report.conflicts[0].command.as_deref(),
            Some("cursor.down")
        );
        assert!(imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::FileSave
                && format_key_for_config(&binding.keys) == "ctrl+s"
        }));
        assert!(
            !imported
                .bindings
                .iter()
                .any(|binding| binding.action == EditorAction::CursorDown),
            "the overwritten binding must not remain in the generated output"
        );
    }

    /// `cmd+ctrl+s` under `--cmd=ctrl` collapses to plain `ctrl+s` (Super
    /// dropped, Ctrl already present) rather than a doubled modifier.
    #[test]
    fn ctrl_strategy_cmd_ctrl_chord_collapses_to_ctrl() {
        let fixture = r#"[{ "key": "cmd+ctrl+s", "command": "workbench.action.files.save" }]"#;

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Ctrl)
                .unwrap();

        assert_eq!(imported.report.imported[0].key.as_deref(), Some("ctrl+s"));
    }

    /// A sequence converts every Super chord independently: `cmd+k cmd+s`
    /// becomes `ctrl+k ctrl+s`.
    #[test]
    fn ctrl_strategy_converts_every_chord_in_a_sequence() {
        let fixture = r#"[{ "key": "cmd+k cmd+s", "command": "workbench.action.files.save" }]"#;

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Ctrl)
                .unwrap();

        assert_eq!(
            imported.report.imported[0].key.as_deref(),
            Some("ctrl+k ctrl+s")
        );
    }

    /// `--cmd=both` registers both the original Super binding and a
    /// synthesized Ctrl variant, each landing in the generated output; only
    /// the synthesized one carries the remap note.
    #[test]
    fn both_strategy_registers_original_and_converted_variant() {
        let fixture = r#"[{ "key": "cmd+s", "command": "workbench.action.files.save" }]"#;

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Both)
                .unwrap();

        assert_eq!(
            imported.report.summary().imported,
            2,
            "{:?}",
            imported.report
        );
        assert_eq!(imported.report.total_classified(), 2);
        assert!(imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::FileSave
                && format_key_for_config(&binding.keys) == "cmd+s"
        }));
        assert!(imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::FileSave
                && format_key_for_config(&binding.keys) == "ctrl+s"
        }));
        assert_eq!(
            imported
                .report
                .imported
                .iter()
                .filter(|entry| entry.reason.contains("cmd remapped to ctrl"))
                .count(),
            1,
            "{:?}",
            imported.report.imported
        );
    }

    /// The synthesized Ctrl variant from `--cmd=both` participates in
    /// conflict detection exactly like a normal binding: colliding with an
    /// explicit `ctrl+s` moves the explicit one into `conflicts`, while both
    /// the original `cmd+s` and the winning synthesized `ctrl+s` stay
    /// `imported` — this also exercises the `entries.len() + synthesized ==
    /// total_classified()` invariant (`debug_assert_eq!` in
    /// `import_vscode_keybindings`) with a non-trivial synthesized count.
    #[test]
    fn both_strategy_synthesized_variant_can_conflict() {
        let fixture = r#"[
            { "key": "ctrl+s", "command": "cursorDown" },
            { "key": "cmd+s", "command": "workbench.action.files.save" }
        ]"#;

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Both)
                .unwrap();
        let summary = imported.report.summary();

        assert_eq!(summary.conflicts, 1, "{:?}", imported.report);
        assert_eq!(summary.imported, 2, "{:?}", imported.report);
        assert_eq!(imported.report.total_classified(), 3);
        assert!(
            !imported
                .bindings
                .iter()
                .any(|binding| binding.action == EditorAction::CursorDown),
            "the overwritten explicit ctrl+s must not remain in the generated output"
        );
    }

    // --- Inactive context bucket (reserved-for-future-UI `when` keys) ---

    /// A binding whose `when` positively requires a reserved-for-future-UI
    /// context key (SPEC-0002/0004: `suggestVisible`/`quickOpenVisible` are
    /// always false in the MVP) is classified as inactive rather than
    /// imported, but is still written to the generated bindings output so it
    /// activates automatically if a future coda version starts setting the
    /// flag.
    #[test]
    fn inactive_context_binding_is_reported_separately_but_still_generated() {
        let fixture =
            r#"[{ "key": "ctrl+n", "command": "cursorDown", "when": "suggestWidgetVisible" }]"#;

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Keep)
                .unwrap();
        let summary = imported.report.summary();

        assert_eq!(summary.inactive_contexts, 1, "{:?}", imported.report);
        assert_eq!(summary.imported, 0, "{:?}", imported.report);
        assert!(
            imported.report.inactive_contexts[0]
                .reason
                .contains("suggestVisible"),
            "{:?}",
            imported.report.inactive_contexts[0]
        );
        assert!(imported.bindings.iter().any(|binding| {
            binding.action == EditorAction::CursorDown
                && binding
                    .when
                    .as_ref()
                    .is_some_and(|when| when.to_string() == "suggestVisible")
        }));
    }

    /// A positive reserved-key term anywhere in a conjunction blocks the
    /// whole binding, not just a bare single-term predicate.
    #[test]
    fn inactive_context_detected_within_conjunction() {
        let fixture = r#"[{
            "key": "ctrl+n",
            "command": "cursorDown",
            "when": "editorFocus && suggestWidgetVisible"
        }]"#;

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Keep)
                .unwrap();

        assert_eq!(imported.report.summary().inactive_contexts, 1);
    }

    /// A *negated* reserved-key term (`!suggestWidgetVisible`) is
    /// always-true, so the binding must land in `imported`, not
    /// `inactive_contexts` — precision matters here per the importer's
    /// design (negation must not be mistaken for a positive requirement).
    #[test]
    fn negated_reserved_context_is_not_inactive() {
        let fixture = r#"[{
            "key": "ctrl+n",
            "command": "cursorDown",
            "when": "!suggestWidgetVisible"
        }]"#;

        let imported =
            import_vscode_keybindings(fixture, &KeyboardCapabilities::modern(), CmdStrategy::Keep)
                .unwrap();
        let summary = imported.report.summary();

        assert_eq!(summary.inactive_contexts, 0, "{:?}", imported.report);
        assert_eq!(summary.imported, 1, "{:?}", imported.report);
    }

    /// The "Inactive" section only appears in the rendered report when the
    /// bucket is non-empty, matching every other bucket's convention
    /// (TASK-260712-17).
    #[test]
    fn inactive_section_appears_in_render_text_only_when_non_empty() {
        let without_inactive = import_vscode_keybindings(
            r#"[{ "key": "ctrl+j", "command": "cursorDown" }]"#,
            &KeyboardCapabilities::modern(),
            CmdStrategy::Keep,
        )
        .unwrap();
        assert!(
            !without_inactive
                .report
                .render_text()
                .contains("Inactive (when-context never active in coda) ("),
            "{}",
            without_inactive.report.render_text()
        );

        let with_inactive = import_vscode_keybindings(
            r#"[{ "key": "ctrl+n", "command": "cursorDown", "when": "suggestWidgetVisible" }]"#,
            &KeyboardCapabilities::modern(),
            CmdStrategy::Keep,
        )
        .unwrap();
        let report = with_inactive.report.render_text();
        assert!(
            report.contains("Inactive (when-context never active in coda) (1):"),
            "{report}"
        );
        assert!(
            report.contains("- ctrl+n -> cursor.down [suggestVisible]"),
            "{report}"
        );
    }

    #[test]
    fn os_reserved_cmd_keys_are_unsupported_before_cmd_remapping() {
        for strategy in [CmdStrategy::Keep, CmdStrategy::Ctrl, CmdStrategy::Both] {
            let imported = import_vscode_keybindings(
                r#"[
                    { "key": "cmd+q", "command": "workbench.action.files.save" },
                    { "key": "cmd+tab", "command": "workbench.action.files.save" }
                ]"#,
                &KeyboardCapabilities::modern(),
                strategy,
            )
            .unwrap();
            assert!(imported.bindings.is_empty(), "{strategy:?}");
            assert_eq!(
                imported.report.unsupported_commands.len(),
                2,
                "{strategy:?}"
            );
            assert!(
                imported
                    .report
                    .unsupported_commands
                    .iter()
                    .all(|entry| entry.reason == "OS/terminal reserved")
            );
        }
    }
}
