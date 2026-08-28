# TASK-260828: whitespace 可視化 (renderWhitespace 相当)

## 背景

BACKLOG 由来。indent 設定 (TASK-260828-indent-style-config) で tab/space が混在しうるようになり、SSH 先で他人のファイルを直す際に「どちらで書かれているか」を目視確認する手段が必要になった。tab 展開は `app/editor_view.rs` に集約済みで、描画箇所は `draw_line` (wrap off) / `draw_segment` (wrap on) の 2 関数。

## 決定

- 表示モードは **on/off の bool のみ** (VS Code の boundary/trailing/selection 等の中間モードは作らない。必要になったら拡張)
- `wrap` と同じ 3 点セットのパターンに揃える:
    - `[editor] render_whitespace = false` — 起動時 default (config.toml)
    - `view.toggleWhitespace` — palette からの runtime-only トグル (config.toml へ書き戻さない)
    - `EventLoop` の editor-wide flag (per-document にしない)
- マーカー: space → `·` (U+00B7)、tab → `→` (U+2192) + 空白 3 個で幅 4 を維持。style は gutter / truncation marker と同じ dim
- 選択範囲との style 干渉 (BACKLOG の懸念点): 選択中も marker を表示し `reverse + dim` を重ねる。syntax highlight の fg は marker に適用しない (常に dim 単色)
- 対象は **U+0020 (space) と tab のみ**。NBSP・全角空白・thin space 等は marker 化せず通常描画のまま (VS Code の renderWhitespace も同様の割り切り)
- `→` / `·` は East Asian Width が ambiguous な文字だが、既存の truncation marker `…` と同じく「ambiguous-width は 1 セル」前提の terminal 設定を想定する (Codex レビュー指摘の明文化)
- default binding は割り当てない (VS Code も toggleRenderWhitespace は unbound)。palette 経由で実行する
- import: VS Code `editor.action.toggleRenderWhitespace` → `view.toggleWhitespace` を対応表に追加

## 実装

- `keymap/action.rs`: `ViewToggleWhitespace` (`view.toggleWhitespace`) 追加、`ALL` へ登録
- `keymap/vscode_commands.rs`: `editor.action.toggleRenderWhitespace` の対応追加
- `app/config.rs`: `[editor] render_whitespace` の bool パース (不正値は warning + false fallback)、`SETTINGS_TEMPLATE` 更新
- `app/event_loop.rs`: `render_whitespace: bool` フィールド + `set_render_whitespace()`、dispatch arm (status bar に "whitespace: on/off")、`draw` への配線
- `app/editor_view.rs`: `draw_line` / `draw_segment` に `show_whitespace` を配線し、marker 置換 + dim style を適用
- `app/mod.rs`: 起動時に `set_render_whitespace` 適用

## Codex レビュー対応

- [P2] tab の途中まで水平スクロールした状態で全 4 セルを描画しており、行末では余ったセルに選択 style が漏れていた (marker 有効時は `→` の誤表示にもなる) → tab は 1 セル 1 文字の展開なので、隠れた左側セルを切って可視部分のみ描画するよう修正 + 再現テスト追加
- 非ブロッキング指摘 (対象 whitespace の範囲・ambiguous-width 前提) は上の決定に明文化。wrap + tab / syntax fg 非適用のテストも追加

## 検証

- `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`
- 追加テスト: editor_view の marker 描画 (off で従来通り / on で `·`・`→` + dim / 選択中 reverse 維持 / wrap on の segment 描画)、config の valid/invalid、event_loop の toggle dispatch
