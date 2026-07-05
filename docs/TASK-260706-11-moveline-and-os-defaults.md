# TASK-260706-11: 行移動系 action の実装と OS 標準 default keymap

260706 move lines + OS standard defaults
===

## asis

- 2 回目の dogfood で判明(TASK-260706-11 と同時に修正済みの kitty protocol 順序バグとは別件):
    - `edit.moveLinesUp/Down`・`edit.insertLineAfter/Before` が dispatch で `not implemented` のまま(import 対象なのに動かない)
    - default keymap が最小すぎて、macOS 標準の text 操作(`Cmd+矢印` で行頭行末、`Option+矢印` で単語移動等)が使えない。「OS 慣習キーは import 以前の前提」というフィードバック

## tobe

- moveLines / insertLine 系が editor で動き、undo で正しく戻る
- OS 標準 text 操作キーが default(`Source::Default`)として最初から効く
- default の方針変更(最小 → rescue + OS 慣習)が SPEC-0002 に記録される

## todo

### core 層

- [x] `EditorCore::move_lines_up()` / `move_lines_down()`
    - 対象範囲: selection があれば selection がかかる行ブロック全体、なければ cursor 行
    - ブロックを 1 行分上 / 下と入れ替える。buffer 先頭 / 末尾では何もしない
    - cursor(と selection)はブロックと一緒に移動する(VS Code の挙動)
    - **1 回の undo でブロック移動全体が戻り、cursor / selection 位置も復元される**(既存の EditGroup を使い、forward / inverse を 1 グループに)
    - 実装は既存 primitive(`delete_range` + `insert`)の組合せでよいが、末尾行(trailing newline 境界)の入れ替えで行が消えたり空行が湧いたりしないこと
- [x] `EditorCore::insert_line_after()` / `insert_line_before()`
    - 現在行の下 / 上に空行を作り、cursor をその行頭へ(VS Code の `Ctrl/Cmd+Enter` `Ctrl+Shift+Enter` 相当の動き)
    - selection は解除。1 undo グループ
- [x] `src/app/event_loop.rs` の dispatch に上記 4 action を結線(`EditMoveLinesUp/Down` / `EditInsertLineAfter/Before`)

### default keymap 拡張(app 層)

- [x] `src/app/default_bindings.rs` に OS 標準 text 操作を `Source::Default` で追加:
    - `cmd+left` / `cmd+right` → `cursor.lineStart` / `cursor.lineEnd`(+`shift` で selection 版)
    - `cmd+up` / `cmd+down` → buffer 先頭 / 末尾(action が無ければ `cursor.bufferStart` / `cursor.bufferEnd` を EditorAction に追加し dispatch へ結線。palette にも自動で載る)
    - `alt+left` / `alt+right` → `cursor.wordLeft` / `cursor.wordRight`(+`shift` で selection 版)
    - `alt+backspace` → `edit.deleteWordLeft`
    - `cmd+backspace` → `edit.deleteToLineStart`(新 action。EditorCore に行頭まで削除を追加、1 undo グループ)
    - `alt+up` / `alt+down` → `edit.moveLinesUp` / `edit.moveLinesDown`(VS Code default と同じ)
    - `cmd+enter` / `cmd+shift+enter` → `edit.insertLineAfter` / `edit.insertLineBefore`
- [x] SPEC-0002 の該当箇所に方針変更を追記: 「default binding は『rescue + OS 標準 text 操作の慣習キー』とする。editor 固有機能の default は引き続き最小(import / user が上書き可能)」+ Progress 行

## testcases

core(table-driven):

- [x] 中間行での move_lines_down / up の基本動作と undo 往復(cursor 位置含む)
- [x] 選択 2 行ブロックの move_lines_down で両行が動き、selection が維持される
- [x] 先頭行の move_up / 最終行の move_down が no-op(undo スタックにも積まれない)
- [x] 最終行を跨ぐ move(最終行を上へ / 最終行の 1 つ上を下へ)で行数と内容が壊れない
- [x] insert_line_after / before の位置・cursor・undo
- [x] delete_to_line_start(行頭では no-op)
- [x] default bindings が全て parse でき、`cmd+left` が `cursor.lineStart` に解決される(resolver 経由)
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

## notes

- レビュー指摘なし。codex は行移動を delete+insert 合成ではなく EditOp::ReplaceLines (EOL metadata 非破壊の行単位 op) + EditGroup への selection スナップショット追加で実装 (末尾行の罠を構造的に回避)。main agent の PTY E2E 済み (2026-07-06): alt+down で行移動 → cmd+right で行末 → 編集 → 保存が期待どおり

- 新規依存 crate の追加は禁止
- `cursor.bufferStart/End`・`edit.deleteToLineStart` を EditorAction に追加する場合、`Display`/`FromStr`/`ALL` の 3 箇所を揃えること(palette に自動掲載される)
- moveLines の undo は「複数 EditOp を 1 グループに積む」既存機構で表現できるはず。EditKind は Other(グルーピング併合させない)
- OS 標準キーのうち `cmd+*` は super が届く terminal でのみ有効(Ghostty 実測済み)。届かない環境では単に無反応で、capability 検出タスクで警告対象になる
- commit は人間または main agent が行う(AGENTS.md)
