260828 LICENSE 整備と THIRD-PARTY-NOTICES 生成
===

## asis

- repo に LICENSE ファイルがなく、`Cargo.toml` にも `license` フィールドがない (公開 repo + release 済みなのにライセンス未指定 = 他者は法的に利用不能)
- 依存 crate (syntect / serde 等) の多くは MIT で、バイナリ配布物にも copyright notice の同梱が本来求められるが、release archive はバイナリのみ
- TASK-260828-markdown-fence-embed-highlight で sublimehq/Packages 由来の syntax 定義を同梱した (こちらは表記義務なしの permissive、出所記録のみ必要)

## tobe

- coda 本体が MIT ライセンスであることが LICENSE / `Cargo.toml` で明示されている
- release archive (tar.gz / zip) に LICENSE と THIRD-PARTY-NOTICES.md が同梱される
- THIRD-PARTY-NOTICES.md は出荷 5 target の依存から機械的に再生成でき、手書きメンテしない

## todo

- [x] LICENSE (MIT) を repo 直下に追加、`Cargo.toml` に `license = "MIT"` を追加
- [x] `src/highlight/assets/LICENSE-sublimehq-packages.txt` に同梱 syntax の出所記録を置く
- [x] `cargo xtask licenses` を実装: 出荷 target の `cargo tree` 依存を union し、`~/.cargo/registry/src` の LICENSE テキストを集めて THIRD-PARTY-NOTICES.md を生成 (sublimehq notice も含める)
- [x] `.goreleaser.yaml` の archive files に LICENSE / THIRD-PARTY-NOTICES.md を追加
- [x] `release:prepare` で notices を再生成し、鮮度を release flow で担保する
- [x] mise task `licenses:generate` を追加

## testcases

- [x] `cargo xtask licenses` が THIRD-PARTY-NOTICES.md を生成し、syntect / serde / windows-sys 等の主要依存が含まれる
- [x] 生成物に coda 自身と xtask が含まれない
- [x] `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` が通る

## notes

- 生成は cargo-about 等の外部ツールでなく xtask (外部依存なし) に寄せた。repo の既存方針 (TASK-260712-release-xtask-migration) に合わせるため
- 依存の license 式は全て MIT / Apache-2.0 / Unlicense / 0BSD / Zlib / Unicode-3.0 系 permissive で MIT プロジェクトと両立することを確認済み (ec4rs のみ Apache-2.0 単独だが依存利用は問題なし)
- crate の LICENSE ファイルは local の `~/.cargo/registry/src` から読み、`A AND B` 型の複合ライセンスで必須本文を落とさないよう選抜せず全ファイル (LICENSE / COPYING / COPYRIGHT / NOTICE 系) を収録する。未展開・本文なしの crate は生成を失敗させて明示する (Apache-2.0 のみ canonical text を `xtask/assets/` から補う。§4(a) が本文コピーの提供を要求し、リンクでは代替できないため)
- sublimehq/Packages のライセンスは notice 保持条項を持たないため、バイナリ同梱は義務ではないが THIRD-PARTY-NOTICES に含めておく (利用者への説明可能性のため)
