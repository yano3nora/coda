260828 markdown fenced code block の埋め込みハイライト
===

## asis

- `.md` の fenced code block (```` ```sh ```` など) の中身が単色で表示される
- 原因: `SyntaxSet::load_defaults_newlines()` が同梱する Markdown 定義が古く、fence 内への他言語 `embed` を持たない
- coda 側の highlight cache (`highlight/cache.rs`) は `ParseState` を行またぎで持ち回しており、複数行の埋め込み構文自体は扱える

## tobe

- `.md` の fenced code block 内が、info string の言語 (sh / rust / js / json など) でハイライトされる
- 未対応言語の fence・info string なしの fence は従来どおり単色 (黙って壊れない)
- ハイライトは表示専用のまま (ADR-0006 の壁を維持)

## todo

- [x] Sublime Text Packages v3211 の `Markdown.sublime-syntax` (version 1・`embed` 対応) を `src/highlight/assets/` に同梱する
- [x] `HighlightEngine::new` で default set を `into_builder()` し、同梱定義を追加してから build する (後勝ちで default の Markdown を上書き)
- [x] fence 内で言語別の色が付くことを確認する test を追加する

## testcases

- [x] `.md` の ```` ```sh ```` fence 内の行が、keyword / 文字列で複数色にハイライトされる
- [x] ```` ```rust ```` fence 内で keyword に色が付く
- [x] fence 外の Markdown (見出し等) のハイライトが従来どおり効く
- [x] `.rs` など Markdown 以外の syntax 解決に影響がない
- [x] `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` が通る

## notes

- 取得元: <https://raw.githubusercontent.com/sublimehq/Packages/v3211/Markdown/Markdown.sublime-syntax> (ST 3.2 相当)。最新 master は ST4 の version 2 機能を使い syntect が読めない可能性があるため v3211 に固定した
- embed 先は `scope:source.shell.bash` / `scope:source.rust` など scope 参照。default set に存在しない scope (source.dot 等) は syntect 5.3 が Plain Text にフォールバックしてリンクするため、該当 fence が単色になるだけで落ちない
- syntect は同名 syntax が複数ある場合に後から追加したものを優先するため、builder への追加順で default の Markdown を差し替えられる
