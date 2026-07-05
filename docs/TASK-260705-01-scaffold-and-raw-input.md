# TASK-260705-01: Cargo scaffold と raw input 表示プログラム

260705 cargo scaffold + raw input echo
===

## asis

- repo には docs (ADR-0001〜0005 / SPEC-0001〜0005) と設定ファイル雛形のみが存在し、Rust プロジェクトは未作成
- 実装順序 (ADR-0004) の step 1「terminal raw input を表示する最小プログラム」が未着手

## tobe

- `cargo run -- inspect-key` で terminal の raw input を目視確認できる
- 以降のタスクが乗る module skeleton (ADR-0004 の `core/ input/ keymap/ ui/ app/`) が存在する
- fmt / clippy / test が通る状態が baseline として確立している

## todo

- [ ] `cargo init` で単一 crate `coda` を作成する (edition 2024。binary crate)
- [ ] ADR-0004 の module 構成に対応する skeleton を作る: `src/core/` `src/input/` `src/keymap/` `src/ui/` `src/app/` (各 `mod.rs` は空でよい。`core/` `keymap/` から `ui/` への依存を作らないこと)
- [ ] CLI 引数処理を実装する: 引数なし・ファイルパスは「not implemented」メッセージで終了、`inspect-key` サブコマンドのみ動作する (clap 等の CLI crate 使用可)
- [ ] `inspect-key`: terminal を raw mode にし、受信 byte 列を chunk ごとに hex (`\x1b[106;6u` 形式の escape 表記併記) で 1 行ずつ表示する。`Ctrl+c` (0x03) または `Ctrl+d` (0x04) で終了する
- [ ] raw mode の設定・復元は RAII guard で行い、panic 時にも terminal 状態が復元されること (termios は `rustix` または `libc` を直接使用。crossterm 等の高レベル crate は本タスクでは使わない — raw bytes をそのまま見せるのが目的のため)
- [ ] `mise.toml` に rust toolchain と `tasks.pre-commit` (`cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test`) を設定する
- [ ] `.gitignore` に `target/` を追加する

## testcases

- [ ] `cargo run -- inspect-key` 起動後、`a` を押すと `0x61` 相当の表示が出る
- [ ] 矢印キーで `\x1b[A` 等の escape sequence が 1 chunk として表示される
- [ ] `Ctrl+c` で終了し、shell に戻った後も terminal 表示が乱れない (echo / canonical mode が復元されている)
- [ ] `inspect-key` 実行中に kill した場合でも `reset` なしで shell が使える (RAII guard の動作確認。panic 経路は unit test でも可)
- [ ] `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` がすべて通る

## notes

- crate 名 `coda` は working name (ADR-0001 Open Questions: 名称衝突あり、公開前に再検討)
- raw bytes の解釈 (normalized key event 化) は TASK-260705-02 で行う。本タスクは「bytes が見える」ところまで
- commit は人間が行う (AGENTS.md)
