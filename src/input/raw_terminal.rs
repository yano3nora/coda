//! RAII terminal raw mode guard.
//!
//! Uses the `libc` crate for termios so platform-specific struct layouts are
//! not maintained by hand. This module is intentionally scoped to `input`;
//! raw terminal state must not leak to other layers.

use std::{
    io,
    mem::MaybeUninit,
    os::fd::RawFd,
    ptr,
    sync::{
        Once,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
};

use libc::{
    SIGHUP, SIGINT, SIGQUIT, SIGTERM, STDIN_FILENO, TCSAFLUSH, VMIN, VTIME, termios as Termios,
};

static INSTALL_SIGNAL_HANDLERS: Once = Once::new();
static SIGNAL_RESTORE_ACTIVE: AtomicBool = AtomicBool::new(false);
static SIGNAL_RESTORE_FD: AtomicI32 = AtomicI32::new(-1);
static mut SIGNAL_RESTORE_TERMIOS: MaybeUninit<Termios> = MaybeUninit::uninit();
static KEYBOARD_PROTOCOL_PUSHED: AtomicBool = AtomicBool::new(false);
static ALT_SCREEN_ACTIVE: AtomicBool = AtomicBool::new(false);
static BRACKETED_PASTE_ACTIVE: AtomicBool = AtomicBool::new(false);
static MOUSE_REPORTING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// kitty keyboard protocol: pop the flags we pushed (`CSI < u`).
const KITTY_POP: &[u8] = b"\x1b[<u";
/// Alternate screen: show cursor and return to the main screen.
const ALT_SCREEN_LEAVE: &[u8] = b"\x1b[?25h\x1b[?1049l";
/// Bracketed paste mode: disable paste envelopes before returning to the shell.
const BRACKETED_PASTE_DISABLE: &[u8] = b"\x1b[?2004l";
/// SGR mouse reporting: stop the terminal from sending mouse sequences into
/// the user's shell after an abnormal exit (ADR-0008).
const MOUSE_REPORTING_DISABLE: &[u8] = b"\x1b[?1006l\x1b[?1002l";

/// Tracks whether a kitty protocol mode was pushed, so the signal handler can
/// pop it before exiting. Leaving the mode pushed would corrupt the shell's
/// key handling after an abnormal exit.
pub(crate) fn set_keyboard_protocol_pushed(pushed: bool) {
    KEYBOARD_PROTOCOL_PUSHED.store(pushed, Ordering::SeqCst);
}

/// Tracks whether alternate screen mode is active, so the signal handler can
/// restore the user's shell view before keyboard protocol and termios cleanup.
pub(crate) fn set_alt_screen_active(active: bool) {
    ALT_SCREEN_ACTIVE.store(active, Ordering::SeqCst);
}

/// Tracks whether bracketed paste mode is active, so abnormal exits do not
/// leave the user's shell wrapping future paste operations in CSI 200/201.
pub(crate) fn set_bracketed_paste_active(active: bool) {
    BRACKETED_PASTE_ACTIVE.store(active, Ordering::SeqCst);
}

/// Tracks whether SGR mouse reporting is active, so abnormal exits do not
/// leave the user's shell receiving mouse escape sequences.
pub(crate) fn set_mouse_reporting_active(active: bool) {
    MOUSE_REPORTING_ACTIVE.store(active, Ordering::SeqCst);
}

/// Waits until `fd` has input available, or until `timeout_ms` elapses.
///
/// Callers own decoding and timeout policy; this wrapper only keeps the
/// terminal-specific readiness API inside the input layer.
pub(crate) fn poll_readable(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    let mut fds = [libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }];
    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result > 0 && fds[0].revents & libc::POLLIN != 0)
    }
}

/// Restores the original terminal attributes when dropped.
#[derive(Debug)]
pub struct RawModeGuard {
    fd: RawFd,
    original: Termios,
    restored: bool,
}

impl RawModeGuard {
    /// Enables raw mode for stdin.
    pub fn enable_stdin() -> io::Result<Self> {
        Self::enable(STDIN_FILENO)
    }

    fn enable(fd: RawFd) -> io::Result<Self> {
        let original = tcgetattr_checked(fd)?;
        install_signal_handlers_once();
        arm_signal_restore(fd, original);

        let mut raw = original;

        // `cfmakeraw` applies the platform's canonical raw-mode flag changes.
        // We then set VMIN/VTIME to make `read` return after at least one byte
        // while allowing short escape sequences to arrive in the same read on
        // typical terminals.
        unsafe { libc::cfmakeraw(&mut raw) };
        set_read_behavior(&mut raw, 1, 0);
        tcsetattr_checked(fd, TCSAFLUSH, &raw)?;

        Ok(Self {
            fd,
            original,
            restored: false,
        })
    }

    /// Restores the saved terminal attributes before `Drop`.
    ///
    /// This is mostly useful for explicit error handling. `Drop` still attempts
    /// restoration on unwind paths where returning an error is impossible.
    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }

        tcsetattr_checked(self.fd, TCSAFLUSH, &self.original)?;
        disarm_signal_restore();
        self.restored = true;
        Ok(())
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn install_signal_handlers_once() {
    INSTALL_SIGNAL_HANDLERS.call_once(|| unsafe {
        // These process-ending signals do not run Rust destructors by default.
        // The handler restores terminal attributes before exiting. SIGKILL cannot
        // be handled by any user process, so it is intentionally absent.
        let handler = restore_then_exit as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(SIGHUP, handler);
        libc::signal(SIGINT, handler);
        libc::signal(SIGQUIT, handler);
        libc::signal(SIGTERM, handler);
    });
}

fn arm_signal_restore(fd: RawFd, original: Termios) {
    unsafe {
        ptr::addr_of_mut!(SIGNAL_RESTORE_TERMIOS).write(MaybeUninit::new(original));
    }
    SIGNAL_RESTORE_FD.store(fd, Ordering::SeqCst);
    SIGNAL_RESTORE_ACTIVE.store(true, Ordering::SeqCst);
}

fn disarm_signal_restore() {
    SIGNAL_RESTORE_ACTIVE.store(false, Ordering::SeqCst);
    SIGNAL_RESTORE_FD.store(-1, Ordering::SeqCst);
}

extern "C" fn restore_then_exit(signal_number: libc::c_int) {
    // Only async-signal-safe calls are allowed here (write / tcsetattr / _exit are).
    // Pop the keyboard protocol BEFORE leaving the alternate screen: kitty
    // tracks the keyboard mode stack per screen, and the editor pushes its
    // mode on the alternate screen (see EventLoop::run).
    if MOUSE_REPORTING_ACTIVE.load(Ordering::SeqCst) {
        unsafe {
            libc::write(
                1,
                MOUSE_REPORTING_DISABLE.as_ptr().cast(),
                MOUSE_REPORTING_DISABLE.len(),
            );
        }
    }
    if BRACKETED_PASTE_ACTIVE.load(Ordering::SeqCst) {
        unsafe {
            libc::write(
                1,
                BRACKETED_PASTE_DISABLE.as_ptr().cast(),
                BRACKETED_PASTE_DISABLE.len(),
            );
        }
    }
    if KEYBOARD_PROTOCOL_PUSHED.load(Ordering::SeqCst) {
        unsafe {
            libc::write(1, KITTY_POP.as_ptr().cast(), KITTY_POP.len());
        }
    }
    if ALT_SCREEN_ACTIVE.load(Ordering::SeqCst) {
        unsafe {
            libc::write(1, ALT_SCREEN_LEAVE.as_ptr().cast(), ALT_SCREEN_LEAVE.len());
        }
    }
    if SIGNAL_RESTORE_ACTIVE.load(Ordering::SeqCst) {
        let fd = SIGNAL_RESTORE_FD.load(Ordering::SeqCst);
        if fd >= 0 {
            let original = unsafe { (*ptr::addr_of!(SIGNAL_RESTORE_TERMIOS)).as_ptr() };
            unsafe {
                libc::tcsetattr(fd, TCSAFLUSH, original);
            }
        }
    }

    unsafe { libc::_exit(128 + signal_number) };
}

fn tcgetattr_checked(fd: RawFd) -> io::Result<Termios> {
    let mut termios = MaybeUninit::<Termios>::uninit();
    let result = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }

    Ok(unsafe { termios.assume_init() })
}

fn tcsetattr_checked(fd: RawFd, optional_actions: i32, termios: &Termios) -> io::Result<()> {
    let result = unsafe { libc::tcsetattr(fd, optional_actions, termios) };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

fn set_read_behavior(termios: &mut Termios, min_bytes: u8, timeout_deciseconds: u8) {
    termios.c_cc[VMIN] = min_bytes;
    termios.c_cc[VTIME] = timeout_deciseconds;
}

#[cfg(test)]
mod tests {
    use super::{Termios, set_read_behavior};
    use libc::{VMIN, VTIME};

    #[test]
    fn set_read_behavior_updates_vmin_and_vtime_only() {
        let mut termios = zeroed_termios();
        set_read_behavior(&mut termios, 7, 9);

        assert_eq!(termios.c_cc[VMIN], 7);
        assert_eq!(termios.c_cc[VTIME], 9);
        assert_eq!(termios.c_cc.iter().filter(|byte| **byte != 0).count(), 2);
    }

    fn zeroed_termios() -> Termios {
        // libc::termios has platform-specific fields; zeroed init avoids
        // enumerating them per platform in this test.
        unsafe { std::mem::zeroed() }
    }
}
