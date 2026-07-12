//! File loading and saving for the app layer.

use std::{
    fs,
    io::{self, Read},
    path::Path,
    time::SystemTime,
};

use crate::core::buffer::{LoadError as BufferLoadError, TextBuffer};

/// Files larger than this are opened read-only (TASK-260712 Gate 1: large
/// file protection) instead of refusing to open at all — "must always
/// start" (AGENTS.md). Editing/saving is blocked at the app layer
/// (`Document::readonly`, `EditorContext::is_readonly`), not here.
pub const LARGE_FILE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct LoadInfo {
    pub is_new: bool,
    pub mixed_line_endings: bool,
    /// True when the file exceeds `LARGE_FILE_BYTES`.
    pub readonly: bool,
    /// On-disk mtime observed at open time, later compared against
    /// `current_mtime` by `Document::save` to detect an external change.
    /// `None` for a file that does not exist yet.
    pub mtime: Option<SystemTime>,
}

#[derive(Debug)]
pub enum LoadError {
    Io(io::Error),
    InvalidUtf8,
}

pub fn open(path: &Path) -> Result<(TextBuffer, LoadInfo), LoadError> {
    match fs::File::open(path) {
        Ok(mut handle) => {
            // Single open handle for both the size/mtime stat and the read,
            // rather than a separate `fs::metadata` call before `fs::read`:
            // avoids a second syscall and a (harmless but pointless) TOCTOU
            // gap between the two.
            let metadata = handle.metadata().map_err(LoadError::Io)?;
            let mut bytes = Vec::new();
            handle.read_to_end(&mut bytes).map_err(LoadError::Io)?;
            let (buffer, info) = TextBuffer::from_bytes(&bytes).map_err(|error| match error {
                BufferLoadError::InvalidUtf8 => LoadError::InvalidUtf8,
            })?;
            Ok((
                buffer,
                LoadInfo {
                    is_new: false,
                    mixed_line_endings: info.mixed_line_endings,
                    readonly: metadata.len() > LARGE_FILE_BYTES,
                    mtime: metadata.modified().ok(),
                },
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((
            TextBuffer::default(),
            LoadInfo {
                is_new: true,
                mixed_line_endings: false,
                readonly: false,
                mtime: None,
            },
        )),
        Err(error) => Err(LoadError::Io(error)),
    }
}

/// Writes `buffer` to `path` and returns the mtime observed right after the
/// write, so the caller (`Document::save`) can remember it for the next
/// external-change check. A stat failure after a successful write falls
/// back to `SystemTime::now()` rather than turning the save itself into an
/// error — the write already succeeded.
pub fn save(path: &Path, buffer: &TextBuffer) -> io::Result<SystemTime> {
    fs::write(path, buffer.to_bytes())?;
    let mtime = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or_else(|_| SystemTime::now());
    Ok(mtime)
}

/// Current on-disk mtime, if the file exists. Used by `Document::save` to
/// detect an external change since the buffer was opened/last saved
/// (TASK-260712 Gate 1: a check at save time, not a continuous watch).
pub fn current_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "failed to open file: {error}"),
            Self::InvalidUtf8 => formatter.write_str("file is not valid UTF-8"),
        }
    }
}

impl std::error::Error for LoadError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "coda-test-file-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ))
    }

    #[test]
    fn open_reports_new_file_with_no_mtime_and_not_readonly() {
        let path = temp_path("missing");
        let _ = fs::remove_file(&path);

        let (_, info) = open(&path).unwrap();

        assert!(info.is_new);
        assert!(!info.readonly);
        assert_eq!(info.mtime, None);
    }

    /// TASK-260712 Gate 1 testcase: exactly `LARGE_FILE_BYTES` stays
    /// editable, one byte over flips `readonly`.
    #[test]
    fn open_marks_readonly_only_strictly_above_the_large_file_threshold() {
        let cases = [
            ("at-threshold", LARGE_FILE_BYTES, false),
            ("over-threshold", LARGE_FILE_BYTES + 1, true),
        ];
        for (name, size, expected_readonly) in cases {
            let path = temp_path(name);
            fs::write(&path, vec![b'a'; size as usize]).unwrap();

            let (_, info) = open(&path).unwrap();

            assert_eq!(info.readonly, expected_readonly, "{name}");
            assert!(info.mtime.is_some(), "{name}");

            let _ = fs::remove_file(&path);
        }
    }

    #[test]
    fn save_returns_a_mtime_that_matches_current_mtime() {
        let path = temp_path("save-mtime");
        let (buffer, _) = TextBuffer::from_bytes(b"a").unwrap();

        let mtime = save(&path, &buffer).unwrap();

        assert_eq!(current_mtime(&path), Some(mtime));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn current_mtime_is_none_for_a_missing_file() {
        let path = temp_path("current-mtime-missing");
        let _ = fs::remove_file(&path);

        assert_eq!(current_mtime(&path), None);
    }
}
