//! Environment-independent normalized keyboard events.
//!
//! `KeyEvent` is the boundary type that leaves `input/`. Keymap resolution must
//! consume this representation instead of terminal raw bytes.

use std::fmt;

/// A normalized key without terminal escape-sequence details.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    F(u8),
    /// A complete but unsupported terminal sequence.
    ///
    /// Keeping the original bytes is intentional: `inspect-key` and later import
    /// reports can explain exactly what arrived instead of silently discarding it.
    Unknown(Vec<u8>),
}

/// Modifier bitset for normalized key events.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct Modifiers(u8);

impl Modifiers {
    const CTRL: u8 = 0b001;
    const ALT: u8 = 0b010;
    const SHIFT: u8 = 0b100;

    pub const fn none() -> Self {
        Self(0)
    }

    pub const fn ctrl() -> Self {
        Self(Self::CTRL)
    }

    pub const fn alt() -> Self {
        Self(Self::ALT)
    }

    pub const fn shift() -> Self {
        Self(Self::SHIFT)
    }

    pub const fn with_ctrl(self) -> Self {
        Self(self.0 | Self::CTRL)
    }

    pub const fn with_alt(self) -> Self {
        Self(self.0 | Self::ALT)
    }

    pub const fn with_shift(self) -> Self {
        Self(self.0 | Self::SHIFT)
    }

    pub const fn contains_ctrl(self) -> bool {
        self.0 & Self::CTRL != 0
    }

    pub const fn contains_alt(self) -> bool {
        self.0 & Self::ALT != 0
    }

    pub const fn contains_shift(self) -> bool {
        self.0 & Self::SHIFT != 0
    }

    pub(crate) const fn from_kitty_encoded(encoded: u16) -> Self {
        // kitty CSI u uses xterm-style encoding: value 1 means no modifiers,
        // then bit 0=Shift, bit 1=Alt, bit 2=Ctrl. Extra bits are intentionally
        // ignored for MVP because SPEC-0003 only exposes Ctrl/Alt/Shift.
        let bits = encoded.saturating_sub(1);
        let mut result = Self::none();
        if bits & 0b001 != 0 {
            result = result.with_shift();
        }
        if bits & 0b010 != 0 {
            result = result.with_alt();
        }
        if bits & 0b100 != 0 {
            result = result.with_ctrl();
        }
        result
    }
}

/// A decoded keyboard event: a key plus Ctrl/Alt/Shift state.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl KeyEvent {
    pub const fn new(key: Key, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }

    pub const fn plain(key: Key) -> Self {
        Self::new(key, Modifiers::none())
    }
}

impl fmt::Display for KeyEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains_ctrl() {
            formatter.write_str("Ctrl+")?;
        }
        if self.modifiers.contains_alt() {
            formatter.write_str("Alt+")?;
        }
        if self.modifiers.contains_shift() {
            formatter.write_str("Shift+")?;
        }

        match &self.key {
            // A bare space glyph is invisible next to "Ctrl+", so name it.
            Key::Char(' ') => formatter.write_str("Space"),
            Key::Char(character) => {
                write!(formatter, "{}", display_char(*character, self.modifiers))
            }
            Key::Enter => formatter.write_str("Enter"),
            Key::Esc => formatter.write_str("Esc"),
            Key::Tab => formatter.write_str("Tab"),
            Key::Backspace => formatter.write_str("Backspace"),
            Key::Up => formatter.write_str("Up"),
            Key::Down => formatter.write_str("Down"),
            Key::Left => formatter.write_str("Left"),
            Key::Right => formatter.write_str("Right"),
            Key::Home => formatter.write_str("Home"),
            Key::End => formatter.write_str("End"),
            Key::PageUp => formatter.write_str("PageUp"),
            Key::PageDown => formatter.write_str("PageDown"),
            Key::Delete => formatter.write_str("Delete"),
            Key::F(number) => write!(formatter, "F{number}"),
            Key::Unknown(bytes) => {
                write!(formatter, "Unknown({})", crate::input::escape_bytes(bytes))
            }
        }
    }
}

fn display_char(character: char, modifiers: Modifiers) -> char {
    if (modifiers.contains_ctrl() || modifiers.contains_alt() || modifiers.contains_shift())
        && character.is_ascii_alphabetic()
    {
        character.to_ascii_uppercase()
    } else {
        character
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, KeyEvent, Modifiers};

    #[test]
    fn display_formats_modifier_prefixes_in_stable_order() {
        let cases = [
            (
                KeyEvent::new(Key::Char('j'), Modifiers::ctrl().with_shift()),
                "Ctrl+Shift+J",
            ),
            (KeyEvent::plain(Key::F(1)), "F1"),
            (KeyEvent::new(Key::Enter, Modifiers::alt()), "Alt+Enter"),
            (
                KeyEvent::new(Key::Char(' '), Modifiers::ctrl()),
                "Ctrl+Space",
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(event.to_string(), expected);
        }
    }
}
