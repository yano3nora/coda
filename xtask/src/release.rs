// prepare / publish の flow 本体。scripts/release.ts の prepare()/publish() 相当。
use crate::process::{RunOptions, run};
use crate::toolchain::assert_release_toolchain;
use crate::version::{assert_cli_version, bump_version};
use std::env;

pub const PUBLISH_FLAG: &str = "--i-understand-this-pushes-and-publishes";

pub fn assert_clean_tree() -> Result<(), String> {
    let stdout = run("git", &["status", "--porcelain"], RunOptions::default())?;
    if !stdout.trim().is_empty() {
        return Err(
            "Working tree must be clean before publishing. Commit the version bump first."
                .to_string(),
        );
    }
    Ok(())
}

// goreleaser は tag 済みコミットからビルドする前提のため、tag が HEAD を指すことを保証する
pub fn assert_tag_at_head(tag: &str) -> Result<(), String> {
    let verify_ref = format!("{tag}^{{commit}}");
    let tag_commit = run(
        "git",
        &["rev-parse", "--verify", &verify_ref],
        RunOptions {
            quiet: true,
            ..Default::default()
        },
    )
    .map_err(|_| format!("Tag {tag} does not exist. Create it yourself first: git tag {tag}"))?
    .trim()
    .to_string();

    let head = run(
        "git",
        &["rev-parse", "HEAD"],
        RunOptions {
            quiet: true,
            ..Default::default()
        },
    )?
    .trim()
    .to_string();

    if tag_commit != head {
        return Err(format!(
            "Tag {tag} does not point at HEAD. Move the tag to the release commit or check out the tagged commit."
        ));
    }
    Ok(())
}

pub fn prepare(version: &str) -> Result<(), String> {
    bump_version(version)?;
    assert_cli_version(version)?;
    run(
        "mise",
        &["run", "pre-commit"],
        RunOptions {
            stream: true,
            ..Default::default()
        },
    )?;
    assert_release_toolchain()?;

    // publish で失敗しうるビルド〜archive〜checksum をここで全部失敗させておく。
    // --snapshot は tag 不要・publish なしで全工程を回すドライラン。
    // CODA_RELEASE_VERSION は .goreleaser.yaml の snapshot.version_template が
    // 参照し、asset 名を release 時と同じ形にする
    run(
        "goreleaser",
        &["release", "--snapshot", "--clean"],
        RunOptions {
            env: &[("CODA_RELEASE_VERSION", version)],
            stream: true,
            ..Default::default()
        },
    )?;

    println!(
        "\nSnapshot assets are ready in dist/ (validation only; publish rebuilds from the tag)."
    );
    println!(
        "Review the diff, commit the bump, tag it (git tag v{version}), then run release:publish."
    );
    Ok(())
}

pub fn publish(version: &str, publish_allowed: bool) -> Result<(), String> {
    if !publish_allowed {
        return Err(format!(
            "Refusing to push tags or publish a GitHub Release without {PUBLISH_FLAG}."
        ));
    }

    let tag = format!("v{version}");

    assert_cli_version(version)?;
    assert_clean_tree()?;
    assert_tag_at_head(&tag)?;

    // goreleaser は push を行わず GitHub API しか叩かないため、
    // Release が正しいコミットを指すように commit と tag を先に remote へ揃える
    run("git", &["push", "origin", "HEAD"], RunOptions::default())?;
    run("git", &["push", "origin", &tag], RunOptions::default())?;

    // goreleaser は GITHUB_TOKEN を要求する。gh の認証を使い回して token 管理を増やさない
    let token = match env::var("GITHUB_TOKEN") {
        Ok(value) => value,
        Err(_) => run(
            "gh",
            &["auth", "token"],
            RunOptions {
                quiet: true,
                ..Default::default()
            },
        )?
        .trim()
        .to_string(),
    };

    run(
        "goreleaser",
        &["release", "--clean"],
        RunOptions {
            env: &[("GITHUB_TOKEN", &token)],
            stream: true,
            ..Default::default()
        },
    )?;

    println!("Published {tag}.");
    Ok(())
}
