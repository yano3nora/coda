//! Unix raw mode: termios via the `libc` crate (platform-specific struct
//! layouts are not maintained by hand) + signal-based restore.

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

use super::CLEANUP_STEPS;

static INSTALL_SIGNAL_HANDLERS: Once = Once::new();
static SIGNAL_RESTORE_ACTIVE: AtomicBool = AtomicBool::new(false);
static SIGNAL_RESTORE_FD: AtomicI32 = AtomicI32::new(-1);
static mut SIGNAL_RESTORE_TERMIOS: MaybeUninit<Termios> = MaybeUninit::uninit();

/// Waits until stdin has input available, or until `timeout_ms` elapses.
///
/// Callers own decoding and timeout policy; this wrapper only keeps the
/// terminal-specific readiness API inside the input layer.
pub(crate) fn poll_stdin_readable(timeout_ms: i32) -> io::Result<bool> {
    poll_fd_readable(STDIN_FILENO, timeout_ms)
}

fn poll_fd_readable(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    // Absolute deadline (negative timeout = wait forever) so EINTR retries
    // use the remaining time. Retrying with the full timeout each time would
    // starve the event loop's housekeeping under a stream of SIGWINCH during
    // an interactive resize drag.
    let deadline = u64::try_from(timeout_ms)
        .ok()
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
    let mut remaining_ms = timeout_ms;
    loop {
        let mut fds = [libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        let result =
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, remaining_ms) };
        if result >= 0 {
            return Ok(result > 0 && fds[0].revents & libc::POLLIN != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
        // Signals (SIGWINCH on resize) interrupt poll with EINTR; retry for
        // the remaining time instead of erroring so the event loop survives.
        // Returning "no input" early instead would make callers treat an
        // interrupt as an elapsed escape-flush timeout and misfire a bare ESC.
        if let Some(deadline) = deadline {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            remaining_ms = i32::try_from(left.as_millis().max(1)).unwrap_or(i32::MAX);
        }
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
    // Only async-signal-safe calls are allowed here (write / tcsetattr / _exit
    // are). CLEANUP_STEPS is a const table, so no allocation happens either.
    for (active, sequence) in CLEANUP_STEPS {
        if active.load(Ordering::SeqCst) {
            unsafe {
                libc::write(1, sequence.as_ptr().cast(), sequence.len());
            }
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
    use super::{Termios, poll_fd_readable, set_read_behavior};
    use libc::{VMIN, VTIME};

    /// Regression test (TASK-260820 resize crash): a signal interrupting
    /// `poll` (SIGWINCH on resize) must not surface as an error that unwinds
    /// the event loop and closes the editor. SIGUSR1 stands in for SIGWINCH
    /// so the test does not touch the production resize handler, which other
    /// tests in this process may depend on; EINTR mechanics are identical.
    #[test]
    fn poll_interrupted_by_signal_retries_instead_of_erroring() {
        extern "C" fn noop(_signal_number: libc::c_int) {}

        let previous_action = unsafe {
            // sigaction with sa_flags = 0 (no SA_RESTART) mirrors the worst
            // case: the syscall is interrupted rather than auto-restarted.
            let mut action: libc::sigaction = std::mem::zeroed();
            let mut previous: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = noop as extern "C" fn(libc::c_int) as libc::sighandler_t;
            assert_eq!(libc::sigaction(libc::SIGUSR1, &action, &mut previous), 0);
            previous
        };

        let mut pipe_fds = [0_i32; 2];
        assert_eq!(unsafe { libc::pipe(pipe_fds.as_mut_ptr()) }, 0);
        let read_fd = pipe_fds[0];

        let (sender, receiver) = std::sync::mpsc::channel();
        let poller = std::thread::spawn(move || {
            sender
                .send(unsafe { libc::pthread_self() } as usize)
                .unwrap();
            // Short timeout keeps the retried poll (post-fix behavior) from
            // stalling the test suite; pre-fix this returned Err(EINTR).
            poll_fd_readable(read_fd, 400)
        });

        let thread_id = receiver.recv().unwrap() as libc::pthread_t;
        std::thread::sleep(std::time::Duration::from_millis(100));
        // Asserting delivery matters: if the signal were never sent, the
        // natural timeout would also yield Ok(false) and hide a regression.
        assert_eq!(
            unsafe { libc::pthread_kill(thread_id, libc::SIGUSR1) },
            0,
            "failed to interrupt the polling thread"
        );

        let result = poller.join().unwrap();
        assert!(matches!(result, Ok(false)), "got {result:?}");

        unsafe {
            assert_eq!(
                libc::sigaction(libc::SIGUSR1, &previous_action, std::ptr::null_mut()),
                0
            );
            libc::close(pipe_fds[0]);
            libc::close(pipe_fds[1]);
        }
    }

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
