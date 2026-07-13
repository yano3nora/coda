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
    /// At least one win32-input-mode sequence (`CSI ..._`, TASK-260713) was
    /// seen in this chunk. Emitted once per drained chunk as capability
    /// evidence: the terminal honored our `CSI ?9001h` request, so modifier
    /// fidelity is available. Not a keystroke.
    Win32InputMode,
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
            | InputEvent::Mouse(_)
            | InputEvent::Win32InputMode => None,
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

    // win32-input-mode pre-pass (TASK-260713): conhost/ConPTY may deliver
    // query responses as injected key events. Unwrap them back into plain
    // bytes BEFORE ordinary decoding so a wrapped `CSI ?...c` reassembles
    // into the DA1 reply it actually is. Seeing any win32 sequence at all is
    // capability evidence, surfaced as one non-key event per chunk.
    if unwrap_win32_injected(buffer) {
        events.push(InputEvent::Win32InputMode);
    }

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
            // win32 key-repeat: the same keystroke `count` times (Rc field).
            One::Repeated(event, count, consumed) => {
                for _ in 0..count {
                    events.push(event.clone());
                }
                offset += consumed;
            }
            // Complete but eventless input (win32 key-up, modifier-only key).
            One::Skip(consumed) => offset += consumed,
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
    /// One key event repeated N times in `usize` consumed bytes (win32 Rc).
    Repeated(InputEvent, u16, usize),
    /// Complete input that produces no event (win32 key-up / modifier-only).
    Skip(usize),
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
            One::Repeated(event, count, consumed) => One::Repeated(event, count, consumed + 1),
            One::Skip(consumed) => One::Skip(consumed + 1),
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

            // win32-input-mode key event (`CSI Vk;Sc;Uc;Kd;Cs;Rc _`,
            // TASK-260713). Sent by Windows Terminal after `CSI ?9001h`.
            if final_byte == b'_' {
                return decode_win32_key(params, bytes, consumed);
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
            2 => MouseEventKind::WheelLeft,
            3 => MouseEventKind::WheelRight,
            _ => unreachable!("masked to two bits"),
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

/// Parsed fields of a win32-input-mode sequence (`CSI Vk;Sc;Uc;Kd;Cs;Rc _`,
/// microsoft/terminal spec #4999). `Sc` (scan code) is layout hardware detail
/// the normalized event never needs, so it is parsed but dropped.
struct Win32KeyParams {
    /// Virtual key code (`wVirtualKeyCode`). 0 marks an injected event: the
    /// console host writing a query response into the input stream.
    vk: u16,
    /// UTF-16 code unit of the character (`UnicodeChar`), 0 for non-character
    /// keys. Astral-plane characters arrive as a surrogate pair split across
    /// two consecutive sequences.
    uc: u16,
    key_down: bool,
    /// `dwControlKeyState` bitmask.
    control: u32,
    repeat: u16,
}

const WIN32_RIGHT_ALT: u32 = 0x0001;
const WIN32_LEFT_ALT: u32 = 0x0002;
const WIN32_RIGHT_CTRL: u32 = 0x0004;
const WIN32_LEFT_CTRL: u32 = 0x0008;
const WIN32_SHIFT: u32 = 0x0010;

/// Upper bound for honoring the win32 repeat count. Real coalesced key
/// repeat is small; a damaged sequence claiming 65535 repeats must not flood
/// the editor with phantom keystrokes.
const WIN32_REPEAT_CAP: u16 = 64;

impl Win32KeyParams {
    /// All params are optional and empty params keep their default (0,
    /// except repeat which defaults to 1), per the win32-input-mode spec.
    fn parse(params: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(params).ok()?;
        let mut values: [u32; 6] = [0, 0, 0, 0, 0, 1];
        if !text.is_empty() {
            for (index, part) in text.split(';').enumerate() {
                if index >= values.len() {
                    return None;
                }
                if !part.is_empty() {
                    values[index] = part.parse::<u32>().ok()?;
                }
            }
        }
        Some(Self {
            vk: u16::try_from(values[0]).ok()?,
            uc: u16::try_from(values[2]).ok()?,
            key_down: values[3] != 0,
            control: values[4],
            repeat: u16::try_from(values[5]).ok()?,
        })
    }

    fn modifiers(&self) -> Modifiers {
        let mut modifiers = Modifiers::none();
        if self.control & (WIN32_RIGHT_CTRL | WIN32_LEFT_CTRL) != 0 {
            modifiers = modifiers.with_ctrl();
        }
        if self.control & (WIN32_RIGHT_ALT | WIN32_LEFT_ALT) != 0 {
            modifiers = modifiers.with_alt();
        }
        if self.control & WIN32_SHIFT != 0 {
            modifiers = modifiers.with_shift();
        }
        modifiers
    }

    /// AltGr arrives as RightAlt+LeftCtrl on Windows. A character typed via
    /// AltGr (e.g. `@` on many non-US layouts) is text input, not a
    /// Ctrl+Alt chord — treating it as one would make those characters
    /// untypeable.
    fn is_alt_gr(&self) -> bool {
        self.control & (WIN32_RIGHT_ALT | WIN32_LEFT_CTRL) == (WIN32_RIGHT_ALT | WIN32_LEFT_CTRL)
    }

    /// The console host injects query responses (DA1 etc.) into the input
    /// stream as key events with no virtual key. Their `uc` values are the
    /// response's characters, one event per character. Surrogate halves are
    /// excluded: `vk=0` also occurs for IME/SendInput text, and an astral
    /// character split across two events must reach the key path's surrogate
    /// pairing instead of being spliced away (Codex review 260713).
    fn is_injected_text(&self) -> bool {
        self.vk == 0
            && self.key_down
            && self.uc != 0
            && !is_high_surrogate(self.uc)
            && !is_low_surrogate(self.uc)
    }
}

fn is_high_surrogate(unit: u16) -> bool {
    (0xd800..=0xdbff).contains(&unit)
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xdc00..=0xdfff).contains(&unit)
}

fn combine_surrogates(high: u16, low: u16) -> Option<char> {
    let code = 0x10000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
    char::from_u32(code)
}

/// Virtual keys that never produce a key event on their own: pressing and
/// releasing a modifier or lock key is not a keystroke to bind.
fn is_modifier_only_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12          // VK_SHIFT / VK_CONTROL / VK_MENU
            | 0x14                  // VK_CAPITAL
            | 0x5b | 0x5c           // VK_LWIN / VK_RWIN
            | 0x90 | 0x91           // VK_NUMLOCK / VK_SCROLL
            | 0xa0..=0xa5 // VK_LSHIFT..VK_RMENU
    )
}

/// Non-character virtual keys with a dedicated `Key` variant. Checked before
/// the character path because Ctrl mangles their `uc` (Ctrl+Enter arrives
/// with uc=0x0a) while `vk` stays stable — this is exactly the fidelity
/// kitty CSI u provides on unix (supports_ctrl_enter etc., SPEC-0003).
fn named_vk_key(vk: u16) -> Option<Key> {
    Some(match vk {
        0x08 => Key::Backspace,
        0x09 => Key::Tab,
        0x0d => Key::Enter,
        0x1b => Key::Esc,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x23 => Key::End,
        0x24 => Key::Home,
        0x25 => Key::Left,
        0x26 => Key::Up,
        0x27 => Key::Right,
        0x28 => Key::Down,
        0x2e => Key::Delete,
        0x70..=0x7b => Key::F((vk - 0x6f) as u8),
        _ => return None,
    })
}

fn decode_win32_key(params: &[u8], bytes: &[u8], consumed: usize) -> One {
    let unknown = || {
        key_event(
            KeyEvent::plain(Key::Unknown(bytes[..consumed].to_vec())),
            consumed,
        )
    };
    let Some(win32) = Win32KeyParams::parse(params) else {
        return unknown();
    };

    // Key-up and bare modifier presses are stream noise, not keystrokes.
    if !win32.key_down || is_modifier_only_vk(win32.vk) {
        return One::Skip(consumed);
    }
    // Injected response text is normally unwrapped by the pre-pass in
    // `drain_input_events`; a stray remnant here is still not a keystroke.
    if win32.vk == 0 && win32.uc == 0 {
        return One::Skip(consumed);
    }

    if let Some(key) = named_vk_key(win32.vk) {
        return repeated_key(
            KeyEvent::new(key, win32.modifiers()),
            win32.repeat,
            consumed,
        );
    }

    // Surrogate pair: the low half arrives as the immediately following
    // win32 sequence. Wait for it rather than emitting half a character.
    if is_high_surrogate(win32.uc) {
        return match next_win32_sequence(&bytes[consumed..]) {
            NextWin32::Incomplete => One::Incomplete,
            NextWin32::Complete(next, next_len) => {
                match (next.key_down, combine_surrogates(win32.uc, next.uc)) {
                    (true, Some(character)) if is_low_surrogate(next.uc) => repeated_key(
                        KeyEvent::new(Key::Char(character), win32.modifiers()),
                        win32.repeat,
                        consumed + next_len,
                    ),
                    // Unpaired high surrogate: drop it, leave the follower
                    // for the main loop to judge on its own.
                    _ => One::Skip(consumed),
                }
            }
            NextWin32::NotWin32 => One::Skip(consumed),
        };
    }
    if is_low_surrogate(win32.uc) {
        return One::Skip(consumed);
    }

    let character = char::from_u32(u32::from(win32.uc)).filter(|c| !c.is_control());
    let event = match character {
        Some(character) => {
            // Same normalization as kitty CSI u: letters are stored
            // lowercase with Shift kept as a modifier. AltGr characters are
            // plain text input (see is_alt_gr).
            let modifiers = if win32.is_alt_gr() {
                Modifiers::none()
            } else {
                win32.modifiers()
            };
            KeyEvent::new(Key::Char(character.to_ascii_lowercase()), modifiers)
        }
        None => {
            // Ctrl mangled `uc` into a control byte (Ctrl+A → 0x01) or the
            // key carries no character at all. Recover letters/digits/space
            // from the virtual key; anything else stays explainable as
            // Unknown for `:inspect-key`.
            let recovered = match win32.vk {
                0x20 => Some(' '),
                0x30..=0x39 => char::from_u32(u32::from(win32.vk)),
                0x41..=0x5a => char::from_u32(u32::from(win32.vk) + 0x20),
                _ => None,
            };
            match recovered {
                Some(character) => KeyEvent::new(Key::Char(character), win32.modifiers()),
                None => return unknown(),
            }
        }
    };
    repeated_key(event, win32.repeat, consumed)
}

fn repeated_key(event: KeyEvent, repeat: u16, consumed: usize) -> One {
    let count = repeat.clamp(1, WIN32_REPEAT_CAP);
    if count == 1 {
        key_event(event, consumed)
    } else {
        One::Repeated(InputEvent::Key(event), count, consumed)
    }
}

enum NextWin32 {
    Complete(Win32KeyParams, usize),
    Incomplete,
    NotWin32,
}

/// Peeks the next complete win32 sequence at the start of `bytes` (surrogate
/// pairing). `Incomplete` when the follower may still be arriving.
fn next_win32_sequence(bytes: &[u8]) -> NextWin32 {
    if bytes.is_empty() || (bytes.len() == 1 && bytes[0] == 0x1b) {
        return NextWin32::Incomplete;
    }
    if !bytes.starts_with(b"\x1b[") {
        return NextWin32::NotWin32;
    }
    let Some(final_index) = bytes
        .iter()
        .enumerate()
        .skip(2)
        .find_map(|(index, byte)| (0x40..=0x7e).contains(byte).then_some(index))
    else {
        return NextWin32::Incomplete;
    };
    if bytes[final_index] != b'_' {
        return NextWin32::NotWin32;
    }
    match Win32KeyParams::parse(&bytes[2..final_index]) {
        Some(params) => NextWin32::Complete(params, final_index + 1),
        None => NextWin32::NotWin32,
    }
}

/// Rewrites console-host-injected win32 events (query responses wrapped as
/// `vk=0` key events, one per character) back into the plain bytes they
/// carry, so a wrapped DA1 reply reassembles into `CSI ?...c` and decodes
/// through the ordinary paths. Returns whether ANY complete win32 sequence
/// (injected or real) was seen — the capability evidence for
/// `InputEvent::Win32InputMode`. Bracketed-paste envelopes are skipped
/// verbatim: their content is literal text that must never be rewritten.
fn unwrap_win32_injected(buffer: &mut Vec<u8>) -> bool {
    let mut saw_win32 = false;
    let mut index = 0;

    while index < buffer.len() {
        let remaining = &buffer[index..];
        if remaining.starts_with(BRACKETED_PASTE_START) {
            let content_start = index + BRACKETED_PASTE_START.len();
            match find_subslice(&buffer[content_start..], BRACKETED_PASTE_END) {
                Some(relative_end) => {
                    index = content_start + relative_end + BRACKETED_PASTE_END.len();
                    continue;
                }
                // Incomplete paste envelope: everything after it is paste
                // content still in flight; nothing left to unwrap this chunk.
                None => break,
            }
        }
        if remaining.len() >= 2 && remaining[0] == 0x1b && remaining[1] == b'[' {
            let Some(final_index) = remaining
                .iter()
                .enumerate()
                .skip(2)
                .find_map(|(offset, byte)| (0x40..=0x7e).contains(byte).then_some(offset))
            else {
                // Incomplete CSI at the tail: keep for the next chunk.
                break;
            };
            let sequence_len = final_index + 1;
            if remaining[final_index] == b'_'
                && let Some(params) = Win32KeyParams::parse(&remaining[2..final_index])
            {
                // Only a parseable sequence counts as capability evidence: a
                // malformed `_`-final CSI must not upgrade the terminal to
                // "full fidelity" (Codex review 260713).
                saw_win32 = true;
                if params.is_injected_text() {
                    // Injected units are response text (surrogate halves are
                    // excluded by is_injected_text and take the key path's
                    // surrogate pairing instead).
                    let replacement: Vec<u8> = char::from_u32(u32::from(params.uc))
                        .map(|character| {
                            let mut buf = [0_u8; 4];
                            let encoded = character.encode_utf8(&mut buf).as_bytes().to_vec();
                            let count = usize::from(params.repeat.clamp(1, WIN32_REPEAT_CAP));
                            encoded.repeat(count)
                        })
                        .unwrap_or_default();
                    let replacement_len = replacement.len();
                    buffer.splice(index..index + sequence_len, replacement);
                    index += replacement_len;
                    continue;
                }
            }
            index += sequence_len;
            continue;
        }
        index += 1;
    }

    saw_win32
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
                "horizontal wheel left / right",
                b"\x1b[<66;1;1M\x1b[<67;1;1M",
                vec![
                    mouse(MouseEventKind::WheelLeft, Modifiers::default(), 1, 1),
                    mouse(MouseEventKind::WheelRight, Modifiers::default(), 1, 1),
                ],
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

    /// Table-driven win32-input-mode decode (TASK-260713): `CSI ..._` key
    /// events from Windows Terminal after `CSI ?9001h`. Every chunk that
    /// contains a win32 sequence also announces `Win32InputMode` once as
    /// capability evidence.
    #[test]
    fn decode_win32_input_mode_table_driven() {
        fn key(event: KeyEvent) -> InputEvent {
            InputEvent::Key(event)
        }

        let cases: &[(&str, &[u8], Vec<InputEvent>)] = &[
            (
                "plain letter (numlock bit ignored)",
                b"\x1b[65;30;97;1;32;1_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::plain(Key::Char('a'))),
                ],
            ),
            (
                "key-up produces no keystroke",
                b"\x1b[65;30;97;0;32;1_",
                vec![InputEvent::Win32InputMode],
            ),
            (
                "Ctrl+Shift+J (uc mangled to 0x0a, letter recovered from vk)",
                b"\x1b[74;36;10;1;24;1_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::new(
                        Key::Char('j'),
                        Modifiers::ctrl().with_shift(),
                    )),
                ],
            ),
            (
                "Ctrl+I stays a letter, distinguished from Tab",
                b"\x1b[73;23;9;1;8;1_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::new(Key::Char('i'), Modifiers::ctrl())),
                ],
            ),
            (
                "Tab itself resolves via the named vk table",
                b"\x1b[9;15;9;1;0;1_",
                vec![InputEvent::Win32InputMode, key(KeyEvent::plain(Key::Tab))],
            ),
            (
                "Ctrl+Enter (vk wins over the mangled uc)",
                b"\x1b[13;28;10;1;8;1_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::new(Key::Enter, Modifiers::ctrl())),
                ],
            ),
            (
                "Shift+Enter",
                b"\x1b[13;28;13;1;16;1_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::new(Key::Enter, Modifiers::shift())),
                ],
            ),
            (
                "AltGr character is text input, not a Ctrl+Alt chord",
                b"\x1b[81;16;64;1;9;1_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::plain(Key::Char('@'))),
                ],
            ),
            (
                "modifier-only press (Shift down) produces no keystroke",
                b"\x1b[16;42;0;1;16;1_",
                vec![InputEvent::Win32InputMode],
            ),
            (
                "repeat count replays the keystroke",
                b"\x1b[65;30;97;1;0;3_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::plain(Key::Char('a'))),
                    key(KeyEvent::plain(Key::Char('a'))),
                    key(KeyEvent::plain(Key::Char('a'))),
                ],
            ),
            (
                "arrow key via vk (uc is 0)",
                b"\x1b[38;72;0;1;0;1_",
                vec![InputEvent::Win32InputMode, key(KeyEvent::plain(Key::Up))],
            ),
            (
                "function key via vk",
                b"\x1b[116;63;0;1;0;1_",
                vec![InputEvent::Win32InputMode, key(KeyEvent::plain(Key::F(5)))],
            ),
            (
                "surrogate pair combines into one astral character",
                b"\x1b[231;0;55357;1;0;1_\x1b[231;0;56832;1;0;1_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::plain(Key::Char('\u{1f600}'))),
                ],
            ),
            (
                "vk=0 surrogate pair (IME-injected text) is NOT unwrapped away",
                b"\x1b[0;0;55357;1;0;1_\x1b[0;0;56832;1;0;1_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::plain(Key::Char('\u{1f600}'))),
                ],
            ),
            (
                "key without character or named vk stays explainable",
                b"\x1b[45;82;0;1;0;1_",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::plain(Key::Unknown(
                        b"\x1b[45;82;0;1;0;1_".to_vec(),
                    ))),
                ],
            ),
            (
                "empty params are all defaults (key-up), not an error",
                b"\x1b[_",
                vec![InputEvent::Win32InputMode],
            ),
            (
                "injected DA1 reply unwraps back into DeviceAttributes",
                b"\x1b[0;0;27;1;0;1_\x1b[0;0;91;1;0;1_\x1b[0;0;63;1;0;1_\x1b[0;0;54;1;0;1_\x1b[0;0;50;1;0;1_\x1b[0;0;99;1;0;1_",
                vec![InputEvent::Win32InputMode, InputEvent::DeviceAttributes],
            ),
            (
                "win32 key mixed with a plain key in one chunk",
                b"\x1b[65;30;97;1;0;1_b",
                vec![
                    InputEvent::Win32InputMode,
                    key(KeyEvent::plain(Key::Char('a'))),
                    key(KeyEvent::plain(Key::Char('b'))),
                ],
            ),
        ];

        for (name, input, expected) in cases {
            let mut buffer = input.to_vec();
            assert_eq!(&drain_input_events(&mut buffer), expected, "{name}");
            assert!(buffer.is_empty(), "{name}: buffer not fully drained");
        }
    }

    #[test]
    fn win32_high_surrogate_waits_for_its_low_half() {
        let mut buffer = b"\x1b[231;0;55357;1;0;1_".to_vec();
        assert_eq!(
            drain_input_events(&mut buffer),
            vec![InputEvent::Win32InputMode],
            "high half alone announces the protocol but emits no key"
        );
        assert_eq!(
            buffer, b"\x1b[231;0;55357;1;0;1_",
            "high half stays buffered until its pair arrives"
        );

        buffer.extend_from_slice(b"\x1b[231;0;56832;1;0;1_");
        assert_eq!(
            drain_input_events(&mut buffer),
            vec![
                InputEvent::Win32InputMode,
                InputEvent::Key(KeyEvent::plain(Key::Char('\u{1f600}'))),
            ]
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn win32_repeat_count_is_capped_against_damaged_sequences() {
        let mut buffer = b"\x1b[65;30;97;1;0;65535_".to_vec();
        let events = drain_input_events(&mut buffer);
        // 1 protocol announcement + at most the documented cap of keystrokes.
        assert_eq!(events.len(), 1 + 64);
    }

    #[test]
    fn win32_injected_bytes_inside_paste_envelope_stay_literal() {
        // A paste that happens to contain a win32-looking sequence must not
        // be rewritten by the unwrap pre-pass.
        let mut buffer = b"\x1b[200~\x1b[0;0;65;1;0;1_\x1b[201~".to_vec();
        assert_eq!(
            drain_input_events(&mut buffer),
            vec![InputEvent::Paste("\x1b[0;0;65;1;0;1_".to_string())],
            "paste content is literal even when it looks like win32 input"
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
