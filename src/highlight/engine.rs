//! Syntax and theme loading for display-only highlighting.

use std::path::Path;

use syntect::{
    highlighting::{Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};

/// User-selectable bundled theme choice.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

impl ThemeChoice {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

/// Owns syntect's syntax and theme sets so cache/view code only borrows them.
pub struct HighlightEngine {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    theme_choice: ThemeChoice,
}

impl HighlightEngine {
    pub fn new(theme_choice: ThemeChoice) -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            theme_choice,
        }
    }

    pub fn syntax_for_path(&self, path: &Path) -> Option<&SyntaxReference> {
        let extension = path.extension()?.to_str()?;
        self.syntax_set.find_syntax_by_extension(extension)
    }

    pub fn syntax_set(&self) -> &SyntaxSet {
        &self.syntax_set
    }

    pub fn theme(&self) -> &Theme {
        let name = match self.theme_choice {
            ThemeChoice::Dark => "base16-ocean.dark",
            ThemeChoice::Light => "InspiredGitHub",
        };
        &self.theme_set.themes[name]
    }
}

#[cfg(test)]
mod tests {
    use super::{HighlightEngine, ThemeChoice};
    use std::path::Path;

    #[test]
    fn syntax_for_path_finds_rust_and_returns_none_for_unknown_extension() {
        let engine = HighlightEngine::new(ThemeChoice::Dark);

        assert!(engine.syntax_for_path(Path::new("main.rs")).is_some());
        assert!(
            engine
                .syntax_for_path(Path::new("file.codaunknown"))
                .is_none()
        );
    }
}
