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
}

pub fn load() -> AppConfig {
    let Some(base_dir) = config_base_dir() else {
        return AppConfig {
            user_bindings: Vec::new(),
            warnings: vec!["HOME is not set; skipped user/imported bindings".to_string()],
            theme: ThemeChoice::Dark,
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

    let theme = load_theme_config(&base_dir.join("config.toml"), &mut warnings);

    AppConfig {
        user_bindings: bindings,
        warnings,
        theme,
    }
}

fn load_theme_config(path: &Path, warnings: &mut Vec<String>) -> ThemeChoice {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ThemeChoice::Dark,
        Err(error) => {
            warnings.push(format!(
                "{}: {error}; using default dark theme",
                path.display()
            ));
            return ThemeChoice::Dark;
        }
    };

    let value = match toml::from_str::<toml::Value>(&text) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!(
                "{}: {error}; using default dark theme",
                path.display()
            ));
            return ThemeChoice::Dark;
        }
    };

    let Some(theme) = value
        .get("appearance")
        .and_then(|appearance| appearance.get("theme"))
        .and_then(toml::Value::as_str)
    else {
        return ThemeChoice::Dark;
    };

    ThemeChoice::parse(theme).unwrap_or_else(|| {
        warnings.push(format!(
            "{}: unsupported appearance.theme={theme:?}; using default dark theme",
            path.display()
        ));
        ThemeChoice::Dark
    })
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

#[cfg(test)]
mod tests {
    use super::load_from_base_dir;
    use crate::{
        highlight::ThemeChoice,
        keymap::{EditorAction, Source},
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
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("using default dark theme"));
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
}
