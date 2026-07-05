//! coda library crate.
//!
//! Module boundaries follow ADR-0004: terminal-specific code stays in `input`,
//! and keymap/core logic must not depend on UI code.

pub mod app;
pub mod core;
pub mod highlight;
pub mod input;
pub mod keymap;
pub mod ui;
