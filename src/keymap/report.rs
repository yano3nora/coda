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
    // TODO(SPEC-0003): wire terminal capability detection into the importer.
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

    pub fn render_text(&self) -> String {
        let s = self.summary();
        let mut lines = vec![
            "VS Code keybinding import completed.".to_string(),
            String::new(),
            format!("Imported: {}", s.imported),
            format!("Ignored: {}", s.ignored),
            format!("Unsupported commands: {}", s.unsupported_commands),
            format!("Unsupported conditions: {}", s.unsupported_conditions),
            format!("Invalid keys: {}", s.invalid_keys),
            format!("Conflicts: {}", s.conflicts),
            format!(
                "Disabled by terminal capability: {}",
                s.disabled_by_terminal_capability
            ),
            String::new(),
            "Examples:".to_string(),
        ];

        self.push_example(&mut lines, "Imported", self.imported.first());
        self.push_example(&mut lines, "Ignored", self.ignored.first());
        self.push_example(
            &mut lines,
            "Unsupported command",
            self.unsupported_commands.first(),
        );
        self.push_example(
            &mut lines,
            "Unsupported condition",
            self.unsupported_conditions.first(),
        );
        self.push_example(&mut lines, "Invalid key", self.invalid_keys.first());
        self.push_example(&mut lines, "Conflict", self.conflicts.first());
        self.push_example(
            &mut lines,
            "Disabled",
            self.disabled_by_terminal_capability.first(),
        );
        lines.push(String::new());
        lines.join("\n")
    }

    fn push_example(&self, lines: &mut Vec<String>, label: &str, entry: Option<&ReportEntry>) {
        if let Some(entry) = entry {
            lines.push(format!("- {label} {}", format_entry(entry)));
        }
    }
}

fn format_entry(entry: &ReportEntry) -> String {
    let key = entry.key.as_deref().unwrap_or("<missing key>");
    let command = entry.command.as_deref().unwrap_or("<missing command>");
    let when = entry
        .when
        .as_ref()
        .map(|when| format!(" [{when}]"))
        .unwrap_or_default();
    format!("{key} -> {command}{when} [{}]", entry.reason)
}
