//! Terminal byte stream to normalized `KeyEvent` decoder.
//!
//! The decoder accepts both legacy escape sequences and kitty CSI u events. It
//! intentionally returns `Incomplete` for prefixes such as a bare ESC or an
//! unfinished CSI so the caller can wait for more bytes instead of guessing.

use super::{
    Key, KeyEvent, Modifiers,
    mouse::{MouseButton, MouseEvent, MouseEventKind},
};

/// Result of decoding one input chunk.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum DecodeResult {
    Complete(Vec<KeyEvent>),
    Incomplete,
}

/// Normalized input events emitted by the terminal input layer.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum InputEvent {
    Key(KeyEvent),
    Paste(String),
    /// Reply to our startup `CSI ?u` flags query (TASK-260712-16). Carries the
    /// raw kitty flags bitmask; capability interpretation (bit 0 = disambiguate)
    /// lives in `input::capabilities`, not here.
    CapabilityReply(u16),
    /// Reply to our startup `CSI c` (Primary Device Attributes) query. Its
    /// mere presence — without a preceding `CapabilityReply` — is the DA1
    /// fallback signal that a terminal does not understand kitty CSI u at all
    /// (SPEC-0003 detection design 260712).
    DeviceAttributes,
    /// SGR mouse report (`CSI < Cb;Cx;Cy M|m`, ADR-0008). Kept out of the
    /// key channel so mouse motion can never look like a keystroke to the
    /// resolver (SPEC-0003).
    Mouse(MouseEvent),
}

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

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
    drain_input_events(buffer)
        .into_iter()
        .filter_map(|event| match event {
            InputEvent::Key(key) => Some(key),
            // Capability query replies are not keystrokes; leaking them here
            // as `Key::Unknown` would make the resolver, importer, and
            // `inspect-key` all see a phantom keypress every time the
            // terminal answers our startup probe (TASK-260712-16). The
            // exhaustive match keeps this filter honest as new event kinds
            // are added.
            InputEvent::Paste(_)
            | InputEvent::CapabilityReply(_)
            | InputEvent::DeviceAttributes
            | InputEvent::Mouse(_) => None,
        })
        .collect()
}

/// Drains complete normalized input events from a streaming byte buffer.
///
/// Bracketed paste envelopes are emitted as a single [`InputEvent::Paste`]. The
/// pasted bytes are never recursively decoded as keys, so escape sequences
/// inside pasted text remain literal text. If the closing envelope has not
/// arrived yet, the whole paste prefix is retained for the next read chunk.
pub fn drain_input_events(buffer: &mut Vec<u8>) -> Vec<InputEvent> {
    let mut events = Vec::new();
    let mut offset = 0;

    while offset < buffer.len() {
        let remaining = &buffer[offset..];
        if remaining.starts_with(BRACKETED_PASTE_START) {
            let content_start = offset + BRACKETED_PASTE_START.len();
            let Some(relative_end) = find_subslice(&buffer[content_start..], BRACKETED_PASTE_END)
            else {
                break;
            };
            let content_end = content_start + relative_end;
            let text = normalize_paste_text(&buffer[content_start..content_end]);
            events.push(InputEvent::Paste(text));
            offset = content_end + BRACKETED_PASTE_END.len();
            continue;
        }

        match decode_one(remaining) {
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

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn normalize_paste_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

enum One {
    Event(InputEvent, usize),
    Incomplete,
}

/// Wraps a decoded `KeyEvent` as `One::Event` — the common case for every
/// branch below except capability query replies (`decode_capability_query`),
/// which are not keystrokes.
fn key_event(event: KeyEvent, consumed: usize) -> One {
    One::Event(InputEvent::Key(event), consumed)
}

fn decode_one(bytes: &[u8]) -> One {
    match bytes[0] {
        // NUL is what legacy terminals send for Ctrl+Space (and Ctrl+@).
        // Without this arm it would fall into the UTF-8 branch and either
        // stall as Incomplete or swallow the following byte.
        0x00 => key_event(KeyEvent::new(Key::Char(' '), Modifiers::ctrl()), 1),
        0x01..=0x08 | 0x0b..=0x0c | 0x0e..=0x1a => decode_c0_control(bytes[0]),
        b'\t' => key_event(KeyEvent::plain(Key::Tab), 1),
        b'\r' | b'\n' => key_event(KeyEvent::plain(Key::Enter), 1),
        0x1b => decode_escape(bytes),
        0x7f => key_event(KeyEvent::plain(Key::Backspace), 1),
        0x20..=0x7e => key_event(KeyEvent::plain(Key::Char(char::from(bytes[0]))), 1),
        _ => decode_utf8_or_unknown(bytes),
    }
}

fn decode_c0_control(byte: u8) -> One {
    let character = char::from(b'a' + byte - 1);
    key_event(KeyEvent::new(Key::Char(character), Modifiers::ctrl()), 1)
}

fn decode_escape(bytes: &[u8]) -> One {
    if bytes.len() == 1 {
        return One::Incomplete;
    }

    match bytes[1] {
        b'[' => decode_csi(bytes),
        b'O' => decode_ss3(bytes),
        _ => match decode_one(&bytes[1..]) {
            One::Event(InputEvent::Key(mut event), consumed) => {
                event.modifiers = event.modifiers.with_alt();
                key_event(event, consumed + 1)
            }
            // Alt-prefixed capability replies are not a real terminal output
            // (query responses are never "alt-pressed"), but the match must
            // stay exhaustive; pass the event through unmodified instead of
            // asserting an unreachable case.
            One::Event(other, consumed) => One::Event(other, consumed + 1),
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

            // `?`-prefixed params mark a private-mode reply. The only ones we
            // ever provoke are our own startup capability queries (`CSI ?u`
            // flags, `CSI c` DA1 — DA1 replies are also `?`-prefixed), so
            // route those to the capability decoder instead of the ordinary
            // key paths below (TASK-260712-16).
            if let Some(query_params) = params.strip_prefix(b"?") {
                return decode_capability_query(query_params, final_byte, bytes, consumed);
            }

            // `<`-prefixed params are SGR mouse reports (ADR-0008). Like
            // capability replies they are not keystrokes and must never fall
            // through to the key paths below.
            if let Some(mouse_params) = params.strip_prefix(b"<") {
                return decode_sgr_mouse(mouse_params, final_byte, bytes, consumed);
            }

            if final_byte == b'u' {
                return decode_kitty_csi_u(params, bytes, consumed);
            }

            if let Some(event) = decode_legacy_csi(params, final_byte) {
                key_event(event, consumed)
            } else {
                key_event(
                    KeyEvent::plain(Key::Unknown(bytes[..consumed].to_vec())),
                    consumed,
                )
            }
        }
        None => One::Incomplete,
    }
}

/// Decodes replies to our startup capability queries. These are query
/// *responses*, not key presses, so they must never leak into
/// `drain_key_events` as `Key::Unknown` — the importer and resolver would
/// otherwise see a phantom keystroke every time the terminal answers a probe.
/// Any other `?`-prefixed CSI (a private mode report we did not ask for)
/// falls back to `Key::Unknown`, unchanged from before this function existed.
fn decode_capability_query(
    query_params: &[u8],
    final_byte: u8,
    bytes: &[u8],
    consumed: usize,
) -> One {
    match final_byte {
        b'u' => {
            // An unparseable or empty flags value is treated as "no flags
            // set" (0) rather than falling back to Unknown: it is still
            // unambiguously a capability reply, just one we conservatively
            // read as legacy (bit 0 unset).
            let flags = std::str::from_utf8(query_params)
                .ok()
                .and_then(|text| text.parse::<u16>().ok())
                .unwrap_or(0);
            One::Event(InputEvent::CapabilityReply(flags), consumed)
        }
        b'c' => One::Event(InputEvent::DeviceAttributes, consumed),
        _ => key_event(
            KeyEvent::plain(Key::Unknown(bytes[..consumed].to_vec())),
            consumed,
        ),
    }
}

/// Decodes an SGR mouse report (`CSI < Cb;Cx;Cy M|m`, ADR-0008). `M` is a
/// press / drag / wheel, `m` a release; `Cb` packs button (bits 0-1),
/// Shift/Alt/Ctrl (bits 2-4), drag motion (bit 5), and wheel (bit 6).
/// Anything that doesn't parse falls back to `Key::Unknown` like any other
/// unrecognized CSI — never `Incomplete`, since the final byte already
/// arrived.
fn decode_sgr_mouse(mouse_params: &[u8], final_byte: u8, bytes: &[u8], consumed: usize) -> One {
    let unknown = || {
        key_event(
            KeyEvent::plain(Key::Unknown(bytes[..consumed].to_vec())),
            consumed,
        )
    };

    if final_byte != b'M' && final_byte != b'm' {
        return unknown();
    }
    let Some(values) = std::str::from_utf8(mouse_params)
        .ok()
        .and_then(parse_semicolon_numbers)
    else {
        return unknown();
    };
    let [code, column, row] = values[..] else {
        return unknown();
    };

    let mut modifiers = Modifiers::default();
    if code & 4 != 0 {
        modifiers = modifiers.with_shift();
    }
    if code & 8 != 0 {
        modifiers = modifiers.with_alt();
    }
    if code & 16 != 0 {
        modifiers = modifiers.with_ctrl();
    }

    let kind = if code & 64 != 0 {
        match code & 3 {
            0 => MouseEventKind::WheelUp,
            1 => MouseEventKind::WheelDown,
            // Horizontal wheel (66/67): not part of the ADR-0008 scope.
            _ => return unknown(),
        }
    } else {
        let button = match code & 3 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            // 3 = "no button": only DECSET 1003 (any-motion) reports this,
            // which we never enable.
            _ => return unknown(),
        };
        if final_byte == b'm' {
            MouseEventKind::Release(button)
        } else if code & 32 != 0 {
            MouseEventKind::Drag(button)
        } else {
            MouseEventKind::Press(button)
        }
    };

    One::Event(
        InputEvent::Mouse(MouseEvent {
            kind,
            modifiers,
            column,
            row,
        }),
        consumed,
    )
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

    key_event(KeyEvent::plain(key), 3)
}

fn decode_kitty_csi_u(params: &[u8], bytes: &[u8], consumed: usize) -> One {
    let Some(values) = std::str::from_utf8(params)
        .ok()
        .and_then(parse_semicolon_numbers)
    else {
        return key_event(
            KeyEvent::plain(Key::Unknown(bytes[..consumed].to_vec())),
            consumed,
        );
    };

    let Some(codepoint) = values.first().copied() else {
        return key_event(
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

    key_event(KeyEvent::new(key, modifiers), consumed)
}

fn decode_utf8_or_unknown(bytes: &[u8]) -> One {
    for width in 2..=4 {
        if bytes.len() < width {
            return One::Incomplete;
        }
        if let Ok(text) = std::str::from_utf8(&bytes[..width])
            && let Some(character) = text.chars().next()
        {
            return key_event(KeyEvent::plain(Key::Char(character)), width);
        }
    }

    key_event(KeyEvent::plain(Key::Unknown(vec![bytes[0]])), 1)
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
    use super::{
        DecodeResult, InputEvent, decode_key_events, drain_input_events, drain_key_events,
        flush_pending_escape,
    };
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
            (
                // A `?`-prefixed CSI we did not ask for (not a `u` or `c`
                // reply to our own queries) must fall back to Unknown exactly
                // like before capability replies existed (TASK-260712-16).
                "unparseable ? CSI stays Key::Unknown",
                b"\x1b[?25h",
                complete([KeyEvent::plain(Key::Unknown(b"\x1b[?25h".to_vec()))]),
            ),
        ];

        for (name, input, expected) in cases {
            assert_eq!(decode_key_events(input), *expected, "{name}");
        }
    }

    /// Table-driven per TASK-260712-16 testcases: our startup `CSI ?u` /
    /// `CSI c` queries must decode to `InputEvent::CapabilityReply` /
    /// `InputEvent::DeviceAttributes`, mix cleanly with surrounding key
    /// events in the same chunk, and never surface through
    /// `drain_key_events` (covered separately below).
    #[test]
    fn decode_capability_query_replies_table_driven() {
        let cases: &[(&str, &[u8], Vec<InputEvent>)] = &[
            (
                "kitty flags reply, disambiguate bit set",
                b"\x1b[?1u",
                vec![InputEvent::CapabilityReply(1)],
            ),
            (
                "kitty flags reply, no flags set",
                b"\x1b[?0u",
                vec![InputEvent::CapabilityReply(0)],
            ),
            (
                "DA1 reply",
                b"\x1b[?62;22c",
                vec![InputEvent::DeviceAttributes],
            ),
            (
                "capability reply mixed with a following key in one chunk",
                b"\x1b[?1ua",
                vec![
                    InputEvent::CapabilityReply(1),
                    InputEvent::Key(KeyEvent::plain(Key::Char('a'))),
                ],
            ),
        ];

        for (name, input, expected) in cases {
            let mut buffer = input.to_vec();
            assert_eq!(&drain_input_events(&mut buffer), expected, "{name}");
            assert!(buffer.is_empty(), "{name}: buffer not fully drained");
        }
    }

    /// Table-driven SGR mouse decode (ADR-0008): press / drag / release /
    /// wheel / modifier bits, plus malformed reports falling back to
    /// `Key::Unknown` instead of stalling or leaking into the key channel.
    #[test]
    fn decode_sgr_mouse_reports_table_driven() {
        use crate::input::mouse::{MouseButton, MouseEvent, MouseEventKind};

        fn mouse(kind: MouseEventKind, modifiers: Modifiers, column: u16, row: u16) -> InputEvent {
            InputEvent::Mouse(MouseEvent {
                kind,
                modifiers,
                column,
                row,
            })
        }

        let cases: &[(&str, &[u8], Vec<InputEvent>)] = &[
            (
                "left press",
                b"\x1b[<0;10;5M",
                vec![mouse(
                    MouseEventKind::Press(MouseButton::Left),
                    Modifiers::default(),
                    10,
                    5,
                )],
            ),
            (
                "left drag (motion bit 32)",
                b"\x1b[<32;11;5M",
                vec![mouse(
                    MouseEventKind::Drag(MouseButton::Left),
                    Modifiers::default(),
                    11,
                    5,
                )],
            ),
            (
                "left release (final byte m)",
                b"\x1b[<0;11;5m",
                vec![mouse(
                    MouseEventKind::Release(MouseButton::Left),
                    Modifiers::default(),
                    11,
                    5,
                )],
            ),
            (
                "right press",
                b"\x1b[<2;3;4M",
                vec![mouse(
                    MouseEventKind::Press(MouseButton::Right),
                    Modifiers::default(),
                    3,
                    4,
                )],
            ),
            (
                "wheel up / wheel down",
                b"\x1b[<64;1;1M\x1b[<65;1;1M",
                vec![
                    mouse(MouseEventKind::WheelUp, Modifiers::default(), 1, 1),
                    mouse(MouseEventKind::WheelDown, Modifiers::default(), 1, 1),
                ],
            ),
            (
                "shift+drag carries the shift modifier for terminal passthrough",
                b"\x1b[<36;7;8M",
                vec![mouse(
                    MouseEventKind::Drag(MouseButton::Left),
                    Modifiers::shift(),
                    7,
                    8,
                )],
            ),
            (
                "ctrl+click carries the ctrl modifier",
                b"\x1b[<16;2;2M",
                vec![mouse(
                    MouseEventKind::Press(MouseButton::Left),
                    Modifiers::ctrl(),
                    2,
                    2,
                )],
            ),
            (
                "mouse report mixed with a following key in one chunk",
                b"\x1b[<0;1;1Ma",
                vec![
                    mouse(
                        MouseEventKind::Press(MouseButton::Left),
                        Modifiers::default(),
                        1,
                        1,
                    ),
                    InputEvent::Key(KeyEvent::plain(Key::Char('a'))),
                ],
            ),
            (
                "malformed params fall back to Unknown",
                b"\x1b[<0;1M",
                vec![InputEvent::Key(KeyEvent::plain(Key::Unknown(
                    b"\x1b[<0;1M".to_vec(),
                )))],
            ),
            (
                "horizontal wheel is out of scope and falls back to Unknown",
                b"\x1b[<66;1;1M",
                vec![InputEvent::Key(KeyEvent::plain(Key::Unknown(
                    b"\x1b[<66;1;1M".to_vec(),
                )))],
            ),
        ];

        for (name, input, expected) in cases {
            let mut buffer = input.to_vec();
            assert_eq!(&drain_input_events(&mut buffer), expected, "{name}");
            assert!(buffer.is_empty(), "{name}: buffer not fully drained");
        }
    }

    #[test]
    fn split_sgr_mouse_report_waits_for_the_final_byte() {
        use crate::input::mouse::{MouseButton, MouseEvent, MouseEventKind};

        let mut buffer = b"\x1b[<0;5;5".to_vec();
        assert_eq!(drain_input_events(&mut buffer), Vec::new());
        assert_eq!(
            buffer, b"\x1b[<0;5;5",
            "incomplete prefix must stay buffered"
        );

        buffer.extend_from_slice(b"M");
        assert_eq!(
            drain_input_events(&mut buffer),
            vec![InputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Press(MouseButton::Left),
                modifiers: Modifiers::default(),
                column: 5,
                row: 5,
            })]
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn drain_key_events_does_not_leak_mouse_reports() {
        let mut buffer = b"\x1b[<0;10;5M\x1b[<32;11;5Ma".to_vec();
        assert_eq!(
            drain_key_events(&mut buffer),
            vec![KeyEvent::plain(Key::Char('a'))],
            "only the trailing 'a' is a real keystroke"
        );
    }

    #[test]
    fn drain_key_events_does_not_leak_capability_or_device_attribute_replies() {
        let mut buffer = b"\x1b[?1u\x1b[?62;22ca".to_vec();
        assert_eq!(
            drain_key_events(&mut buffer),
            vec![KeyEvent::plain(Key::Char('a'))],
            "only the trailing 'a' is a real keystroke"
        );
    }

    #[test]
    fn split_capability_reply_arrival_waits_for_the_final_byte() {
        let mut buffer = b"\x1b[?1".to_vec();
        assert_eq!(drain_input_events(&mut buffer), Vec::new());
        assert_eq!(buffer, b"\x1b[?1", "incomplete prefix must stay buffered");

        buffer.extend_from_slice(b"u");
        assert_eq!(
            drain_input_events(&mut buffer),
            vec![InputEvent::CapabilityReply(1)]
        );
        assert!(buffer.is_empty());
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

    #[test]
    fn bracketed_paste_envelope_cases_are_stream_safe() {
        let cases = [
            (
                "single chunk",
                vec![b"\x1b[200~hello\x1b[201~".to_vec()],
                vec![InputEvent::Paste("hello".to_string())],
            ),
            (
                "split into envelope body and end",
                vec![
                    b"\x1b[200~".to_vec(),
                    b"hello".to_vec(),
                    b"\x1b[201~".to_vec(),
                ],
                vec![InputEvent::Paste("hello".to_string())],
            ),
            (
                "escape sequence remains pasted text",
                vec![b"\x1b[200~a\x1b[Ab\x1b[201~".to_vec()],
                vec![InputEvent::Paste("a\x1b[Ab".to_string())],
            ),
            (
                "crlf and cr normalize to lf",
                vec![b"\x1b[200~a\r\nb\rc\x1b[201~".to_vec()],
                vec![InputEvent::Paste("a\nb\nc".to_string())],
            ),
        ];

        for (name, chunks, expected) in cases {
            let mut buffer = Vec::new();
            let mut actual = Vec::new();
            for chunk in chunks {
                buffer.extend_from_slice(&chunk);
                actual.extend(drain_input_events(&mut buffer));
            }
            assert_eq!(actual, expected, "case {name}");
            assert!(buffer.is_empty(), "case {name}");
        }
    }

    #[test]
    fn bracketed_paste_keeps_incomplete_envelope_buffered() {
        let mut buffer = b"\x1b[200~hello".to_vec();
        assert_eq!(drain_input_events(&mut buffer), Vec::new());
        assert_eq!(buffer, b"\x1b[200~hello");

        buffer.extend_from_slice(b"\x1b[201~");
        assert_eq!(
            drain_input_events(&mut buffer),
            vec![InputEvent::Paste("hello".to_string())]
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn keys_around_paste_remain_key_events() {
        let mut buffer = b"a\x1b[200~\x1b[A\x1b[201~b".to_vec();
        assert_eq!(
            drain_input_events(&mut buffer),
            vec![
                InputEvent::Key(KeyEvent::plain(Key::Char('a'))),
                InputEvent::Paste("\x1b[A".to_string()),
                InputEvent::Key(KeyEvent::plain(Key::Char('b'))),
            ]
        );
        assert!(buffer.is_empty());
    }

    fn complete<const N: usize>(events: [KeyEvent; N]) -> DecodeResult {
        DecodeResult::Complete(events.into())
    }
}
