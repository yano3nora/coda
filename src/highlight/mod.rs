//! Display-only syntax highlighting and theme/color capability integration.

pub mod cache;
pub mod engine;

pub use cache::{HighlightCache, HighlightSpan};
pub use engine::{HighlightEngine, ThemeChoice};
