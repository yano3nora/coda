// Release flow の薄い wrapper。compile / archive / checksum / GitHub Release 作成は
// goreleaser (.goreleaser.yaml) に任せ、ここには goreleaser に寄せられないものだけを残す:
// version bump・check/test・cross build toolchain の事前検査・tag と version の整合チェック・
// 人間による publish ゲート。
//
// Flow:
//   1. prepare : bump + check/test + toolchain 検査 + `goreleaser release --snapshot` (publish なしの全工程ドライラン)
//   2. 人間    : version bump を commit し、`git tag v<version>` を打つ
//   3. publish : 整合チェック → commit + tag を push → `goreleaser release` (tag 済みコミットから再ビルド)
//
// ⚠️ publish は「人間専用」。AI Agent は prepare までしか実行してはならない
// (AGENTS.md の Push / Publish 規則)。publish は remote への push と GitHub Release
// 作成を伴うため、PUBLISH_FLAG を明示した人間の手でのみ実行する。
//
// 旧実装は TypeScript 製の外部 script runtime 経由 (scripts/release.ts) だったが、
// repo 内でその runtime の用途がこの script の実行のみだったため cargo xtask パターンへ移植した
// (docs/TASK-260712-release-xtask-migration.md)。

mod process;
mod release;
mod toolchain;
mod version;

use release::PUBLISH_FLAG;
use std::env;
use std::process::ExitCode;
use version::is_simple_semver;

enum CliCommand {
    Prepare,
    Publish,
}

struct Args {
    command: CliCommand,
    version: String,
    publish_allowed: bool,
}

fn usage() -> String {
    format!(
        "Usage:\n  \
         cargo xtask prepare <version>\n  \
         cargo xtask publish <version> {flag}\n\n\
         Examples:\n  \
         mise run release:prepare -- 0.1.0\n  \
         mise run release:publish -- 0.1.0 {flag}\n",
        flag = PUBLISH_FLAG
    )
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let command = match raw.first().map(String::as_str) {
        Some("prepare") => CliCommand::Prepare,
        Some("publish") => CliCommand::Publish,
        _ => return Err(format!("Unknown command.\n\n{}", usage())),
    };

    // version は第 2 引数、なければ env CODA_RELEASE_VERSION を fallback
    let version = raw
        .get(1)
        .cloned()
        .or_else(|| env::var("CODA_RELEASE_VERSION").ok());
    let version = match version {
        Some(v) if is_simple_semver(&v) => v,
        _ => {
            return Err(format!(
                "Release version must be semver-like, for example 0.1.0.\n\n{}",
                usage()
            ));
        }
    };

    let publish_allowed = raw.iter().skip(2).any(|arg| arg == PUBLISH_FLAG);

    Ok(Args {
        command,
        version,
        publish_allowed,
    })
}

fn main() -> ExitCode {
    let raw_args: Vec<String> = env::args().skip(1).collect();

    let args = match parse_args(&raw_args) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    let result = match args.command {
        CliCommand::Prepare => release::prepare(&args.version),
        CliCommand::Publish => release::publish(&args.version, args.publish_allowed),
    };

    if let Err(message) = result {
        eprintln!("{message}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
