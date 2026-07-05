//! App configuration discovery and user binding loading.

use std::{env, fs, path::PathBuf};

use crate::keymap::{Binding, load_user_bindings};

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub user_bindings: Vec<Binding>,
    pub warnings: Vec<String>,
}

pub fn load() -> AppConfig {
    let Some(path) = bindings_path() else {
        return AppConfig {
            user_bindings: Vec::new(),
            warnings: vec!["HOME is not set; skipped user bindings".to_string()],
        };
    };

    match fs::read_to_string(&path) {
        Ok(text) => match load_user_bindings(&text) {
            Ok(loaded) => {
                // Status-bar real estate is scarce: lead with the issue, not a
                // long absolute path. The file location is the standard one.
                let warnings = loaded
                    .issues
                    .iter()
                    .map(|issue| format!("bindings.json: {issue}"))
                    .collect();
                AppConfig {
                    user_bindings: loaded.bindings,
                    warnings,
                }
            }
            Err(error) => AppConfig {
                user_bindings: Vec::new(),
                warnings: vec![format!("{}: {error}", path.display())],
            },
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => AppConfig::default(),
        Err(error) => AppConfig {
            user_bindings: Vec::new(),
            warnings: vec![format!("{}: {error}", path.display())],
        },
    }
}

fn bindings_path() -> Option<PathBuf> {
    if let Some(base) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(base).join("coda").join("bindings.json"));
    }
    env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("coda")
            .join("bindings.json")
    })
}
