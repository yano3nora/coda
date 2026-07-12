//! Per-buffer document state owned by the app event loop.
//!
//! A [`Document`] is the boundary for state that must travel with a buffer:
//! text, cursor/undo state, viewport, saved snapshot, and highlight cache.

use std::{
    fmt, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    core::{buffer::TextBuffer, editor::EditorCore},
    highlight::HighlightCache,
};

use super::{editor_view::EditorView, file};

pub struct Document {
    pub path: Option<PathBuf>,
    pub editor: EditorCore,
    pub view: EditorView,
    pub saved_snapshot: Vec<u8>,
    pub highlight_cache: HighlightCache,
    /// On-disk mtime as of the last open/save, used by `save` to detect an
    /// external change (TASK-260712 Gate 1). `None` for a buffer never
    /// backed by an existing file (unnamed, or `buffer.new`).
    pub saved_mtime: Option<SystemTime>,
    /// True for files opened over `file::LARGE_FILE_BYTES` (TASK-260712 Gate
    /// 1: large file protection). This is the single source of truth
    /// `event_loop` consults both for the keymap context
    /// (`EditorContext::is_readonly`) and its own dispatch-level guard.
    pub readonly: bool,
}

/// Failure modes for [`Document::save`]. Distinct from [`file::LoadError`]:
/// save has recoverable cases (no path yet, external change) that the event
/// loop handles very differently from a hard I/O error.
#[derive(Debug)]
pub enum SaveError {
    /// The buffer has never been saved to a path (unnamed / `buffer.new`).
    /// The event loop responds by opening the Save As prompt rather than
    /// just reporting this as a terminal error.
    NoPath,
    /// The document is read-only (large file protection); saving is always
    /// blocked, `force` included. `event_loop::dispatch` already blocks
    /// `file.save`/`file.saveAs` on a readonly document before reaching
    /// here — this is defense in depth, not the primary enforcement point.
    Readonly,
    /// The file on disk changed since this document last read or wrote it.
    /// Never raised when `force` is set (the caller's explicit "overwrite
    /// anyway" confirmation).
    Conflict,
    Io(io::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPath => formatter.write_str("save as: no path set"),
            Self::Readonly => formatter.write_str("read-only buffer (large file)"),
            Self::Conflict => formatter.write_str("file changed on disk; save again to overwrite"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl Document {
    pub fn open(path: PathBuf) -> Result<(Self, file::LoadInfo), file::LoadError> {
        let (buffer, load_info) = file::open(&path)?;
        let mut document = Self::from_buffer(Some(path), buffer);
        document.saved_mtime = load_info.mtime;
        document.readonly = load_info.readonly;
        Ok((document, load_info))
    }

    pub fn unnamed() -> Self {
        Self::from_buffer(None, TextBuffer::default())
    }

    pub fn is_modified(&self) -> bool {
        self.editor.buffer.to_bytes() != self.saved_snapshot
    }

    /// Saves to `self.path`. `force` bypasses the external-change check
    /// (the event loop's second-attempt confirmation, mirroring
    /// `QuitGuard`'s two-stage pattern) but never bypasses `readonly`.
    pub fn save(&mut self, force: bool) -> Result<(), SaveError> {
        if self.readonly {
            return Err(SaveError::Readonly);
        }
        let Some(path) = self.path.clone() else {
            return Err(SaveError::NoPath);
        };
        if !force && self.has_external_change(&path) {
            return Err(SaveError::Conflict);
        }
        let mtime = file::save(&path, &self.editor.buffer).map_err(SaveError::Io)?;
        self.saved_snapshot = self.editor.buffer.to_bytes();
        self.saved_mtime = Some(mtime);
        Ok(())
    }

    /// True when the file on disk has a different mtime than the one this
    /// document last observed (open, or last save). A target with nothing
    /// on disk yet (`current_mtime` returns `None`) is never a conflict.
    fn has_external_change(&self, path: &Path) -> bool {
        match (self.saved_mtime, file::current_mtime(path)) {
            (Some(expected), Some(current)) => expected != current,
            _ => false,
        }
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("[No Name]")
            .to_string()
    }

    fn from_buffer(path: Option<PathBuf>, buffer: TextBuffer) -> Self {
        let saved_snapshot = buffer.to_bytes();
        Self {
            path,
            editor: EditorCore::new(buffer),
            view: EditorView::default(),
            saved_snapshot,
            highlight_cache: HighlightCache::default(),
            saved_mtime: None,
            readonly: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Document, SaveError};
    use std::{fs, time::SystemTime};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "coda-test-document-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ))
    }

    #[test]
    fn unnamed_document_has_no_name_and_reports_no_path_on_save() {
        let mut document = Document::unnamed();

        assert_eq!(document.display_name(), "[No Name]");
        assert!(!document.is_modified());

        document.editor.insert_text("draft");
        let error = document.save(false).unwrap_err();

        assert!(matches!(error, SaveError::NoPath));
        assert_eq!(error.to_string(), "save as: no path set");
    }

    #[test]
    fn save_succeeds_and_records_a_new_mtime() {
        let path = temp_path("save-ok");
        fs::write(&path, b"base").unwrap();
        let (mut document, _) = Document::open(path.clone()).unwrap();

        document.editor.insert_text("!");
        document.save(false).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"!base");
        assert!(document.saved_mtime.is_some());
        assert!(!document.is_modified());

        let _ = fs::remove_file(&path);
    }

    /// TASK-260712 Gate 1 testcase: an external change since open (or the
    /// last save) blocks a plain save with `Conflict`, and only proceeds
    /// once the caller passes `force: true` (the event loop's two-stage
    /// confirmation). `saved_mtime` is pinned to a deliberately-stale fixed
    /// timestamp (`UNIX_EPOCH`) instead of racing a real external writer
    /// against filesystem mtime resolution — this keeps the test
    /// deterministic while still exercising the real comparison logic.
    #[test]
    fn save_reports_conflict_on_external_change_and_force_overwrites() {
        let path = temp_path("conflict");
        fs::write(&path, b"base").unwrap();
        let (mut document, _) = Document::open(path.clone()).unwrap();
        document.saved_mtime = Some(SystemTime::UNIX_EPOCH);

        document.editor.insert_text("!");
        let error = document.save(false).unwrap_err();
        assert!(matches!(error, SaveError::Conflict));
        assert_eq!(
            fs::read(&path).unwrap(),
            b"base",
            "a conflicting save must not write"
        );

        document.save(true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"!base");
        assert_ne!(document.saved_mtime, Some(SystemTime::UNIX_EPOCH));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn readonly_document_blocks_save_even_when_forced() {
        let path = temp_path("readonly-save");
        fs::write(&path, b"base").unwrap();
        let (mut document, _) = Document::open(path.clone()).unwrap();
        document.readonly = true;

        let error = document.save(true).unwrap_err();

        assert!(matches!(error, SaveError::Readonly));
        assert_eq!(fs::read(&path).unwrap(), b"base");

        let _ = fs::remove_file(&path);
    }
}
