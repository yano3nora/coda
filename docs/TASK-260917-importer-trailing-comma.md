# TASK-260917: importer の trailing comma 許容

260917 importer jsonc trailing comma
===

## asis

VS Code の `keybindings.json` を import すると、末尾カンマで
`InvalidJson (trailing comma)` になり import 全体が中断する。

- VS Code は `keybindings.json` を JSONC として扱い、末尾カンマを許容する
- TASK-260820 で `bindings.json` 側は `strip_trailing_commas` で対応済み
- importer (`vscode_import.rs`) は `strip_jsonc_comments` しか呼んでいなかった

## tobe

importer も `bindings.json` と同じ JSONC 前処理を通す。
理由: SPEC-0004 は「VS Code の JSONC を読み込む」と定義している。

## todo

- [x] `import_vscode_keybindings` で comment 除去後に `strip_trailing_commas` を適用
- [x] 新規コードは足さず、`user_bindings.rs` の既存 stripper を再利用

## testcases

- [x] unit: 行 / block comment と末尾カンマ混在の fixture が 2 件 Imported になる
  (`accepts_jsonc_comments_and_trailing_commas_like_vscode`。修正前に fail を確認)
- [x] `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` (347 tests)

## notes

- stripper 自体の edge case (string 内 `,]` / escape / multibyte) は TASK-260820 の unit test が担保する
- TASK-260820 は「generated 側は同じ loader を通る」とだけ書き、import 入口を見落としていた
- Codex レビュー指摘: 共有 stripper は `[,]` も `[]` として受理する。
  bindings.json 側と共通の既存挙動で、結果は空 import に留まるため今回は対応しない
- Codex レビュー指摘を反映: fixture の末尾カンマと `]` の間に行 comment を置き、
  comment 除去 → カンマ除去の順序も検証する
