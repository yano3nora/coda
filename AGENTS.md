# AGENTS - Development Guide
## Overview
- `coda` (working name): keymap-first な TUI text editor。GUI editor (VS Code 等) で育てた keybinding を import し、terminal 内での短時間編集 (SSH 先・git rebase・設定ファイル修正) を筋肉記憶のまま行えるようにする
- 技術スタック: Rust (edition 2024)、macOS / Linux 向け単体バイナリ。TUI framework は未選定 (keybinding engine 完成後に判断。ADR-0004)
- 最重要ドキュメント:
    - 製品の目的と Non-goals: `docs/ADR-0001-keymap-first-tui-editor.md`
    - keybinding 解決モデル: `docs/ADR-0002-*.md` / `docs/SPEC-0002-*.md`
    - MVP スコープと受け入れ基準: `docs/SPEC-0001-mvp-scope.md`
- 関連 repo / 移植元: なし (greenfield)。参照仕様は VS Code `keybindings.json` と kitty keyboard protocol

### 🎯 Role & Objective
あなたはエキスパートソフトウェアエンジニアとして、この repo の設計・実装・テストを行うこと。

### 🚨 CRITICAL: Architecture
- **Keymap-first**: 本製品の中核は描画や編集機能の豊富さではなく「keymap の import と解決」。迷ったら keybinding engine の正確さ・test 容易性・説明可能性 (`:which-key` / `:inspect-key`) を優先する
- **依存境界**: `core/` と `keymap/` は `ui/` に依存してはならない。`input/` のみが terminal に依存し、出力は環境非依存の normalized key event。keymap resolver は raw bytes を直接見ない (ADR-0004)
- **状態管理**: editor state は `app/event-loop` が所有し、`input -> context -> resolver -> action -> state update -> render` の単方向 flow を守る。global singleton を作らない
- **失敗モード (前提にする)**: terminal capability 不足・設定ファイル破損・import 失敗・巨大ファイル。いずれでも「必ず起動する」「黙って壊れない (disabled / conflict を明示する)」「`F1` の command palette は常に効く」を守る
- **YAGNI / Scope creep 禁止**: LSP・plugin・file tree・Git UI・workspace・VS Code 全 command 互換へ広げない。syntax highlighting はあるが syntax-aware 編集 (fold / rename) は作らない (ADR-0006)。機能追加は「terminal での短時間編集を改善するか」だけで評価する (ADR-0001 Non-goals)

### 📂 Code Organization Constraints
- **`core/`**: buffer / cursor / selection / undo / search / replace。pure logic、terminal・UI 非依存
- **`input/`**: terminal-decoder / keyboard-capabilities / key-chord / key-sequence。terminal 依存はここに隔離
- **`keymap/`**: parser / resolver / context / conflict-detector / vscode-importer / report。unit test 必須の中核
- **`highlight/`**: syntect wrapper / theme / color-capabilities。**表示専用** — syntax 情報を編集操作・keymap 解決に使ってはならない (ADR-0006)
- **`ui/`**: renderer / status-bar / tab-bar / overlays / split-view
- **`app/`**: event-loop / commands / config
- **型 / 境界**: 層間の受け渡しは normalized key event・`EditorContext`・`EditorAction`・`KeyboardCapabilities` の boundary 型で行う (SPEC-0002 / SPEC-0003)

### 🛠️ Workflow & Development Rules
- **Secrets**: 企業名・製品名・機密情報などがあった場合、コード上に残らないように汎用・一般名称に差し替えること。
- **Commit**: `git commit` は基本的には人間判断で行うため、指示されたとき以外はコミットせず人間に判断を委ねること。
- **Push / Publish**: `github push` や `npm publish` など、外部へ公開・配布する操作は Agent が実行しない。人間が判断して実行する。
- **Testing**: タスク完了前に実行する検証を書く
    - lint / format: `cargo fmt --check` と `cargo clippy -- -D warnings` を通すこと
    - unit test: `cargo test`。特に `keymap/` (resolver / importer / conflict-detector) と `input/` の decode は raw bytes・binding 入力に対する table-driven test を必須とする
    - integration / e2e: MVP 期は importer に実 VS Code `keybindings.json` サンプルを食わせる snapshot test (import report の出力比較) を整備する。TUI の e2e は当面対象外
    - bugfix 時: 修正前に失敗する再現テストを書き、修正後に pass することを確認する
- **Documentation**:
    - 技術的な意思決定や検討は `docs/ADR-XXXX-*.md` に記録し、大きな変更の前には既存 ADR を確認する
    - 設計・仕様の検討・決定事項は `docs/SPEC-XXXX-*.md` に記録する
    - 原則、全開発タスクが適切な粒度で `docs/TASK-YYMMDD-*.md` に残るようにする
    - 画像などは `docs/assets/` へ配置してリンクする
- **Versioning / Release**: MVP 到達まで `0.x`。tag / release / crates.io publish は人間が判断・実行する (Push / Publish 規則に準ずる)

## Domains
- `binding` / `KeyChord` / `KeySequence`
    - keybinding は `Key + Context -> Action` で解決する。単一打鍵が chord、`Ctrl+x Ctrl+s` のような連続が sequence (SPEC-0002)
- `EditorContext` / `ContextPredicate`
    - focus・overlay・selection 等の現在状態と、binding の `when` 条件。解決は「context filter -> source 優先度 (rescue > user > imported > default) -> 限定性」の順 (SPEC-0002)
- `command palette` / `rescue`
    - 全 EditorAction を検索・実行できる唯一の rescue 入口。`F1` は resolver を経由せず常に有効 (SPEC-0002)
- `KeyboardCapabilities` / `fallback mode`
    - terminal が何を区別できるかの抽象。不足時は binding を黙って壊さず disabled / conflict として明示する (SPEC-0003)
- `import report`
    - import 結果の分類 (Imported / Ignored / Unsupported / Conflict / Disabled)。「利用者が何を失ったか分かること」が import 成功率より優先 (SPEC-0004)
