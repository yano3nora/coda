# TASK-260706-13: Clipboard (OSC 52 copy + bracketed paste)

260706 clipboard
===

## asis

- copy / cut / paste の action が存在せず、terminal からの貼り付け(bracketed paste)も key 入力として流れ込む(TASK-08 時点の既知課題)
- 方針は ADR-0008 で決定済み: 書込は OSC 52(SSH でも動く)、読出は terminal の bracketed paste を第一経路、内部 clipboard は常に保持

## tobe

- `Ctrl+C/X/V`(default)で copy / cut / paste が動き、copy した内容が **OS clipboard に入る**(OSC 52)
- `Cmd+V`(terminal の paste)が bracketed paste として安全に挿入される(key 解決を通らない)
- SSH 先でも copy が手元の OS clipboard に届く(OSC 52 の性質。実機確認は人間)

## todo

### input 層

- [ ] bracketed paste mode の管理: `CSI ?2004h` を有効化し、drop / signal 時に `?2004l` で解除する(`KeyboardProtocolGuard` と同じ AtomicBool + RAII パターン。有効化は kitty push と同タイミングでよい)
- [ ] decoder に paste 対応を追加: `\x1b[200~ ... \x1b[201~` の envelope を検出する
    - 既存 `drain_key_events` を `drain_input_events(buffer) -> Vec<InputEvent>` に拡張(`enum InputEvent { Key(KeyEvent), Paste(String) }`。既存呼び出しは互換 wrapper でも移行でもよいが、event loop は InputEvent を受ける)
    - envelope が read 境界で分割されても正しく結合する(終端 `201~` が来るまで buffer に保持)
    - **paste 内容は一切 key 解決しない**(SPEC-0003)。内容中の escape sequence も文字列として扱う。不正 UTF-8 は lossy 変換でよい(表示用入力のため)
    - CRLF / CR は LF に正規化する

### core / app 層

- [ ] `EditorAction` に `edit.copy` / `edit.cut` / `edit.paste` を追加(Display / FromStr / ALL)
- [ ] `EditorCore`:
    - `copy_text() -> Option<String>`: selection のテキスト。**selection が無い場合は現在行全体 + 改行**(VS Code の line copy)
    - `cut()`: copy 相当のテキストを返しつつ削除(selection 無しは行削除)。1 undo グループ
    - paste は既存 `insert_text`(selection 置換込み)で足りる
- [ ] 内部 clipboard(`String`)を event loop に保持。copy / cut で更新、`edit.paste` で挿入
- [ ] OSC 52 書込: copy / cut 時に `\x1b]52;c;{base64}\x07` を stdout に出す
    - base64 encoder は自前実装(新規依存禁止。標準 alphabet、padding あり。RFC 4648 のテストベクタで検証)
    - 巨大 selection の対策として 1MB 超は OSC 52 送信をスキップし内部 clipboard のみ(status bar に明示)
- [ ] event loop:
    - `InputEvent::Paste(text)` → palette / search overlay 表示中は「改行を除去して focus 中の入力欄へ」、editor なら `insert_text`(1 undo グループ)
    - copy / cut 実行時に status bar へ `copied` / `cut` 表示
- [ ] default bindings: `ctrl+c` → `edit.copy`、`ctrl+x` → `edit.cut`、`ctrl+v` → `edit.paste`(`Source::Default`。ADR-0002 で温存していた `Ctrl+C` の本来用途)

## testcases

- [ ] base64: RFC 4648 ベクタ(`""`→`""`、`"f"`→`"Zg=="`、`"fo"`→`"Zm8="`、`"foo"`→`"Zm9v"`)+ 日本語バイト列
- [ ] paste envelope: 一括到着 / `200~` と本文と `201~` が 3 chunk に分割 / 本文に `\x1b[A` を含む(key にならず文字列として出る)/ CRLF 正規化
- [ ] paste 中に通常 key が混ざらない(envelope 前後の key は正しく Key event になる)
- [ ] `copy_text`: selection あり / なし(行 copy)/ 空 buffer
- [ ] `cut`: selection 削除と行削除、undo 1 回で復元
- [ ] OSC 52 出力形式(`\x1b]52;c;Zm9v\x07`)
- [ ] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

手動(main agent PTY + 人間):

- [ ] PTY: bracketed paste envelope 送信でテキストが挿入され、内容の escape が実行されない
- [ ] PTY: ctrl+c で OSC 52 sequence が出力に現れ、base64 decode すると copy したテキストに一致
- [ ] 人間: Ghostty で copy → 他アプリに Cmd+V で貼れる(clipboard-write の許可 prompt が出る場合あり)

## notes

- 新規依存 crate の追加は禁止(base64 は 20 行程度の自前実装で足りる)
- OSC 52 の terminal 側許可(Ghostty `clipboard-write`)は環境依存。拒否されても内部 clipboard は機能する(ADR-0008 の fallback 設計)
- OSC 52 read(terminal からの clipboard 読出)は MVP では実装しない(ADR-0008)
- `Ctrl+C` はもはや interrupt ではない(SIGINT は raw mode で無効化済み、rescue は F1 / Ctrl+Space / Esc)。ADR-0002 の設計どおり
- commit は人間または main agent が行う(AGENTS.md)
