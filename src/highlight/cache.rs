//! Incremental, line-oriented syntax highlight cache.
//!
//! The cache returns grapheme-indexed foreground spans only. Syntax scopes never
//! leave this presentation layer, which preserves ADR-0006's display-only wall.

use std::ops::Range;

use syntect::{
    easy::HighlightLines,
    highlighting::{Color, HighlightState},
    parsing::{ParseState, SyntaxReference},
};
use unicode_segmentation::UnicodeSegmentation;

use crate::core::buffer::TextBuffer;

use super::engine::HighlightEngine;

pub const MAX_HIGHLIGHT_LINE_BYTES: usize = 2_000;
pub const MAX_HIGHLIGHT_LINES: usize = 20_000;

pub type Rgb = (u8, u8, u8);
pub type HighlightSpan = (Range<usize>, Rgb);

#[derive(Clone)]
struct CachedLine {
    text: String,
    spans: Vec<HighlightSpan>,
    highlight_state: HighlightState,
    parse_state: ParseState,
}

#[derive(Default)]
pub struct HighlightCache {
    lines: Vec<Option<CachedLine>>,
    #[cfg(test)]
    recalculated_lines: Vec<usize>,
}

impl HighlightCache {
    pub fn spans_for(
        &mut self,
        buffer: &TextBuffer,
        viewport: Range<usize>,
        engine: &HighlightEngine,
        syntax: Option<&SyntaxReference>,
    ) -> Vec<Vec<HighlightSpan>> {
        let end = viewport.end.min(buffer.line_count());
        let start = viewport.start.min(end);
        let Some(syntax) = syntax else {
            return vec![Vec::new(); end.saturating_sub(start)];
        };
        if buffer.line_count() > MAX_HIGHLIGHT_LINES {
            return vec![Vec::new(); end.saturating_sub(start)];
        }

        self.lines.resize_with(end, || None);
        let first_dirty = (0..end).find(|&index| {
            let text = buffer.line(index).unwrap_or_default();
            self.lines
                .get(index)
                .and_then(Option::as_ref)
                .is_none_or(|cached| cached.text != text)
        });

        if let Some(first_dirty) = first_dirty {
            self.lines.truncate(first_dirty);
            self.recalculate_from(first_dirty, end, buffer, engine, syntax);
        }

        (start..end)
            .map(|index| {
                self.lines
                    .get(index)
                    .and_then(Option::as_ref)
                    .map(|line| line.spans.clone())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn recalculate_from(
        &mut self,
        start: usize,
        end: usize,
        buffer: &TextBuffer,
        engine: &HighlightEngine,
        syntax: &SyntaxReference,
    ) {
        let mut highlighter = if start == 0 {
            HighlightLines::new(syntax, engine.theme())
        } else if let Some(previous) = self.lines.get(start - 1).and_then(Option::as_ref) {
            HighlightLines::from_state(
                engine.theme(),
                previous.highlight_state.clone(),
                previous.parse_state.clone(),
            )
        } else {
            HighlightLines::new(syntax, engine.theme())
        };

        for index in start..end {
            let text = buffer.line(index).unwrap_or_default().to_string();
            let spans = if text.len() > MAX_HIGHLIGHT_LINE_BYTES {
                Vec::new()
            } else {
                highlight_line_spans(&mut highlighter, &text, engine)
            };
            let (highlight_state, parse_state) = highlighter.state();
            self.lines.push(Some(CachedLine {
                text,
                spans,
                highlight_state: highlight_state.clone(),
                parse_state: parse_state.clone(),
            }));
            highlighter = HighlightLines::from_state(engine.theme(), highlight_state, parse_state);
            #[cfg(test)]
            self.recalculated_lines.push(index);
        }
    }

    #[cfg(test)]
    fn take_recalculated_lines(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.recalculated_lines)
    }
}

fn highlight_line_spans(
    highlighter: &mut HighlightLines<'_>,
    text: &str,
    engine: &HighlightEngine,
) -> Vec<HighlightSpan> {
    let parse_text = format!("{text}\n");
    let Ok(ranges) = highlighter.highlight_line(&parse_text, engine.syntax_set()) else {
        return Vec::new();
    };

    let mut byte_start = 0;
    let mut spans = Vec::new();
    for (style, segment) in ranges {
        let byte_end = (byte_start + segment.len()).min(text.len());
        if byte_start < text.len() && byte_start < byte_end {
            let start = byte_to_grapheme(text, byte_start);
            let end = byte_to_grapheme(text, byte_end);
            let rgb = color_to_rgb(style.foreground);
            if start < end {
                spans.push((start..end, rgb));
            }
        }
        byte_start += segment.len();
        if byte_start >= text.len() {
            break;
        }
    }
    spans
}

fn color_to_rgb(color: Color) -> Rgb {
    (color.r, color.g, color.b)
}

fn byte_to_grapheme(text: &str, byte_offset: usize) -> usize {
    text.grapheme_indices(true)
        .take_while(|(byte, _)| *byte < byte_offset)
        .count()
}

#[cfg(test)]
mod tests {
    use super::{HighlightCache, MAX_HIGHLIGHT_LINE_BYTES};
    use crate::{
        core::buffer::TextBuffer,
        highlight::engine::{HighlightEngine, ThemeChoice},
    };
    use std::path::Path;

    fn buffer(text: &str) -> TextBuffer {
        TextBuffer::from_bytes(text.as_bytes()).unwrap().0
    }

    #[test]
    fn cache_recalculates_from_first_changed_line_only() {
        let engine = HighlightEngine::new(ThemeChoice::Dark);
        let syntax = engine.syntax_for_file(Path::new("main.rs"), None);
        let mut cache = HighlightCache::default();
        let original = buffer("fn a() {}\nfn b() {}\nfn c() {}\n");

        let first = cache.spans_for(&original, 0..3, &engine, syntax);
        assert_eq!(cache.take_recalculated_lines(), vec![0, 1, 2]);

        let edited = buffer("fn a() {}\nlet b = 1;\nfn c() {}\n");
        let second = cache.spans_for(&edited, 0..3, &engine, syntax);

        assert_eq!(cache.take_recalculated_lines(), vec![1, 2]);
        assert_eq!(first[0], second[0]);
        assert_ne!(first[1], second[1]);
    }

    #[test]
    fn spans_are_returned_as_grapheme_ranges_for_japanese_text() {
        let engine = HighlightEngine::new(ThemeChoice::Dark);
        let syntax = engine.syntax_for_file(Path::new("main.rs"), None);
        let mut cache = HighlightCache::default();
        let buffer = buffer("// 日本語\n");

        let spans = cache.spans_for(&buffer, 0..1, &engine, syntax);

        assert!(spans[0].iter().any(|(range, _)| range.end <= 6));
        assert!(spans[0].iter().all(|(range, _)| range.end <= 6));
    }

    #[test]
    fn markdown_fenced_code_blocks_embed_language_highlighting() {
        let engine = HighlightEngine::new(ThemeChoice::Dark);
        let syntax = engine.syntax_for_file(Path::new("note.md"), None);
        let mut cache = HighlightCache::default();
        let text = "# title\n\nplain text\n\n```sh\nif true; then echo \"hi\"; fi\n```\n\n```rust\nfn main() { let x = 1; }\n```\n";
        let buffer = buffer(text);

        let spans = cache.spans_for(&buffer, 0..buffer.line_count(), &engine, syntax);

        let distinct_colors = |line: &[super::HighlightSpan]| {
            let mut colors: Vec<_> = line.iter().map(|(_, rgb)| *rgb).collect();
            colors.sort_unstable();
            colors.dedup();
            colors.len()
        };
        // fence 内 (sh: line 5 / rust: line 9) は keyword や文字列で複数色になる
        assert!(distinct_colors(&spans[5]) >= 2, "sh fence: {:?}", spans[5]);
        assert!(
            distinct_colors(&spans[9]) >= 2,
            "rust fence: {:?}",
            spans[9]
        );
        // fence 外の Markdown は従来どおり: 見出しは本文と異なる色、本文は単色
        assert_eq!(distinct_colors(&spans[2]), 1, "plain: {:?}", spans[2]);
        assert_ne!(spans[0], spans[2]);
    }

    #[test]
    fn bundled_extra_syntaxes_produce_multiple_colors() {
        let engine = HighlightEngine::new(ThemeChoice::Dark);
        let cases: &[(&str, &str)] = &[
            (
                "app.ts",
                "export function f(a: string): number { return 1; }\n",
            ),
            (
                "app.tsx",
                "const view = <div className=\"x\">{label}</div>;\n",
            ),
            (
                "config.toml",
                "[package]\nname = \"coda\"\nedition = 2024\n",
            ),
            ("php.ini", "[section]\nkey = value ; note\n"),
            ("Dockerfile", "FROM ubuntu:24.04\nRUN apt-get update\n"),
            (
                "COMMIT_EDITMSG",
                "feat: subject\n\n# Please enter the commit message\n",
            ),
            (
                "git-rebase-todo",
                "pick 1234567 first commit\nsquash 89abcde second\n",
            ),
            (".gitconfig", "[user]\n\tname = coda\n"),
            (".gitignore", "# build output\n/target\n"),
        ];
        for (path, text) in cases {
            let syntax = engine.syntax_for_file(Path::new(path), None);
            assert!(syntax.is_some(), "no syntax for {path}");
            let buffer = buffer(text);
            let mut cache = HighlightCache::default();
            let spans = cache.spans_for(&buffer, 0..buffer.line_count(), &engine, syntax);
            let mut colors: Vec<_> = spans.iter().flatten().map(|(_, rgb)| *rgb).collect();
            colors.sort_unstable();
            colors.dedup();
            assert!(colors.len() >= 2, "{path}: {spans:?}");
        }
    }

    #[test]
    fn long_lines_and_too_many_lines_are_uncolored() {
        let engine = HighlightEngine::new(ThemeChoice::Dark);
        let syntax = engine.syntax_for_file(Path::new("main.rs"), None);
        let mut cache = HighlightCache::default();
        let long = format!("{}\n", "a".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1));
        let long_buffer = buffer(&long);

        assert_eq!(
            cache.spans_for(&long_buffer, 0..1, &engine, syntax),
            vec![vec![]]
        );

        let huge_text = (0..20_001).map(|_| "fn x() {}\n").collect::<String>();
        let huge = buffer(&huge_text);
        assert_eq!(
            cache.spans_for(&huge, 0..2, &engine, syntax),
            vec![vec![], vec![]]
        );
    }
}
