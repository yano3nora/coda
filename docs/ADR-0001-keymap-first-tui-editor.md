# ADR-0001: Keymap-first TUI Text Editor

- Status: Accepted
- Date: 2026-07-05

## Context

既存の terminal editor は、Vim / Emacs など固有の操作体系の習得を前提にしているものが多い。

しかし、日常的には VS Code など GUI editor を使い、独自に育てた keybinding が筋肉記憶になっている開発者にとって、terminal 上だけ別の操作体系を覚えるコストは高い。

主な利用場面:

- SSH 接続先での設定ファイル・ソースコードの軽微な編集
- `git rebase` / merge commit 編集時
- terminal 内での短時間のコード確認・修正
- VS Code を起動するほどではないテキスト編集

本プロダクトは Vim / Emacs の代替も、IDE の代替も目指さない。目的は次の一点に絞る。

> GUI editor で使っているキー操作をできるだけ引き継ぎ、terminal 内で軽量にテキスト編集する。

## Decision

### 1. Keymap-first

- editor の中核機能は描画や編集機能の豊富さではなく「keymap の import と解決」とする
- default keymap は極小にし、実用 keymap は外部 editor 設定の import で生成する
- 最初の import 対象は VS Code `keybindings.json` とする

### 2. Context-aware resolution

- keybinding は `Key + Context -> Action` として解決する(詳細: [ADR-0002](ADR-0002-context-aware-keybinding-resolution.md))
- 単純な `Map<Key, Action>` にはしない

### 3. Terminal capability abstraction

- modern keyboard protocol を優先的に利用し、非対応 terminal では「黙って壊れる」のではなく衝突・無効化を明示する(詳細: [ADR-0003](ADR-0003-terminal-keyboard-capability.md))

### 4. Explicit incompatibility

- 移植できない binding は黙って捨てず、import report で成功・無視・未対応・衝突を明示する
- import 成功率ではなく「利用者が何を失ったか分かること」を重視する

### 5. Scope 制限

MVP では以下を作らない(Non-goals)。

- Vim / Emacs compatibility、modal editing
- plugin system / marketplace、VS Code extension 互換
- LSP、syntax-aware refactor(rename / references など)
- file explorer、workspace management、Git UI
- terminal emulator、embedded browser、collaborative editing
- VS Code command の完全互換、full IDE replacement

## Alternatives Considered

- **Vim / Emacs 互換 editor**: 対象ユーザーが「習得したくない操作体系」そのものなので不採用。
- **既存 CUA 系 editor(nano / micro)の利用・fork**: micro は CUA keybinding を標準提供するが、「ユーザー自身が育てた keymap の import」「context-aware な解決」「非互換の明示的 report」を持たない。本製品の差別化はまさにそこにあるため、keymap engine を中核に据えた新規設計とする。
- **VS Code Remote / `code` CLI の利用**: SSH 先へのサーバー導入が重く、起動も遅い。「terminal 内で完結する軽量編集」の要件を満たさない。

## Consequences

### 良くなること

- Vim / Emacs の習得を前提にしない
- VS Code の既存 keybinding を活かせ、terminal 編集時の認知負荷を減らせる
- プロダクトの責務を小さく保てる
- import / report を中核機能として差別化できる

### リスク・コスト

- **Scope creep(最大のリスク)**: 「あと少しで VS Code の代替になる」方向に進むと失敗する。
    - 対策: feature request を「terminal での短時間編集を改善するか」だけで評価する。LSP / plugin / Git UI / workspace は明確に reject し、roadmap に IDE feature を置かない。
- **Terminal 互換性**: terminal ごとに key event が異なる。
    - 対策: capability abstraction と raw input inspector を最初に作る(ADR-0003)。
- **VS Code `when` clause の複雑さ**: 完全再現しようとすると終わらない。
    - 対策: MVP は対応 predicate を限定し、未対応は report する(SPEC-0004)。
- 操作不能防止(rescue)は command palette 単一入口方式で提供し、`Ctrl+c` 等の一等地キーを user keymap に開放する(ADR-0002 / SPEC-0002)。

## Migration Notes

Greenfield のため既存実装・利用者への影響はない。

## Open Questions

- SSH 先へ binary を導入する bootstrap を、GitHub Release からの手動 copy より簡単にする必要があるか。

## Progress

- 2026-07-05: 初版作成(Proposed)。関連 doc: ADR-0002〜0005、SPEC-0001〜0005。
- 2026-07-12: 製品名を `coda` に確定。既存製品との同名リスクは認識した上で、現時点の利用規模では rename コストを正当化しないと判断した。
- 2026-07-12: v0.1 の配布を GitHub Releases の macOS / Linux 向け単体 binary とし、mise の GitHub backend から導入できる asset 命名に決定した。crates.io / Homebrew / 自動 release workflow は利用実績ができるまで追加しない。
