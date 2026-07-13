//! Keyboard protocol capability detection (SPEC-0003, ADR-0003,
//! TASK-260712-16).
//!
//! `KeyboardCapabilities` is the boundary type the keymap resolver and VS
//! Code importer are allowed to see: they must reason about *what a
//! terminal can deliver*, never about protocol names or raw bytes (ADR-0003
//! / AGENTS.md dependency rule). `CapabilityDetection` sits one layer below
//! it and exists purely for explainability — the startup warning, the
//! `:inspect-key` overlay, and the import CLI all need to say *why* a
//! terminal was judged legacy, not just report the boolean outcome.
//!
//! Detection itself is split in two:
//! - `CapabilityProbe` is a pure state machine (`on_event` / `on_tick`) with
//!   no terminal I/O, so the event-loop wiring and the detection logic can be
//!   unit-tested independently of a real terminal.
//! - `probe_blocking` is the one function in this module that touches a real
//!   terminal, for the `coda keymap import vscode` CLI, which has no event
//!   loop of its own to drive a probe incrementally.

use std::{
    io::{self, Read, Write},
    time::{Duration, Instant},
};

use super::{InputEvent, RawModeGuard, drain_input_events, poll_stdin_readable, tty};

/// What this terminal can deliver to us, expressed as capabilities rather
/// than protocol names (SPEC-0003). The keymap importer and resolver only
/// ever see this type.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct KeyboardCapabilities {
    pub supports_modified_keys: bool,
    pub supports_shift_ctrl_distinction: bool,
    pub supports_shift_enter: bool,
    pub supports_ctrl_enter: bool,
}

impl KeyboardCapabilities {
    /// Everything SPEC-0003 lists as distinguishable (kitty CSI u active).
    pub const fn modern() -> Self {
        Self {
            supports_modified_keys: true,
            supports_shift_ctrl_distinction: true,
            supports_shift_enter: true,
            supports_ctrl_enter: true,
        }
    }

    /// Nothing beyond what legacy escape sequences can carry. This is the
    /// conservative fallback: detection failures and unresolved probes must
    /// never claim a capability the terminal cannot actually deliver
    /// (Invariants, SPEC-0003).
    pub const fn legacy() -> Self {
        Self {
            supports_modified_keys: false,
            supports_shift_ctrl_distinction: false,
            supports_shift_enter: false,
            supports_ctrl_enter: false,
        }
    }
}

/// The judgment behind a resolved `KeyboardCapabilities`, kept around for
/// user-facing explanations (inspector protocol line, CLI capability line,
/// startup warning source).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CapabilityDetection {
    /// A `CSI ? <flags> u` reply arrived. Bit 0 (disambiguate escape codes)
    /// decides modern vs. legacy; the terminal is expected to set it because
    /// `KeyboardProtocolGuard::push` already pushed that exact flag.
    KittyFlags(u16),
    /// DA1 (`CSI ? ... c`) answered before any `CSI ?u` reply did. Almost
    /// every terminal answers DA1, so this is the fast, non-timeout path to
    /// "legacy" (design decision 260712).
    LegacyDeviceAttributes,
    /// win32-input-mode sequences arrived (TASK-260713): the terminal
    /// honored our `CSI ?9001h` request, so full modifier fidelity is
    /// available — Windows Terminal's equivalent of kitty CSI u.
    Win32InputMode,
    /// Neither query was answered before the deadline.
    LegacyTimeout,
    /// stdin or stdout is not a TTY (import CLI run under a pipe/redirect/CI):
    /// there is no terminal to negotiate with, so modern is assumed rather
    /// than treating every super/shift-enter/ctrl-enter binding as
    /// undeliverable.
    AssumedModern,
}

impl CapabilityDetection {
    /// Resolves this judgment to the capability booleans the keymap layer
    /// consumes. All-or-nothing: SPEC-0003's negotiation has no notion of
    /// "partially modern" (a `KittyFlags` reply either sets the disambiguate
    /// bit we requested, or the terminal is treated as not supporting the
    /// protocol at all).
    pub fn capabilities(self) -> KeyboardCapabilities {
        match self {
            Self::KittyFlags(flags) if flags & 1 != 0 => KeyboardCapabilities::modern(),
            Self::KittyFlags(_) | Self::LegacyDeviceAttributes | Self::LegacyTimeout => {
                KeyboardCapabilities::legacy()
            }
            Self::Win32InputMode | Self::AssumedModern => KeyboardCapabilities::modern(),
        }
    }

    /// Short explanation for the `:inspect-key` overlay's protocol line.
    pub fn description(self) -> String {
        match self {
            Self::KittyFlags(flags) if flags & 1 != 0 => {
                format!("kitty CSI u (flags={flags})")
            }
            Self::KittyFlags(flags) => {
                format!("legacy (kitty flags={flags}, disambiguate bit unset)")
            }
            Self::LegacyDeviceAttributes => "legacy (DA1 answered, no CSI ?u reply)".to_string(),
            Self::Win32InputMode => "win32-input-mode (Windows Terminal)".to_string(),
            Self::LegacyTimeout => "legacy (query timed out)".to_string(),
            Self::AssumedModern => {
                "not detected (not an interactive terminal); assuming modern".to_string()
            }
        }
    }
}

/// A pure state machine that resolves `KeyboardCapabilities`. It performs no
/// terminal I/O; the caller (the event loop) feeds it decoded `InputEvent`s
/// and clock ticks. This split is what makes detection unit-testable without
/// a real terminal.
#[derive(Debug, Clone, Copy)]
pub struct CapabilityProbe {
    deadline: Instant,
    resolved: bool,
    /// Whether a DA1 reply alone proves "legacy". True on unix (design
    /// decision 260712). False on windows: with win32-input-mode requested,
    /// it is unverified whether the DA1 reply arrives wrapped or plain
    /// (TASK-260713), so a plain DA1 must keep waiting for win32 evidence
    /// instead of mis-resolving a fidelity-capable terminal as legacy.
    da1_resolves_legacy: bool,
}

impl CapabilityProbe {
    /// Arms a probe that resolves to `LegacyTimeout` once `on_tick` is
    /// called with a time at or past `deadline`, unless a reply resolves it
    /// first.
    pub fn arm(deadline: Instant) -> Self {
        Self::with_da1_policy(deadline, cfg!(not(windows)))
    }

    /// Testable constructor: the DA1 policy is platform behavior, but tests
    /// must be able to exercise both sides from any host OS.
    pub(crate) fn with_da1_policy(deadline: Instant, da1_resolves_legacy: bool) -> Self {
        Self {
            deadline,
            resolved: false,
            da1_resolves_legacy,
        }
    }

    /// Feeds one decoded input event. Returns `Some` the first time a
    /// capability-relevant event resolves the probe; after that (or after a
    /// timeout resolution), this always returns `None` — a DA1 reply
    /// arriving late after a `CapabilityReply` already resolved things (or
    /// vice versa) must not re-decide the outcome.
    pub fn on_event(&mut self, event: &InputEvent) -> Option<CapabilityDetection> {
        if self.resolved {
            return None;
        }
        let detection = match event {
            InputEvent::CapabilityReply(flags) => Some(CapabilityDetection::KittyFlags(*flags)),
            InputEvent::Win32InputMode => Some(CapabilityDetection::Win32InputMode),
            InputEvent::DeviceAttributes if self.da1_resolves_legacy => {
                Some(CapabilityDetection::LegacyDeviceAttributes)
            }
            InputEvent::DeviceAttributes
            | InputEvent::Key(_)
            | InputEvent::Paste(_)
            | InputEvent::Mouse(_) => None,
        };
        if detection.is_some() {
            self.resolved = true;
        }
        detection
    }

    /// Called once per event-loop iteration (idle poll granularity is
    /// sufficient — SPEC-0003 Open Question answer, design decision 260712).
    /// Resolves to `LegacyTimeout` once `now` reaches the deadline and
    /// nothing else has resolved yet.
    pub fn on_tick(&mut self, now: Instant) -> Option<CapabilityDetection> {
        if self.resolved || now < self.deadline {
            return None;
        }
        self.resolved = true;
        Some(CapabilityDetection::LegacyTimeout)
    }
}

/// Detects capabilities for the `coda keymap import vscode` CLI, which has
/// no event loop of its own to drive `CapabilityProbe` incrementally, so it
/// blocks for up to `timeout`.
///
/// A non-TTY stdin (piped input, CI) cannot answer an escape-sequence query
/// at all — probing would just burn `timeout` and then report legacy,
/// silently disabling every super/shift-enter/ctrl-enter binding on every
/// non-interactive run. Assuming modern there is the safer default: the
/// *runtime* truth is re-detected by the editor's own startup probe in
/// `EventLoop::run`, so a wrong assumption here only affects the import
/// report, not actual key delivery (SPEC-0004 design decision 260712).
///
/// stdout must be a TTY too: with `... > report.txt` the queries would land
/// in the redirected file (leaking escape bytes into it), no reply could
/// ever arrive, and the probe would burn the timeout into a false "legacy".
pub fn probe_blocking(timeout: Duration) -> CapabilityDetection {
    if !tty::stdin_is_tty() || !tty::stdout_is_tty() {
        return CapabilityDetection::AssumedModern;
    }

    probe_blocking_tty(timeout).unwrap_or(CapabilityDetection::LegacyTimeout)
}

fn probe_blocking_tty(timeout: Duration) -> io::Result<CapabilityDetection> {
    let _raw_mode = RawModeGuard::enable_stdin()?;
    let mut stdout = io::stdout().lock();
    // `KeyboardProtocolGuard` pushes/pops `CSI >1u` / `CSI <u` for us
    // (matching the event-loop's own probe sequence); DA1 is sent
    // separately since the guard only knows about the kitty push+query pair.
    let _protocol = super::KeyboardProtocolGuard::push(&mut stdout)?;
    stdout.write_all(b"\x1b[c")?;
    stdout.flush()?;
    drop(stdout);

    let deadline = Instant::now() + timeout;
    let mut probe = CapabilityProbe::arm(deadline);
    let mut stdin = io::stdin();
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 128];

    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(CapabilityDetection::LegacyTimeout);
        }
        let remaining_ms = (deadline - now).as_millis().min(i32::MAX as u128) as i32;
        if !poll_stdin_readable(remaining_ms)? {
            // `poll` timed out; loop back around to the deadline check above
            // rather than trusting `remaining_ms` twice.
            continue;
        }

        let read = stdin.read(&mut chunk)?;
        if read == 0 {
            return Ok(CapabilityDetection::LegacyTimeout);
        }
        buffer.extend_from_slice(&chunk[..read]);
        for event in drain_input_events(&mut buffer) {
            if let Some(detection) = probe.on_event(&event) {
                return Ok(detection);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CapabilityDetection, CapabilityProbe, KeyboardCapabilities};
    use crate::input::InputEvent;
    use std::time::{Duration, Instant};

    #[test]
    fn kitty_flags_with_disambiguate_bit_resolve_modern() {
        let mut probe = CapabilityProbe::arm(Instant::now() + Duration::from_millis(500));
        let detection = probe.on_event(&InputEvent::CapabilityReply(1));
        assert_eq!(detection, Some(CapabilityDetection::KittyFlags(1)));
        assert_eq!(
            detection.unwrap().capabilities(),
            KeyboardCapabilities::modern()
        );
    }

    #[test]
    fn kitty_flags_without_disambiguate_bit_resolve_legacy() {
        let mut probe = CapabilityProbe::arm(Instant::now() + Duration::from_millis(500));
        let detection = probe.on_event(&InputEvent::CapabilityReply(0));
        assert_eq!(detection, Some(CapabilityDetection::KittyFlags(0)));
        assert_eq!(
            detection.unwrap().capabilities(),
            KeyboardCapabilities::legacy()
        );
    }

    #[test]
    fn device_attributes_arriving_first_resolves_legacy() {
        let mut probe = CapabilityProbe::arm(Instant::now() + Duration::from_millis(500));
        let detection = probe.on_event(&InputEvent::DeviceAttributes);
        assert_eq!(detection, Some(CapabilityDetection::LegacyDeviceAttributes));
        assert_eq!(
            detection.unwrap().capabilities(),
            KeyboardCapabilities::legacy()
        );
    }

    #[test]
    fn win32_input_mode_resolves_modern() {
        let mut probe = CapabilityProbe::arm(Instant::now() + Duration::from_millis(500));
        let detection = probe.on_event(&InputEvent::Win32InputMode);
        assert_eq!(detection, Some(CapabilityDetection::Win32InputMode));
        assert_eq!(
            detection.unwrap().capabilities(),
            KeyboardCapabilities::modern()
        );
    }

    /// Windows probe policy (TASK-260713): a DA1 reply alone must not
    /// resolve legacy, because win32-input-mode evidence may still follow —
    /// only the timeout or a win32/kitty signal decides.
    #[test]
    fn da1_does_not_resolve_when_policy_defers_it() {
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut probe = CapabilityProbe::with_da1_policy(deadline, false);
        assert_eq!(probe.on_event(&InputEvent::DeviceAttributes), None);

        // win32 evidence arriving later still wins ...
        assert_eq!(
            probe.on_event(&InputEvent::Win32InputMode),
            Some(CapabilityDetection::Win32InputMode)
        );

        // ... and without it, the deadline resolves legacy as usual.
        let mut timeout_probe = CapabilityProbe::with_da1_policy(Instant::now(), false);
        assert_eq!(timeout_probe.on_event(&InputEvent::DeviceAttributes), None);
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(
            timeout_probe.on_tick(Instant::now()),
            Some(CapabilityDetection::LegacyTimeout)
        );
    }

    #[test]
    fn tick_past_deadline_resolves_legacy_timeout() {
        let deadline = Instant::now();
        let mut probe = CapabilityProbe::arm(deadline);
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(
            probe.on_tick(Instant::now()),
            Some(CapabilityDetection::LegacyTimeout)
        );
    }

    #[test]
    fn tick_before_deadline_stays_pending() {
        let mut probe = CapabilityProbe::arm(Instant::now() + Duration::from_secs(60));
        assert_eq!(probe.on_tick(Instant::now()), None);
    }

    #[test]
    fn events_and_ticks_after_resolution_are_ignored() {
        let mut probe = CapabilityProbe::arm(Instant::now() + Duration::from_millis(500));
        assert!(probe.on_event(&InputEvent::CapabilityReply(1)).is_some());

        // A late DA1 reply must not override the already-resolved verdict.
        assert_eq!(probe.on_event(&InputEvent::DeviceAttributes), None);
        // Nor should a tick past the deadline re-fire a timeout resolution.
        assert_eq!(
            probe.on_tick(Instant::now() + Duration::from_secs(3600)),
            None
        );
    }

    #[test]
    fn key_and_paste_events_do_not_resolve_the_probe() {
        let mut probe = CapabilityProbe::arm(Instant::now() + Duration::from_millis(500));
        assert_eq!(
            probe.on_event(&InputEvent::Key(crate::input::KeyEvent::plain(
                crate::input::Key::Char('a')
            ))),
            None
        );
        assert_eq!(probe.on_event(&InputEvent::Paste("hi".to_string())), None);
    }
}
