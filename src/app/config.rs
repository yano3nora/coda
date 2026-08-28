//! App configuration discovery and key binding loading.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    core::editor::{IndentConfig, IndentStyle},
    highlight::ThemeChoice,
    keymap::{Binding, Source, load_bindings_with_source, load_user_bindings, parse_key_chord},
};

use crate::input::KeyEvent;

use super::import_cli::config_base_dir;

/// Default for `[keymap] sequence_timeout_ms` (SPEC-0005); mirrors the
/// event loop's historical hardcoded value.
pub(crate) const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 800;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub user_bindings: Vec<Binding>,
    pub warnings: Vec<String>,
    pub theme: ThemeChoice,
    /// Startup default for visual line wrap (`[editor] wrap`,
    /// TASK-260711-18). `view.toggleWrap` flips it at runtime.
    pub wrap: bool,
    /// `[editor] indent_style` / `indent_width` (TASK-260828): unit used by
    /// Tab insertion and `edit.indent`/`edit.outdent`.
    pub indent: IndentConfig,
    /// `[keymap] sequence_timeout_ms`: how long a pending key sequence waits
    /// for its next chord before the exact match (if any) fires (SPEC-0002).
    pub sequence_timeout_ms: u64,
    /// `[keymap] palette_key`: replacement chord for the `ctrl+space`
    /// convenience rescue binding. `None` keeps the default. F1 stays
    /// hardwired regardless (SPEC-0002 rescue rule).
    pub palette_key: Option<KeyEvent>,
    /// `[terminal] capability_warning`: whether the startup legacy-terminal
    /// warning is shown. The detection itself always runs — this only gates
    /// the status-bar message (SPEC-0005).
    pub capability_warning: bool,
    /// Explicit Ctrl+C behavior. `false` preserves the safe copy default;
    /// users who have verified Cmd+C delivery may opt into quit.
    pub ctrl_c_quits: bool,
    /// Chords measured as mismatched for the current terminal identity.
    pub disabled_chords: Vec<KeyEvent>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            user_bindings: Vec::new(),
            warnings: Vec::new(),
            theme: ThemeChoice::Dark,
            wrap: false,
            indent: IndentConfig::default(),
            sequence_timeout_ms: DEFAULT_SEQUENCE_TIMEOUT_MS,
            palette_key: None,
            capability_warning: true,
            ctrl_c_quits: false,
            disabled_chords: Vec::new(),
        }
    }
}

pub fn load() -> AppConfig {
    let Some(base_dir) = config_base_dir() else {
        return AppConfig {
            warnings: vec!["HOME is not set; skipped user/imported bindings".to_string()],
            ..AppConfig::default()
        };
    };
    load_from_base_dir(&base_dir)
}

pub(crate) fn load_from_base_dir(base_dir: &Path) -> AppConfig {
    let mut bindings = Vec::new();
    let mut warnings = Vec::new();

    let generated = load_bindings_file(
        &base_dir.join("generated").join("vscode-bindings.json"),
        Source::Imported,
        "generated/vscode-bindings.json",
    );
    bindings.extend(generated.bindings);
    warnings.extend(generated.warnings);

    let user = load_bindings_file(
        &base_dir.join("bindings.json"),
        Source::User,
        "bindings.json",
    );
    bindings.extend(user.bindings);
    warnings.extend(user.warnings);

    let mut config = load_config_toml(&base_dir.join("config.toml"), &mut warnings);

    config.user_bindings = bindings;
    config.disabled_chords = super::verify_cli::load_disabled_chords(base_dir, &mut warnings);
    config.warnings = warnings;
    config
}

/// Reads `config.toml` once and extracts every supported setting into an
/// `AppConfig` (bindings/warnings are filled in by the caller). Broken or
/// missing values never abort startup: each falls back to its default with a
/// warning (silent-breakage rule, AGENTS.md).
fn load_config_toml(path: &Path, warnings: &mut Vec<String>) -> AppConfig {
    let mut config = AppConfig::default();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return config,
        Err(error) => {
            warnings.push(format!(
                "{}: {error}; using default settings",
                path.display()
            ));
            return config;
        }
    };

    let value = match toml::from_str::<toml::Value>(&text) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!(
                "{}: {error}; using default settings",
                path.display()
            ));
            return config;
        }
    };

    if let Some(theme) = value
        .get("appearance")
        .and_then(|appearance| appearance.get("theme"))
        .and_then(toml::Value::as_str)
    {
        config.theme = ThemeChoice::parse(theme).unwrap_or_else(|| {
            warnings.push(format!(
                "{}: unsupported appearance.theme={theme:?}; using default dark theme",
                path.display()
            ));
            ThemeChoice::Dark
        });
    }

    let editor = value.get("editor");

    match editor.and_then(|editor| editor.get("wrap")) {
        Some(toml::Value::Boolean(wrap)) => config.wrap = *wrap,
        Some(other) => {
            warnings.push(format!(
                "{}: editor.wrap must be true or false, got {other}; using wrap = false",
                path.display()
            ));
        }
        None => {}
    }

    match editor.and_then(|editor| editor.get("indent_style")) {
        Some(toml::Value::String(style)) if style == "space" => {
            config.indent.style = IndentStyle::Space;
        }
        Some(toml::Value::String(style)) if style == "tab" => {
            config.indent.style = IndentStyle::Tab;
        }
        Some(other) => {
            warnings.push(format!(
                "{}: editor.indent_style must be \"space\" or \"tab\", got {other}; using space",
                path.display()
            ));
        }
        None => {}
    }

    match editor.and_then(|editor| editor.get("indent_width")) {
        // Zero would turn indent/outdent and Tab insertion into no-ops under
        // the space style; the upper bound is IndentConfig::MAX_WIDTH (see
        // its doc). Out-of-range values are configuration mistakes.
        Some(toml::Value::Integer(width))
            if (1..=IndentConfig::MAX_WIDTH as i64).contains(width) =>
        {
            config.indent.width = *width as usize;
        }
        Some(other) => {
            warnings.push(format!(
                "{}: editor.indent_width must be an integer between 1 and {}, got {other}; using 4",
                path.display(),
                IndentConfig::MAX_WIDTH
            ));
        }
        None => {}
    }

    let keymap = value.get("keymap");

    match keymap.and_then(|keymap| keymap.get("sequence_timeout_ms")) {
        // Zero would fire the exact match before a sequence can ever be
        // typed, effectively disabling multi-chord bindings — treat it as a
        // configuration mistake, not a feature.
        Some(toml::Value::Integer(ms)) if *ms > 0 => config.sequence_timeout_ms = *ms as u64,
        Some(other) => {
            warnings.push(format!(
                "{}: keymap.sequence_timeout_ms must be a positive integer, got {other}; \
                 using {DEFAULT_SEQUENCE_TIMEOUT_MS}",
                path.display()
            ));
        }
        None => {}
    }

    match keymap.and_then(|keymap| keymap.get("palette_key")) {
        Some(toml::Value::String(chord)) => match parse_key_chord(chord) {
            Ok(key) => config.palette_key = Some(key),
            Err(error) => {
                warnings.push(format!(
                    "{}: keymap.palette_key {chord:?} is invalid ({error}); keeping ctrl+space",
                    path.display()
                ));
            }
        },
        Some(other) => {
            warnings.push(format!(
                "{}: keymap.palette_key must be a string like \"ctrl+space\", got {other}; \
                 keeping ctrl+space",
                path.display()
            ));
        }
        None => {}
    }

    match keymap.and_then(|keymap| keymap.get("ctrl_c")) {
        Some(toml::Value::String(value)) if value == "copy" => config.ctrl_c_quits = false,
        Some(toml::Value::String(value)) if value == "quit" => config.ctrl_c_quits = true,
        Some(other) => warnings.push(format!(
            "{}: keymap.ctrl_c must be \"copy\" or \"quit\", got {other}; using copy",
            path.display()
        )),
        None => {}
    }

    match value
        .get("terminal")
        .and_then(|terminal| terminal.get("capability_warning"))
    {
        Some(toml::Value::Boolean(enabled)) => config.capability_warning = *enabled,
        Some(other) => {
            warnings.push(format!(
                "{}: terminal.capability_warning must be true or false, got {other}; \
                 using capability_warning = true",
                path.display()
            ));
        }
        None => {}
    }

    config
}

struct LoadedFile {
    bindings: Vec<Binding>,
    warnings: Vec<String>,
}

fn load_bindings_file(path: &Path, source: Source, label: &str) -> LoadedFile {
    match fs::read_to_string(path) {
        Ok(text) => {
            let loaded = if source == Source::User {
                load_user_bindings(&text)
            } else {
                load_bindings_with_source(&text, source)
            };
            match loaded {
                Ok(loaded) => LoadedFile {
                    bindings: loaded.bindings,
                    warnings: loaded
                        .issues
                        .iter()
                        .map(|issue| format!("{label}: {issue}"))
                        .collect(),
                },
                Err(error) => LoadedFile {
                    bindings: Vec::new(),
                    warnings: vec![format!("{}: {error}", path.display())],
                },
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LoadedFile {
            bindings: Vec::new(),
            warnings: Vec::new(),
        },
        Err(error) => LoadedFile {
            bindings: Vec::new(),
            warnings: vec![format!("{}: {error}", path.display())],
        },
    }
}

#[allow(dead_code)]
fn _bindings_path_for_docs_only(base_dir: &Path) -> PathBuf {
    base_dir.join("bindings.json")
}

/// Scaffold written into `config.toml` when `config.openSettings` (palette,
/// TASK-260711-19) opens it for the first time. Kept close to the SPEC-0005
/// example so a first-time editor matches the documented shape.
pub(crate) const SETTINGS_TEMPLATE: &str = "\
# coda configuration (docs/SPEC-0005-cli-and-config.md)

[appearance]
theme = \"dark\"  # \"dark\" | \"light\"

[editor]
wrap = false            # visual line wrap; toggle at runtime with view.toggleWrap (alt+z)
indent_style = \"space\"  # \"space\" | \"tab\"; what Tab / edit.indent inserts
indent_width = 4        # spaces per indent level, 1-16 (also caps space removal on outdent)

[keymap]
sequence_timeout_ms = 800     # pending key sequence wait before the exact match fires
palette_key = \"ctrl+space\"    # palette convenience key; F1 always works
ctrl_c = \"copy\"               # \"copy\" | \"quit\"; quit keeps unsaved-change confirmation

[terminal]
capability_warning = true  # startup warning when the terminal cannot distinguish modified keys
";

/// Scaffold written into `bindings.json` when `config.openKeybindings`
/// (palette, TASK-260711-19) opens it for the first time. `command` values
/// are coda's internal action names (SPEC-0004 conversion table), not VS
/// Code command IDs.
pub(crate) const KEYBINDINGS_TEMPLATE: &str = "\
// User key bindings (docs/SPEC-0005-cli-and-config.md). JSONC: comments and
// trailing commas are allowed. `command` is coda's internal action name, e.g. \"cursor.down\" —
// run the command palette (F1) to see every available action.
[
  // { \"key\": \"ctrl+j\", \"command\": \"cursor.down\", \"when\": \"editorFocus\" }
]
";

/// Absolute path to `config.toml`, or `None` when `HOME`/`XDG_CONFIG_HOME`
/// cannot be resolved (mirrors `load()`'s own fallback).
pub(crate) fn settings_path() -> Option<PathBuf> {
    config_base_dir().map(|dir| dir.join("config.toml"))
}

/// Absolute path to the acknowledged-environment state file, or `None` when
/// `HOME`/`XDG_CONFIG_HOME` cannot be resolved.
pub(crate) fn environment_ack_path() -> Option<PathBuf> {
    config_base_dir().map(|dir| dir.join("environment-info.json"))
}

/// Terminal identity key for the acknowledgement map: matching bindings must
/// be re-acknowledged per terminal program *and* version, the same rule
/// `keymap verify` applies to measured chords (a Ghostty update can change
/// its keybind table).
pub(crate) fn terminal_identity() -> String {
    let program = std::env::var("TERM_PROGRAM").unwrap_or_else(|_| "unknown".to_string());
    let version = std::env::var("TERM_PROGRAM_VERSION").unwrap_or_else(|_| "unknown".to_string());
    format!("{program} {version}")
}

/// Acknowledgement state: environment-info lines the user has dismissed,
/// keyed by terminal identity. The lines themselves are the comparison key —
/// the panel reappears exactly when the facts change, with no hashing to go
/// stale across versions.
#[derive(serde::Serialize, serde::Deserialize)]
struct EnvironmentAckState {
    schema_version: u32,
    acknowledged: std::collections::BTreeMap<String, Vec<String>>,
}

const ENVIRONMENT_ACK_SCHEMA: u32 = 1;

/// Lines previously acknowledged for the current terminal. Corrupt or
/// unreadable state is reported and treated as "nothing acknowledged" — the
/// panel shows again, which is the safe direction (never hide facts because
/// a state file broke).
pub(crate) fn load_environment_ack(path: &Path, warnings: &mut Vec<String>) -> Vec<String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warnings.push(format!("{}: {error}; ignored ack state", path.display()));
            return Vec::new();
        }
    };
    let state: EnvironmentAckState = match serde_json::from_str(&text) {
        Ok(state) => state,
        Err(error) => {
            warnings.push(format!("{}: {error}; ignored ack state", path.display()));
            return Vec::new();
        }
    };
    if state.schema_version != ENVIRONMENT_ACK_SCHEMA {
        return Vec::new();
    }
    state
        .acknowledged
        .get(&terminal_identity())
        .cloned()
        .unwrap_or_default()
}

/// Records `lines` as acknowledged for the current terminal, preserving other
/// terminals' entries. A failure must never disturb the editing session, but
/// the caller surfaces it (status bar) — a silently lost acknowledgement
/// makes the panel's reappearance next launch inexplicable.
pub(crate) fn save_environment_ack(path: &Path, lines: &[String]) -> std::io::Result<()> {
    let mut state = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<EnvironmentAckState>(&text).ok())
        .filter(|state| state.schema_version == ENVIRONMENT_ACK_SCHEMA)
        .unwrap_or(EnvironmentAckState {
            schema_version: ENVIRONMENT_ACK_SCHEMA,
            acknowledged: std::collections::BTreeMap::new(),
        });
    state
        .acknowledged
        .insert(terminal_identity(), lines.to_vec());
    let mut body = serde_json::to_vec_pretty(&state)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    body.push(b'\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)
}

/// Absolute path to the user `bindings.json`, or `None` when
/// `HOME`/`XDG_CONFIG_HOME` cannot be resolved.
pub(crate) fn keybindings_path() -> Option<PathBuf> {
    config_base_dir().map(|dir| dir.join("bindings.json"))
}

#[cfg(test)]
mod tests {
    use super::{KEYBINDINGS_TEMPLATE, SETTINGS_TEMPLATE, load_from_base_dir};
    use crate::{
        highlight::ThemeChoice,
        keymap::{EditorAction, Source, load_user_bindings},
    };
    use std::fs;

    #[test]
    fn load_reads_light_theme_from_config_toml() {
        let temp = temp_config_dir("light-theme");
        fs::create_dir_all(&temp).unwrap();
        fs::write(
            temp.join("config.toml"),
            "[appearance]\ntheme = \"light\"\n",
        )
        .unwrap();

        let loaded = load_from_base_dir(&temp);

        assert_eq!(loaded.theme, ThemeChoice::Light);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn load_defaults_to_dark_when_theme_is_missing() {
        let temp = temp_config_dir("missing-theme");
        fs::create_dir_all(&temp).unwrap();

        let loaded = load_from_base_dir(&temp);

        assert_eq!(loaded.theme, ThemeChoice::Dark);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn load_warns_and_defaults_to_dark_for_broken_toml() {
        let temp = temp_config_dir("broken-theme");
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("config.toml"), "[appearance\n").unwrap();

        let loaded = load_from_base_dir(&temp);

        assert_eq!(loaded.theme, ThemeChoice::Dark);
        assert!(!loaded.wrap);
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("using default settings"));
        fs::remove_dir_all(&temp).unwrap();
    }

    /// TASK-260711-18: `[editor] wrap` startup default. A wrong type must not
    /// break startup — it warns and falls back to false.
    #[test]
    fn load_reads_editor_wrap_and_rejects_non_boolean() {
        let temp = temp_config_dir("wrap");
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("config.toml"), "[editor]\nwrap = true\n").unwrap();
        let loaded = load_from_base_dir(&temp);
        assert!(loaded.wrap);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);

        fs::write(temp.join("config.toml"), "[editor]\nwrap = \"yes\"\n").unwrap();
        let loaded = load_from_base_dir(&temp);
        assert!(!loaded.wrap);
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("editor.wrap"));
        fs::remove_dir_all(&temp).unwrap();
    }

    /// TASK-260828: `[editor] indent_style` / `indent_width`. Valid values
    /// load, invalid ones warn and fall back to space/4 without breaking
    /// startup.
    #[test]
    fn load_reads_indent_options_and_rejects_invalid_values() {
        use crate::core::editor::{IndentConfig, IndentStyle};

        let temp = temp_config_dir("indent");
        fs::create_dir_all(&temp).unwrap();
        fs::write(
            temp.join("config.toml"),
            "[editor]\nindent_style = \"tab\"\nindent_width = 2\n",
        )
        .unwrap();
        let loaded = load_from_base_dir(&temp);
        assert_eq!(
            loaded.indent,
            IndentConfig {
                style: IndentStyle::Tab,
                width: 2
            }
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);

        // Upper boundary of the accepted range.
        fs::write(temp.join("config.toml"), "[editor]\nindent_width = 16\n").unwrap();
        let loaded = load_from_base_dir(&temp);
        assert_eq!(loaded.indent.width, 16);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);

        let invalid_cases: &[(&str, &str)] = &[
            ("[editor]\nindent_style = \"tabs\"\n", "indent_style"),
            ("[editor]\nindent_style = 4\n", "indent_style"),
            ("[editor]\nindent_width = 0\n", "indent_width"),
            ("[editor]\nindent_width = 17\n", "indent_width"),
            ("[editor]\nindent_width = \"four\"\n", "indent_width"),
        ];
        for (toml, expected_in_warning) in invalid_cases {
            fs::write(temp.join("config.toml"), toml).unwrap();
            let loaded = load_from_base_dir(&temp);
            assert_eq!(loaded.indent, IndentConfig::default(), "{toml}");
            assert_eq!(loaded.warnings.len(), 1, "{toml}: {:?}", loaded.warnings);
            assert!(
                loaded.warnings[0].contains(expected_in_warning),
                "{toml}: {:?}",
                loaded.warnings
            );
        }
        fs::remove_dir_all(&temp).unwrap();
    }

    /// SPEC-0005 `[keymap]` / `[terminal]` wiring: each option accepts a
    /// valid value, and a wrong type warns and falls back without breaking
    /// startup (silent-breakage rule).
    #[test]
    fn load_reads_keymap_and_terminal_options() {
        let temp = temp_config_dir("keymap-terminal");
        fs::create_dir_all(&temp).unwrap();
        fs::write(
            temp.join("config.toml"),
            "[keymap]\nsequence_timeout_ms = 1200\npalette_key = \"ctrl+k\"\n\n\
             [terminal]\ncapability_warning = false\n",
        )
        .unwrap();

        let loaded = load_from_base_dir(&temp);

        assert_eq!(loaded.sequence_timeout_ms, 1200);
        assert_eq!(
            loaded.palette_key,
            Some(crate::keymap::parse_key_chord("ctrl+k").unwrap())
        );
        assert!(!loaded.capability_warning);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn load_warns_and_defaults_for_invalid_keymap_and_terminal_options() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "timeout-type",
                "[keymap]\nsequence_timeout_ms = \"soon\"\n",
                "sequence_timeout_ms",
            ),
            (
                "timeout-zero",
                "[keymap]\nsequence_timeout_ms = 0\n",
                "sequence_timeout_ms",
            ),
            (
                "palette-bad-chord",
                "[keymap]\npalette_key = \"nope+x+y\"\n",
                "palette_key",
            ),
            (
                "palette-type",
                "[keymap]\npalette_key = 12\n",
                "palette_key",
            ),
            (
                "capability-type",
                "[terminal]\ncapability_warning = \"off\"\n",
                "capability_warning",
            ),
        ];
        for (label, toml, expected_in_warning) in cases {
            let temp = temp_config_dir(&format!("invalid-{label}"));
            fs::create_dir_all(&temp).unwrap();
            fs::write(temp.join("config.toml"), toml).unwrap();

            let loaded = load_from_base_dir(&temp);

            assert_eq!(loaded.sequence_timeout_ms, 800, "{label}");
            assert_eq!(loaded.palette_key, None, "{label}");
            assert!(loaded.capability_warning, "{label}");
            assert_eq!(loaded.warnings.len(), 1, "{label}: {:?}", loaded.warnings);
            assert!(
                loaded.warnings[0].contains(expected_in_warning),
                "{label}: {:?}",
                loaded.warnings
            );
            fs::remove_dir_all(&temp).unwrap();
        }
    }

    #[test]
    fn load_defaults_keymap_and_terminal_options_when_missing() {
        let temp = temp_config_dir("missing-keymap-terminal");
        fs::create_dir_all(&temp).unwrap();

        let loaded = load_from_base_dir(&temp);

        assert_eq!(loaded.sequence_timeout_ms, 800);
        assert_eq!(loaded.palette_key, None);
        assert!(loaded.capability_warning);
        fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn environment_ack_round_trips_and_ignores_corruption() {
        let temp = temp_config_dir("env-ack");
        fs::create_dir_all(&temp).unwrap();
        let path = temp.join("environment-info.json");
        let lines = vec!["Ghostty intercepts 2 binding(s):".to_string()];

        super::save_environment_ack(&path, &lines).unwrap();
        let mut warnings = Vec::new();
        assert_eq!(super::load_environment_ack(&path, &mut warnings), lines);
        assert!(warnings.is_empty(), "clean state loads silently");

        // Re-acknowledging different facts replaces this terminal's entry.
        let changed = vec!["Ghostty intercepts 5 binding(s):".to_string()];
        super::save_environment_ack(&path, &changed).unwrap();
        let mut warnings = Vec::new();
        assert_eq!(super::load_environment_ack(&path, &mut warnings), changed);

        // Corrupt state must warn and read as "nothing acknowledged" — the
        // panel shows again, which is the safe direction, and startup never
        // fails over a broken state file.
        fs::write(&path, "not json").unwrap();
        let mut warnings = Vec::new();
        assert!(super::load_environment_ack(&path, &mut warnings).is_empty());
        assert_eq!(warnings.len(), 1, "corruption is reported: {warnings:?}");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn environment_ack_save_failure_surfaces_as_an_error() {
        let temp = temp_config_dir("env-ack-fail");
        fs::create_dir_all(&temp).unwrap();
        // The would-be parent directory is a plain file: create_dir_all and
        // the write both fail, and the caller must get to see that.
        let blocker = temp.join("blocker");
        fs::write(&blocker, "file").unwrap();
        let path = blocker.join("child.json");

        assert!(super::save_environment_ack(&path, &["x".to_string()]).is_err());

        let _ = fs::remove_dir_all(&temp);
    }

    fn temp_config_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "coda-config-{label}-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn load_reads_generated_bindings_as_imported_source() {
        let temp = temp_config_dir("bindings");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(temp.join("generated")).unwrap();
        fs::write(
            temp.join("generated/vscode-bindings.json"),
            r#"[{ "key": "ctrl+j", "command": "cursor.down", "when": "editorFocus" }]"#,
        )
        .unwrap();

        let loaded = load_from_base_dir(&temp);

        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.user_bindings.len(), 1);
        assert_eq!(loaded.user_bindings[0].source, Source::Imported);
        assert_eq!(loaded.user_bindings[0].action, EditorAction::CursorDown);
        fs::remove_dir_all(&temp).unwrap();
    }

    /// TASK-260711-19: the scaffold written into a first-opened
    /// `config.toml` (`config.openSettings`) must itself parse cleanly back
    /// through the loader — a template that fails its own loader would defeat
    /// the point of scaffolding it.
    #[test]
    fn settings_template_parses_back_to_the_dark_default_without_warnings() {
        let temp = temp_config_dir("settings-template");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("config.toml"), SETTINGS_TEMPLATE).unwrap();

        let loaded = load_from_base_dir(&temp);

        assert_eq!(loaded.theme, ThemeChoice::Dark);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        fs::remove_dir_all(&temp).unwrap();
    }

    /// TASK-260711-19: same round-trip guarantee for the `bindings.json`
    /// scaffold (`config.openKeybindings`) — its commented-out example must
    /// stay inert (an empty JSONC array with no live entries or issues).
    #[test]
    fn keybindings_template_parses_as_an_empty_jsonc_array() {
        let loaded = load_user_bindings(KEYBINDINGS_TEMPLATE).unwrap();
        assert!(loaded.bindings.is_empty(), "{:?}", loaded.bindings);
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
    }
}
