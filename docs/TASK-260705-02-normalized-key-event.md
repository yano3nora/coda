# TASK-260705-02: Normalized key event と modifier 判定

260705 normalized key event decoder
===

## asis

- TASK-260705-01 により raw bytes の表示までは可能 (前提: 本タスクは 01 完了後に着手)
- raw bytes を環境非依存の key 表現へ変換する層 (SPEC-0003 の normalized key event) が存在しない

## tobe

- `input/` module が raw bytes を `KeyEvent` (key + modifiers) に decode できる
- legacy sequence と kitty keyboard protocol (CSI u) の双方を decode できる
- `inspect-key` が raw bytes と併せて `Pressed: Ctrl+Shift+J` 形式の解釈結果を表示する

## todo

- [x] `src/input/key_event.rs`: `KeyEvent` 型を定義する
    - `key`: 文字 (`Char(char)`) と named key (`Enter` / `Esc` / `Tab` / `Backspace` / `Up` / `Down` / `Left` / `Right` / `Home` / `End` / `PageUp` / `PageDown` / `Delete` / `F(1..=12)`) を表現する enum
    - `modifiers`: `ctrl` / `alt` / `shift` の bitflags
    - `Display` 実装: `Ctrl+Shift+J` / `F1` / `Alt+Enter` 形式
- [x] `src/input/decoder.rs`: byte 列 → `Vec<KeyEvent>` の decoder を実装する
    - C0 control (0x01-0x1a → `Ctrl+<letter>`。ただし 0x09=`Tab`、0x0d=`Enter`、0x1b は escape 処理へ)
    - ESC prefix による `Alt+<key>`
    - legacy CSI / SS3 sequence (矢印・Home/End/PageUp/PageDown/Delete/F1-F12、`\x1b[1;5C` 等の modifier 付き含む)
    - kitty CSI u 形式 (`\x1b[<codepoint>;<modifiers>u`。modifiers encoding は kitty keyboard protocol 仕様に従う)
    - 不完全な sequence (途中で途切れた ESC 列) は「追加入力待ち」として扱える API にする (`Incomplete` を返す等)
    - 未知の sequence は `Unknown(bytes)` として落とさず返す
- [x] decode 結果に「legacy では区別不能」の情報を残す: legacy の 0x0a/0x0d は `Enter` に、0x09 は `Tab` に解決し、`Ctrl+J` / `Ctrl+I` へは解決しない (SPEC-0003 の区別は CSI u 受信時のみ可能)
- [x] `inspect-key` を拡張し、raw bytes 表示に加えて decode 結果を表示する:
    ```text
    Raw bytes: \x1b[106;6u
    Pressed:   Ctrl+Shift+J
    ```
- [x] table-driven unit test を追加する (AGENTS.md Testing 方針)

## testcases

unit test (最低限以下のケースを含むこと):

- [x] `0x61` → `Char('a')` (modifier なし)
- [x] `0x01` → `Ctrl+A`
- [x] `0x09` → `Tab` (legacy では `Ctrl+I` にしない)
- [x] `0x0d` → `Enter`
- [x] `\x1b[A` → `Up`
- [x] `\x1b[1;5C` → `Ctrl+Right`
- [x] `\x1bOP` → `F1` (SS3 形式)
- [x] `\x1b[106;6u` → `Ctrl+Shift+J` (CSI u)
- [x] `\x1b[13;5u` → `Ctrl+Enter` (CSI u)
- [x] `\x1bf` → `Alt+F`
- [x] `\x1b[1;5` (途切れ) → `Incomplete`
- [x] 未知 sequence → `Unknown` で panic しない

手動確認:

- [x] `cargo run -- inspect-key` で `a` / 矢印 / `Ctrl+a` が正しく表示される (PTY 経由で a / Up / CSI u / Shift+Tab / NUL / Ctrl+C を確認済み)
- [ ] kitty または Ghostty 上で (protocol 有効化はまだ実装しないため) legacy 表示になることを確認する
- [x] `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` がすべて通る (12 tests passed)

## notes

- レビューでの修正 3 件:
    - `0x00` (Ctrl+Space) が UTF-8 分岐に落ちて Incomplete 停滞・後続 byte 誤消費するバグを修正 (`Ctrl+Space` に解決)
    - `\x1b[Z` (Shift+Tab) が `Unknown` になっていたため `Shift+Tab` へ正規化
    - `Ctrl+ ` と表示されて見えない問題を `Ctrl+Space` 表示に修正

- kitty keyboard protocol の有効化 negotiation・`KeyboardCapabilities` 検出は次タスク以降 (SPEC-0003)。本タスクは「受信した bytes を正しく decode する」まで (protocol を有効化していなくても、CSI u を送る terminal への備えとして decoder は先に用意する)
- `keymap/` からは本 module の `KeyEvent` のみを参照させる。raw bytes を `input/` の外に漏らさない (ADR-0004 依存境界)
- kitty keyboard protocol 仕様: https://sw.kovidgoyal.net/kitty/keyboard-protocol/
- commit は人間が行う (AGENTS.md)
