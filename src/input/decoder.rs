//! Terminal byte stream to normalized `KeyEvent` decoder.
//!
//! The decoder accepts both legacy escape sequences and kitty CSI u events. It
//! intentionally returns `Incomplete` for prefixes such as a bare ESC or an
//! unfinished CSI so the caller can wait for more bytes instead of guessing.

use super::{Key, KeyEvent, Modifiers};

/// Result of decoding one input chunk.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum DecodeResult {
    Complete(Vec<KeyEvent>),
    Incomplete,
}

/// Decodes a byte slice into normalized key events.
pub fn decode_key_events(bytes: &[u8]) -> DecodeResult {
    let mut pending = bytes.to_vec();
    let events = drain_key_events(&mut pending);
    if pending.is_empty() {
        DecodeResult::Complete(events)
    } else {
        DecodeResult::Incomplete
    }
}

/// Drains every complete key event from the front of a streaming byte buffer.
///
/// Terminal escape sequences often cross `read(2)` boundaries. This function
/// consumes only fully decoded prefixes and intentionally keeps an incomplete
/// suffix in `buffer` so the event loop can append the next chunk before
/// trying again.
pub fn drain_key_events(buffer: &mut Vec<u8>) -> Vec<KeyEvent> {
    let mut events = Vec::new();
    let mut offset = 0;

    while offset < buffer.len() {
        match decode_one(&buffer[offset..]) {
            One::Event(event, consumed) => {
                events.push(event);
                offset += consumed;
            }
            One::Incomplete => break,
        }
    }

    if offset > 0 {
        buffer.drain(..offset);
    }
    events
}

/// Converts a timed-out lone ESC byte into an `Esc` key event.
///
/// A single ESC is ambiguous with the prefix of many terminal sequences. The
/// event loop calls this after its ESC timeout; any non-lone pending bytes are
/// left untouched because they may still be a damaged or slow sequence.
pub fn flush_pending_escape(buffer: &mut Vec<u8>) -> Option<KeyEvent> {
    if buffer.as_slice() == [0x1b] {
        buffer.clear();
        Some(KeyEvent::plain(Key::Esc))
    } else {
        None
    }
}

enum One {
    Event(KeyEvent, usize),
    Incomplete,
}

fn decode_one(bytes: &[u8]) -> One {
    match bytes[0] {
        // NUL is what legacy terminals send for Ctrl+Space (and Ctrl+@).
        // Without this arm it would fall into the UTF-8 branch and either
        // stall as Incomplete or swallow the following byte.
        0x00 => One::Event(KeyEvent::new(Key::Char(' '), Modifiers::ctrl()), 1),
        0x01..=0x08 | 0x0b..=0x0c | 0x0e..=0x1a => decode_c0_control(bytes[0]),
        b'\t' => One::Event(KeyEvent::plain(Key::Tab), 1),
        b'\r' | b'\n' => One::Event(KeyEvent::plain(Key::Enter), 1),
        0x1b => decode_escape(bytes),
        0x7f => One::Event(KeyEvent::plain(Key::Backspace), 1),
        0x20..=0x7e => One::Event(KeyEvent::plain(Key::Char(char::from(bytes[0]))), 1),
        _ => decode_utf8_or_unknown(bytes),
    }
}

fn decode_c0_control(byte: u8) -> One {
    let character = char::from(b'a' + byte - 1);
    One::Event(KeyEvent::new(Key::Char(character), Modifiers::ctrl()), 1)
}

fn decode_escape(bytes: &[u8]) -> One {
    if bytes.len() == 1 {
        return One::Incomplete;
    }

    match bytes[1] {
        b'[' => decode_csi(bytes),
        b'O' => decode_ss3(bytes),
        _ => match decode_one(&bytes[1..]) {
            One::Event(mut event, consumed) => {
                event.modifiers = event.modifiers.with_alt();
                One::Event(event, consumed + 1)
            }
            One::Incomplete => One::Incomplete,
        },
    }
}

fn decode_csi(bytes: &[u8]) -> One {
    match bytes
        .iter()
        .enumerate()
        .skip(2)
        .find_map(|(index, byte)| (0x40..=0x7e).contains(byte).then_some(index))
    {
        Some(final_index) => {
            let final_byte = bytes[final_index];
            let params = &bytes[2..final_index];
            let consumed = final_index + 1;

            if final_byte == b'u' {
                return decode_kitty_csi_u(params, bytes, consumed);
            }

            if let Some(event) = decode_legacy_csi(params, final_byte) {
                One::Event(event, consumed)
            } else {
                One::Event(
                    KeyEvent::plain(Key::Unknown(bytes[..consumed].to_vec())),
                    consumed,
                )
            }
        }
        None => One::Incomplete,
    }
}

fn decode_legacy_csi(params: &[u8], final_byte: u8) -> Option<KeyEvent> {
    let text = std::str::from_utf8(params).ok()?;
    let values = parse_semicolon_numbers(text)?;
    let modifiers = legacy_modifiers(values.get(1).copied());

    let key = match final_byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        // CSI Z is the dedicated back-tab sequence; normalize it to
        // Shift+Tab so bindings can treat Tab/Shift+Tab uniformly.
        b'Z' => return Some(KeyEvent::new(Key::Tab, modifiers.with_shift())),
        b'~' => legacy_tilde_key(*values.first()?)?,
        _ => return None,
    };

    Some(KeyEvent::new(key, modifiers))
}

fn legacy_tilde_key(code: u16) -> Option<Key> {
    match code {
        1 | 7 => Some(Key::Home),
        3 => Some(Key::Delete),
        4 | 8 => Some(Key::End),
        5 => Some(Key::PageUp),
        6 => Some(Key::PageDown),
        11 => Some(Key::F(1)),
        12 => Some(Key::F(2)),
        13 => Some(Key::F(3)),
        14 => Some(Key::F(4)),
        15 => Some(Key::F(5)),
        17 => Some(Key::F(6)),
        18 => Some(Key::F(7)),
        19 => Some(Key::F(8)),
        20 => Some(Key::F(9)),
        21 => Some(Key::F(10)),
        23 => Some(Key::F(11)),
        24 => Some(Key::F(12)),
        _ => None,
    }
}

fn decode_ss3(bytes: &[u8]) -> One {
    if bytes.len() < 3 {
        return One::Incomplete;
    }

    let key = match bytes[2] {
        b'P' => Key::F(1),
        b'Q' => Key::F(2),
        b'R' => Key::F(3),
        b'S' => Key::F(4),
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        _ => Key::Unknown(bytes[..3].to_vec()),
    };

    One::Event(KeyEvent::plain(key), 3)
}

fn decode_kitty_csi_u(params: &[u8], bytes: &[u8], consumed: usize) -> One {
    let Some(values) = std::str::from_utf8(params)
        .ok()
        .and_then(parse_semicolon_numbers)
    else {
        return One::Event(
            KeyEvent::plain(Key::Unknown(bytes[..consumed].to_vec())),
            consumed,
        );
    };

    let Some(codepoint) = values.first().copied() else {
        return One::Event(
            KeyEvent::plain(Key::Unknown(bytes[..consumed].to_vec())),
            consumed,
        );
    };

    let modifiers = values
        .get(1)
        .copied()
        .map(Modifiers::from_kitty_encoded)
        .unwrap_or_default();

    let key = match codepoint {
        9 => Key::Tab,
        13 => Key::Enter,
        27 => Key::Esc,
        127 => Key::Backspace,
        _ => char::from_u32(u32::from(codepoint)).map_or_else(
            || Key::Unknown(bytes[..consumed].to_vec()),
            |character| Key::Char(character.to_ascii_lowercase()),
        ),
    };

    One::Event(KeyEvent::new(key, modifiers), consumed)
}

fn decode_utf8_or_unknown(bytes: &[u8]) -> One {
    for width in 2..=4 {
        if bytes.len() < width {
            return One::Incomplete;
        }
        if let Ok(text) = std::str::from_utf8(&bytes[..width])
            && let Some(character) = text.chars().next()
        {
            return One::Event(KeyEvent::plain(Key::Char(character)), width);
        }
    }

    One::Event(KeyEvent::plain(Key::Unknown(vec![bytes[0]])), 1)
}

fn parse_semicolon_numbers(text: &str) -> Option<Vec<u16>> {
    if text.is_empty() {
        return Some(Vec::new());
    }

    text.split(';')
        .map(|part| part.parse::<u16>().ok())
        .collect()
}

fn legacy_modifiers(encoded: Option<u16>) -> Modifiers {
    encoded
        .map(Modifiers::from_kitty_encoded)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{DecodeResult, decode_key_events, drain_key_events, flush_pending_escape};
    use crate::input::{Key, KeyEvent, Modifiers};

    #[test]
    fn drain_key_events_keeps_split_escape_sequence_until_complete() {
        let mut buffer = b"\x1b[1;5".to_vec();
        assert_eq!(drain_key_events(&mut buffer), Vec::new());
        assert_eq!(buffer, b"\x1b[1;5");

        buffer.extend_from_slice(b"C");
        assert_eq!(
            drain_key_events(&mut buffer),
            vec![KeyEvent::new(Key::Right, Modifiers::ctrl())]
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn flush_pending_escape_turns_lone_esc_into_event() {
        let mut buffer = vec![0x1b];
        assert_eq!(
            flush_pending_escape(&mut buffer),
            Some(KeyEvent::plain(Key::Esc))
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn decode_required_key_event_cases() {
        let cases: &[(&str, &[u8], DecodeResult)] = &[
            (
                "plain ASCII",
                b"a",
                complete([KeyEvent::plain(Key::Char('a'))]),
            ),
            (
                "C0 Ctrl+A",
                &[0x01],
                complete([KeyEvent::new(Key::Char('a'), Modifiers::ctrl())]),
            ),
            (
                "legacy Tab is not Ctrl+I",
                &[0x09],
                complete([KeyEvent::plain(Key::Tab)]),
            ),
            (
                "legacy LF is Enter, not Ctrl+J",
                &[0x0a],
                complete([KeyEvent::plain(Key::Enter)]),
            ),
            (
                "legacy CR is Enter",
                &[0x0d],
                complete([KeyEvent::plain(Key::Enter)]),
            ),
            (
                "legacy CSI Up",
                b"\x1b[A",
                complete([KeyEvent::plain(Key::Up)]),
            ),
            (
                "legacy modified CSI Ctrl+Right",
                b"\x1b[1;5C",
                complete([KeyEvent::new(Key::Right, Modifiers::ctrl())]),
            ),
            ("SS3 F1", b"\x1bOP", complete([KeyEvent::plain(Key::F(1))])),
            (
                "kitty CSI u Ctrl+Shift+J",
                b"\x1b[106;6u",
                complete([KeyEvent::new(
                    Key::Char('j'),
                    Modifiers::ctrl().with_shift(),
                )]),
            ),
            (
                "kitty CSI u Ctrl+Enter",
                b"\x1b[13;5u",
                complete([KeyEvent::new(Key::Enter, Modifiers::ctrl())]),
            ),
            (
                "kitty CSI u Super+S is not a plain s",
                b"\x1b[115;9u",
                complete([KeyEvent::new(Key::Char('s'), Modifiers::super_key())]),
            ),
            (
                "ESC prefix Alt+F",
                b"\x1bf",
                complete([KeyEvent::new(Key::Char('f'), Modifiers::alt())]),
            ),
            (
                "legacy NUL is Ctrl+Space",
                &[0x00],
                complete([KeyEvent::new(Key::Char(' '), Modifiers::ctrl())]),
            ),
            (
                "NUL followed by another key does not swallow it",
                &[0x00, b'a'],
                complete([
                    KeyEvent::new(Key::Char(' '), Modifiers::ctrl()),
                    KeyEvent::plain(Key::Char('a')),
                ]),
            ),
            (
                "CSI Z is Shift+Tab",
                b"\x1b[Z",
                complete([KeyEvent::new(Key::Tab, Modifiers::shift())]),
            ),
            (
                "truncated CSI waits for more",
                b"\x1b[1;5",
                DecodeResult::Incomplete,
            ),
            (
                "unknown complete CSI does not panic",
                b"\x1b[999z",
                complete([KeyEvent::plain(Key::Unknown(b"\x1b[999z".to_vec()))]),
            ),
        ];

        for (name, input, expected) in cases {
            assert_eq!(decode_key_events(input), *expected, "{name}");
        }
    }

    #[test]
    fn decode_multiple_ascii_events_from_one_chunk() {
        assert_eq!(
            decode_key_events(b"ab"),
            complete([
                KeyEvent::plain(Key::Char('a')),
                KeyEvent::plain(Key::Char('b'))
            ])
        );
    }

    fn complete<const N: usize>(events: [KeyEvent; N]) -> DecodeResult {
        DecodeResult::Complete(events.into())
    }
}
