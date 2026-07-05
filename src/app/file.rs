//! File loading and saving for the app layer.

use std::{fs, io, path::Path};

use crate::core::buffer::{LoadError as BufferLoadError, TextBuffer};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct LoadInfo {
    pub is_new: bool,
    pub mixed_line_endings: bool,
}

#[derive(Debug)]
pub enum LoadError {
    Io(io::Error),
    InvalidUtf8,
}

pub fn open(path: &Path) -> Result<(TextBuffer, LoadInfo), LoadError> {
    match fs::read(path) {
        Ok(bytes) => {
            let (buffer, info) = TextBuffer::from_bytes(&bytes).map_err(|error| match error {
                BufferLoadError::InvalidUtf8 => LoadError::InvalidUtf8,
            })?;
            Ok((
                buffer,
                LoadInfo {
                    is_new: false,
                    mixed_line_endings: info.mixed_line_endings,
                },
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((
            TextBuffer::default(),
            LoadInfo {
                is_new: true,
                mixed_line_endings: false,
            },
        )),
        Err(error) => Err(LoadError::Io(error)),
    }
}

pub fn save(path: &Path, buffer: &TextBuffer) -> io::Result<()> {
    fs::write(path, buffer.to_bytes())
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
