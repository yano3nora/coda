//! Import report model and text renderer.
//!
//! Reports are intentionally user-facing: every VS Code entry must land in one
//! bucket so users can see what was imported, ignored, or lost.

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ImportReport {
    pub imported: Vec<ReportEntry>,
    pub ignored: Vec<ReportEntry>,
    pub unsupported_commands: Vec<ReportEntry>,
    pub unsupported_conditions: Vec<ReportEntry>,
    pub invalid_keys: Vec<ReportEntry>,
    pub conflicts: Vec<ReportEntry>,
    /// Chords the importer classified as undeliverable on the detected
    /// `KeyboardCapabilities` (SPEC-0003 / TASK-260712-16) — e.g. Cmd/Super
    /// on a terminal without modifier delivery, or Ctrl+Shift+<char> on a
    /// terminal that cannot distinguish it from plain Ctrl+<char>.
    pub disabled_by_terminal_capability: Vec<ReportEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReportEntry {
    pub key: Option<String>,
    pub command: Option<String>,
    pub when: Option<String>,
    pub reason: String,
}

impl ReportEntry {
    pub fn new(
        key: Option<String>,
        command: Option<String>,
        when: Option<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            key,
            command,
            when,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct ImportSummary {
    pub imported: usize,
    pub ignored: usize,
    pub unsupported_commands: usize,
    pub unsupported_conditions: usize,
    pub invalid_keys: usize,
    pub conflicts: usize,
    pub disabled_by_terminal_capability: usize,
}

impl ImportReport {
    pub fn summary(&self) -> ImportSummary {
        ImportSummary {
            imported: self.imported.len(),
            ignored: self.ignored.len(),
            unsupported_commands: self.unsupported_commands.len(),
            unsupported_conditions: self.unsupported_conditions.len(),
            invalid_keys: self.invalid_keys.len(),
            conflicts: self.conflicts.len(),
            disabled_by_terminal_capability: self.disabled_by_terminal_capability.len(),
        }
    }

    pub fn total_classified(&self) -> usize {
        let summary = self.summary();
        summary.imported
            + summary.ignored
            + summary.unsupported_commands
            + summary.unsupported_conditions
            + summary.invalid_keys
            + summary.conflicts
            + summary.disabled_by_terminal_capability
    }

    /// Renders only the summary block (counts per bucket). Extracted so the
    /// CLI (`src/app/import_cli.rs`) and `render_text()` share one
    /// implementation instead of the summary being hand-built twice and
    /// risking drift or duplicate printing (TASK-260712-17).
    pub fn render_summary(&self) -> String {
        self.render_summary_with(&ReportStyle::plain())
    }

    /// Same as `render_summary()`, but wraps the title line and each
    /// `Bucket: N` line in `style`'s SGR prefix/reset so the CLI can colorize
    /// stdout while the saved report file stays plain (TASK-260712-18).
    /// `render_summary()` is a thin wrapper calling this with
    /// `ReportStyle::plain()` so plain output and existing tests are
    /// unaffected.
    pub fn render_summary_with(&self, style: &ReportStyle) -> String {
        let s = self.summary();
        [
            format!(
                "{}VS Code keybinding import completed.{}",
                style.title, style.reset
            ),
            String::new(),
            colorize(style, style.imported, format!("Imported: {}", s.imported)),
            colorize(style, style.ignored, format!("Ignored: {}", s.ignored)),
            colorize(
                style,
                style.unsupported,
                format!("Unsupported commands: {}", s.unsupported_commands),
            ),
            colorize(
                style,
                style.unsupported,
                format!("Unsupported conditions: {}", s.unsupported_conditions),
            ),
            colorize(
                style,
                style.invalid,
                format!("Invalid keys: {}", s.invalid_keys),
            ),
            colorize(style, style.invalid, format!("Conflicts: {}", s.conflicts)),
            colorize(
                style,
                style.disabled,
                format!(
                    "Disabled by terminal capability: {}",
                    s.disabled_by_terminal_capability
                ),
            ),
        ]
        .join("\n")
    }

    /// Renders the summary plus every entry in every non-empty bucket
    /// (SPEC-0004 invariant: every import-scope entry must appear in the
    /// report, not just a sampled "Examples:" line — TASK-260712-17). Buckets
    /// with zero entries are omitted entirely rather than printed empty.
    pub fn render_text(&self) -> String {
        self.render_text_with(&ReportStyle::plain())
    }

    /// Same as `render_text()`, but colorizes bucket headings/summary lines
    /// per `style` and dims each entry's trailing `[reason]` (TASK-260712-18
    /// design: entries keep their default color for `key -> command`, only
    /// the reason annotation is dimmed). `render_text()` wraps this with
    /// `ReportStyle::plain()`, so plain output stays byte-identical to
    /// before this struct existed.
    pub fn render_text_with(&self, style: &ReportStyle) -> String {
        let mut sections = vec![self.render_summary_with(style)];
        push_section(
            &mut sections,
            style,
            style.imported,
            "Imported",
            &self.imported,
        );
        push_section(
            &mut sections,
            style,
            style.ignored,
            "Ignored",
            &self.ignored,
        );
        push_section(
            &mut sections,
            style,
            style.unsupported,
            "Unsupported commands",
            &self.unsupported_commands,
        );
        push_section(
            &mut sections,
            style,
            style.unsupported,
            "Unsupported conditions",
            &self.unsupported_conditions,
        );
        push_section(
            &mut sections,
            style,
            style.invalid,
            "Invalid keys",
            &self.invalid_keys,
        );
        push_section(
            &mut sections,
            style,
            style.invalid,
            "Conflicts",
            &self.conflicts,
        );
        push_section(
            &mut sections,
            style,
            style.disabled,
            "Disabled by terminal capability",
            &self.disabled_by_terminal_capability,
        );
        format!("{}\n", sections.join("\n\n"))
    }
}

/// Wraps `text` in `color` (a `ReportStyle` field) plus `style.reset`. With
/// `ReportStyle::plain()` both are `""`, so this is a no-op — the reason
/// `render_text()`/`render_summary()` stay byte-identical to before
/// `ReportStyle` existed.
fn colorize(style: &ReportStyle, color: &str, text: String) -> String {
    format!("{color}{text}{}", style.reset)
}

fn push_section(
    sections: &mut Vec<String>,
    style: &ReportStyle,
    color: &str,
    label: &str,
    entries: &[ReportEntry],
) {
    if entries.is_empty() {
        return;
    }
    // Heading combines the bucket color with the title's bold, rather than
    // giving every bucket its own bold+color pair — `ReportStyle` only stores
    // one prefix per bucket (TASK-260712-18 design).
    let mut section = format!(
        "{}{color}{label} ({}):{}",
        style.title,
        entries.len(),
        style.reset
    );
    for entry in entries {
        section.push('\n');
        section.push_str(&format!("- {}", format_entry(style, entry)));
    }
    sections.push(section);
}

fn format_entry(style: &ReportStyle, entry: &ReportEntry) -> String {
    let key = entry.key.as_deref().unwrap_or("<missing key>");
    let command = entry.command.as_deref().unwrap_or("<missing command>");
    let when = entry
        .when
        .as_ref()
        .map(|when| format!(" [{when}]"))
        .unwrap_or_default();
    // Only the trailing `[reason]` is dimmed — `key -> command` (and the
    // optional `[when]`) stay default-colored so entry lines don't turn into
    // a wall of color (TASK-260712-18 tobe).
    format!(
        "{key} -> {command}{when} {}[{}]{}",
        style.reason, entry.reason, style.reset
    )
}

/// ANSI SGR (16-color) prefixes for `render_text_with` / `render_summary_with`.
///
/// This is style *data* only — no `isatty`/env lookups live here. The
/// TTY / `NO_COLOR` decision is made in `src/app/import_cli.rs` so `keymap/`
/// stays free of terminal dependencies (AGENTS.md dependency boundary: only
/// `input/` may depend on the terminal).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ReportStyle {
    /// Bold. Applied to the summary title line, and combined with a bucket
    /// color prefix for section headings (`Imported (22):`).
    pub title: &'static str,
    pub imported: &'static str,
    pub ignored: &'static str,
    /// Shared by "Unsupported commands" and "Unsupported conditions" — the
    /// task groups both under yellow.
    pub unsupported: &'static str,
    /// Shared by "Invalid keys" and "Conflicts" — the task groups both under
    /// red.
    pub invalid: &'static str,
    pub disabled: &'static str,
    /// Dims only the trailing `[reason]` on an entry line.
    pub reason: &'static str,
    pub reset: &'static str,
}

impl ReportStyle {
    /// No styling: every prefix and the reset are empty strings, so wrapping
    /// text in them is a byte-identical no-op.
    pub const fn plain() -> Self {
        Self {
            title: "",
            imported: "",
            ignored: "",
            unsupported: "",
            invalid: "",
            disabled: "",
            reason: "",
            reset: "",
        }
    }

    /// 16-color SGR only (no 256-color/truecolor — the report can render
    /// before capability detection completes, so it sticks to the smallest
    /// common denominator, TASK-260712-18 notes).
    pub const fn ansi() -> Self {
        Self {
            title: "\x1b[1m",
            imported: "\x1b[32m",
            ignored: "\x1b[2m",
            unsupported: "\x1b[33m",
            invalid: "\x1b[31m",
            disabled: "\x1b[35m",
            reason: "\x1b[2m",
            reset: "\x1b[0m",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ImportReport, ReportEntry, ReportStyle};

    /// Builds a report with at least one entry in every bucket, so rendering
    /// tests exercise every color branch (`imported`/`ignored`/`unsupported`
    /// x2/`invalid` x2/`disabled`) in one shot.
    fn sample_report() -> ImportReport {
        ImportReport {
            imported: vec![ReportEntry::new(
                Some("ctrl+j".to_string()),
                Some("cursor.down".to_string()),
                None,
                "imported",
            )],
            ignored: vec![ReportEntry::new(
                Some("ctrl+k".to_string()),
                Some("workbench.action.foo".to_string()),
                None,
                "no coda equivalent",
            )],
            unsupported_commands: vec![ReportEntry::new(
                Some("ctrl+l".to_string()),
                Some("workbench.action.bar".to_string()),
                None,
                "unsupported command",
            )],
            unsupported_conditions: vec![ReportEntry::new(
                Some("ctrl+m".to_string()),
                Some("cursor.up".to_string()),
                Some("editorFocus && weirdCondition".to_string()),
                "unsupported condition",
            )],
            invalid_keys: vec![ReportEntry::new(
                None,
                Some("cursor.up".to_string()),
                None,
                "missing key",
            )],
            conflicts: vec![ReportEntry::new(
                Some("ctrl+n".to_string()),
                Some("cursor.down".to_string()),
                None,
                "conflicts with existing binding",
            )],
            disabled_by_terminal_capability: vec![ReportEntry::new(
                Some("cmd+t".to_string()),
                Some("terminal.new".to_string()),
                None,
                "cmd/super not deliverable on this terminal",
            )],
        }
    }

    /// Strips exactly the SGR sequences `ReportStyle::ansi()` can emit. A
    /// literal-list stripper (rather than a regex crate) is fine since the
    /// set of possible codes is small, fixed, and owned by this same module.
    fn strip_ansi(text: &str) -> String {
        let style = ReportStyle::ansi();
        let mut out = text.to_string();
        for code in [
            style.title,
            style.imported,
            style.ignored,
            style.unsupported,
            style.invalid,
            style.disabled,
            style.reason,
            style.reset,
        ] {
            out = out.replace(code, "");
        }
        out
    }

    #[test]
    fn render_text_with_plain_matches_render_text() {
        let report = sample_report();
        assert_eq!(
            report.render_text_with(&ReportStyle::plain()),
            report.render_text()
        );
    }

    #[test]
    fn render_summary_with_plain_matches_render_summary() {
        let report = sample_report();
        assert_eq!(
            report.render_summary_with(&ReportStyle::plain()),
            report.render_summary()
        );
    }

    #[test]
    fn render_text_with_ansi_contains_bucket_colors_and_reset() {
        let report = sample_report();
        let style = ReportStyle::ansi();
        let rendered = report.render_text_with(&style);

        assert!(
            rendered.contains(&format!("{}Imported (1):", style.imported)),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!("{}Ignored (1):", style.ignored)),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!("{}Unsupported commands (1):", style.unsupported)),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!("{}Unsupported conditions (1):", style.unsupported)),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!("{}Invalid keys (1):", style.invalid)),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!("{}Conflicts (1):", style.invalid)),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!(
                "{}Disabled by terminal capability (1):",
                style.disabled
            )),
            "{rendered}"
        );
        assert!(rendered.contains(style.reset), "{rendered}");
        // Bucket headings additionally carry the title's bold prefix.
        assert!(
            rendered.contains(&format!("{}{}Imported (1):", style.title, style.imported)),
            "{rendered}"
        );
    }

    #[test]
    fn render_summary_with_ansi_colors_the_count_lines() {
        let report = sample_report();
        let style = ReportStyle::ansi();
        let rendered = report.render_summary_with(&style);

        assert!(
            rendered.contains(&format!(
                "{}VS Code keybinding import completed.{}",
                style.title, style.reset
            )),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!("{}Imported: 1{}", style.imported, style.reset)),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!("{}Ignored: 1{}", style.ignored, style.reset)),
            "{rendered}"
        );
    }

    /// TASK-260712-18 testcase: stripping the SGR sequences from an ansi
    /// render must yield exactly the plain render — color must never change
    /// the text content, only decorate it.
    #[test]
    fn stripping_ansi_sequences_from_ansi_render_matches_plain_render() {
        let report = sample_report();
        let ansi_text = report.render_text_with(&ReportStyle::ansi());
        assert_eq!(strip_ansi(&ansi_text), report.render_text());

        let ansi_summary = report.render_summary_with(&ReportStyle::ansi());
        assert_eq!(strip_ansi(&ansi_summary), report.render_summary());
    }

    /// TASK-260712-18 testcase: only the trailing `[reason]` on an entry
    /// line is dimmed — `key -> command` (and an optional `[when]`) must
    /// appear with no color prefix at all.
    #[test]
    fn entry_reason_is_dimmed_but_key_and_command_are_not_colored() {
        let report = sample_report();
        let style = ReportStyle::ansi();
        let rendered = report.render_text_with(&style);

        // `key -> command` must appear completely unadorned.
        assert!(rendered.contains("- ctrl+j -> cursor.down "), "{rendered}");
        // The `[when]` bracket (when present) must also stay unadorned.
        assert!(
            rendered.contains("cursor.up [editorFocus && weirdCondition] "),
            "{rendered}"
        );
        // The trailing `[reason]` is wrapped in the dim prefix + reset.
        assert!(
            rendered.contains(&format!("{}[imported]{}", style.reason, style.reset)),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!(
                "{}[unsupported condition]{}",
                style.reason, style.reset
            )),
            "{rendered}"
        );
    }
}
