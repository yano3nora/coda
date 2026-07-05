//! Terminal UI rendering, status bar, overlays, tabs, and split view.

pub mod alt_screen;
pub mod render;
pub mod screen;
pub mod terminal_size;

pub use alt_screen::AltScreenGuard;
pub use render::{render_diff, render_full};
pub use screen::{Cell, Screen, Style};
pub use terminal_size::{take_pending_resize, terminal_size};
