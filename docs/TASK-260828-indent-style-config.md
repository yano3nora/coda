# TASK-260828: indent style / width の設定対応

## 背景

TASK-260711-19 で `edit.indent`/`edit.outdent` を実装した際、tab-vs-space と幅の設定は future work としてハードコード (space 4 個、Tab 打鍵はリテラル `\t` 挿入) にしていた。挙動が「打鍵は tab、ブロックインデントは space」と混在していたため、`config.toml` で統一的に設定できるようにする。

## 決定

- `[editor]` に 2 項目を追加 (SPEC-0005 更新済み):
    - `indent_style = "space" | "tab"` — default `"space"`
    - `indent_width` — 整数 1〜16、default `4` (上限は Codex レビュー指摘: 無制限だと Tab 打鍵ごとの `" ".repeat(width)` が巨大値で過大メモリ確保になるため)
- default は **space** に変更。従来の「選択なし Tab はリテラル `\t`」から挙動が変わる (space 4 個挿入)
- 適用範囲は 3 経路で統一: 選択なし Tab のリテラル挿入 / `edit.indent` / `edit.outdent`
- `outdent` は従来通り「先頭 tab 1 個、または先頭 space を最大 `indent_width` 個」除去 — style によらず mixed indent が 1 レベルずつ崩れる挙動を維持
- 不正値は warning + default fallback (黙って壊れないルール)。`indent_width = 0` は space style で全経路が no-op になるため設定ミス扱い

## 実装

- `core/editor.rs`: `IndentStyle` / `IndentConfig` (pure、`Default = space×4`)。`indent()`/`outdent()` は引数で受け取る — core に設定状態を持たせない
- `app/event_loop.rs`: `indent: IndentConfig` フィールド + `set_indent()` (`wrap` と同パターン)。Tab 挿入と dispatch が参照
- `app/config.rs`: `[editor] indent_style` / `indent_width` のパース、`SETTINGS_TEMPLATE` 更新
- `app/mod.rs`: 起動時に `set_indent` 適用

## 追記: palette からの一時切り替え

- `editor.toggleIndentStyle` — space⇔tab を即時トグル (status bar に結果表示)
- `editor.setIndentWidth` — `PromptOverlay` (`PromptPurpose::IndentWidth`) で 1〜`IndentConfig::MAX_WIDTH` を入力
- どちらも `view.toggleWrap` と同じ runtime-only 契約: config.toml へは書き戻さず、次回起動時は設定値 (または default) に戻る
- `.editorconfig` 読み込みは BACKLOG へ (優先順位: palette 一時変更 > .editorconfig > config.toml)。whitespace 可視化も BACKLOG へ

## 検証

- `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` (326 passed)
- 追加テスト: core の tab-style indent / narrow-width outdent、config の valid/invalid テーブル、event_loop の Tab 挿入 + dispatch 配線
