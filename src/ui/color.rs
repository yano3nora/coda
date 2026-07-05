//! Terminal color capability detection and RGB degradation helpers.
//!
//! This module is UI-local and pure-testable: environment lookup is kept at the
//! renderer boundary, while detection itself accepts explicit values.

/// Terminal color output mode used when serializing [`Style`](super::Style).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
    Mono,
}

impl ColorMode {
    /// Detects color support from environment values without reading globals.
    pub fn detect(colorterm: Option<&str>, term: Option<&str>) -> Self {
        let colorterm = colorterm.unwrap_or_default().to_ascii_lowercase();
        if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            return Self::TrueColor;
        }

        let term = term.unwrap_or_default().to_ascii_lowercase();
        if term.contains("256color") {
            return Self::Ansi256;
        }

        Self::Mono
    }

    /// Detects color support from the current process environment.
    pub fn detect_from_env() -> Self {
        Self::detect(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    }
}

/// Converts RGB to an xterm-compatible ANSI 256-color index.
///
/// The conversion intentionally considers both the 6x6x6 color cube and the
/// grayscale ramp, then chooses the nearest Euclidean color. This keeps pure
/// grays from being forced into tinted cube entries.
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let cube = cube_index_and_rgb(r, g, b);
    let gray = grayscale_index_and_rgb(r, g, b);

    if distance_sq((r, g, b), gray.1) < distance_sq((r, g, b), cube.1) {
        gray.0
    } else {
        cube.0
    }
}

fn cube_index_and_rgb(r: u8, g: u8, b: u8) -> (u8, (u8, u8, u8)) {
    let ri = cube_component(r);
    let gi = cube_component(g);
    let bi = cube_component(b);
    let index = 16 + 36 * ri + 6 * gi + bi;
    (index, (cube_value(ri), cube_value(gi), cube_value(bi)))
}

fn cube_component(value: u8) -> u8 {
    ((u16::from(value) * 5 + 127) / 255) as u8
}

fn cube_value(index: u8) -> u8 {
    if index == 0 { 0 } else { 55 + index * 40 }
}

fn grayscale_index_and_rgb(r: u8, g: u8, b: u8) -> (u8, (u8, u8, u8)) {
    let luminance = (u16::from(r) * 30 + u16::from(g) * 59 + u16::from(b) * 11) / 100;
    let step = if luminance <= 8 {
        0
    } else if luminance >= 238 {
        23
    } else {
        ((luminance - 8 + 5) / 10).min(23) as u8
    };
    let value = 8 + step * 10;
    (232 + step, (value, value, value))
}

fn distance_sq(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let dr = i32::from(a.0) - i32::from(b.0);
    let dg = i32::from(a.1) - i32::from(b.1);
    let db = i32::from(a.2) - i32::from(b.2);
    (dr * dr + dg * dg + db * db) as u32
}

#[cfg(test)]
mod tests {
    use super::{ColorMode, rgb_to_ansi256};

    #[test]
    fn rgb_to_ansi256_converts_known_vectors() {
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
        assert_eq!(rgb_to_ansi256(0, 255, 0), 46);
        assert_eq!(rgb_to_ansi256(0, 0, 255), 21);
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
        assert_eq!(rgb_to_ansi256(128, 128, 128), 244);
    }

    #[test]
    fn color_mode_detection_uses_colorterm_then_term() {
        assert_eq!(
            ColorMode::detect(Some("truecolor"), Some("xterm-256color")),
            ColorMode::TrueColor
        );
        assert_eq!(
            ColorMode::detect(None, Some("xterm-256color")),
            ColorMode::Ansi256
        );
        assert_eq!(ColorMode::detect(None, Some("dumb")), ColorMode::Mono);
    }
}
