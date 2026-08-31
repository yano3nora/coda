//! Syntax and theme loading for display-only highlighting.

use std::path::Path;

use syntect::{
    highlighting::{Theme, ThemeSet},
    parsing::{SyntaxDefinition, SyntaxReference, SyntaxSet},
};

/// ST 3.2 (Packages v3211) の Markdown 定義。syntect 同梱版は fenced code block
/// への他言語 embed を持たないため、後勝ちで差し替える。
const MARKDOWN_SYNTAX: &str = include_str!("assets/Markdown.sublime-syntax");

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
            syntax_set: load_syntax_set(),
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

fn load_syntax_set() -> SyntaxSet {
    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    // 定義が読めない場合でも起動は続行し、default の Markdown (単色 fence) に留める
    if let Ok(markdown) = SyntaxDefinition::load_from_str(MARKDOWN_SYNTAX, true, None) {
        builder.add(markdown);
    }
    builder.build()
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
