//! Terminal input handling.
//!
//! This module is the only place in the current scaffold that talks to terminal
//! APIs directly. Raw bytes are decoded into normalized key events before any
//! future keymap resolver sees them.

mod capabilities;
mod decoder;
mod key_event;
mod mouse;
pub mod quirks;
pub(crate) mod raw_terminal;

use std::io::{self, Read, Write};

pub use capabilities::{
    CapabilityDetection, CapabilityProbe, KeyboardCapabilities, probe_blocking,
};
pub use decoder::{
    DecodeResult, InputEvent, decode_key_events, drain_input_events, drain_key_events,
    flush_pending_escape,
};
pub use key_event::{Key, KeyEvent, Modifiers};
pub use mouse::{MouseButton, MouseEvent, MouseEventKind};
pub use raw_terminal::RawModeGuard;
pub(crate) use raw_terminal::poll_readable;

const EXIT_CTRL_C: u8 = 0x03;
const EXIT_CTRL_D: u8 = 0x04;

/// Starts the raw input inspector.
///
/// The inspector prints each received byte chunk in both hexadecimal and escaped
/// forms. It exits on `Ctrl+C` or `Ctrl+D` after restoring terminal settings via
/// `RawModeGuard`.
pub fn inspect_key() -> io::Result<()> {
    let _raw_mode = RawModeGuard::enable_stdin()?;
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut buffer = [0_u8; 128];

    // Opt in to the kitty keyboard protocol "disambiguate" mode for this
    // session and immediately query the active flags. A supporting terminal
    // answers with `CSI ? <flags> u`, which shows up as the first input line —
    // that reply doubles as a visible capability probe for the user.
    let _protocol = KeyboardProtocolGuard::push(&mut stdout)?;

    write_raw_line(
        &mut stdout,
        "inspect-key: press keys to show raw bytes. Ctrl+C or Ctrl+D exits.",
    )?;
    write_raw_line(
        &mut stdout,
        "kitty keyboard protocol requested. A \\x1b[?..u line below means your terminal supports it.",
    )?;

    loop {
        let read = stdin.read(&mut buffer)?;
        if read == 0 {
            // EOF / hangup. Continuing here would busy-loop forever, so exit
            // and let RawModeGuard restore the terminal.
            write_raw_line(&mut stdout, "stdin closed. exit")?;
            return Ok(());
        }

        let chunk = &buffer[..read];
        for line in format_inspect_chunk(chunk) {
            write_raw_line(&mut stdout, &line)?;
        }

        if chunk.contains(&EXIT_CTRL_C) || chunk.contains(&EXIT_CTRL_D) {
            write_raw_line(&mut stdout, "exit")?;
            return Ok(());
        }
    }
}

/// Pushes kitty keyboard protocol flags for its lifetime and pops them on drop.
///
/// Terminals that do not support the protocol ignore these sequences, so this
/// is safe to emit unconditionally (progressive enhancement, ADR-0003).
pub struct KeyboardProtocolGuard;

impl KeyboardProtocolGuard {
    /// `CSI > 1 u`: push "disambiguate escape codes" onto the terminal's
    /// keyboard mode stack, then `CSI ? u`: query the resulting flags.
    pub fn push(stdout: &mut impl Write) -> io::Result<Self> {
        stdout.write_all(b"\x1b[>1u\x1b[?u")?;
        stdout.flush()?;
        raw_terminal::set_keyboard_protocol_pushed(true);
        Ok(Self)
    }
}

impl Drop for KeyboardProtocolGuard {
    fn drop(&mut self) {
        // `CSI < u`: pop our pushed mode so the shell keeps normal input.
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(b"\x1b[<u");
        let _ = stdout.flush();
        raw_terminal::set_keyboard_protocol_pushed(false);
    }
}

/// Enables bracketed paste for its lifetime and disables it on drop.
///
/// This makes terminal paste arrive as `CSI 200~ ... CSI 201~`, letting the
/// decoder bypass key resolution for pasted content. Unsupported terminals
/// ignore these DECSET/DECRST sequences.
pub struct BracketedPasteGuard;

impl BracketedPasteGuard {
    pub fn enable(stdout: &mut impl Write) -> io::Result<Self> {
        stdout.write_all(b"\x1b[?2004h")?;
        stdout.flush()?;
        raw_terminal::set_bracketed_paste_active(true);
        Ok(Self)
    }
}

impl Drop for BracketedPasteGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(b"\x1b[?2004l");
        let _ = stdout.flush();
        raw_terminal::set_bracketed_paste_active(false);
    }
}

/// Enables SGR mouse reporting for its lifetime and disables it on drop
/// (ADR-0008 §3): DECSET 1002 (button-event tracking, so drags report) +
/// 1006 (SGR extended coordinates). Unsupported terminals ignore both.
/// While active the terminal's native mouse selection is taken over;
/// Terminal-native selection relies on the terminal reserving Shift+drag
/// before it delivers an SGR event to the application.
pub struct MouseReportingGuard;

impl MouseReportingGuard {
    pub fn enable(stdout: &mut impl Write) -> io::Result<Self> {
        stdout.write_all(b"\x1b[?1002h\x1b[?1006h")?;
        stdout.flush()?;
        raw_terminal::set_mouse_reporting_active(true);
        Ok(Self)
    }
}

impl Drop for MouseReportingGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(b"\x1b[?1006l\x1b[?1002l");
        let _ = stdout.flush();
        raw_terminal::set_mouse_reporting_active(false);
    }
}

fn write_raw_line(stdout: &mut impl Write, line: &str) -> io::Result<()> {
    // Raw mode disables output post-processing on many terminals, so use CRLF
    // explicitly instead of relying on `\n` -> `\r\n` translation.
    stdout.write_all(line.as_bytes())?;
    stdout.write_all(b"\r\n")?;
    stdout.flush()
}

fn format_inspect_chunk(chunk: &[u8]) -> Vec<String> {
    let mut lines = vec![format!("Raw bytes: {}", escape_bytes(chunk))];
    let mut pending = chunk.to_vec();
    let events = drain_input_events(&mut pending);

    if pending.is_empty() {
        // Keystrokes still get their own comma-joined "Pressed:" line
        // (unchanged from before capability replies existed), but only when
        // the chunk actually decoded to at least one key — a chunk that is
        // purely a capability-query reply has nothing to report as
        // "pressed".
        let pressed = events
            .iter()
            .filter_map(|event| match event {
                InputEvent::Key(key) => Some(key.to_string()),
                InputEvent::Paste(_)
                | InputEvent::CapabilityReply(_)
                | InputEvent::DeviceAttributes
                | InputEvent::Mouse(_) => None,
            })
            .collect::<Vec<_>>();
        if !pressed.is_empty() || events.is_empty() {
            lines.push(format!("Pressed:   {}", pressed.join(", ")));
        }
        for event in &events {
            if let Some(protocol_line) = format_protocol_line(event) {
                lines.push(protocol_line);
            }
        }
    } else {
        lines.push("Pressed:   <incomplete sequence>".to_string());
    }

    lines.push(format!("Hex:       {}", hex_bytes(chunk)));
    lines
}

/// Renders our startup capability-query replies (`CSI ?u` flags / DA1) as a
/// friendly `Protocol:` line for the raw input inspector, instead of letting
/// them silently vanish as filtered-out non-key events.
fn format_protocol_line(event: &InputEvent) -> Option<String> {
    match event {
        InputEvent::CapabilityReply(flags) if flags & 1 != 0 => Some(format!(
            "Protocol:  kitty keyboard protocol supported (flags={flags})"
        )),
        InputEvent::CapabilityReply(flags) => Some(format!(
            "Protocol:  kitty CSI u replied without the disambiguate flag (flags={flags}) — treated as legacy"
        )),
        InputEvent::DeviceAttributes => Some(
            "Protocol:  primary device attributes received (no kitty CSI u reply — legacy terminal)"
                .to_string(),
        ),
        // Mouse reporting is not enabled by `inspect-key`, but a stray
        // report is still not a keystroke — name it instead of hiding it.
        InputEvent::Mouse(mouse) => Some(format!("Mouse:     {mouse:?}")),
        InputEvent::Key(_) | InputEvent::Paste(_) => None,
    }
}

fn hex_bytes(chunk: &[u8]) -> String {
    chunk
        .iter()
        .map(|byte| format!("0x{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
fn format_chunk(chunk: &[u8]) -> String {
    format!(
        "Raw bytes: {} | Hex: {}",
        escape_bytes(chunk),
        hex_bytes(chunk)
    )
}

pub fn escape_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| match *byte {
            b'\\' => "\\\\".to_string(),
            b'\n' => "\\n".to_string(),
            b'\r' => "\\r".to_string(),
            b'\t' => "\\t".to_string(),
            0x20..=0x7e => char::from(*byte).to_string(),
            _ => format!("\\x{byte:02x}"),
        })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::{escape_bytes, format_chunk, format_inspect_chunk};

    #[test]
    fn escape_printable_ascii_without_hex_noise() {
        assert_eq!(escape_bytes(b"aZ9"), "aZ9");
    }

    #[test]
    fn escape_control_bytes_as_backslash_x() {
        assert_eq!(escape_bytes(&[0x1b, b'[', b'A']), "\\x1b[A");
    }

    #[test]
    fn escape_backslash_and_common_whitespace_readably() {
        assert_eq!(escape_bytes(b"\\\n\r\t"), "\\\\\\n\\r\\t");
    }

    #[test]
    fn format_chunk_contains_escaped_and_hex_forms() {
        assert_eq!(
            format_chunk(&[0x1b, b'[', b'1', b'0', b'6', b';', b'6', b'u']),
            r"Raw bytes: \x1b[106;6u | Hex: 0x1b 0x5b 0x31 0x30 0x36 0x3b 0x36 0x75"
        );
    }

    #[test]
    fn format_inspect_chunk_adds_decoded_pressed_line() {
        assert_eq!(
            format_inspect_chunk(&[0x1b, b'[', b'1', b'0', b'6', b';', b'6', b'u']),
            vec![
                r"Raw bytes: \x1b[106;6u".to_string(),
                "Pressed:   Ctrl+Shift+J".to_string(),
                "Hex:       0x1b 0x5b 0x31 0x30 0x36 0x3b 0x36 0x75".to_string(),
            ]
        );
    }

    #[test]
    fn format_inspect_chunk_shows_friendly_line_for_kitty_flags_reply() {
        assert_eq!(
            format_inspect_chunk(b"\x1b[?1u"),
            vec![
                r"Raw bytes: \x1b[?1u".to_string(),
                "Protocol:  kitty keyboard protocol supported (flags=1)".to_string(),
                "Hex:       0x1b 0x5b 0x3f 0x31 0x75".to_string(),
            ]
        );
    }

    #[test]
    fn format_inspect_chunk_shows_friendly_line_for_da1_reply() {
        assert_eq!(
            format_inspect_chunk(b"\x1b[?62;22c"),
            vec![
                r"Raw bytes: \x1b[?62;22c".to_string(),
                "Protocol:  primary device attributes received (no kitty CSI u reply — legacy terminal)"
                    .to_string(),
                "Hex:       0x1b 0x5b 0x3f 0x36 0x32 0x3b 0x32 0x32 0x63".to_string(),
            ]
        );
    }
}
