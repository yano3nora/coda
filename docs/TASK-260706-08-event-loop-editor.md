# TASK-260706-08: Event loop・editor view・minimal palette 統合

260706 event loop + editor + palette
===

## asis

- core(buffer/movement/undo/EditorCore)、input(decoder)、keymap(resolver/loader)、ui(renderer)が個別に完成
- `coda <path>` は「not implemented」を返すだけで、editor として起動しない

## tobe

- `coda <path>` で editor が起動し、編集・保存・終了ができる(ADR-0004 実装順序 step 5 相当)
- `F1` / `Ctrl+Space` の command palette が rescue 入口として機能する(SPEC-0002)
- `~/.config/coda/bindings.json` の user binding が効く

## todo

### input 拡張

- [ ] `src/input/decoder.rs` に streaming API を追加する(既存 `decode_key_events` は inspect-key 用に残す)
    - `drain_key_events(buffer: &mut Vec<u8>) -> Vec<KeyEvent>`: 先頭から decode できる分だけ event 化して消費し、末尾の不完全 sequence は buffer に残す
    - `flush_pending_escape(buffer: &mut Vec<u8>) -> Option<KeyEvent>`: buffer が lone ESC(`[0x1b]`)のとき `Esc` を返して空にする(poll timeout 時に呼ぶ。SPEC-0003 の ESC timeout の解決)
- [ ] `KeyboardProtocolGuard` を inspect-key 専用から `input/` の公開 API に昇格する(event loop でも使う)

### app 統合

- [ ] `src/app/file.rs`: `open(path) -> (TextBuffer, LoadInfo)`(存在しないパスは空 buffer + 新規フラグ)、`save(path, &TextBuffer)`。`LoadError::InvalidUtf8` はエラーメッセージで終了
- [ ] `src/app/config.rs`: `$XDG_CONFIG_HOME/coda/bindings.json`(未設定時 `~/.config/coda/`)を読む。ファイルなし → 空。parse 失敗・issues → 起動後に status bar へ警告表示(起動は止めない)
- [ ] `src/app/default_bindings.rs`: `Source::Default` の最小 binding 集(SPEC-0001「default は極小」)
    - 移動: `up/down/left/right/home/end/pageup/pagedown`(+`shift` で selection、`ctrl+left/right` で word、`ctrl+shift+left/right` で word selection)
    - 編集: `backspace` `delete` `ctrl+backspace`(word 削除)`ctrl+z`(undo)`ctrl+shift+z`(redo)`ctrl+a`(select all)
    - ファイル: `ctrl+s`(save)`ctrl+q`(quit)
    - rescue(`Source::Rescue`): `ctrl+space`(palette.open)、`escape`(palette close。`when: commandPaletteVisible`)
- [ ] `src/app/palette.rs`: command palette 状態と描画データ
    - 全 `EditorAction` を対象に、入力文字列の**部分一致**(case-insensitive)で filter
    - 表示: action 名 + 現在 bind されている key(resolver の binding から逆引き。複数あれば最優先の 1 つ)
    - `up/down` で選択移動、`enter` で実行して close、`escape` で close
    - palette 表示中は `commandPaletteVisible = true` / `editorFocus = false` の context にする
- [ ] `src/app/editor_view.rs`: `EditorCore` + viewport を `Screen` に描く
    - viewport: `top_line` の垂直 scroll と、cursor が常に見える範囲の水平 scroll(display column 基準)
    - Tab は幅 4 で展開(ADR-0009 Open Question を 4 で決定)
    - selection 範囲は `Style { reverse: true }`
    - 最下行に status bar(reverse): `filename [+] | Ln {line},Col {col} | 警告/メッセージ | pending keys`
- [ ] `src/app/event_loop.rs`: メインループ
    - 初期化順: RawModeGuard → KeyboardProtocolGuard → AltScreenGuard(復元は逆順。signal 復元も同順で対応済みであること)
    - `libc::poll` で stdin を待つ。timeout は「sequence 待機中は残り時間(800ms 固定でよい)、それ以外は resize フラグ確認用に 100ms」
    - key 処理の順序(SPEC-0002 を厳密に):
        1. `F1` → palette toggle(**resolver を経由しない**)
        2. palette 表示中は palette 入力(文字 / up/down / enter / escape)を優先
        3. pending buffer に key を積み resolver へ。`Matched` → 実行、`Pending` → status bar に候補表示して待機、`NoMatch` → pending をクリアして再解決(先頭 1 key を捨てる単純規則でよい)
        4. sequence timeout 時: `Pending.exact` があれば実行、なければ破棄
        5. どの binding にも当たらない `Char`(ctrl/alt/super なし)で `textInputFocus` → `insert_text`。`enter` → `"\n"` 挿入、`tab` → `"\t"` 挿入
    - action dispatch: `EditorAction` → `EditorCore` / app 操作(`file.save` / `app.quit` / `palette.open`)。未実装 action(search 系・view 系等)は status bar に `"{action}: not implemented yet"` を表示(黙って無視しない)
    - `app.quit`: 未保存変更があれば 1 回目は status bar 警告、変更なしまたは 2 回連続で終了
    - resize フラグ → terminal_size 再取得 → 全再描画
- [ ] `src/app/mod.rs`: `coda <path>` で event loop を起動する(引数なしは usage 表示。複数ファイルは先頭のみ開いて警告)
- [ ] unit test(pure な部分): palette filter、default bindings が全て parse 可能、`drain_key_events` の分割入力(escape sequence が 2 read に跨るケース)、quit の未保存ガード状態遷移

## testcases

自動:

- [x] `drain_key_events`: `\x1b[1;5` + `C` の 2 回に分けた入力で `Ctrl+Right` が 1 個出る(chunk 跨ぎ)
- [x] `flush_pending_escape`: lone ESC が timeout で `Esc` になる
- [x] palette filter: `"sav"` で `file.save` / `file.saveAs` が残り、大文字入力でも一致する
- [x] default bindings 全 entry が parse エラーなく `Resolver` に載る
- [x] quit ガード: modified 状態の quit 1 回目 → 継続、2 回目 → 終了判定
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

手動(人間 + main agent の PTY smoke):

- [x] `cargo run -- /tmp/test.txt` で起動し、文字入力・矢印移動・`ctrl+s` 保存・`ctrl+q` 終了ができる
- [x] `F1` → palette が開き、`sav` 部分一致 → enter で保存される
- [ ] 日本語・絵文字を含む行でカーソル移動が乱れない
- [x] 未保存で `ctrl+q` → 警告が出て、もう一度で終了する
- [ ] 終了後に shell が乱れない(alt screen / raw mode / kitty protocol の復元)

## notes

- main agent の PTY 検証済み (2026-07-06): 入力→Ctrl+S 保存→Ctrl+Q 終了、F1→palette→sav→Enter 保存、未保存 quit ガード (警告→2 回目で終了、ファイル無変更)、alt screen enter/leave の対応。日本語行のカーソル挙動と実 terminal の復元は人間の目視確認待ち

- 新規依存 crate の追加は禁止
- event loop は「状態を持つ薄い層」に徹し、判定ロジック(palette filter・quit ガード・pending 規則)はテスト可能な関数に切り出すこと
- clipboard(copy/cut/paste + OSC 52)、search overlay、multi-buffer は後続タスク(ADR-0008 / SPEC-0001)。本タスクの action dispatch では not implemented 表示でよい
- `Ctrl+c` は SIGINT を無効化済みのため通常 key として届くが、本タスクでは binding を割り当てない(将来 copy 用に温存。ADR-0002)
- commit は人間または main agent が行う(AGENTS.md)
