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
        match wait_readable_once(fd, remaining_ms) {
            Ok(ready) => return Ok(ready),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
        // Signals (SIGWINCH on resize) interrupt the wait with EINTR; retry
        // for the remaining time instead of erroring so the event loop
        // survives. Returning "no input" early instead would make callers
        // treat an interrupt as an elapsed escape-flush timeout and misfire a
        // bare ESC.
        if let Some(deadline) = deadline {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            remaining_ms = i32::try_from(left.as_millis().max(1)).unwrap_or(i32::MAX);
        }
    }
}

/// One readiness wait via `poll(2)`.
///
/// `POLLNVAL` is an error, not "no input": reporting it as a timeout makes
/// the event loop spin forever without ever reading (TASK-260918 dev-tty
/// freeze), which violates the "never break silently" rule.
#[cfg(not(target_os = "macos"))]
fn wait_readable_once(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    let mut fds = [libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }];
    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    if fds[0].revents & libc::POLLNVAL != 0 {
        // POLLNVAL means "not an open descriptor", which is exactly EBADF.
        // A raw OS error also keeps this path allocation-free, which the
        // forked test probes rely on.
        return Err(io::Error::from_raw_os_error(libc::EBADF));
    }
    Ok(result > 0 && fds[0].revents & libc::POLLIN != 0)
}

/// One readiness wait via `select(2)`.
///
/// macOS `poll(2)` is broken for the `/dev/tty` cloning device: it returns
/// immediately with `POLLNVAL`. Tools such as fzf hand `/dev/tty` to the
/// commands they spawn as stdin, so an editor launched from there would
/// never see input (TASK-260918). `select` works on every tty, which is the
/// same workaround fzf and crossterm's `filedescriptor` crate use.
#[cfg(target_os = "macos")]
fn wait_readable_once(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    if fd < 0 || fd >= libc::FD_SETSIZE as RawFd {
        // Outside the fd_set range; raw OS error keeps the path
        // allocation-free (see the poll variant).
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    // macOS select(2) silently ignores closed descriptors instead of failing
    // with EBADF, so validate up front to keep "invalid fd" an error rather
    // than an endless "no input" timeout.
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut read_set = MaybeUninit::<libc::fd_set>::uninit();
    unsafe {
        libc::FD_ZERO(read_set.as_mut_ptr());
        libc::FD_SET(fd, read_set.as_mut_ptr());
    }
    let mut read_set = unsafe { read_set.assume_init() };
    let mut timeout = libc::timeval {
        tv_sec: libc::time_t::from(timeout_ms / 1000),
        tv_usec: libc::suseconds_t::from((timeout_ms % 1000) * 1000),
    };
    // Negative timeout = wait forever, matching poll(2)'s convention.
    let timeout_ptr = if timeout_ms < 0 {
        ptr::null_mut()
    } else {
        &mut timeout
    };
    let result = unsafe {
        libc::select(
            fd + 1,
            &mut read_set,
            ptr::null_mut(),
            ptr::null_mut(),
            timeout_ptr,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(result > 0 && unsafe { libc::FD_ISSET(fd, &read_set) })
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
    use std::{
        os::fd::{FromRawFd, OwnedFd},
        ptr,
    };

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

    /// Exit codes of the forked probes below. The child must not panic
    /// (unwinding after `fork` in a multi-threaded process is unsafe), so it
    /// reports through `_exit` and the parent asserts on the code.
    const PROBE_OK: i32 = 0;
    const PROBE_WRONG_RESULT: i32 = 1;
    const PROBE_RETURNED_EARLY: i32 = 2;
    const PROBE_SETUP_FAILED: i32 = 3;

    /// Runs `body` in a forked child and returns its exit code. A fresh
    /// single-threaded child is the only place where "this fd is closed" or
    /// "this tty has no pending input" can be guaranteed: other test threads
    /// in the parent may open descriptors or reuse numbers at any time.
    ///
    /// The test process is multi-threaded, so the child inherits whatever
    /// locks other threads held at fork time. `body` must therefore avoid
    /// allocation and panics: raw syscalls, `Instant`, and the readiness
    /// wait (whose error paths are raw OS errors) are all it may use.
    fn exit_code_of_forked(body: impl FnOnce() -> i32) -> i32 {
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed: {}", std::io::Error::last_os_error());
        if pid == 0 {
            let code = body();
            unsafe { libc::_exit(code) };
        }
        let mut status = 0;
        loop {
            if unsafe { libc::waitpid(pid, &mut status, 0) } == pid {
                break;
            }
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::Interrupted,
                "waitpid failed: {error}"
            );
        }
        assert!(
            libc::WIFEXITED(status),
            "probe child did not exit normally (status {status})"
        );
        libc::WEXITSTATUS(status)
    }

    /// Regression test (TASK-260918 dev-tty freeze): a readiness wait that
    /// the kernel rejects must surface as an error. Pre-fix, `POLLNVAL` was
    /// reported as "no input", so the event loop spun forever without ever
    /// reading stdin.
    #[test]
    fn readiness_wait_on_closed_fd_errors_instead_of_reporting_no_input() {
        let code = exit_code_of_forked(|| {
            let mut pipe_fds = [0_i32; 2];
            if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
                return PROBE_SETUP_FAILED;
            }
            let closed_fd = pipe_fds[0];
            unsafe {
                libc::close(pipe_fds[0]);
                libc::close(pipe_fds[1]);
            }
            // Nothing else runs in this child, so the number stays closed.
            match poll_fd_readable(closed_fd, 0) {
                Err(_) => PROBE_OK,
                Ok(_) => PROBE_WRONG_RESULT,
            }
        });
        assert_eq!(code, PROBE_OK, "probe exit code {code}");
    }

    /// Regression test (TASK-260918 dev-tty freeze): macOS `poll(2)` returns
    /// `POLLNVAL` immediately for the `/dev/tty` cloning device, which fzf
    /// passes as stdin to the commands it spawns. The wait must honor its
    /// timeout there instead of returning at once. The child gets its own
    /// pty as controlling terminal so `/dev/tty` resolves to a terminal
    /// nobody writes to; the developer's real terminal (which may have
    /// pending input, or not exist in CI) is never involved.
    #[test]
    fn readiness_wait_on_dev_tty_honors_timeout() {
        let mut master = 0;
        let mut slave = 0;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            },
            0,
            "openpty failed: {}",
            std::io::Error::last_os_error()
        );
        // Closed on every exit path, including a failing assertion below.
        let _master = unsafe { OwnedFd::from_raw_fd(master) };
        let _slave = unsafe { OwnedFd::from_raw_fd(slave) };

        let timeout = std::time::Duration::from_millis(80);
        let code = exit_code_of_forked(|| {
            // New session + TIOCSCTTY: the pty becomes this child's
            // controlling terminal, so /dev/tty now names it.
            if unsafe { libc::setsid() } < 0
                || unsafe { libc::ioctl(slave, libc::TIOCSCTTY as _, 0) } < 0
            {
                return PROBE_SETUP_FAILED;
            }
            let fd = unsafe { libc::open(c"/dev/tty".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
            if fd < 0 {
                return PROBE_SETUP_FAILED;
            }
            let started = std::time::Instant::now();
            let result = poll_fd_readable(fd, timeout.as_millis() as i32);
            let elapsed = started.elapsed();
            match result {
                Ok(false) if elapsed >= timeout / 2 => PROBE_OK,
                // Slack for timer granularity; pre-fix this returned in
                // microseconds.
                Ok(false) => PROBE_RETURNED_EARLY,
                Ok(true) | Err(_) => PROBE_WRONG_RESULT,
            }
        });
        assert_eq!(
            code, PROBE_OK,
            "probe exit code {code} (1 = wrong result, 2 = returned before the timeout, 3 = setup failed)"
        );
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
