//! Application-level CLI routing and event-loop ownership.

mod config;
mod default_bindings;
mod editor_view;
mod event_loop;
mod file;
mod import_cli;
mod palette;

use std::{env, ffi::OsString, path::PathBuf};

use crate::input;
use event_loop::EventLoop;
use import_cli::ImportOptions;

/// Runs the CLI entrypoint and returns a process exit code.
pub fn run() -> i32 {
    match Command::parse(env::args_os().skip(1)) {
        Command::InvalidUsage(message) => {
            eprintln!("{message}");
            2
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

fn run_editor(paths: Vec<PathBuf>) -> i32 {
    if paths.is_empty() {
        eprintln!("usage: coda <path> | coda inspect-key");
        return 2;
    }

    let mut warnings = Vec::new();
    if paths.len() > 1 {
        warnings.push(format!(
            "multiple files are not implemented yet; opened {} only",
            paths[0].display()
        ));
    }

    let loaded_config = config::load();
    warnings.extend(loaded_config.warnings);

    match EventLoop::open(paths[0].clone(), warnings, loaded_config.user_bindings) {
        Ok(loop_) => match loop_.run() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("editor failed: {error}");
                1
            }
        },
        Err(error) => {
            eprintln!("failed to open {}: {error}", paths[0].display());
            1
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    InvalidUsage(String),
    InspectKey,
    KeymapImportVscode(ImportOptions),
    OpenFiles(Vec<PathBuf>),
}

impl Command {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Self {
        let args = args.into_iter().collect::<Vec<_>>();
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

    #[test]
    fn parse_no_args_as_open_without_paths() {
        assert_eq!(Command::parse([]), Command::OpenFiles(vec![]));
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
