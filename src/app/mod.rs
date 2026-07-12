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

use std::{env, ffi::OsString, path::PathBuf};

use crate::input;
use event_loop::EventLoop;
use import_cli::ImportOptions;

/// Printed by `coda --help`/`-h`, and (still) the usage line for a malformed
/// subcommand. With no path argument, `coda` opens an empty unnamed buffer
/// rather than erroring (TASK-260711-19) — the `[path...]` bracket reflects
/// that it is optional.
const USAGE: &str = "\
usage: coda [path...]
       coda inspect-key
       coda keymap import vscode <path> [--dry-run] [--print-report]

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
    if args.len() < 3 || args[1] != "import" || args[2] != "vscode" {
        return Some(Command::InvalidUsage(
            "usage: coda keymap import vscode <path> [--dry-run] [--print-report]".to_string(),
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
    };
    for flag in &args[4..] {
        match flag.to_string_lossy().as_ref() {
            "--dry-run" => options.dry_run = true,
            // Accepted for backwards compatibility; overwriting is now the default.
            "--replace" => {}
            "--print-report" => options.print_report = true,
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
            })
        );
    }
}
