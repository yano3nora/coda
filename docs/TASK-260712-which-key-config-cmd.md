# TASK-260712: which-key / config 残項目 / import --cmd (backlog P1)

[backlog](TASK-999999-backlog.md) P1 の 3 項目を一括で実施する。

## asis

- `:which-key`: pending sequence の候補は status bar の `message` に
  `pending: <keys> -> <action>, ...` として詰め込まれるのみ (`event_loop.rs`)。
  候補が多いと読めない
- `config.toml`: `[appearance] theme` と `[editor] wrap` のみ結線済み。
  SPEC-0005 の `[keymap] sequence_timeout_ms` / `[keymap] palette_key` /
  `[terminal] capability_warning` は未実装 (struct にも存在しない)
    - sequence timeout は `event_loop.rs` の `SEQUENCE_TIMEOUT` 定数 (800ms) 固定
    - palette 便宜キーは `default_bindings.rs` の rescue binding `ctrl+space` 固定
    - capability warning は常に表示 (gate なし)
- import: `cmd+*` binding は super のまま取り込まれ、super 非対応 terminal では
  disabled になるだけで退路がない (ADR-0007 §3 は Proposed のまま未実装)

## tobe

- sequence prefix 入力中、続く候補一覧が overlay で見える (which-key)。
  timeout で発火する exact match も表示する
- `config.toml` で sequence timeout・palette 便宜キー・capability warning の
  有無を制御できる。parse 不能値は警告して default に fallback (黙って壊れない)
- `coda keymap import vscode <path> --cmd=keep|ctrl|both` で cmd 戦略を選べる
  (ADR-0007 §3)。super が届かない環境の report は `--cmd=ctrl` を提案する

## todo

### :which-key

- [x] `src/app/which_key.rs`: 純関数 `which_key_lines(pending, candidates, exact)`
      と boxed overlay 描画 (inspector overlay の様式を踏襲)
- [x] `event_loop.rs`: `ResolveResult::Pending` 時に `message` 詰め込みをやめ、
      which-key overlay を表示。解決・timeout・NoMatch で消す
      (描画のたびに pending prefix を再 resolve するので cache 不整合が起きない)
- [x] 候補行は「継続キー → action」形式。exact があれば `(wait)` 行を先頭に出す

### config.toml 結線

- [x] `AppConfig` に `sequence_timeout_ms: u64` (default 800) /
      `palette_key: Option<KeyEvent>` / `capability_warning: bool` (default true)
- [x] `[keymap]` / `[terminal]` section の読み取り。型不正は warning + default
      (既存 `editor.wrap` と同パターン。`sequence_timeout_ms = 0` も不正扱い)
- [x] `palette_key` は `parse_key_chord` で検証し、rescue の `ctrl+space` binding
      を置き換える。F1 は常に有効のまま (SPEC-0002)
- [x] `EventLoop` へ setter で注入 (`set_wrap` と同パターン)
- [x] `SETTINGS_TEMPLATE` に新 section を追記し round-trip test を維持

### import --cmd

- [x] `ImportOptions` に `cmd: CmdStrategy` (Keep default / Ctrl / Both)。
      `--cmd=<v>` を parse、不正値は InvalidUsage。USAGE 更新
- [x] `ctrl`: super chord を ctrl chord へ変換して取り込む。変換後の衝突は
      conflict として report
- [x] `both`: 原本と ctrl 変換の両方を登録。synthesized 件数を数えて
      `entries.len() + synthesized == total_classified()` の invariant を維持
- [x] `keep` + super 非対応環境: disabled reason に `--cmd=ctrl` の提案を含める
- [x] SPEC-0004 / SPEC-0005 / SPEC-0002 を実装に合わせて更新。ADR-0007 を
      Accepted に更新 (--cmd と verify の実装をもって)

## testcases

- [x] which-key: 純関数の table-driven test (候補 1/複数、exact あり/なし、
      上限超過時の "… N more") + event loop 経由の描画 test
- [x] config: 各項目の 有効値 / 型不正 (warning + default) / 欠落 (default) test
- [x] palette_key 変更後も F1 で palette が開く
- [x] `--cmd=ctrl`: `cmd+s` が `ctrl+s` として import され、既存 `ctrl+s` binding
      と衝突する場合 conflict bucket に載る (`cmd+ctrl+s` の縮退・sequence 変換も)
- [x] `--cmd=both`: 2 binding 登録と report 件数の整合
- [x] `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` pass (269 tests)

## notes

- `:which-key Ctrl+j` (SPEC-0005 の引数付き command 形式) は palette が引数を
  サポートしていないため見送り。自動表示のみ実装し、必要になったら再検討
- OS-reserved key (Cmd+Q 等) の Unsupported 分類 (ADR-0007 §4) は backlog 外の
  ため本 TASK では扱わない
