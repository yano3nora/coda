//! Application-level CLI routing and future event loop ownership.

use std::{env, ffi::OsString, path::PathBuf};

use crate::input;

/// Runs the CLI entrypoint and returns a process exit code.
///
/// This deliberately keeps argument handling minimal until SPEC-0005 subcommands
/// are implemented. `inspect-key` is the only active command in this scaffold.
pub fn run() -> i32 {
    match Command::parse(env::args_os().skip(1)) {
        Command::InspectKey => match input::inspect_key() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("inspect-key failed: {error}");
                1
            }
        },
        Command::OpenFiles(paths) => {
            if paths.is_empty() {
                println!("coda editor is not implemented yet. Try: coda inspect-key");
            } else {
                println!(
                    "opening files is not implemented yet: {}",
                    paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            0
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
    fn parse_no_args_as_not_implemented_open() {
        assert_eq!(Command::parse([]), Command::OpenFiles(vec![]));
    }

    #[test]
    fn parse_paths_as_not_implemented_open() {
        assert_eq!(
            Command::parse([OsString::from("a.txt"), OsString::from("b.txt")]),
            Command::OpenFiles(vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")])
        );
    }
}
