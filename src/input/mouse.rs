//! Normalized mouse events (SGR protocol, ADR-0008).
//!
//! Mouse sequences never enter key resolution: the decoder emits them as
//! [`crate::input::InputEvent::Mouse`], a separate channel from keys
//! (SPEC-0003). Coordinates stay 1-based terminal cells here; mapping to
//! buffer positions is the UI layer's job.

use super::Modifiers;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MouseEventKind {
    Press(MouseButton),
    /// Motion while a button is held (DECSET 1002 button-event tracking; we
    /// never enable 1003, so motion always carries a button).
    Drag(MouseButton),
    Release(MouseButton),
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    /// Shift / Alt / Ctrl as encoded in the SGR button code. Shift-modified
    /// events that reach coda are ignored; terminal-native selection only
    /// works when the terminal reserves Shift+drag before delivery.
    pub modifiers: Modifiers,
    /// 1-based terminal cell column, as sent by SGR.
    pub column: u16,
    /// 1-based terminal cell row, as sent by SGR.
    pub row: u16,
}
