//! Application-level CLI routing and event-loop ownership.

mod config;
mod default_bindings;
mod editor_view;
mod event_loop;
mod file;
mod palette;

use std::{env, ffi::OsString, path::PathBuf};

use crate::input;
use event_loop::EventLoop;

/// Runs the CLI entrypoint and returns a process exit code.
pub fn run() -> i32 {
    match Command::parse(env::args_os().skip(1)) {
        Command::InspectKey => match input::inspect_key() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("inspect-key failed: {error}");
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
    InspectKey,
    OpenFiles(Vec<PathBuf>),
}

impl Command {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Self {
        let args = args.into_iter().collect::<Vec<_>>();
        if args.len() == 1 && args[0] == "inspect-key" {
            return Self::InspectKey;
        }

        Self::OpenFiles(args.into_iter().map(PathBuf::from).collect())
    }
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
}
