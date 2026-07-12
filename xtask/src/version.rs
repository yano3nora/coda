// Cargo.toml の version bump と、CLI から見える version の整合チェック。
use crate::process::{RunOptions, run};
use std::fs;

/// `\d+\.\d+\.\d+` 相当の簡易 semver 判定 (regex crate は使わない)
pub fn is_simple_semver(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// Cargo.toml の `[package]` 直下 version 行のみを置換する pure function。
/// dependencies の inline version (`serde = { version = "1.0" }` 等) は行頭に
/// `version = "..."` の形で現れないため巻き込まれない。
///
/// 戻り値: (置換後の全文, 変更があったかどうか)。同値なら変更なしで元の文字列を返す
pub fn bump_cargo_toml_version(source: &str, version: &str) -> Result<(String, bool), String> {
    let lines: Vec<&str> = source.split('\n').collect();
    let target_idx = lines.iter().position(|line| {
        line.strip_prefix("version = \"")
            .and_then(|rest| rest.strip_suffix('"'))
            .is_some_and(is_simple_semver)
    });

    let Some(idx) = target_idx else {
        return Err("version declaration was not found in Cargo.toml.".to_string());
    };

    // strip_prefix/suffix は上の position 判定で存在確認済みなので unwrap して問題ない
    let current = lines[idx]
        .strip_prefix("version = \"")
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("target_idx line must match the version pattern");

    if current == version {
        return Ok((source.to_string(), false));
    }

    let mut new_lines = lines;
    let replaced = format!("version = \"{version}\"");
    new_lines[idx] = &replaced;
    Ok((new_lines.join("\n"), true))
}

/// Cargo.toml を読み、version 行を bump する (同値なら noop ログのみ)
pub fn bump_version(version: &str) -> Result<(), String> {
    let path = "Cargo.toml";
    let source = fs::read_to_string(path).map_err(|err| format!("failed to read {path}: {err}"))?;
    let (updated, changed) = bump_cargo_toml_version(&source, version)?;

    if !changed {
        println!("Cargo.toml version is already {version}; leaving it unchanged.");
        return Ok(());
    }

    fs::write(path, updated).map_err(|err| format!("failed to write {path}: {err}"))
}

// binary の --version (CARGO_PKG_VERSION 由来) と tag の食い違いを publish 前に止める最後の網。
// -p coda を明示するのは、workspace 化で root package が暗黙のデフォルトかどうかに依存させないため。
// cargo run が Cargo.lock の version 同期も兼ねる
pub fn assert_cli_version(version: &str) -> Result<(), String> {
    let stdout = run(
        "cargo",
        &["run", "--quiet", "-p", "coda", "--", "--version"],
        RunOptions::default(),
    )?;
    let actual = stdout.trim();
    let expected = format!("coda {version}");

    if actual != expected {
        return Err(format!(
            "CLI version mismatch: expected \"{expected}\", got \"{actual}\"."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_package_version_line() {
        let source = "[package]\nname = \"coda\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
        let (updated, changed) = bump_cargo_toml_version(source, "0.2.0").unwrap();
        assert!(changed);
        assert!(updated.contains("version = \"0.2.0\""));
        assert!(!updated.contains("version = \"0.1.0\""));
    }

    #[test]
    fn is_noop_when_version_is_already_current() {
        let source = "[package]\nname = \"coda\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
        let (updated, changed) = bump_cargo_toml_version(source, "0.1.0").unwrap();
        assert!(!changed);
        assert_eq!(updated, source);
    }

    #[test]
    fn errors_when_no_version_line_is_found() {
        let source = "[package]\nname = \"coda\"\nedition = \"2024\"\n";
        let result = bump_cargo_toml_version(source, "0.2.0");
        assert_eq!(
            result.unwrap_err(),
            "version declaration was not found in Cargo.toml."
        );
    }

    #[test]
    fn does_not_touch_dependencies_inline_version() {
        // [package] の version 行は無いが、dependencies の inline version はある。
        // 行頭アンカーでない inline version を誤検出しないことを確認する
        let source = "[package]\nname = \"coda\"\nedition = \"2024\"\n\n[dependencies]\nserde = { version = \"1.0.228\", features = [\"derive\"] }\n";
        let result = bump_cargo_toml_version(source, "0.2.0");
        assert_eq!(
            result.unwrap_err(),
            "version declaration was not found in Cargo.toml."
        );
    }

    #[test]
    fn keeps_dependencies_inline_version_untouched_when_bumping() {
        let source = "[package]\nname = \"coda\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = { version = \"1.0.228\", features = [\"derive\"] }\n";
        let (updated, changed) = bump_cargo_toml_version(source, "0.2.0").unwrap();
        assert!(changed);
        assert!(updated.contains("version = \"0.2.0\""));
        assert!(updated.contains("serde = { version = \"1.0.228\", features = [\"derive\"] }"));
    }

    #[test]
    fn accepts_simple_semver() {
        assert!(is_simple_semver("0.1.0"));
        assert!(is_simple_semver("12.34.56"));
    }

    #[test]
    fn rejects_non_simple_semver() {
        assert!(!is_simple_semver("0.1"));
        assert!(!is_simple_semver("0.1.0-rc1"));
        assert!(!is_simple_semver("v0.1.0"));
        assert!(!is_simple_semver(""));
    }
}
