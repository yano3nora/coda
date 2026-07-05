//! Terminal size and resize notification flag.

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

extern "C" fn mark_resize_pending(_signal_number: libc::c_int) {
    PENDING_RESIZE.store(true, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::{mark_resize_pending, take_pending_resize, terminal_size};

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
