//! Alternate screen RAII guard.
//!
//! The guard mirrors raw mode / keyboard protocol ownership: enter on creation,
//! leave on explicit `leave` or `Drop`, and keep a signal-handler flag so an
//! abnormal process-ending signal restores the terminal before exit.

use std::io::{self, Write};

use crate::input::raw_terminal;

const ALT_SCREEN_ENTER: &[u8] = b"\x1b[?1049h";
const ALT_SCREEN_LEAVE: &[u8] = b"\x1b[?1049l";
const CURSOR_HIDE: &[u8] = b"\x1b[?25l";
const CURSOR_SHOW: &[u8] = b"\x1b[?25h";

pub struct AltScreenGuard<W: Write> {
    writer: W,
    active: bool,
}

impl<W: Write> AltScreenGuard<W> {
    pub fn enter(mut writer: W) -> io::Result<Self> {
        writer.write_all(ALT_SCREEN_ENTER)?;
        writer.write_all(CURSOR_HIDE)?;
        writer.flush()?;
        raw_terminal::set_alt_screen_active(true);
        Ok(Self {
            writer,
            active: true,
        })
    }

    pub fn leave(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }

        self.writer.write_all(CURSOR_SHOW)?;
        self.writer.write_all(ALT_SCREEN_LEAVE)?;
        self.writer.flush()?;
        raw_terminal::set_alt_screen_active(false);
        self.active = false;
        Ok(())
    }

    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    pub fn into_inner(mut self) -> W {
        let _ = self.leave();
        self.active = false;
        // Moving a field out of a Drop type is not allowed directly. Replace it
        // with an empty sink-like Vec for test ergonomics is not generic, so use
        // ManuallyDrop to keep the generic writer movable after leave.
        let this = std::mem::ManuallyDrop::new(self);
        unsafe { std::ptr::read(&this.writer) }
    }
}

impl<W: Write> Drop for AltScreenGuard<W> {
    fn drop(&mut self) {
        let _ = self.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::AltScreenGuard;
    use std::{cell::RefCell, io, rc::Rc};

    #[test]
    fn alt_screen_guard_enters_and_explicitly_leaves_with_expected_sequences() {
        let mut guard = AltScreenGuard::enter(Vec::new()).unwrap();
        assert_eq!(guard.writer_mut(), b"\x1b[?1049h\x1b[?25l");

        guard.leave().unwrap();
        assert_eq!(
            guard.writer_mut(),
            b"\x1b[?1049h\x1b[?25l\x1b[?25h\x1b[?1049l"
        );
    }

    #[test]
    fn alt_screen_guard_leaves_on_drop() {
        let shared = SharedWriter::default();
        let bytes = Rc::clone(&shared.bytes);

        {
            let _guard = AltScreenGuard::enter(shared).unwrap();
        }

        assert_eq!(
            bytes.borrow().as_slice(),
            b"\x1b[?1049h\x1b[?25l\x1b[?25h\x1b[?1049l"
        );
    }

    #[derive(Default)]
    struct SharedWriter {
        bytes: Rc<RefCell<Vec<u8>>>,
    }

    impl io::Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
