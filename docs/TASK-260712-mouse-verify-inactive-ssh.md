# TASK-260712: mouse / keymap verify / inactive 表示 / SSH bootstrap (backlog P2)

[backlog](TASK-999999-backlog.md) P2 の 4 項目を一括で実施する。

## asis

- mouse: 未実装。decoder に SGR mouse の decode はなく、click / drag / wheel は
  すべて terminal 側の挙動に任せている
- keymap verify: ADR-0007 §2(c) で「deliverability の真実は実測」と決めたが、
  実測手段は `inspect-key` (raw dump) しかなく、binding 単位の検証がない
- `suggestVisible` / `quickOpenVisible` を when に持つ imported binding は
  import 成功 (Imported bucket) するが、runtime context が常に false のため
  永久に発火しない。report にもその事実が出ない (黙って死んでいる)
- SSH 先や container への導入は README の手動 curl + tar 手順のみ

## tobe

- SGR mouse (DECSET 1002/1006) で click = カーソル移動、drag = selection、
  wheel = scroll。Shift+drag は terminal 選択として素通し (ADR-0008 §3)
- `coda keymap verify` で imported binding の chord を実際に押して
  delivered / mismatch / skipped を記録し、report を出力する
- 永久 inactive な imported binding が import report で明示される
- `scripts/bootstrap.sh` を `curl | sh` で実行すると OS/arch を判定して
  release asset を取得・展開し、`~/.local/bin` へ導入できる

## todo

### mouse support (SGR)

- [x] `decoder.rs`: `InputEvent::Mouse(MouseEvent)` と `CSI < Cb;Cx;Cy M|m` の
      decode (button / drag / wheel / modifiers)。分割到着は Incomplete 扱い。
      不正 params・横 wheel (66/67)・no-button (1003) は `Key::Unknown` fallback
- [x] `input/mod.rs`: `MouseReportingGuard` (`?1002h ?1006h` / drop で `l`)。
      `raw_terminal.rs` の signal cleanup に disable を追加 (alt-screen leave より前)
- [x] `editor_view.rs`: screen→buffer 逆変換 (`screen_to_buffer`)。gutter click は
      行頭へ snap、左スクロール / wrap segment / tab・全角幅を考慮し、EOF/EOL は clamp。
      非最終 segment の行末越え click は同 visual 行の末尾 grapheme に留める
- [x] `event_loop.rs`: `handle_mouse_event`。left click = カーソル移動 +
      selection 解除 (`EditorCore::set_cursor_position` 新設)、drag =
      `select_range(anchor, pos)`、wheel = 3 行 scroll。Shift 付き mouse event は
      無視 (terminal 素通し)。palette / prompt / inspector 表示中は無視
- [x] wheel scroll は cursor を追わない free scroll (`follow_cursor` フラグで
      `ensure_cursor_visible` を抑止)。次の keystroke / click / paste で再追従

### keymap verify

- [x] 純粋 state machine `VerifySession` (期待 chord 列 + KeyEvent 入力 →
      delivered / mismatch(実際に届いた chord) / skipped)
- [x] `Command::KeymapVerify`: imported binding の chord (sequence は各 chord に
      分解して dedupe) を raw mode + kitty protocol で 1 chord ずつ実測。
      Esc = skip、Ctrl+C = 中断 (期待 chord が Esc / Ctrl+C なら一致判定を優先)
- [x] 結果 summary を stdout と `import-reports/latest-verify.txt` へ出力。
      super 不達が出た場合は `--cmd=ctrl` を案内。非 TTY は明確なエラーで exit 1
- [x] verify 結果の resolver への反映 (chord 単位 deliverability 保存) は
      ADR-0007 Open Question のまま見送り

### inactive 表示の明確化

- [x] `EditorContext::RESERVED_FALSE_KEYS` (`suggestVisible` / `quickOpenVisible`)
- [x] `ContextPredicate::positive_term_matching` で肯定参照のみ検出
      (`!suggestVisible` は inactive にしない)
- [x] `ImportReport.inactive_contexts` bucket を追加 (generated へは書き出す)。
      render / summary / total_classified を更新

### SSH bootstrap script

- [x] `scripts/bootstrap.sh`: `uname` で OS (macos/linux) / arch (x64/arm64) 判定、
      version 引数 (default: latest release の redirect から解決、jq 不要) で asset を
      取得、sha256 検証 (ツールがあれば)、`~/.local/bin` (または `CODA_INSTALL_DIR`) へ展開
- [x] POSIX sh 互換 (dash -n も通過)、`set -eu`、curl/wget fallback、失敗時は明確なメッセージ
- [x] README に `curl | sh` 導入手順を追記

## testcases

- [x] decoder: SGR mouse の table-driven test (press / drag / release / wheel /
      shift 付き / ctrl 付き / key 混在 / 不正 params / 分割到着 / key channel 非漏出)
- [x] screen_to_buffer: wrap off/on、gutter 内 click、EOF/EOL 越え、tab・全角、
      横スクロール加算、非最終 segment clamp
- [x] VerifySession: 一致 / 不一致 / skip / 中断 / 制御キー期待 chord の遷移 test
- [x] inactive: `suggestVisible` 依存 binding が inactive bucket に載り、
      `editorFocus && suggestWidgetVisible` のような複合 when も検出される
- [x] bootstrap.sh: `sh -n` / `dash -n` 構文検査、`--help` 動作、OS/arch 判定ロジック確認
- [x] `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` pass (269 tests)

## notes

- wheel の単位は 3 行固定で開始 (ADR-0008 Open Question。加速はやらない)
- double / triple click (単語・行選択) は v0.2 でも見送り継続 (ADR-0008)
- mouse reporting 有効中は terminal ネイティブ選択が奪われるため、Shift+drag
  素通しを README / help に明記する
- verify 結果の永続化形式 (TERM_PROGRAM キー等) は ADR-0007 Open Question を
  維持し、本 TASK では report 出力まで
