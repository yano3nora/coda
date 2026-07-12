//! App configuration discovery and key binding loading.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    highlight::ThemeChoice,
    keymap::{Binding, Source, load_bindings_with_source, load_user_bindings},
};

use super::import_cli::config_base_dir;

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub user_bindings: Vec<Binding>,
    pub warnings: Vec<String>,
    pub theme: ThemeChoice,
    /// Startup default for visual line wrap (`[editor] wrap`,
    /// TASK-260711-18). `view.toggleWrap` flips it at runtime.
    pub wrap: bool,
}

pub fn load() -> AppConfig {
    let Some(base_dir) = config_base_dir() else {
        return AppConfig {
            user_bindings: Vec::new(),
            warnings: vec!["HOME is not set; skipped user/imported bindings".to_string()],
            theme: ThemeChoice::Dark,
            wrap: false,
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

    let (theme, wrap) = load_config_toml(&base_dir.join("config.toml"), &mut warnings);

    AppConfig {
        user_bindings: bindings,
        warnings,
        theme,
        wrap,
    }
}

/// Reads `config.toml` once and extracts every supported setting. Broken or
/// missing values never abort startup: each falls back to its default with a
/// warning (silent-breakage rule, AGENTS.md).
fn load_config_toml(path: &Path, warnings: &mut Vec<String>) -> (ThemeChoice, bool) {
    let defaults = (ThemeChoice::Dark, false);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return defaults,
        Err(error) => {
            warnings.push(format!(
                "{}: {error}; using default settings",
                path.display()
            ));
            return defaults;
        }
    };

    let value = match toml::from_str::<toml::Value>(&text) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!(
                "{}: {error}; using default settings",
                path.display()
            ));
            return defaults;
        }
    };

    let theme = match value
        .get("appearance")
        .and_then(|appearance| appearance.get("theme"))
        .and_then(toml::Value::as_str)
    {
        Some(theme) => ThemeChoice::parse(theme).unwrap_or_else(|| {
            warnings.push(format!(
                "{}: unsupported appearance.theme={theme:?}; using default dark theme",
                path.display()
            ));
            ThemeChoice::Dark
        }),
        None => ThemeChoice::Dark,
    };

    let wrap = match value.get("editor").and_then(|editor| editor.get("wrap")) {
        Some(toml::Value::Boolean(wrap)) => *wrap,
        Some(other) => {
            warnings.push(format!(
                "{}: editor.wrap must be true or false, got {other}; using wrap = false",
                path.display()
            ));
            false
        }
        None => false,
    };

    (theme, wrap)
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
wrap = false  # visual line wrap; toggle at runtime with view.toggleWrap (alt+z)
";

/// Scaffold written into `bindings.json` when `config.openKeybindings`
/// (palette, TASK-260711-19) opens it for the first time. `command` values
/// are coda's internal action names (SPEC-0004 conversion table), not VS
/// Code command IDs.
pub(crate) const KEYBINDINGS_TEMPLATE: &str = "\
// User key bindings (docs/SPEC-0005-cli-and-config.md). JSONC: comments are
// allowed. `command` is coda's internal action name, e.g. \"cursor.down\" —
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
