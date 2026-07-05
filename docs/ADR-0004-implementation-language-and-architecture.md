# ADR-0004: Implementation Language and Module Architecture

- Status: Proposed
- Date: 2026-07-05

## Context

本製品は terminal raw input の decode、Unicode / grapheme 単位の cursor 処理、単体 binary 配布(SSH 先での利用)を必要とする。また ADR-0002 のとおり、中核は描画ではなく keymap の import / resolution であり、その部分を UI から独立して test できる構造が必要になる。

## Decision

### 1. 実装言語は Rust

- terminal input / escape sequence 処理に向く
- Unicode / grapheme 単位の cursor 処理を安全に扱いやすい
- macOS / Linux 向け単体バイナリ配布がしやすい
- event loop、buffer、rendering の性能上の余裕がある
- TUI 周辺の ecosystem がある

ただし TUI framework に製品の設計を引っ張られないこと。framework の選定は renderer 実装時まで遅延してよい。

### 2. Module 構成

```text
core/      buffer / cursor / selection / undo / search / replace
input/     terminal-decoder / keyboard-capabilities / key-chord / key-sequence
keymap/    parser / resolver / context / conflict-detector / vscode-importer / report
highlight/ syntect wrapper / theme / color-capabilities (表示専用。ADR-0006)
ui/        renderer / status-bar / tab-bar / overlays / split-view
app/       event-loop / commands / config
```

### 3. 依存境界: `keymap/` は UI framework から独立させる

- 本製品の中核は描画ではなく keymap import / resolution
- keymap resolver は unit test しやすくする必要がある
- 将来的に別 TUI renderer や GUI shell に載せ替えられる余地を残す

`core/` と `keymap/` は `ui/` に依存してはならない。`input/` は terminal 依存を持つが、出力(normalized key event)は環境非依存の型とする。`highlight/` は `core/` の行データを読むだけで、`core/` / `keymap/` から参照されてはならない(表示専用。ADR-0006)。

### 4. Event flow

```text
Terminal input
  -> input decoder
  -> normalized key event
  -> active editor context
  -> keymap resolver
  -> editor action
  -> state update
  -> renderer
```

### 5. 実装順序は keybinding engine が先

最初に作る成果物は editor 画面ではない。VS Code binding 1 件を入力すると、現在の terminal で「受信可能か / どの raw input になるか / どの internal action に変換されるか / fallback では衝突するか」を正確に説明できる keybinding engine を先に完成させる。

1. terminal raw input を表示する最小プログラム
2. normalized key event と modifier 判定
3. single-buffer text editing
4. cursor / selection / undo
5. minimal command palette(rescue 入口。`F1` hardcode + action 検索・実行)
6. context-aware keymap resolver
7. user binding JSON loader
8. VS Code keybindings importer
9. import report
10. find / replace
11. syntax highlighting(syntect + dark / light theme。ADR-0006)
12. multi-buffer tabs
13. split view

## Alternatives Considered

- **Go(bubbletea 等)**: 配布性は同等だが、grapheme / escape sequence 処理の低 level 制御と GC pause の考慮で Rust に劣後。ecosystem は魅力だが framework 主導の設計になりやすい。
- **TypeScript / Node(ink 等)**: SSH 先に Node runtime を要求する時点で「軽量な単体 binary」の要件に反する。不採用。
- **Zig**: 言語・ecosystem の成熟度が足りず、Unicode 処理を自前実装する範囲が広すぎる。不採用。

## Consequences

### 良くなること

- keymap resolver / importer を pure logic として test できる
- terminal 依存が `input/` に隔離され、capability の mock が容易になる

### リスク・コスト

- Rust の開発速度は script 言語より遅く、MVP 到達まで時間がかかる
- TUI framework を採用する場合、その event model と自前 event flow の整合を取る設計コストがある

## Migration Notes

Greenfield のため影響なし。

## Open Questions

- TUI framework(ratatui 等)を使うか、renderer を自前実装するか。keybinding engine 完成後に判断する。
- Windows(ConPTY)対応を scope に入れるか。MVP は macOS / Linux のみとする想定。

## Progress

- 2026-07-05: 初版作成(Proposed)。
