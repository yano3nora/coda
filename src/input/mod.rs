//! Terminal input handling.
//!
//! This module is the only place in the current scaffold that talks to terminal
//! APIs directly. Raw bytes are decoded into normalized key events before any
//! future keymap resolver sees them.

mod decoder;
mod key_event;
pub(crate) mod raw_terminal;

use std::io::{self, Read, Write};

pub use decoder::{DecodeResult, decode_key_events};
pub use key_event::{Key, KeyEvent, Modifiers};
pub use raw_terminal::RawModeGuard;

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
struct KeyboardProtocolGuard;

impl KeyboardProtocolGuard {
    /// `CSI > 1 u`: push "disambiguate escape codes" onto the terminal's
    /// keyboard mode stack, then `CSI ? u`: query the resulting flags.
    fn push(stdout: &mut impl Write) -> io::Result<Self> {
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

fn write_raw_line(stdout: &mut impl Write, line: &str) -> io::Result<()> {
    // Raw mode disables output post-processing on many terminals, so use CRLF
    // explicitly instead of relying on `\n` -> `\r\n` translation.
    stdout.write_all(line.as_bytes())?;
    stdout.write_all(b"\r\n")?;
    stdout.flush()
}

fn format_inspect_chunk(chunk: &[u8]) -> Vec<String> {
    let mut lines = vec![format!("Raw bytes: {}", escape_bytes(chunk))];
    match decode_key_events(chunk) {
        DecodeResult::Complete(events) => {
            let pressed = events
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Pressed:   {pressed}"));
        }
        DecodeResult::Incomplete => lines.push("Pressed:   <incomplete sequence>".to_string()),
    }
    lines.push(format!("Hex:       {}", hex_bytes(chunk)));
    lines
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

fn escape_bytes(bytes: &[u8]) -> String {
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
}
