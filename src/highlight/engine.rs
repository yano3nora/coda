//! Syntax and theme loading for display-only highlighting.

use std::path::Path;

use syntect::{
    highlighting::{Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};

use super::cache::MAX_HIGHLIGHT_LINE_BYTES;

/// build.rs が syntect の default set に `assets/*.sublime-syntax` を足して link 済みで
/// dump したもの。実行時に YAML を parse しない理由は build.rs 参照。
const SYNTAX_SET_DUMP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/syntaxes.packdump"));

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
            syntax_set: syntect::dumps::from_binary(SYNTAX_SET_DUMP),
            theme_set: ThemeSet::load_defaults(),
            theme_choice,
        }
    }

    /// file 名 (`Makefile` / `.bashrc` / `COMMIT_EDITMSG`) -> 拡張子 -> 1 行目 (shebang 等) の
    /// 順で解決する。file 名の完全一致は拡張子より限定的なので先に見る (syntect の
    /// `find_syntax_for_file` と同じ順序)。開いている buffer の内容を使うので file I/O をしない。
    pub fn syntax_for_file(
        &self,
        path: &Path,
        first_line: Option<&str>,
    ) -> Option<&SyntaxReference> {
        if let Some(file_name) = path.file_name().and_then(|name| name.to_str())
            && let Some(syntax) = self.syntax_set.find_syntax_by_extension(file_name)
        {
            return Some(syntax);
        }
        if let Some(extension) = path.extension().and_then(|ext| ext.to_str())
            && let Some(syntax) = self.syntax_set.find_syntax_by_extension(extension)
        {
            return Some(syntax);
        }
        // 名前で決まらないファイルは描画のたびにここへ来る。highlight cache の行長上限より
        // 前段なので、巨大な 1 行 (minified 等) に regex を走らせないよう同じ上限で切る
        let first_line = first_line.map(|line| {
            let mut end = line.len().min(MAX_HIGHLIGHT_LINE_BYTES);
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            &line[..end]
        });
        first_line.and_then(|line| self.syntax_set.find_syntax_by_first_line(line))
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
    use super::{HighlightEngine, MAX_HIGHLIGHT_LINE_BYTES, ThemeChoice};
    use std::path::Path;

    #[test]
    fn syntax_for_file_resolves_by_extension_file_name_then_first_line() {
        let engine = HighlightEngine::new(ThemeChoice::Dark);
        let cases: &[(&str, Option<&str>, Option<&str>)] = &[
            // 拡張子
            ("main.rs", None, Some("Rust")),
            ("app.ts", None, Some("TypeScript")),
            ("app.tsx", None, Some("TypeScriptReact")),
            ("config.toml", None, Some("TOML")),
            ("php.ini", None, Some("INI")),
            ("setup.cfg", None, Some("INI")),
            ("note.md", None, Some("Markdown")),
            // 拡張子なし / dotfile は file 名で解決する
            ("Makefile", None, Some("Makefile")),
            ("Dockerfile", None, Some("Dockerfile")),
            ("Cargo.lock", None, Some("TOML")),
            (".bashrc", None, Some("Bourne Again Shell (bash)")),
            (".gitconfig", None, Some("Git Config")),
            (".gitignore", None, Some("Git Ignore")),
            (".gitattributes", None, Some("Git Attributes")),
            ("COMMIT_EDITMSG", None, Some("Git Commit")),
            ("git-rebase-todo", None, Some("Git Rebase Todo")),
            // file 名の完全一致は拡張子 (erb -> HTML (Rails)) より優先する。同梱セットで
            // 両者が別 syntax を指す唯一の組なので、名前は不自然だが順序の回帰テストとして置く
            ("js.erb", None, Some("JavaScript (Rails)")),
            // 拡張子・file 名で決まらなければ 1 行目
            (
                "run",
                Some("#!/usr/bin/env bash\n"),
                Some("Bourne Again Shell (bash)"),
            ),
            (
                "Dockerfile.dev",
                Some("FROM ubuntu:24.04\n"),
                Some("Dockerfile"),
            ),
            // 決まらなければ None (単色のまま。黙って壊れない)
            ("file.codaunknown", None, None),
            ("noext", None, None),
            ("noext", Some("plain text\n"), None),
        ];
        for (path, first_line, expected) in cases {
            let actual = engine
                .syntax_for_file(Path::new(path), *first_line)
                .map(|syntax| syntax.name.as_str());
            assert_eq!(actual, *expected, "path={path} first_line={first_line:?}");
        }
    }

    #[test]
    fn first_line_detection_is_bounded_and_char_boundary_safe() {
        let engine = HighlightEngine::new(ThemeChoice::Dark);
        let name = |path: &str, line: &str| {
            engine
                .syntax_for_file(Path::new(path), Some(line))
                .map(|syntax| syntax.name.to_owned())
        };
        // Dockerfile の first_line_match は先頭の空白を許すので、切らなければ一致する入力。
        // 上限の外にある手掛かりは見ない (巨大 1 行に regex を走らせない)
        let beyond_limit = format!("{}FROM ubuntu\n", " ".repeat(MAX_HIGHLIGHT_LINE_BYTES));
        assert_eq!(name("noext", &beyond_limit), None);
        let within_limit = format!("{}FROM ubuntu\n", " ".repeat(MAX_HIGHLIGHT_LINE_BYTES - 16));
        assert_eq!(name("noext", &within_limit), Some("Dockerfile".to_owned()));
        // 上限が多バイト文字の途中に落ちても panic しない
        let multibyte = "あ".repeat(MAX_HIGHLIGHT_LINE_BYTES);
        assert_eq!(name("noext", &multibyte), None);
    }
}
