//! `coda keymap import vscode` CLI implementation.
//!
//! This is the only layer that touches the filesystem. The keymap importer stays
//! pure and receives text, which keeps report classification unit-testable.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    input::{CapabilityDetection, probe_blocking},
    keymap::{VsCodeImportError, import_vscode_keybindings, render_generated_bindings},
};

/// How long the CLI blocks a TTY stdin for a capability-query reply before
/// falling back to legacy (mirrors the event loop's own deadline, SPEC-0003
/// design decision 260712).
const CAPABILITY_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ImportOptions {
    pub path: PathBuf,
    pub dry_run: bool,
    pub print_report: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ImportOutput {
    pub stdout: String,
    pub generated_path: PathBuf,
    pub report_path: PathBuf,
}

pub fn run_vscode_import(options: &ImportOptions) -> Result<ImportOutput, String> {
    let base_dir = config_base_dir().ok_or_else(|| "HOME is not set".to_string())?;
    run_vscode_import_in_base(options, &base_dir)
}

pub(crate) fn run_vscode_import_in_base(
    options: &ImportOptions,
    base_dir: &Path,
) -> Result<ImportOutput, String> {
    let input = fs::read_to_string(&options.path)
        .map_err(|error| format!("{}: {error}", options.path.display()))?;

    // Detect once per invocation (TASK-260712-16): a piped/CI stdin cannot
    // answer an escape-sequence query at all, so `probe_blocking` assumes
    // modern there rather than spending the full timeout discovering nothing
    // and then silently disabling every super/shift-enter/ctrl-enter binding
    // on every non-interactive run.
    let detection = probe_blocking(CAPABILITY_PROBE_TIMEOUT);
    let capabilities = detection.capabilities();
    let imported =
        import_vscode_keybindings(&input, &capabilities).map_err(|error| match error {
            VsCodeImportError::InvalidJson(detail) => {
                format!("invalid VS Code keybindings JSON: {detail}")
            }
            VsCodeImportError::RootNotArray => {
                "VS Code keybindings root must be an array".to_string()
            }
        })?;

    let generated_path = base_dir.join("generated").join("vscode-bindings.json");
    let report_path = base_dir
        .join("import-reports")
        .join("latest-vscode-import.txt");
    let capability_line = capability_report_line(detection);
    let report_body = imported.report.render_text();
    let report_text = format!("{capability_line}\n\n{report_body}");

    if !options.dry_run {
        // generated/ is importer-owned output; re-import is the normal
        // workflow, so it is always overwritten (user bindings are separate).
        write_parented(
            &generated_path,
            render_generated_bindings(&imported.bindings).as_bytes(),
        )
        .map_err(|error| format!("{}: {error}", generated_path.display()))?;
        write_parented(&report_path, report_text.as_bytes())
            .map_err(|error| format!("{}: {error}", report_path.display()))?;
    }

    let summary = imported.report.summary();
    let mut stdout = format!(
        "{capability_line}\nVS Code keybinding import completed.\nImported: {}\nIgnored: {}\nUnsupported commands: {}\nUnsupported conditions: {}\nInvalid keys: {}\nConflicts: {}\nDisabled by terminal capability: {}\n",
        summary.imported,
        summary.ignored,
        summary.unsupported_commands,
        summary.unsupported_conditions,
        summary.invalid_keys,
        summary.conflicts,
        summary.disabled_by_terminal_capability
    );
    if options.print_report {
        stdout.push('\n');
        stdout.push_str(&report_body);
    }

    Ok(ImportOutput {
        stdout,
        generated_path,
        report_path,
    })
}

/// Formats the CLI's single capability summary line (TASK-260712-16). This
/// is intentionally coarser than `CapabilityDetection::description` (used by
/// the in-editor inspector): the CLI only needs "modern / legacy / not
/// detected", not which of the two legacy sub-reasons (DA1-first vs.
/// timeout) applied.
fn capability_report_line(detection: CapabilityDetection) -> String {
    match detection {
        CapabilityDetection::KittyFlags(flags) if flags & 1 != 0 => {
            format!("Terminal capability: modern (kitty CSI u, flags={flags})")
        }
        CapabilityDetection::AssumedModern => {
            "Terminal capability: not detected (not an interactive terminal); assuming modern"
                .to_string()
        }
        _ => "Terminal capability: legacy (no CSI ?u reply)".to_string(),
    }
}

pub(crate) fn config_base_dir() -> Option<PathBuf> {
    if let Some(base) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(base).join("coda"));
    }
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("coda"))
}

fn write_parented(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::{ImportOptions, run_vscode_import_in_base};
    use std::fs;

    #[test]
    fn dry_run_does_not_write_files() {
        let temp = std::env::temp_dir().join(format!("coda-import-dry-run-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();
        let input_path = temp.join("keybindings.json");
        fs::write(
            &input_path,
            r#"[{ "key": "ctrl+j", "command": "cursorDown", "when": "editorFocus" }]"#,
        )
        .unwrap();

        let output = run_vscode_import_in_base(
            &ImportOptions {
                path: input_path,
                dry_run: true,
                print_report: false,
            },
            &temp.join("config"),
        )
        .unwrap();

        assert!(output.stdout.contains("Imported: 1"));
        assert!(!output.generated_path.exists());
        assert!(!output.report_path.exists());
        fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn reimport_overwrites_existing_generated_without_any_flag() {
        let temp = std::env::temp_dir().join(format!("coda-import-replace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let base = temp.join("config");
        fs::create_dir_all(base.join("generated")).unwrap();
        fs::write(base.join("generated/vscode-bindings.json"), "stale").unwrap();
        let input_path = temp.join("keybindings.json");
        fs::write(
            &input_path,
            r#"[{ "key": "ctrl+j", "command": "cursorDown" }]"#,
        )
        .unwrap();

        run_vscode_import_in_base(
            &ImportOptions {
                path: input_path,
                dry_run: false,
                print_report: false,
            },
            &base,
        )
        .unwrap();

        let generated = fs::read_to_string(base.join("generated/vscode-bindings.json")).unwrap();
        assert!(generated.contains("cursor.down"), "{generated}");
        fs::remove_dir_all(&temp).unwrap();
    }

    /// TASK-260712-16: the CLI's stdin under `cargo test` is not a TTY, so
    /// `probe_blocking` takes the fast `AssumedModern` path — this asserts
    /// the exact wording that path produces shows up at the top of both
    /// stdout and the written report file, without needing a real terminal.
    #[test]
    fn stdout_and_report_file_lead_with_the_terminal_capability_line() {
        let temp = std::env::temp_dir().join(format!(
            "coda-import-capability-line-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp);
        let base = temp.join("config");
        let input_path = temp.join("keybindings.json");
        fs::create_dir_all(&temp).unwrap();
        fs::write(
            &input_path,
            r#"[{ "key": "ctrl+j", "command": "cursorDown", "when": "editorFocus" }]"#,
        )
        .unwrap();

        let output = run_vscode_import_in_base(
            &ImportOptions {
                path: input_path,
                dry_run: false,
                print_report: false,
            },
            &base,
        )
        .unwrap();

        const EXPECTED_LINE: &str =
            "Terminal capability: not detected (not an interactive terminal); assuming modern";
        assert!(
            output.stdout.starts_with(EXPECTED_LINE),
            "{}",
            output.stdout
        );
        let report = fs::read_to_string(&output.report_path).unwrap();
        assert!(report.starts_with(EXPECTED_LINE), "{report}");
        assert!(
            report.contains("Disabled by terminal capability: 0"),
            "{report}"
        );

        fs::remove_dir_all(&temp).unwrap();
    }
}
