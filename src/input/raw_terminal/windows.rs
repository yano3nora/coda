//! Windows raw mode: Console API via `windows-sys`.
//!
//! Input relies on `ENABLE_VIRTUAL_TERMINAL_INPUT`: ConPTY translates key
//! input into VT byte sequences, so the shared byte decoder keeps working
//! unchanged (win32-input-mode fidelity is negotiated separately with
//! `CSI ?9001h`, see `KeyboardProtocolGuard`). Terminals whose console host
//! cannot enable VT modes (legacy conhost) get an explicit startup error
//! instead of silently broken input (TASK-260713 / ADR-0003).

use std::{
    io,
    sync::{
        Once,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Console::{
        CONSOLE_MODE, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS,
        ENABLE_LINE_INPUT, ENABLE_MOUSE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT,
        ENABLE_QUICK_EDIT_MODE, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        ENABLE_WINDOW_INPUT, GetConsoleMode, GetStdHandle, INPUT_RECORD, KEY_EVENT,
        PeekConsoleInputW, ReadConsoleInputW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        SetConsoleCtrlHandler, SetConsoleMode, WriteConsoleA,
    },
    System::Threading::{ExitProcess, INFINITE, WaitForSingleObject},
};

use super::CLEANUP_STEPS;

static INSTALL_CTRL_HANDLER: Once = Once::new();
static CTRL_RESTORE_ACTIVE: AtomicBool = AtomicBool::new(false);
static CTRL_RESTORE_STDIN_MODE: AtomicU32 = AtomicU32::new(0);
static CTRL_RESTORE_STDOUT_MODE: AtomicU32 = AtomicU32::new(0);
static CTRL_RESTORE_STDOUT_VALID: AtomicBool = AtomicBool::new(false);

fn stdin_handle() -> io::Result<HANDLE> {
    checked_handle(unsafe { GetStdHandle(STD_INPUT_HANDLE) })
}

fn stdout_handle() -> io::Result<HANDLE> {
    checked_handle(unsafe { GetStdHandle(STD_OUTPUT_HANDLE) })
}

fn checked_handle(handle: HANDLE) -> io::Result<HANDLE> {
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(handle)
    }
}

/// Waits until stdin has key input available, or until `timeout_ms` elapses
/// (negative means wait forever, mirroring `poll(2)` semantics).
///
/// A console handle is signaled by ANY queued input record, including
/// non-key records such as window resizes. Those never produce bytes, so
/// treating them as "readable" would make the following `read` block until
/// the next real keystroke and freeze the event loop. Peek first, discard
/// non-key records, and only report readable for KEY_EVENT records (which,
/// under ConPTY, are synthesized from the VT byte stream and therefore
/// always carry bytes).
pub(crate) fn poll_stdin_readable(timeout_ms: i32) -> io::Result<bool> {
    let handle = stdin_handle()?;
    let deadline = if timeout_ms < 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout_ms as u64))
    };

    loop {
        // Elapsed deadlines wait with 0ms instead of returning early:
        // `poll(2)` with timeout 0 still performs one readiness check, and
        // the discard loop below must be able to drain already-queued
        // non-key records even at the deadline boundary.
        let wait_ms = match deadline {
            None => INFINITE,
            Some(deadline) => deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u128::from(INFINITE - 1)) as u32,
        };

        match unsafe { WaitForSingleObject(handle, wait_ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Ok(false),
            WAIT_FAILED => return Err(io::Error::last_os_error()),
            _ => return Err(io::Error::other("unexpected WaitForSingleObject result")),
        }

        let mut record: INPUT_RECORD = unsafe { std::mem::zeroed() };
        let mut count: u32 = 0;
        if unsafe { PeekConsoleInputW(handle, &mut record, 1, &mut count) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            // Signaled but empty (another reader raced us): wait again.
            continue;
        }
        if u32::from(record.EventType) == KEY_EVENT {
            return Ok(true);
        }
        // Discard the non-key record (resize / focus / menu) so it cannot
        // wedge the next blocking read.
        if unsafe { ReadConsoleInputW(handle, &mut record, 1, &mut count) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
}

/// Restores the original console modes when dropped.
#[derive(Debug)]
pub struct RawModeGuard {
    stdin_original: CONSOLE_MODE,
    stdout_original: Option<CONSOLE_MODE>,
    restored: bool,
}

impl RawModeGuard {
    /// Enables raw (VT) mode for the console: stdin stops line-buffering and
    /// echoing and starts delivering VT input sequences; stdout starts
    /// interpreting VT output sequences.
    pub fn enable_stdin() -> io::Result<Self> {
        let stdin = stdin_handle()?;
        let mut stdin_original: CONSOLE_MODE = 0;
        if unsafe { GetConsoleMode(stdin, &mut stdin_original) } == 0 {
            return Err(io::Error::last_os_error());
        }

        // PROCESSED_INPUT off means Ctrl+C arrives as byte 0x03 instead of
        // raising a console ctrl event — same as unix raw mode. WINDOW/MOUSE
        // records are disabled because resize is detected by size polling
        // (ui/terminal_size) and mouse arrives as SGR VT sequences.
        let raw_in = (stdin_original
            & !(ENABLE_LINE_INPUT
                | ENABLE_ECHO_INPUT
                | ENABLE_PROCESSED_INPUT
                | ENABLE_QUICK_EDIT_MODE
                | ENABLE_WINDOW_INPUT
                | ENABLE_MOUSE_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT
            | ENABLE_EXTENDED_FLAGS;
        if unsafe { SetConsoleMode(stdin, raw_in) } == 0 {
            // The one supported failure mode: a console host too old for VT
            // input. Refuse loudly instead of running with broken keys
            // (ADR-0003 "silently broken is forbidden").
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this console does not support VT input (ENABLE_VIRTUAL_TERMINAL_INPUT); \
                 coda requires Windows Terminal or another ConPTY-based terminal",
            ));
        }

        // A redirected stdout has no console mode; skip it (the render path
        // will fail on its own if someone pipes the editor, same as unix).
        // But a real console that REFUSES VT output must be a hard error:
        // silently proceeding would paint raw escape bytes over the screen
        // (Codex review 260713 / ADR-0003).
        let mut stdout_original: Option<CONSOLE_MODE> = None;
        if let Ok(stdout) = stdout_handle() {
            let mut mode: CONSOLE_MODE = 0;
            if unsafe { GetConsoleMode(stdout, &mut mode) } != 0 {
                let vt_out = mode
                    | ENABLE_PROCESSED_OUTPUT
                    | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                    | DISABLE_NEWLINE_AUTO_RETURN;
                if unsafe { SetConsoleMode(stdout, vt_out) } == 0 {
                    unsafe { SetConsoleMode(stdin, stdin_original) };
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "this console does not support VT output \
                         (ENABLE_VIRTUAL_TERMINAL_PROCESSING); coda requires Windows \
                         Terminal or another ConPTY-based terminal",
                    ));
                }
                stdout_original = Some(mode);
            }
        }

        install_ctrl_handler_once();
        arm_ctrl_restore(stdin_original, stdout_original);

        Ok(Self {
            stdin_original,
            stdout_original,
            restored: false,
        })
    }

    /// Restores the saved console modes before `Drop`.
    ///
    /// This is mostly useful for explicit error handling. `Drop` still attempts
    /// restoration on unwind paths where returning an error is impossible.
    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }

        let stdin = stdin_handle()?;
        if unsafe { SetConsoleMode(stdin, self.stdin_original) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if let Some(mode) = self.stdout_original {
            let stdout = stdout_handle()?;
            // A failed stdout restore must surface (VT flags would leak into
            // the shell); `restored` stays false so Drop retries once more.
            if unsafe { SetConsoleMode(stdout, mode) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        disarm_ctrl_restore();
        self.restored = true;
        Ok(())
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn install_ctrl_handler_once() {
    INSTALL_CTRL_HANDLER.call_once(|| unsafe {
        // Covers window close / logoff / Ctrl+Break. Plain Ctrl+C cannot fire
        // here while raw mode is active (PROCESSED_INPUT is off, so it arrives
        // as input byte 0x03 instead), matching the unix SIGINT-handler role
        // for the paths that still deliver a ctrl event.
        SetConsoleCtrlHandler(Some(restore_then_exit), 1);
    });
}

fn arm_ctrl_restore(stdin_mode: CONSOLE_MODE, stdout_mode: Option<CONSOLE_MODE>) {
    CTRL_RESTORE_STDIN_MODE.store(stdin_mode, Ordering::SeqCst);
    CTRL_RESTORE_STDOUT_VALID.store(stdout_mode.is_some(), Ordering::SeqCst);
    CTRL_RESTORE_STDOUT_MODE.store(stdout_mode.unwrap_or(0), Ordering::SeqCst);
    CTRL_RESTORE_ACTIVE.store(true, Ordering::SeqCst);
}

fn disarm_ctrl_restore() {
    CTRL_RESTORE_ACTIVE.store(false, Ordering::SeqCst);
}

unsafe extern "system" fn restore_then_exit(_ctrl_type: u32) -> i32 {
    // The ctrl handler runs on its own thread while the process is being torn
    // down; write the same cleanup sequences as the unix signal handler, then
    // restore console modes and exit without running destructors.
    if let Ok(stdout) = stdout_handle() {
        for (active, sequence) in CLEANUP_STEPS {
            if active.load(Ordering::SeqCst) {
                let mut written: u32 = 0;
                unsafe {
                    WriteConsoleA(
                        stdout,
                        sequence.as_ptr(),
                        sequence.len() as u32,
                        &mut written,
                        std::ptr::null(),
                    );
                }
            }
        }
    }
    if CTRL_RESTORE_ACTIVE.load(Ordering::SeqCst) {
        if let Ok(stdin) = stdin_handle() {
            unsafe {
                SetConsoleMode(stdin, CTRL_RESTORE_STDIN_MODE.load(Ordering::SeqCst));
            }
        }
        if CTRL_RESTORE_STDOUT_VALID.load(Ordering::SeqCst)
            && let Ok(stdout) = stdout_handle()
        {
            unsafe {
                SetConsoleMode(stdout, CTRL_RESTORE_STDOUT_MODE.load(Ordering::SeqCst));
            }
        }
    }

    // 128 + SIGINT equivalent, mirroring the unix handler's exit code shape.
    unsafe { ExitProcess(130) };
}
