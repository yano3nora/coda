//! App configuration discovery and key binding loading.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::keymap::{Binding, Source, load_bindings_with_source, load_user_bindings};

use super::import_cli::config_base_dir;

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub user_bindings: Vec<Binding>,
    pub warnings: Vec<String>,
}

pub fn load() -> AppConfig {
    let Some(base_dir) = config_base_dir() else {
        return AppConfig {
            user_bindings: Vec::new(),
            warnings: vec!["HOME is not set; skipped user/imported bindings".to_string()],
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

    AppConfig {
        user_bindings: bindings,
        warnings,
    }
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
    use crate::keymap::{EditorAction, Source};
    use std::fs;

    #[test]
    fn load_reads_generated_bindings_as_imported_source() {
        let temp = std::env::temp_dir().join(format!("coda-config-load-{}", std::process::id()));
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
