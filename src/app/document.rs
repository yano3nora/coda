//! Per-buffer document state owned by the app event loop.
//!
//! A [`Document`] is the boundary for state that must travel with a buffer:
//! text, cursor/undo state, viewport, saved snapshot, and highlight cache.

use std::{
    io,
    path::{Path, PathBuf},
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
}

impl Document {
    pub fn open(path: PathBuf) -> Result<(Self, file::LoadInfo), file::LoadError> {
        let (buffer, load_info) = file::open(&path)?;
        Ok((Self::from_buffer(Some(path), buffer), load_info))
    }

    pub fn unnamed() -> Self {
        Self::from_buffer(None, TextBuffer::default())
    }

    pub fn is_modified(&self) -> bool {
        self.editor.buffer.to_bytes() != self.saved_snapshot
    }

    pub fn save(&mut self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "save as: not implemented yet",
            ));
        };
        file::save(path, &self.editor.buffer)?;
        self.saved_snapshot = self.editor.buffer.to_bytes();
        Ok(())
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Document;

    #[test]
    fn unnamed_document_has_no_name_and_cannot_save_without_save_as() {
        let mut document = Document::unnamed();

        assert_eq!(document.display_name(), "[No Name]");
        assert!(!document.is_modified());

        document.editor.insert_text("draft");
        let error = document.save().unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), "save as: not implemented yet");
    }
}
