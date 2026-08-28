//! Application-level CLI routing and event-loop ownership.

mod clipboard;
mod config;
mod default_bindings;
mod document;
mod editor_view;
mod event_loop;
mod file;
mod import_cli;
mod info;
mod inspector;
mod palette;
mod prompt_overlay;
mod search_overlay;
mod verify_cli;
mod which_key;

use std::{env, ffi::OsString, path::PathBuf};

use crate::input;
use crate::keymap::CmdStrategy;
use event_loop::EventLoop;
use import_cli::ImportOptions;

/// Printed by `coda --help`/`-h`, and (still) the usage line for a malformed
/// subcommand. With no path argument, `coda` opens an empty unnamed buffer
/// rather than erroring (TASK-260711-19) — the `[path...]` bracket reflects
/// that it is optional.
const USAGE: &str = "\
usage: coda [+N] [path...]
       coda inspect-key
       coda keymap import vscode <path> [--dry-run] [--print-report] [--cmd=keep|ctrl|both]
       coda keymap verify

With no path, coda opens a single empty unnamed buffer.
+N jumps to line N of the first file (vim-compatible; works as lazygit's
editAtLine target). Arguments after `--` are always treated as paths.";

/// Runs the CLI entrypoint and returns a process exit code.
pub fn run() -> i32 {
    match Command::parse(env::args_os().skip(1)) {
        Command::InvalidUsage(message) => {
            eprintln!("{message}");
            2
        }
        Command::Help => {
            println!("{USAGE}");
            0
        }
        // release script (scripts/release.ts) が tag と binary の version 整合
        // チェックに使うため、出力は "coda <semver>" の形を維持すること
        Command::Version => {
            println!("coda {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::InspectKey => match input::inspect_key() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("inspect-key failed: {error}");
                1
            }
        },
        Command::KeymapVerify => verify_cli::run_keymap_verify(),
        Command::KeymapImportVscode(options) => match import_cli::run_vscode_import(&options) {
            Ok(output) => {
                print!("{}", output.stdout);
                0
            }
            Err(error) => {
                eprintln!("keymap import failed: {error}");
                1
            }
        },
        Command::OpenFiles { paths, line } => run_editor(paths, line),
    }
}

/// Opens the editor. Empty `paths` is not an error: it opens a single
/// unnamed buffer (TASK-260711-19), the same buffer `buffer.new` creates —
/// its Save writes to disk only once the user picks a location.
fn run_editor(paths: Vec<PathBuf>, line: Option<usize>) -> i32 {
    let loaded_config = config::load();

    match EventLoop::open_many(
        paths,
        Vec::new(),
        loaded_config.user_bindings,
        loaded_config.theme,
    ) {
        Ok(mut loop_) => {
            loop_.set_wrap(loaded_config.wrap);
            loop_.set_indent(loaded_config.indent);
            loop_.set_sequence_timeout(std::time::Duration::from_millis(
                loaded_config.sequence_timeout_ms,
            ));
            loop_.set_capability_warning(loaded_config.capability_warning);
            loop_.set_ctrl_c_quits(loaded_config.ctrl_c_quits);
            // palette_key BEFORE disable_chords: verify may have measured the
            // default ctrl+space as undeliverable, and replacing an already
            // removed rescue binding would silently ignore the config value.
            // This order lets disable_chords judge the user's actual chord.
            if let Some(palette_key) = loaded_config.palette_key {
                loop_.set_palette_key(palette_key);
            }
            loop_.disable_chords(&loaded_config.disabled_chords);
            // Config-breakage warnings and the environment report go to the
            // startup info panel; the status bar keeps only the palette hint
            // and short per-file notices (TASK-260820-environment-info-panel).
            // Runs after the binding mutations above so the interception
            // report reflects the live binding set.
            let mut config_warnings = loaded_config.warnings;
            let ack_path = config::environment_ack_path();
            let acknowledged = ack_path
                .as_deref()
                .map(|path| config::load_environment_ack(path, &mut config_warnings))
                .unwrap_or_default();
            loop_.set_startup_info(
                config_warnings,
                &acknowledged,
                ack_path,
                input::quirks::is_ghostty(),
            );
            if let Some(line) = line {
                loop_.set_initial_line(line);
            }
            match loop_.run() {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("editor failed: {error}");
                    1
                }
            }
        }
        Err(error) => {
            eprintln!("failed to open file: {error}");
            1
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    InvalidUsage(String),
    Help,
    Version,
    InspectKey,
    KeymapVerify,
    KeymapImportVscode(ImportOptions),
    OpenFiles {
        paths: Vec<PathBuf>,
        /// 1-based line for the vim-compatible `+N` argument (TASK-260729);
        /// applied to the first file by `run_editor`.
        line: Option<usize>,
    },
}

impl Command {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Self {
        let args = args.into_iter().collect::<Vec<_>>();
        if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
            return Self::Help;
        }
        if args.len() == 1 && (args[0] == "--version" || args[0] == "-V") {
            return Self::Version;
        }
        if args.len() == 1 && args[0] == "inspect-key" {
            return Self::InspectKey;
        }
        if let Some(command) = parse_keymap_import_vscode(&args) {
            return command;
        }

        parse_open_args(args)
    }
}

/// Parses the default "open files" form: `coda [+N] [path...] [-- path...]`.
/// `+N` may appear anywhere before `--` (vim accepts both orders, and
/// lazygit's default editAtLine template emits `+{{line}} -- {{filename}}`).
/// Everything after `--` is a path, so files whose names start with `+` stay
/// openable. Bad `+` arguments are rejected loudly rather than reinterpreted
/// as file names — silently opening a file named `+1O` when the caller meant
/// a line jump would violate the "no silent breakage" rule.
fn parse_open_args(args: Vec<OsString>) -> Command {
    let mut paths = Vec::new();
    let mut line = None;
    let mut rest_are_paths = false;
    for arg in args {
        if !rest_are_paths {
            if arg == "--" {
                rest_are_paths = true;
                continue;
            }
            // Non-UTF-8 arguments can only be paths; `+N` is always ASCII.
            if let Some(raw) = arg.to_str().and_then(|text| text.strip_prefix('+')) {
                if line.is_some() {
                    return Command::InvalidUsage(
                        "multiple +N arguments; pass a single line number".to_string(),
                    );
                }
                match raw.parse::<usize>() {
                    Ok(parsed) if parsed > 0 => line = Some(parsed),
                    _ => {
                        return Command::InvalidUsage(format!(
                            "invalid line number in +{raw} (expected +N with N >= 1; \
                             use `--` before file names starting with '+')"
                        ));
                    }
                }
                continue;
            }
        }
        paths.push(PathBuf::from(arg));
    }
    Command::OpenFiles { paths, line }
}

fn parse_keymap_import_vscode(args: &[OsString]) -> Option<Command> {
    if args.first()? != "keymap" {
        return None;
    }
    if args.len() >= 2 && args[1] == "verify" {
        return Some(if args.len() == 2 {
            Command::KeymapVerify
        } else {
            Command::InvalidUsage("keymap verify takes no arguments".to_string())
        });
    }
    if args.len() < 3 || args[1] != "import" || args[2] != "vscode" {
        return Some(Command::InvalidUsage(
            "usage: coda keymap import vscode <path> [--dry-run] [--print-report] [--cmd=keep|ctrl|both]\n       coda keymap verify"
                .to_string(),
        ));
    }
    if args.len() < 4 {
        return Some(Command::InvalidUsage(
            "missing path: coda keymap import vscode <path>".to_string(),
        ));
    }

    let path = PathBuf::from(&args[3]);
    let mut options = ImportOptions {
        path,
        dry_run: false,
        print_report: false,
        cmd: CmdStrategy::Keep,
    };
    for flag in &args[4..] {
        let flag_text = flag.to_string_lossy();
        match flag_text.as_ref() {
            "--dry-run" => options.dry_run = true,
            // Accepted for backwards compatibility; overwriting is now the default.
            "--replace" => {}
            "--print-report" => options.print_report = true,
            "--cmd" => {
                return Some(Command::InvalidUsage(
                    "missing value for --cmd (expected --cmd=keep|ctrl|both)".to_string(),
                ));
            }
            value if value.starts_with("--cmd=") => {
                let raw = &value["--cmd=".len()..];
                options.cmd = match raw {
                    "keep" => CmdStrategy::Keep,
                    "ctrl" => CmdStrategy::Ctrl,
                    "both" => CmdStrategy::Both,
                    other => {
                        return Some(Command::InvalidUsage(format!(
                            "invalid --cmd value: {other} (expected keep|ctrl|both)"
                        )));
                    }
                };
            }
            unknown => {
                return Some(Command::InvalidUsage(format!(
                    "unknown keymap import flag: {unknown}"
                )));
            }
        }
    }
    Some(Command::KeymapImportVscode(options))
}

#[cfg(test)]
mod tests {
    use super::Command;
    use crate::keymap::CmdStrategy;
    use std::{ffi::OsString, path::PathBuf};

    #[test]
    fn parse_inspect_key_subcommand() {
        assert_eq!(
            Command::parse([OsString::from("inspect-key")]),
            Command::InspectKey
        );
    }

    /// TASK-260711-19: empty args must still route to `OpenFiles(vec![])` —
    /// `run_editor` (not `Command::parse`) is what turns that into an
    /// unnamed-buffer startup, exercised end-to-end via `EventLoop::open_many`
    /// in `event_loop.rs`'s own tests.
    #[test]
    fn parse_no_args_as_open_without_paths() {
        assert_eq!(
            Command::parse([]),
            Command::OpenFiles {
                paths: vec![],
                line: None,
            }
        );
    }

    /// TASK-260711-19: `--help`/`-h` must not be swallowed as a literal
    /// filename to open (`OpenFiles(["--help"])` would try to open a file by
    /// that name), now that empty args no longer trip a usage error on their
    /// own.
    #[test]
    fn parse_help_flags_as_help_command() {
        assert_eq!(Command::parse([OsString::from("--help")]), Command::Help);
        assert_eq!(Command::parse([OsString::from("-h")]), Command::Help);
    }

    #[test]
    fn parse_version_flags_as_version_command() {
        assert_eq!(
            Command::parse([OsString::from("--version")]),
            Command::Version
        );
        assert_eq!(Command::parse([OsString::from("-V")]), Command::Version);
    }

    #[test]
    fn parse_paths_as_editor_open() {
        assert_eq!(
            Command::parse([OsString::from("a.txt"), OsString::from("b.txt")]),
            Command::OpenFiles {
                paths: vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
                line: None,
            }
        );
    }

    /// TASK-260729: vim-compatible `+N` line jump, table-driven over argument
    /// order and the `--` separator. `+N` before or after the path must both
    /// work (vim accepts either), while anything after `--` is a literal path
    /// — that keeps files named like `+10` reachable and matches lazygit's
    /// default `+{{line}} -- {{filename}}` editAtLine template.
    #[test]
    fn parse_plus_line_argument_table_driven() {
        let cases: &[(&[&str], &[&str], Option<usize>)] = &[
            (&["+10", "a.txt"], &["a.txt"], Some(10)),
            (&["a.txt", "+10"], &["a.txt"], Some(10)),
            (&["+10"], &[], Some(10)),
            (&["+5", "--", "+10"], &["+10"], Some(5)),
            (&["--", "+10"], &["+10"], None),
            (&["--"], &[], None),
        ];

        for (args, expected_paths, expected_line) in cases {
            assert_eq!(
                Command::parse(args.iter().map(OsString::from)),
                Command::OpenFiles {
                    paths: expected_paths.iter().map(PathBuf::from).collect(),
                    line: *expected_line,
                },
                "{args:?}"
            );
        }
    }

    /// A malformed `+` argument must fail loudly instead of being opened as
    /// a file by that name: the caller (a script, lazygit template, muscle
    /// memory) meant a line jump, and silently editing a new file called
    /// `+abc` is exactly the kind of quiet breakage coda promises to avoid.
    /// Duplicate `+N` is ambiguous, so it is rejected rather than picking one.
    #[test]
    fn parse_plus_line_rejects_malformed_and_duplicate() {
        let cases: &[&[&str]] = &[
            &["+0", "a.txt"],
            &["+abc", "a.txt"],
            &["+", "a.txt"],
            &["+1", "+2", "a.txt"],
        ];

        for args in cases {
            assert!(
                matches!(
                    Command::parse(args.iter().map(OsString::from)),
                    Command::InvalidUsage(_)
                ),
                "{args:?}"
            );
        }
    }

    /// ADR-0007 §2(c): `keymap verify` parses as its own subcommand and
    /// rejects stray arguments.
    #[test]
    fn parse_keymap_verify_subcommand() {
        assert_eq!(
            Command::parse([OsString::from("keymap"), OsString::from("verify")]),
            Command::KeymapVerify
        );
        assert!(matches!(
            Command::parse([
                OsString::from("keymap"),
                OsString::from("verify"),
                OsString::from("--fast"),
            ]),
            Command::InvalidUsage(_)
        ));
    }

    #[test]
    fn parse_vscode_import_subcommand() {
        assert_eq!(
            Command::parse([
                OsString::from("keymap"),
                OsString::from("import"),
                OsString::from("vscode"),
                OsString::from("keys.json"),
                OsString::from("--dry-run"),
                OsString::from("--replace"),
                OsString::from("--print-report"),
            ]),
            Command::KeymapImportVscode(super::ImportOptions {
                path: PathBuf::from("keys.json"),
                dry_run: true,
                print_report: true,
                cmd: CmdStrategy::Keep,
            })
        );
    }

    /// ADR-0007 §3: `--cmd=keep|ctrl|both` parses to the matching
    /// `CmdStrategy`, table-driven over all three valid values (and the
    /// absence of the flag, which must default to `Keep`).
    #[test]
    fn parse_cmd_flag_values_table_driven() {
        let cases: &[(Option<&str>, CmdStrategy)] = &[
            (None, CmdStrategy::Keep),
            (Some("--cmd=keep"), CmdStrategy::Keep),
            (Some("--cmd=ctrl"), CmdStrategy::Ctrl),
            (Some("--cmd=both"), CmdStrategy::Both),
        ];

        for (flag, expected) in cases {
            let mut args = vec![
                OsString::from("keymap"),
                OsString::from("import"),
                OsString::from("vscode"),
                OsString::from("keys.json"),
            ];
            if let Some(flag) = flag {
                args.push(OsString::from(*flag));
            }

            assert_eq!(
                Command::parse(args),
                Command::KeymapImportVscode(super::ImportOptions {
                    path: PathBuf::from("keys.json"),
                    dry_run: false,
                    print_report: false,
                    cmd: *expected,
                }),
                "{flag:?}"
            );
        }
    }

    /// An unrecognized `--cmd` value (e.g. a typo'd modifier name) must be
    /// rejected with `InvalidUsage`, not silently fall back to a default —
    /// silently picking the wrong Cmd strategy could reintroduce
    /// undeliverable Super chords the user was explicitly trying to avoid.
    #[test]
    fn parse_cmd_flag_rejects_invalid_value() {
        let result = Command::parse([
            OsString::from("keymap"),
            OsString::from("import"),
            OsString::from("vscode"),
            OsString::from("keys.json"),
            OsString::from("--cmd=meta"),
        ]);
        match result {
            Command::InvalidUsage(message) => {
                assert!(message.contains("--cmd"), "{message}");
                assert!(message.contains("meta"), "{message}");
            }
            other => panic!("expected InvalidUsage, got {other:?}"),
        }
    }

    /// A bare `--cmd` with no `=value` must be rejected with a message that
    /// explains a value is required, rather than being treated as an unknown
    /// flag with no further context.
    #[test]
    fn parse_cmd_flag_rejects_missing_value() {
        let result = Command::parse([
            OsString::from("keymap"),
            OsString::from("import"),
            OsString::from("vscode"),
            OsString::from("keys.json"),
            OsString::from("--cmd"),
        ]);
        match result {
            Command::InvalidUsage(message) => {
                assert!(message.contains("--cmd"), "{message}");
            }
            other => panic!("expected InvalidUsage, got {other:?}"),
        }
    }
}
