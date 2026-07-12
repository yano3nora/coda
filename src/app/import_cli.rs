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
    keymap::{
        ReportStyle, VsCodeImportError, import_vscode_keybindings, render_generated_bindings,
    },
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
    // isatty/NO_COLOR is decided once here (not inside `run_vscode_import_in_base`)
    // so the base function stays a pure style-in/text-out unit under test
    // (TASK-260712-18 design).
    let stdout_style = if stdout_supports_color() {
        ReportStyle::ansi()
    } else {
        ReportStyle::plain()
    };
    run_vscode_import_in_base(options, &base_dir, &stdout_style)
}

/// Whether stdout should be colorized: stdout must be a real terminal (not a
/// pipe/redirect) and the `NO_COLOR` convention must not opt out.
fn stdout_supports_color() -> bool {
    stdout_is_tty() && no_color_env_allows_color()
}

/// Mirrors the `libc::isatty` precedent in `src/input/capabilities.rs`, but
/// checks stdout specifically (the report is printed there; stdin's TTY-ness
/// is irrelevant to whether stdout is colorized).
fn stdout_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDOUT_FILENO) != 0 }
}

fn no_color_env_allows_color() -> bool {
    no_color_allows_color(env::var_os("NO_COLOR").as_deref())
}

/// NO_COLOR convention (https://no-color.org/): color is disabled only when
/// the variable is *present and non-empty* — `NO_COLOR=` (set but empty)
/// must NOT disable color. Takes the raw value rather than reading the env
/// itself so tests can exercise all three states without mutating global
/// process env (which is flaky under parallel `cargo test`).
fn no_color_allows_color(no_color: Option<&std::ffi::OsStr>) -> bool {
    match no_color {
        None => true,
        Some(value) => value.is_empty(),
    }
}

pub(crate) fn run_vscode_import_in_base(
    options: &ImportOptions,
    base_dir: &Path,
    stdout_style: &ReportStyle,
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

    // `--print-report` prints the full report (which already embeds the
    // summary block), so the default path must not render the summary a
    // second time via a hand-built string — both share
    // `ImportReport::render_summary()` (TASK-260712-17). stdout uses
    // `stdout_style` (ANSI or plain, decided by the caller); the saved file
    // above always uses the plain `report_body` regardless (TASK-260712-18:
    // the file on disk must never contain escape codes).
    let mut stdout = format!("{capability_line}\n");
    if options.print_report {
        stdout.push_str(&imported.report.render_text_with(stdout_style));
    } else {
        stdout.push_str(&imported.report.render_summary_with(stdout_style));
        stdout.push('\n');
    }
    if !options.dry_run {
        stdout.push_str(&format!("\nReport saved to: {}\n", report_path.display()));
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
    use super::{
        ImportOptions, no_color_allows_color, run_vscode_import_in_base, stdout_supports_color,
    };
    use crate::keymap::ReportStyle;
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
            &ReportStyle::plain(),
        )
        .unwrap();

        assert!(output.stdout.contains("Imported: 1"));
        assert!(
            !output.stdout.contains("Report saved to:"),
            "{}",
            output.stdout
        );
        assert!(!output.generated_path.exists());
        assert!(!output.report_path.exists());
        fs::remove_dir_all(&temp).unwrap();
    }

    /// TASK-260712-17 testcase: non-dry-run stdout must tell the user where
    /// the report landed, since `render_summary()` alone never mentions a
    /// path.
    #[test]
    fn non_dry_run_stdout_reports_the_saved_report_path() {
        let temp =
            std::env::temp_dir().join(format!("coda-import-saved-path-{}", std::process::id()));
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
                dry_run: false,
                print_report: false,
            },
            &temp.join("config"),
            &ReportStyle::plain(),
        )
        .unwrap();

        let expected_line = format!("Report saved to: {}", output.report_path.display());
        assert!(output.stdout.contains(&expected_line), "{}", output.stdout);
        fs::remove_dir_all(&temp).unwrap();
    }

    /// TASK-260712-17 testcase: `--print-report` must not print the summary
    /// (`Imported:` line) twice — once from a hand-built CLI string and once
    /// from `render_text()`.
    #[test]
    fn print_report_stdout_contains_summary_exactly_once() {
        let temp =
            std::env::temp_dir().join(format!("coda-import-print-report-{}", std::process::id()));
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
                dry_run: false,
                print_report: true,
            },
            &temp.join("config"),
            &ReportStyle::plain(),
        )
        .unwrap();

        let occurrences = output.stdout.matches("Imported: 1").count();
        assert_eq!(occurrences, 1, "{}", output.stdout);
        assert!(
            output.stdout.contains(&format!(
                "Report saved to: {}",
                output.report_path.display()
            )),
            "{}",
            output.stdout
        );
        // Full listing must also be present exactly once, not just the summary count line.
        assert_eq!(
            output.stdout.matches("Imported (1):").count(),
            1,
            "{}",
            output.stdout
        );
        // TASK-260712-18 testcase: `ReportStyle::plain()` stdout must never
        // contain an escape byte.
        assert!(
            !output.stdout.contains('\u{1b}'),
            "plain style must not emit ANSI escapes: {}",
            output.stdout
        );
        fs::remove_dir_all(&temp).unwrap();
    }

    /// TASK-260712-17 testcase: the saved `latest-vscode-import.txt` must
    /// contain every entry of a multi-entry bucket, not just a sampled one.
    #[test]
    fn saved_report_file_contains_every_entry_of_a_bucket() {
        let temp = std::env::temp_dir().join(format!(
            "coda-import-saved-full-listing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();
        let input_path = temp.join("keybindings.json");
        fs::write(
            &input_path,
            r#"[
                { "key": "cmd+t", "command": "workbench.action.terminal.new" },
                { "key": "cmd+p", "command": "workbench.action.quickOpen" },
                { "key": "cmd+shift+e", "command": "workbench.view.explorer" }
            ]"#,
        )
        .unwrap();

        let output = run_vscode_import_in_base(
            &ImportOptions {
                path: input_path,
                dry_run: false,
                print_report: false,
            },
            &temp.join("config"),
            &ReportStyle::plain(),
        )
        .unwrap();

        let report = fs::read_to_string(&output.report_path).unwrap();
        assert!(report.contains("Ignored (3):"), "{report}");
        for expected in [
            "workbench.action.terminal.new",
            "workbench.action.quickOpen",
            "workbench.view.explorer",
        ] {
            assert!(
                report.contains(expected),
                "missing `{expected}` in\n{report}"
            );
        }
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
            &ReportStyle::plain(),
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
            &ReportStyle::plain(),
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

    /// TASK-260712-18 testcase: `NO_COLOR` is only an opt-out when *present
    /// and non-empty* (https://no-color.org/) — `NO_COLOR=` (set but empty)
    /// must not disable color. Exercised via the pure helper (not real env
    /// vars) so this is deterministic under parallel test execution.
    #[test]
    fn no_color_convention_only_disables_color_when_set_and_non_empty() {
        use std::ffi::OsStr;

        assert!(no_color_allows_color(None), "unset must allow color");
        assert!(
            !no_color_allows_color(Some(OsStr::new("1"))),
            "NO_COLOR=1 must disable color"
        );
        assert!(
            no_color_allows_color(Some(OsStr::new(""))),
            "NO_COLOR= (empty) must not disable color"
        );
    }

    /// TASK-260712-18 testcase: color-disabled (plain style) CLI stdout must
    /// never contain an escape byte. `cargo test`'s stdout is not a TTY
    /// (same assumption the capability-line test above already relies on),
    /// so `stdout_supports_color()` is deterministically false here too.
    #[test]
    fn stdout_supports_color_is_false_under_cargo_test() {
        assert!(
            !stdout_supports_color(),
            "cargo test's stdout is not an interactive terminal"
        );
    }

    /// TASK-260712-18 testcase: color-enabled stdout must contain SGR escape
    /// sequences, but the file written to disk must always stay plain even
    /// when the caller asks for `ReportStyle::ansi()` stdout.
    #[test]
    fn ansi_style_colors_stdout_but_saved_report_file_stays_plain() {
        let temp =
            std::env::temp_dir().join(format!("coda-import-ansi-stdout-{}", std::process::id()));
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
                dry_run: false,
                print_report: true,
            },
            &temp.join("config"),
            &ReportStyle::ansi(),
        )
        .unwrap();

        assert!(
            output.stdout.contains('\u{1b}'),
            "ansi style must emit an escape byte: {}",
            output.stdout
        );
        let report = fs::read_to_string(&output.report_path).unwrap();
        assert!(
            !report.contains('\u{1b}'),
            "saved report file must always stay plain: {report}"
        );
        fs::remove_dir_all(&temp).unwrap();
    }
}
