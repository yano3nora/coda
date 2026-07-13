//! RAII terminal raw mode guard (platform dispatch).
//!
//! unix (`unix.rs`): termios raw mode + signal-based restore.
//! windows (`windows.rs`): Console mode (VT input/output) + ctrl-handler restore.
//!
//! This module owns what both platforms share: the escape sequences an
//! abnormal exit must write back to the terminal, and the flags tracking
//! which of them are currently active. Raw terminal state must not leak to
//! other layers (ADR-0004).

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::RawModeGuard;
#[cfg(unix)]
pub(crate) use unix::poll_stdin_readable;
#[cfg(windows)]
pub use windows::RawModeGuard;
#[cfg(windows)]
pub(crate) use windows::poll_stdin_readable;

static KEYBOARD_PROTOCOL_PUSHED: AtomicBool = AtomicBool::new(false);
static WIN32_INPUT_ACTIVE: AtomicBool = AtomicBool::new(false);
static ALT_SCREEN_ACTIVE: AtomicBool = AtomicBool::new(false);
static BRACKETED_PASTE_ACTIVE: AtomicBool = AtomicBool::new(false);
static MOUSE_REPORTING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// kitty keyboard protocol: pop the flags we pushed (`CSI < u`).
const KITTY_POP: &[u8] = b"\x1b[<u";
/// win32-input-mode (`CSI ?9001`): disable so the shell stops receiving
/// win32 key event sequences after an abnormal exit (TASK-260713 windows).
const WIN32_INPUT_DISABLE: &[u8] = b"\x1b[?9001l";
/// Alternate screen: show cursor and return to the main screen.
const ALT_SCREEN_LEAVE: &[u8] = b"\x1b[?25h\x1b[?1049l";
/// Bracketed paste mode: disable paste envelopes before returning to the shell.
const BRACKETED_PASTE_DISABLE: &[u8] = b"\x1b[?2004l";
/// SGR mouse reporting: stop the terminal from sending mouse sequences into
/// the user's shell after an abnormal exit (ADR-0008).
const MOUSE_REPORTING_DISABLE: &[u8] = b"\x1b[?1006l\x1b[?1002l";

/// Ordered cleanup writes for abnormal exits (signal handler / ctrl handler).
///
/// Order matters: the keyboard protocol must be popped BEFORE leaving the
/// alternate screen — kitty tracks the keyboard mode stack per screen, and
/// the editor pushes its mode on the alternate screen (see EventLoop::run).
/// Each platform handler iterates this table with its own write primitive
/// (unix must stay async-signal-safe, so no allocation here).
const CLEANUP_STEPS: [(&AtomicBool, &[u8]); 5] = [
    (&MOUSE_REPORTING_ACTIVE, MOUSE_REPORTING_DISABLE),
    (&BRACKETED_PASTE_ACTIVE, BRACKETED_PASTE_DISABLE),
    (&WIN32_INPUT_ACTIVE, WIN32_INPUT_DISABLE),
    (&KEYBOARD_PROTOCOL_PUSHED, KITTY_POP),
    (&ALT_SCREEN_ACTIVE, ALT_SCREEN_LEAVE),
];

/// Tracks whether a kitty protocol mode was pushed, so the abnormal-exit
/// handler can pop it before exiting. Leaving the mode pushed would corrupt
/// the shell's key handling after an abnormal exit.
pub(crate) fn set_keyboard_protocol_pushed(pushed: bool) {
    KEYBOARD_PROTOCOL_PUSHED.store(pushed, Ordering::SeqCst);
}

/// Tracks whether win32-input-mode was enabled (only windows builds request
/// it at startup), so abnormal exits disable it.
#[cfg(windows)]
pub(crate) fn set_win32_input_active(active: bool) {
    WIN32_INPUT_ACTIVE.store(active, Ordering::SeqCst);
}

/// Tracks whether alternate screen mode is active, so the abnormal-exit
/// handler can restore the user's shell view before keyboard protocol and
/// terminal attribute cleanup.
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
