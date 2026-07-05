//! Terminal input handling.
//!
//! This module is the only place in the current scaffold that talks to terminal
//! APIs directly. Later tasks should convert raw bytes into normalized key
//! events before keymap resolution sees them.

mod raw_terminal;

use std::io::{self, Read, Write};

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

    write_raw_line(
        &mut stdout,
        "inspect-key: press keys to show raw bytes. Ctrl+C or Ctrl+D exits.",
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
        write_raw_line(&mut stdout, &format_chunk(chunk))?;

        if chunk.contains(&EXIT_CTRL_C) || chunk.contains(&EXIT_CTRL_D) {
            write_raw_line(&mut stdout, "exit")?;
            return Ok(());
        }
    }
}

fn write_raw_line(stdout: &mut impl Write, line: &str) -> io::Result<()> {
    // Raw mode disables output post-processing on many terminals, so use CRLF
    // explicitly instead of relying on `\n` -> `\r\n` translation.
    stdout.write_all(line.as_bytes())?;
    stdout.write_all(b"\r\n")?;
    stdout.flush()
}

fn format_chunk(chunk: &[u8]) -> String {
    let hex = chunk
        .iter()
        .map(|byte| format!("0x{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");

    format!("Raw bytes: {hex} | Escaped: {}", escape_bytes(chunk))
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
    use super::{escape_bytes, format_chunk};

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
    fn format_chunk_contains_hex_and_escaped_forms() {
        assert_eq!(
            format_chunk(&[0x1b, b'[', b'1', b'0', b'6', b';', b'6', b'u']),
            r"Raw bytes: 0x1b 0x5b 0x31 0x30 0x36 0x3b 0x36 0x75 | Escaped: \x1b[106;6u"
        );
    }
}
