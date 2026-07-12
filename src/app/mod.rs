//! Application-level CLI routing and event-loop ownership.

mod clipboard;
mod config;
mod default_bindings;
mod document;
mod editor_view;
mod event_loop;
mod file;
mod import_cli;
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
usage: coda [path...]
       coda inspect-key
       coda keymap import vscode <path> [--dry-run] [--print-report] [--cmd=keep|ctrl|both]
       coda keymap verify

With no path, coda opens a single empty unnamed buffer.";

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
        Command::OpenFiles(paths) => run_editor(paths),
    }
}

/// Opens the editor. Empty `paths` is not an error: it opens a single
/// unnamed buffer (TASK-260711-19), the same buffer `buffer.new` creates —
/// its Save writes to disk only once the user picks a location.
fn run_editor(paths: Vec<PathBuf>) -> i32 {
    let mut warnings = Vec::new();
    let loaded_config = config::load();
    warnings.extend(loaded_config.warnings);

    match EventLoop::open_many(
        paths,
        warnings,
        loaded_config.user_bindings,
        loaded_config.theme,
    ) {
        Ok(mut loop_) => {
            loop_.set_wrap(loaded_config.wrap);
            loop_.set_sequence_timeout(std::time::Duration::from_millis(
                loaded_config.sequence_timeout_ms,
            ));
            loop_.set_capability_warning(loaded_config.capability_warning);
            if let Some(palette_key) = loaded_config.palette_key {
                loop_.set_palette_key(palette_key);
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
    OpenFiles(Vec<PathBuf>),
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

        Self::OpenFiles(args.into_iter().map(PathBuf::from).collect())
    }
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
        assert_eq!(Command::parse([]), Command::OpenFiles(vec![]));
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
            Command::OpenFiles(vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")])
        );
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
