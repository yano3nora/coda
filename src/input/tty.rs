//! Platform-neutral "is this an interactive terminal?" checks.
//!
//! Callers (capability probe, import CLI color decision) must not know how
//! TTY-ness is detected: unix answers via `isatty(3)`, windows via whether
//! the handle has a console mode at all (redirected handles do not).

#[cfg(unix)]
pub(crate) fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) != 0 }
}

#[cfg(unix)]
pub(crate) fn stdout_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDOUT_FILENO) != 0 }
}

#[cfg(windows)]
pub(crate) fn stdin_is_tty() -> bool {
    handle_is_console(windows_sys::Win32::System::Console::STD_INPUT_HANDLE)
}

#[cfg(windows)]
pub(crate) fn stdout_is_tty() -> bool {
    handle_is_console(windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE)
}

#[cfg(windows)]
fn handle_is_console(std_handle: windows_sys::Win32::System::Console::STD_HANDLE) -> bool {
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        System::Console::{GetConsoleMode, GetStdHandle},
    };

    let handle = unsafe { GetStdHandle(std_handle) };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return false;
    }
    let mut mode = 0;
    unsafe { GetConsoleMode(handle, &mut mode) != 0 }
}
