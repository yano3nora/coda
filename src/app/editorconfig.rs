//! `.editorconfig` indent resolution (TASK-260828-editorconfig-indent).
//!
//! The ec4rs dependency is confined to this module: the rest of the app only
//! sees [`IndentOverride`] layers combined by `EventLoop::effective_indent`
//! (palette override > .editorconfig > config.toml).

use std::path::Path;

use crate::core::editor::{IndentConfig, IndentStyle};

/// Indent values one layer may pin. `None` falls through to the next layer.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct IndentOverride {
    pub style: Option<IndentStyle>,
    pub width: Option<usize>,
}

impl IndentOverride {
    /// Applies this layer's pinned values on top of `base`.
    pub fn apply_to(self, base: IndentConfig) -> IndentConfig {
        IndentConfig {
            style: self.style.unwrap_or(base.style),
            width: self.width.unwrap_or(base.width),
        }
    }
}

/// Outcome of resolving a file's `.editorconfig` chain.
pub struct Resolution {
    pub indent: IndentOverride,
    /// Set when the chain itself failed to parse: the caller surfaces it so
    /// the dropped settings are not lost silently. Individual invalid values
    /// carry no warning — the editorconfig spec requires ignoring them, and
    /// warning would fire on every file opened in someone else's repo.
    pub warning: Option<String>,
}

/// Resolves `indent_style` / `indent_size` for the file at `path` from its
/// `.editorconfig` chain (walking up to `root = true`, per spec).
pub fn resolve(path: &Path) -> Resolution {
    let mut properties = match ec4rs::properties_of(path) {
        Ok(properties) => properties,
        Err(error) => {
            return Resolution {
                indent: IndentOverride::default(),
                warning: Some(format!(".editorconfig ignored: {error}")),
            };
        }
    };
    // Spec-compliant fallbacks: `indent_size = tab` resolves via `tab_width`.
    properties.use_fallbacks();

    let style = match properties.get::<ec4rs::property::IndentStyle>() {
        Ok(ec4rs::property::IndentStyle::Tabs) => Some(IndentStyle::Tab),
        Ok(ec4rs::property::IndentStyle::Spaces) => Some(IndentStyle::Space),
        Err(_) => None,
    };
    // Same bounds as `[editor] indent_width` (see IndentConfig::MAX_WIDTH):
    // 0 and huge values are configuration mistakes, treated as unset here.
    let width = match properties.get::<ec4rs::property::IndentSize>() {
        Ok(ec4rs::property::IndentSize::Value(width))
            if (1..=IndentConfig::MAX_WIDTH).contains(&width) =>
        {
            Some(width)
        }
        _ => None,
    };

    Resolution {
        indent: IndentOverride { style, width },
        warning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{IndentOverride, resolve};
    use crate::core::editor::{IndentConfig, IndentStyle};
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coda-test-editorconfig-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn apply_to_layers_pinned_values_over_the_base() {
        let base = IndentConfig::default(); // space x 4
        let override_ = IndentOverride {
            style: Some(IndentStyle::Tab),
            width: None,
        };
        let effective = override_.apply_to(base);
        assert_eq!(effective.style, IndentStyle::Tab);
        assert_eq!(effective.width, 4, "unset width falls through to base");
    }

    #[test]
    fn resolve_reads_indent_style_and_size_for_a_matching_glob() {
        let dir = temp_dir("glob");
        fs::write(
            dir.join(".editorconfig"),
            "root = true\n\n[*.rs]\nindent_style = tab\nindent_size = 8\n",
        )
        .unwrap();
        let target = dir.join("main.rs");
        fs::write(&target, "fn main() {}\n").unwrap();

        let resolution = resolve(&target);
        assert_eq!(resolution.indent.style, Some(IndentStyle::Tab));
        assert_eq!(resolution.indent.width, Some(8));
        assert!(resolution.warning.is_none());

        // A file the section does not match gets no values.
        let other = dir.join("notes.txt");
        fs::write(&other, "text\n").unwrap();
        let resolution = resolve(&other);
        assert_eq!(resolution.indent, IndentOverride::default());

        let _ = fs::remove_dir_all(&dir);
    }

    /// `indent_size = tab` must resolve through `tab_width` (the ec4rs
    /// fallback path), matching how other editors read the spec.
    #[test]
    fn resolve_maps_indent_size_tab_to_tab_width() {
        let dir = temp_dir("size-tab");
        fs::write(
            dir.join(".editorconfig"),
            "root = true\n\n[*]\nindent_style = tab\nindent_size = tab\ntab_width = 3\n",
        )
        .unwrap();
        let target = dir.join("file.txt");
        fs::write(&target, "x\n").unwrap();

        let resolution = resolve(&target);
        assert_eq!(resolution.indent.width, Some(3));

        let _ = fs::remove_dir_all(&dir);
    }

    /// Out-of-range and unparsable values are unset, not errors: the file
    /// still opens with the next layer's settings (silent-breakage rule
    /// applies to the chain, not to individual spec-invalid values).
    #[test]
    fn resolve_ignores_out_of_range_and_invalid_values() {
        let dir = temp_dir("invalid");
        fs::write(
            dir.join(".editorconfig"),
            "root = true\n\n[*]\nindent_style = banana\nindent_size = 99\n",
        )
        .unwrap();
        let target = dir.join("file.txt");
        fs::write(&target, "x\n").unwrap();

        let resolution = resolve(&target);
        assert_eq!(resolution.indent, IndentOverride::default());
        assert!(resolution.warning.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    /// A chain that fails to parse must surface a warning instead of
    /// silently dropping the repo's settings — unlike individually invalid
    /// values. Note the garbage line sits *inside* a section: ec4rs skips a
    /// whole file whose preamble is broken (it never opens), so only
    /// section-body parse failures reach the warning path.
    #[test]
    fn resolve_surfaces_a_warning_for_a_broken_chain() {
        let dir = temp_dir("broken");
        fs::write(
            dir.join(".editorconfig"),
            "root = true\n\n[*]\nindent_style = tab\nthis line is not a pair\n",
        )
        .unwrap();
        let target = dir.join("file.txt");
        fs::write(&target, "x\n").unwrap();

        let resolution = resolve(&target);
        assert_eq!(resolution.indent, IndentOverride::default());
        let warning = resolution.warning.expect("broken chain must warn");
        assert!(warning.contains(".editorconfig ignored"), "{warning}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_without_editorconfig_pins_nothing() {
        let dir = temp_dir("absent");
        let target = dir.join("file.txt");
        fs::write(&target, "x\n").unwrap();

        let resolution = resolve(&target);
        assert_eq!(resolution.indent, IndentOverride::default());
        assert!(resolution.warning.is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
