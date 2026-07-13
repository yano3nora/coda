//! Terminal size and resize notification flag.
//!
//! unix: TIOCGWINSZ + SIGWINCH. windows: GetConsoleScreenBufferInfo, with
//! resize detected by comparing against the last observed size (there is no
//! SIGWINCH equivalent; the event loop calls `take_pending_resize` every
//! tick, so polling granularity matches the unix signal path in practice).

#[cfg(unix)]
mod platform {
    use std::{
        mem::MaybeUninit,
        sync::atomic::{AtomicBool, Ordering},
    };

    use libc::{SIGWINCH, STDOUT_FILENO, TIOCGWINSZ, winsize};

    static INSTALL_RESIZE_HANDLER: std::sync::Once = std::sync::Once::new();
    static PENDING_RESIZE: AtomicBool = AtomicBool::new(false);

    /// Returns terminal size as `(columns, rows)`.
    pub fn terminal_size() -> Option<(u16, u16)> {
        install_resize_handler_once();

        let mut size = MaybeUninit::<winsize>::uninit();
        let result = unsafe { libc::ioctl(STDOUT_FILENO, TIOCGWINSZ, size.as_mut_ptr()) };
        if result == -1 {
            return None;
        }

        let size = unsafe { size.assume_init() };
        if size.ws_col == 0 || size.ws_row == 0 {
            return None;
        }

        Some((size.ws_col, size.ws_row))
    }

    /// Returns and clears whether SIGWINCH has been observed.
    pub fn take_pending_resize() -> bool {
        PENDING_RESIZE.swap(false, Ordering::SeqCst)
    }

    fn install_resize_handler_once() {
        INSTALL_RESIZE_HANDLER.call_once(|| unsafe {
            let handler = mark_resize_pending as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::signal(SIGWINCH, handler);
        });
    }

    pub(super) extern "C" fn mark_resize_pending(_signal_number: libc::c_int) {
        PENDING_RESIZE.store(true, Ordering::SeqCst);
    }
}

#[cfg(windows)]
mod platform {
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE,
    };

    /// Last size observed by `take_pending_resize`, packed as
    /// `columns << 16 | rows`. Zero means "not observed yet" — a real
    /// terminal never reports zero columns or rows, so no sentinel clash.
    static LAST_SIZE: AtomicU32 = AtomicU32::new(0);

    /// Returns terminal size as `(columns, rows)` (visible window extent,
    /// not the scrollback buffer size).
    pub fn terminal_size() -> Option<(u16, u16)> {
        let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        if handle.is_null() {
            return None;
        }
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        if unsafe { GetConsoleScreenBufferInfo(handle, &mut info) } == 0 {
            return None;
        }

        let columns = i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1;
        let rows = i32::from(info.srWindow.Bottom) - i32::from(info.srWindow.Top) + 1;
        if columns <= 0 || rows <= 0 {
            return None;
        }
        Some((columns as u16, rows as u16))
    }

    /// Returns whether the size changed since the previous call. The first
    /// call only records the baseline (the event loop already queried the
    /// initial size itself) and reports no resize.
    pub fn take_pending_resize() -> bool {
        let Some((columns, rows)) = terminal_size() else {
            return false;
        };
        let packed = (u32::from(columns) << 16) | u32::from(rows);
        let previous = LAST_SIZE.swap(packed, Ordering::SeqCst);
        previous != 0 && previous != packed
    }
}

pub use platform::{take_pending_resize, terminal_size};

#[cfg(all(test, unix))]
mod tests {
    use super::platform::mark_resize_pending;
    use super::{take_pending_resize, terminal_size};

    #[test]
    fn terminal_size_does_not_panic_on_non_tty() {
        let _ = terminal_size();
    }

    #[test]
    fn take_pending_resize_returns_and_clears_signal_flag() {
        assert!(!take_pending_resize());
        mark_resize_pending(libc::SIGWINCH);

        assert!(take_pending_resize());
        assert!(!take_pending_resize());
    }
}
