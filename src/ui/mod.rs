//! Terminal UI rendering, status bar, overlays, tabs, and split view.

pub mod alt_screen;
pub mod color;
pub mod render;
pub mod screen;
pub mod terminal_size;

pub use alt_screen::AltScreenGuard;
pub use color::{ColorMode, rgb_to_ansi256};
pub use render::{
    render_diff, render_diff_with_color_mode, render_full, render_full_with_color_mode,
};
pub use screen::{Cell, Screen, Style};
pub use terminal_size::{take_pending_resize, terminal_size};
