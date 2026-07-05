//! Parser for user/imported key strings such as `ctrl+shift+j`.

use std::{fmt, str::FromStr};

use crate::input::{Key, KeyEvent, Modifiers};

/// Error returned when a key chord or sequence cannot be parsed.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ParseKeyError {
    Empty,
    EmptyChord,
    UnknownToken(String),
    DuplicateKey(String),
    MissingKey(String),
}

/// Parses a whitespace-separated key sequence.
pub fn parse_key_sequence(value: &str) -> Result<Vec<KeyEvent>, ParseKeyError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ParseKeyError::Empty);
    }

    value.split_whitespace().map(parse_key_chord).collect()
}

/// Parses a single chord like `ctrl+shift+j`.
pub fn parse_key_chord(value: &str) -> Result<KeyEvent, ParseKeyError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ParseKeyError::EmptyChord);
    }

    let mut modifiers = Modifiers::none();
    let mut key = None;
    for raw_token in value.split('+') {
        let token = raw_token.trim();
        if token.is_empty() {
            return Err(ParseKeyError::UnknownToken(value.to_string()));
        }
        let normalized = token.to_ascii_lowercase();
        match normalized.as_str() {
            "ctrl" | "control" => modifiers = modifiers.with_ctrl(),
            "alt" | "opt" | "option" => modifiers = modifiers.with_alt(),
            "shift" => modifiers = modifiers.with_shift(),
            "super" | "cmd" | "win" | "meta" => modifiers = modifiers.with_super(),
            _ => {
                let parsed_key = parse_key_name(&normalized)
                    .ok_or_else(|| ParseKeyError::UnknownToken(token.to_string()))?;
                if key.replace(parsed_key).is_some() {
                    return Err(ParseKeyError::DuplicateKey(value.to_string()));
                }
            }
        }
    }

    key.map(|key| KeyEvent::new(key, modifiers))
        .ok_or_else(|| ParseKeyError::MissingKey(value.to_string()))
}

fn parse_key_name(value: &str) -> Option<Key> {
    match value {
        "enter" => Some(Key::Enter),
        "escape" | "esc" => Some(Key::Esc),
        "tab" => Some(Key::Tab),
        "space" => Some(Key::Char(' ')),
        "backspace" => Some(Key::Backspace),
        "delete" => Some(Key::Delete),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" => Some(Key::PageUp),
        "pagedown" => Some(Key::PageDown),
        _ => parse_function_key(value).or_else(|| parse_single_alnum(value)),
    }
}

fn parse_function_key(value: &str) -> Option<Key> {
    let number = value.strip_prefix('f')?.parse::<u8>().ok()?;
    (1..=12).contains(&number).then_some(Key::F(number))
}

fn parse_single_alnum(value: &str) -> Option<Key> {
    let mut chars = value.chars();
    let character = chars.next()?;
    (chars.next().is_none() && character.is_ascii_alphanumeric()).then_some(Key::Char(character))
}

impl FromStr for KeyEvent {
    type Err = ParseKeyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_key_chord(value)
    }
}

impl fmt::Display for ParseKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("empty key sequence"),
            Self::EmptyChord => formatter.write_str("empty key chord"),
            Self::UnknownToken(token) => write!(formatter, "unknown key token: {token}"),
            Self::DuplicateKey(chord) => write!(formatter, "multiple key names in chord: {chord}"),
            Self::MissingKey(chord) => write!(formatter, "missing key name in chord: {chord}"),
        }
    }
}

impl std::error::Error for ParseKeyError {}

#[cfg(test)]
mod tests {
    use super::{ParseKeyError, parse_key_chord, parse_key_sequence};
    use crate::input::{Key, KeyEvent, Modifiers};

    #[test]
    fn parses_supported_key_chords() {
        let cases = [
            (
                "ctrl+shift+j",
                KeyEvent::new(Key::Char('j'), Modifiers::ctrl().with_shift()),
            ),
            (
                "cmd+s",
                KeyEvent::new(Key::Char('s'), Modifiers::super_key()),
            ),
            ("f1", KeyEvent::plain(Key::F(1))),
            (
                "ctrl+space",
                KeyEvent::new(Key::Char(' '), Modifiers::ctrl()),
            ),
            ("CTRL+J", KeyEvent::new(Key::Char('j'), Modifiers::ctrl())),
        ];

        for (input, expected) in cases {
            assert_eq!(parse_key_chord(input), Ok(expected), "{input}");
        }
    }

    #[test]
    fn parses_key_sequences() {
        assert_eq!(
            parse_key_sequence("ctrl+x ctrl+s"),
            Ok(vec![
                KeyEvent::new(Key::Char('x'), Modifiers::ctrl()),
                KeyEvent::new(Key::Char('s'), Modifiers::ctrl()),
            ])
        );
    }

    #[test]
    fn rejects_unknown_tokens() {
        let cases = [
            (
                "ctrl+unknown",
                ParseKeyError::UnknownToken("unknown".to_string()),
            ),
            ("foo+j", ParseKeyError::UnknownToken("foo".to_string())),
        ];

        for (input, expected) in cases {
            assert_eq!(parse_key_chord(input), Err(expected), "{input}");
        }
    }
}
