# TASK-260706-09: 初回 dogfood フィードバック修正

260706 first dogfood feedback fixes
===

## asis

TASK-08 の editor を人間が実機(Ghostty)で試した結果、以下のフィードバックを得た。

1. 日本語・絵文字・終了後の terminal 復元は問題なし
2. palette の見た目がモーダルと分かりづらい。リストのスクロールも効かない
3. `bindings.json` に VS Code command 名(`cursorDown`)を書くと unknown command になり、正しい書き方が分からない
4. `app.quit` が保存済みのとき無反応
5. 新規作成したファイルの末尾に改行がなく、zsh が `%` を表示する

## tobe

- 上記 2〜5 が解消され、実機で editor が期待どおり動く

## todo

- [x] **palette 実行 action の QuitDecision 握りつぶしバグ修正**(フィードバック 4 の根因): `handle_palette_key` が bool を返し `let _ = self.dispatch(action)` で終了指示を捨てていた。`Option<QuitDecision>` を返して run loop まで伝播させる
- [x] **palette のモーダル化**(フィードバック 2): `╭─ Command Palette ─╮` の枠 + 内部空白塗り(下の editor テキストが透けない)+ 選択行のフルワイドバー + 件数表示
- [x] **palette リストのスクロール**(フィードバック 2): `scroll_offset`(選択が常に見える窓計算、pure 関数)を追加し table-driven test で固定
- [x] **新規 buffer の trailing newline**(フィードバック 5): `TextBuffer::default()` を `trailing_newline: true` に変更(POSIX 慣行。読込ファイルは従来どおり検出値を維持)
- [x] **VS Code command 名の suggestion**(フィードバック 3): `keymap/vscode_commands.rs` に SPEC-0004 変換表を実装(importer と共用予定)。`unknown command \`cursorDown\` — use \`cursor.down\`` と案内する
- [x] **config 警告の短縮**(フィードバック 3 付随): status bar の幅を長い絶対パスが食い潰していたため、警告は `bindings.json: ...` 形式に変更

## testcases

- [x] unit: palette から `app.quit`(clean 状態)で `QuitDecision::Quit` が返る(regression test)
- [x] unit: `scroll_offset` の窓計算 5 ケース
- [x] unit: `cursorDown` の issue Display に ``use `cursor.down``` が含まれる
- [x] unit: `action_for_vscode_command` が SPEC-0004 の command を map し未知を None にする
- [x] PTY: palette 経由 `app.quit` で pipe を開いたまま coda プロセスが終了する(EOF 待ちでないことを pgrep で確認)
- [x] PTY: 新規ファイル保存が `hello\n`(末尾 0x0a)になる
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

## notes

- PTY 検証の教訓: `script(1)` は子プロセス終了後も stdin EOF まで残るため、「exit までの経過時間」では quit を検証できない。`pgrep` でプロセス生死を直接見ること
- editor 本体のスクロール(viewport)は TASK-08 実装に含まれており、フィードバック 2 は palette リストの話と解釈した。もし editor 側のスクロール不具合だった場合は再報告を受けて別タスク化する
- 80 桁端末では長い warning が依然 truncate される。warning の全文閲覧手段(inspector / report 画面)は importer タスク以降で扱う
- 修正は main agent が直接実施(codex 不使用。診断と修正が一体の作業のため)
