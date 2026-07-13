// cross build に必要な toolchain の不足を、goreleaser の途中失敗ではなく
// prepare 冒頭で「何をどう入れるか」つきで明示的に失敗させる。
use crate::process::{RunOptions, command_exists, run};
use std::collections::HashSet;

// .goreleaser.yaml の builds[].targets と同期させること。
// goreleaser (cargo zigbuild) はここに挙げた rustup target が未導入だと途中で失敗するため、
// prepare の冒頭で不足を明示的に検出して導入手順つきで fail させる
pub const RUST_TARGETS: [&str; 5] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-gnu",
];

pub fn assert_release_toolchain() -> Result<(), String> {
    let mut problems: Vec<String> = Vec::new();

    let installed = run(
        "rustup",
        &["target", "list", "--installed"],
        RunOptions {
            quiet: true,
            ..Default::default()
        },
    )?;
    let installed_targets: HashSet<&str> = installed.trim().lines().collect();
    let missing_targets: Vec<&str> = RUST_TARGETS
        .iter()
        .filter(|target| !installed_targets.contains(*target))
        .copied()
        .collect();
    if !missing_targets.is_empty() {
        problems.push(format!(
            "Missing rustup targets. Run: rustup target add {}",
            missing_targets.join(" ")
        ));
    }

    // Linux (gnu) target の linker は zig。zig 本体と cargo の zigbuild subcommand の両方が要る
    if !command_exists("zig", &["version"]) {
        problems.push("zig not found. Run: mise install (mise.toml defines zig)".to_string());
    }
    if !command_exists("cargo-zigbuild", &["--version"]) {
        problems.push(
            "cargo-zigbuild not found. Run: mise install (mise.toml defines cargo-zigbuild)"
                .to_string(),
        );
    }
    if !command_exists("goreleaser", &["--version"]) {
        problems.push(
            "goreleaser not found. Run: mise install (mise.toml defines goreleaser)".to_string(),
        );
    }
    // macOS target は cargo-zigbuild が system linker へ fallback するため macOS host でのみ build できる
    if !cfg!(target_os = "macos") {
        problems.push(
            "Release builds must run on macOS: apple-darwin targets need the macOS system linker/SDK."
                .to_string(),
        );
    }

    if !problems.is_empty() {
        return Err(format!(
            "Release toolchain is not ready:\n- {}",
            problems.join("\n- ")
        ));
    }
    Ok(())
}
