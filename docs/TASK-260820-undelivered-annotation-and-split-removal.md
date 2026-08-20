# TASK-260820: undelivered binding の dim + 注記 / split action の削除

## 目的

1. split 系 action の削除: BACKLOG で「split は coda の責務範囲ではない」と
   決めた後も `view.splitVertical` / `view.focusNextSplit` /
   `view.focusPreviousSplit` が action 一覧と VS Code import 対応表に残っていた。
   event loop に dispatch 実装すら無い dead action で、palette に「選べるが
   何も起きない」command として露出していたため削除する。対応する VS Code
   command (`workbench.*`) は import 時 `Ignored: outside editor scope` に
   分類される (機能未実装の `Unsupported` ではなく責務範囲外の `Ignored`)。
2. palette / which-key で「この terminal では届かない binding」を dim + 注記
   する (environment info panel の段階 2)。届かないと知るべき瞬間は palette /
   which-key で binding を見た瞬間。SPEC-0003 の「黙って壊さず disabled を
   明示する」を表示面まで届ける。

## 設計

- データ源は既存の 2 つを流用し、新しい検出は作らない:
    - quirks 検出: chord 集合を 2 つに分ける。`blocked_chords` (全 quirk
      trigger — 単打鍵で解決されない trigger も sequence 内の chord を壊すため
      全件) と `harmless_single_chords` (paste 委譲・同一 action への translate
      など、単打鍵 binding に限り実質動作する trigger の免除リスト)。
      免除は sequence 内には適用しない (terminal の書き換えは chain を必ず壊す)
    - keymap verify 実測: `disable_chords` が resolver から除去した binding を
      捨てずに `EventLoop.disabled_bindings` に保持 (extend — 複数回呼ばれても
      落とさない) し、palette の fallback 表示に使う
      (除去自体は従来どおり — mismatch chord は物理的に届かないため)
- warn 判定 (`quirk_intercepted_action`) は `ghostty_intercept_report` と
  `harmless_single_chords` で共有し、info panel と palette 表示が食い違わない
  ようにする
- palette: `PaletteItem` に `undelivered` flag を追加。action 名は通常表示の
  まま (palette からの実行は常に可能)、binding 列 + `✗ undelivered` 注記のみ
  dim。live binding が無く verify で除去された binding がある action は、
  その除去済み binding を dim + 注記付きで表示する。live binding が残っている
  action でも、verify で失われた binding は `✗ <key> undelivered` の dim 注記
  として併記する (失った key を黙って消さない)
- 起動順: `set_palette_key` を `disable_chords` より先に実行する。逆順だと
  default の palette key が verify で除去済みのとき、config の palette_key が
  黙って無視される
- which-key: 続きの chord が intercept される候補は行全体を dim + 注記
  (その sequence は完了し得ないため)

## 検証

- `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test`
- palette: quirk trigger を含む binding と verify 除去 binding が
  undelivered になる table-driven test
- which-key: undelivered chord を含む候補行の注記と dim flag の test

## 非対応・リスク

- verify で除去された binding は which-key 候補に出ない (resolver に無い)。
  palette 側の注記で足りると判断
- quirk warn 判定は `EditorContext::default()` で解決する (info panel と同じ
  代表 context)。context 依存 binding の誤判定は許容
