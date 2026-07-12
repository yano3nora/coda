//! Runtime query of Ghostty's own keybind table (ADR-0007 decision 2(b)).
//!
//! Ghostty does not negotiate which key combinations it reserves — it just
//! consumes or rewrites bytes before they ever reach this program's stdin.
//! `ghostty +list-keybinds` is the one channel that exposes that table (it
//! includes the *user's actual* config, not a static default), so this module
//! shells out to it and turns the output into structured [`TerminalQuirk`]
//! values the app layer can cross-reference against its own bindings.
//!
//! Two design choices matter for correctness and safety:
//!
//! - **Only `super`-modified triggers, or triggers whose action is a
//!   `text:`/`esc:` byte translation, are reported.** Ghostty's `shift`/
//!   `ctrl`-only bindings are mostly "performable" (only fire when the
//!   terminal has an active selection, otherwise transparent), so treating
//!   them as quirks would produce a steady stream of false positives. `super`
//!   combos are consumed unconditionally, and `text:`/`esc:` rewrites arrive
//!   as a *different* keystroke than the one the user pressed, so both are
//!   worth surfacing.
//! - **Any failure — missing binary, non-zero exit, non-UTF-8 output,
//!   unparsable lines — is swallowed and yields an empty `Vec`.** Detecting
//!   quirks is a nice-to-have warning, never a startup precondition (AGENTS.md
//!   failure-mode principle: the app must always start).
//!
//! [`parse_ghostty_keybinds`] is a pure function over the CLI's stdout so it
//! can be fixture-tested without shelling out; [`detect`] is the thin,
//! environment-dependent wrapper that decides whether to run the subprocess
//! at all.

use std::process::Command;

use super::{DecodeResult, Key, KeyEvent, Modifiers, decode_key_events};

/// What happens to a trigger keystroke once Ghostty gets it first.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum QuirkEffect {
    /// Ghostty performs `action` itself; the keystroke never reaches this
    /// program's stdin at all.
    Consumed { action: String },
    /// Ghostty rewrites the keystroke into different bytes before delivery.
    /// `events` is the normalized decode of those bytes (empty if the
    /// decoder judged them an incomplete sequence); `raw` is always populated.
    Translated { events: Vec<KeyEvent>, raw: Vec<u8> },
}

/// One row of Ghostty's keybind table that is relevant to interception.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TerminalQuirk {
    /// The chord the user actually presses (e.g. `super+left`).
    pub trigger: KeyEvent,
    pub effect: QuirkEffect,
    /// The original `<trigger>=<action>` text, kept for warning/diagnostic
    /// messages (e.g. `"super+arrow_left=text:\\x01"`).
    pub source_line: String,
}

/// Detects Ghostty keybind quirks for the current process environment.
///
/// Only runs `ghostty +list-keybinds` when `TERM_PROGRAM` says we are inside
/// Ghostty. Any failure along the way (binary missing, non-zero exit,
/// non-UTF-8 stdout) silently yields an empty `Vec` — this must never block
/// or delay startup.
pub fn detect() -> Vec<TerminalQuirk> {
    let is_ghostty = std::env::var("TERM_PROGRAM")
        .map(|value| value.eq_ignore_ascii_case("ghostty"))
        .unwrap_or(false);
    if !is_ghostty {
        return Vec::new();
    }

    run_list_keybinds()
        .map(|output| parse_ghostty_keybinds(&output))
        .unwrap_or_default()
}

/// Runs `ghostty +list-keybinds` and returns its stdout, or `None` on any
/// failure (binary not found, non-zero exit, non-UTF-8 output). stderr is
/// discarded: a warning about Ghostty's own CLI is not this program's job.
fn run_list_keybinds() -> Option<String> {
    let output = Command::new("ghostty")
        .arg("+list-keybinds")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Parses `ghostty +list-keybinds` output into quirks.
///
/// Pure function: no environment or process access, so it is exercised with
/// fixture text captured from real Ghostty output. Lines that are not
/// `keybind = <trigger>=<action>`, that use unsupported trigger syntax
/// (chord sequences with `>`, `physical:` prefixes, unknown modifier/key
/// tokens), that are `unbind`, or that do not meet the adoption rule (see
/// module docs) are skipped rather than erroring.
pub fn parse_ghostty_keybinds(output: &str) -> Vec<TerminalQuirk> {
    output.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<TerminalQuirk> {
    let trimmed = line.trim();
    let body = trimmed.strip_prefix("keybind")?.trim_start();
    let body = body.strip_prefix('=')?.trim();
    let (trigger_raw, action_raw) = body.split_once('=')?;
    let trigger_raw = trigger_raw.trim();
    let action_raw = action_raw.trim();

    // Chord sequences (`a>b`) and physical-key triggers are out of scope for
    // MVP quirk detection; skip rather than mis-parse them.
    if trigger_raw.contains('>') || trigger_raw.starts_with("physical:") {
        return None;
    }
    // `unbind` means the trigger is transparent to us — not a quirk at all.
    if action_raw == "unbind" {
        return None;
    }

    let (modifiers, key_token) = split_trigger(trigger_raw)?;
    let key = map_key_name(key_token)?;
    let effect = classify_action(action_raw);

    let is_translation = matches!(effect, QuirkEffect::Translated { .. });
    if !modifiers.contains_super() && !is_translation {
        // Ctrl/Alt/Shift-only consuming binds are usually "performable"
        // (terminal-selection-gated) and reporting them as quirks would be a
        // false positive (see module docs).
        return None;
    }

    Some(TerminalQuirk {
        trigger: KeyEvent::new(key, modifiers),
        effect,
        source_line: format!("{trigger_raw}={action_raw}"),
    })
}

/// Splits a trigger like `super+shift+arrow_up` into its modifier set and
/// trailing key token. Returns `None` on any unrecognized modifier name so
/// the caller skips the whole line instead of guessing.
fn split_trigger(trigger: &str) -> Option<(Modifiers, &str)> {
    let mut parts: Vec<&str> = trigger.split('+').collect();
    let key_token = parts.pop()?;
    if key_token.is_empty() {
        return None;
    }

    let mut modifiers = Modifiers::none();
    for part in parts {
        modifiers = match part {
            "super" => modifiers.with_super(),
            "ctrl" => modifiers.with_ctrl(),
            "alt" => modifiers.with_alt(),
            "shift" => modifiers.with_shift(),
            _ => return None,
        };
    }
    Some((modifiers, key_token))
}

/// Maps a Ghostty key-name token to a normalized [`Key`]. Unrecognized
/// tokens return `None` so the line is skipped.
fn map_key_name(token: &str) -> Option<Key> {
    match token {
        "arrow_left" => Some(Key::Left),
        "arrow_right" => Some(Key::Right),
        "arrow_up" => Some(Key::Up),
        "arrow_down" => Some(Key::Down),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "page_up" => Some(Key::PageUp),
        "page_down" => Some(Key::PageDown),
        "enter" => Some(Key::Enter),
        "escape" => Some(Key::Esc),
        "tab" => Some(Key::Tab),
        "backspace" => Some(Key::Backspace),
        "space" => Some(Key::Char(' ')),
        _ => map_digit_or_single_char(token),
    }
}

fn map_digit_or_single_char(token: &str) -> Option<Key> {
    if let Some(digit) = token.strip_prefix("digit_") {
        let mut chars = digit.chars();
        let character = chars.next()?;
        return (chars.next().is_none() && character.is_ascii_digit())
            .then_some(Key::Char(character));
    }

    let mut chars = token.chars();
    let character = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if character.is_ascii_lowercase() || character.is_ascii_digit() {
        return Some(Key::Char(character));
    }
    if matches!(
        character,
        ',' | '.' | '=' | '+' | '-' | '[' | ']' | ';' | '\''
    ) {
        return Some(Key::Char(character));
    }
    None
}

/// Classifies a `+list-keybinds` action string into a [`QuirkEffect`].
///
/// `performable:` is stripped and treated identically to a bare consuming
/// action: whether or not it actually fires depends on terminal-selection
/// state we cannot observe here, so the conservative (warn-worthy) reading is
/// "consumed".
fn classify_action(action: &str) -> QuirkEffect {
    let action = action.strip_prefix("performable:").unwrap_or(action);

    if let Some(payload) = action.strip_prefix("text:") {
        let raw = unescape_hex_payload(payload);
        return QuirkEffect::Translated {
            events: decode_events(&raw),
            raw,
        };
    }

    if let Some(payload) = action.strip_prefix("esc:") {
        let mut raw = vec![0x1b];
        raw.extend(payload.as_bytes());
        return QuirkEffect::Translated {
            events: decode_events(&raw),
            raw,
        };
    }

    QuirkEffect::Consumed {
        action: action.to_string(),
    }
}

fn decode_events(raw: &[u8]) -> Vec<KeyEvent> {
    match decode_key_events(raw) {
        DecodeResult::Complete(events) => events,
        DecodeResult::Incomplete => Vec::new(),
    }
}

/// Unescapes hex byte sequences in a `text:` action payload into raw bytes.
/// Any other character passes through as its own UTF-8 bytes.
///
/// Both `\xNN` and `\\xNN` decode to the byte `NN`: real `+list-keybinds`
/// output escapes the backslash itself (`text:\\x01` on the wire, verified
/// with xxd against Ghostty 1.3.1), while hand-written configs use a single
/// backslash. Treating them differently is exactly the bug that made every
/// `Translated` quirk mis-decode to `[0x5c, byte]` in production.
fn unescape_hex_payload(payload: &str) -> Vec<u8> {
    let bytes = payload.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        // Digits are read byte-wise (never via str slicing): the payload
        // comes from an arbitrary user config, and slicing a &str at a
        // non-char-boundary — e.g. `\x` followed by a multibyte character —
        // panics, which would crash startup.
        if bytes[index] == b'\\' {
            // Skip a doubled backslash when it directly precedes an `xNN`
            // hex escape, so `\\x01` and `\x01` decode identically.
            let x_index = if bytes.get(index + 1) == Some(&b'\\') {
                index + 2
            } else {
                index + 1
            };
            if bytes.get(x_index) == Some(&b'x')
                && let (Some(&high_byte), Some(&low_byte)) =
                    (bytes.get(x_index + 1), bytes.get(x_index + 2))
                && let (Some(high), Some(low)) = (
                    char::from(high_byte).to_digit(16),
                    char::from(low_byte).to_digit(16),
                )
            {
                result.push((high * 16 + low) as u8);
                index = x_index + 3;
                continue;
            }
        }
        result.push(bytes[index]);
        index += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{QuirkEffect, TerminalQuirk, parse_ghostty_keybinds};
    use crate::input::{Key, KeyEvent, Modifiers};

    // Real `ghostty +list-keybinds` output escapes the backslash itself:
    // the bytes on the wire are `text:\\x05` (two literal backslashes),
    // verified with xxd against Ghostty 1.3.1. Keep the fixture faithful —
    // a single-backslash fixture once masked a decode bug here.
    const FIXTURE: &str = "\
keybind = super+arrow_right=text:\\\\x05
keybind = super+arrow_left=text:\\\\x01
keybind = alt+arrow_left=esc:b
keybind = alt+arrow_right=esc:f
keybind = super+c=copy_to_clipboard:mixed
keybind = super+a=select_all
keybind = super+f=start_search
keybind = shift+arrow_left=adjust_selection:left
keybind = super+shift+arrow_up=jump_to_prompt:-1
keybind = super+1=goto_tab:1
keybind = super+digit_1=goto_tab:1
";

    #[test]
    fn drops_non_super_non_translating_binds() {
        let quirks = parse_ghostty_keybinds(FIXTURE);
        assert!(
            quirks
                .iter()
                .all(|quirk| quirk.trigger != KeyEvent::new(Key::Left, Modifiers::shift())),
            "shift+arrow_left must not be reported (performable false-positive risk)"
        );
    }

    #[test]
    fn classifies_text_payloads_as_translated_with_decoded_events() {
        let quirks = parse_ghostty_keybinds(FIXTURE);

        let cmd_right = find(&quirks, KeyEvent::new(Key::Right, Modifiers::super_key()));
        assert_eq!(
            cmd_right.effect,
            QuirkEffect::Translated {
                events: vec![KeyEvent::new(Key::Char('e'), Modifiers::ctrl())],
                raw: vec![0x05],
            }
        );

        let cmd_left = find(&quirks, KeyEvent::new(Key::Left, Modifiers::super_key()));
        assert_eq!(
            cmd_left.effect,
            QuirkEffect::Translated {
                events: vec![KeyEvent::new(Key::Char('a'), Modifiers::ctrl())],
                raw: vec![0x01],
            }
        );
    }

    #[test]
    fn classifies_esc_payloads_as_translated_alt_events() {
        let quirks = parse_ghostty_keybinds(FIXTURE);

        let alt_left = find(&quirks, KeyEvent::new(Key::Left, Modifiers::alt()));
        assert_eq!(
            alt_left.effect,
            QuirkEffect::Translated {
                events: vec![KeyEvent::new(Key::Char('b'), Modifiers::alt())],
                raw: vec![0x1b, b'b'],
            }
        );

        let alt_right = find(&quirks, KeyEvent::new(Key::Right, Modifiers::alt()));
        assert_eq!(
            alt_right.effect,
            QuirkEffect::Translated {
                events: vec![KeyEvent::new(Key::Char('f'), Modifiers::alt())],
                raw: vec![0x1b, b'f'],
            }
        );
    }

    #[test]
    fn classifies_non_translating_super_binds_as_consumed() {
        let quirks = parse_ghostty_keybinds(FIXTURE);

        let cases = [
            (
                KeyEvent::new(Key::Char('c'), Modifiers::super_key()),
                "copy_to_clipboard:mixed",
            ),
            (
                KeyEvent::new(Key::Char('a'), Modifiers::super_key()),
                "select_all",
            ),
            (
                KeyEvent::new(Key::Char('f'), Modifiers::super_key()),
                "start_search",
            ),
            (
                KeyEvent::new(Key::Up, Modifiers::super_key().with_shift()),
                "jump_to_prompt:-1",
            ),
        ];

        for (trigger, action) in cases {
            assert_eq!(
                find(&quirks, trigger.clone()).effect,
                QuirkEffect::Consumed {
                    action: action.to_string()
                },
                "{trigger:?}"
            );
        }
    }

    #[test]
    fn digit_and_digit_underscore_key_names_both_map_to_char() {
        let quirks = parse_ghostty_keybinds(FIXTURE);
        let expected_trigger = KeyEvent::new(Key::Char('1'), Modifiers::super_key());
        let matches: Vec<_> = quirks
            .iter()
            .filter(|quirk| quirk.trigger == expected_trigger)
            .collect();
        assert_eq!(matches.len(), 2, "super+1 and super+digit_1 both parse");
        for quirk in matches {
            assert_eq!(
                quirk.effect,
                QuirkEffect::Consumed {
                    action: "goto_tab:1".to_string()
                }
            );
        }
    }

    #[test]
    fn source_line_preserves_original_trigger_and_action_text() {
        let quirks = parse_ghostty_keybinds(FIXTURE);
        let cmd_left = find(&quirks, KeyEvent::new(Key::Left, Modifiers::super_key()));
        assert_eq!(cmd_left.source_line, "super+arrow_left=text:\\\\x01");
    }

    #[test]
    fn unescape_accepts_single_and_double_backslash_hex_forms() {
        // `+list-keybinds` emits `\\xNN`; hand-written configs may use `\xNN`.
        // Both must decode to the same single byte.
        for line in [
            "keybind = super+arrow_left=text:\\x01",
            "keybind = super+arrow_left=text:\\\\x01",
        ] {
            let quirks = parse_ghostty_keybinds(line);
            assert_eq!(quirks.len(), 1, "{line}");
            assert_eq!(
                quirks[0].effect,
                QuirkEffect::Translated {
                    events: vec![KeyEvent::new(Key::Char('a'), Modifiers::ctrl())],
                    raw: vec![0x01],
                },
                "{line}"
            );
        }
    }

    #[test]
    fn total_quirk_count_excludes_only_the_performable_shift_line() {
        let quirks = parse_ghostty_keybinds(FIXTURE);
        assert_eq!(quirks.len(), 10);
    }

    #[test]
    fn unescape_survives_multibyte_text_payload_without_panicking() {
        // A broken or exotic user config must never crash startup (AGENTS.md
        // failure-mode principle). `\x` followed by a multibyte character
        // used to panic on a non-char-boundary str slice.
        let output = "keybind = super+j=text:\\xあいう\n";
        let quirks = parse_ghostty_keybinds(output);
        assert_eq!(quirks.len(), 1);
        assert!(matches!(quirks[0].effect, QuirkEffect::Translated { .. }));
    }

    #[test]
    fn unbind_and_unrecognized_lines_are_skipped_without_panicking() {
        let output = "\
keybind = super+z=unbind
keybind = physical:super+z=some_action
keybind = super+x>super+y=chord_action
not a keybind line at all
keybind = hyper+left=text:\\x01
";
        assert_eq!(parse_ghostty_keybinds(output), Vec::new());
    }

    fn find(quirks: &[TerminalQuirk], trigger: KeyEvent) -> TerminalQuirk {
        quirks
            .iter()
            .find(|quirk| quirk.trigger == trigger)
            .unwrap_or_else(|| panic!("no quirk for {trigger:?}"))
            .clone()
    }
}
