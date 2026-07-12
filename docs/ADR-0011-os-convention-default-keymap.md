# ADR-0011: OS-Convention Default Keymap(default 層は host OS の text 編集慣行に従う)

- Status: Accepted
- Date: 2026-07-11

## Context

2nd dogfood(TASK-260711-17)で、Ghostty が `Cmd+←/→` を `^A`/`^E` に、`Opt+←/→` を `ESC b`/`ESC f` に変換して渡してくることが実測で確定した。これらの変換先は恣意的な値ではなく、**macOS がネイティブ text field 全体で保証している標準編集キー(emacs 系: `Ctrl+A`=行頭、`Ctrl+E`=行末、`Ctrl+N/P`=上下、`Meta+B/F`=単語移動)**である。terminal は「macOS 標準に翻訳して」アプリに渡している。

一方 coda の default は `ctrl+a` を Windows 流の select-all に bind していたため、`Cmd+←` が「全選択」として誤動作した。誤動作の根因は terminal ではなく、**coda の default 層が host OS の慣行と食い違っていたこと**にある。

## Decision

> **default 層(`Source::Default`)は「host OS の text 編集慣行」を実装する。** OS が全アプリに保証する編集キーは、terminal が変換・翻訳の到達点として使う値でもあるため、これに合わせることで terminal 設定ゼロでも期待どおり動く。

### 1. Platform 抽象

- `Platform { MacOs, Other }` を導入し、default binding table を common + platform 別に分離する
- 実行時の判定は compile-time(`cfg!(target_os = "macos")`)でよいが、**table 自体は全 platform 分を常にコンパイル・テスト可能**にする(`bindings_for(Platform)`)
- Windows 版を作る際は `Platform::Windows` を追加し同じ原則を適用する(Windows 慣行 = `Ctrl+A`=select all、`Ctrl+Home/End` 等。既存の Other がほぼこれ)

### 2. macOS default セット(MVP)

既存 action で賄える macOS 標準キーのみを対象とする:

| Key | Action | 備考 |
| --- | --- | --- |
| `ctrl+a` / `ctrl+e` | `cursor.lineStart` / `cursor.lineEnd` | **`ctrl+a` は select-all から変更**(誤動作の根因) |
| `ctrl+n` / `ctrl+p` | `cursor.down` / `cursor.up` | |
| `ctrl+f` / `ctrl+b` | `cursor.right` / `cursor.left` | |
| `ctrl+d` | `edit.delete` | VS Code の `cmd+d`(addSelectionToNextFindMatch)は imported 層が上書きするので衝突しない |
| `alt+b` / `alt+f` | `cursor.wordLeft` / `cursor.wordRight` | Ghostty の `esc:b/f` 変換の到達点。shift 版は selection |
| `cmd+a` | `selection.all` | Ghostty default では消費される → palette / 推奨 config を導線に(ADR-0008 決定 1 と整合) |

- `ctrl+k`(kill to line end)や kill-ring 系は `EditDeleteToLineEnd` action が未実装のため対象外。action 追加時に再訪する
- select-all の Windows/Linux default(`ctrl+a`)は従来どおり維持する

### 3. 優先度体系は不変

source 優先度(rescue > user > imported > default)に変更はない。OS 慣行はあくまで **default 層の内容**であり、VS Code import や user 設定は従来どおりこれを上書きする。

## Consequences

- Ghostty 素の状態(ユーザー config なし)で `Cmd+←/→`(→ `^A`/`^E`)、`Opt+←/→`(→ `ESC b/f`)、`Ctrl+N/P` が期待どおり動く。**「terminal 設定が必須の製品」にならない**
- macOS で `ctrl+a` が select-all でなくなる。macOS の VS Code 筋肉記憶は `cmd+a` なので影響は軽微だが、Windows 流の指を macOS で使うユーザーは user 設定での上書きが必要(explainability: `:inspect-key` で由来 default / platform を表示できること)
- default table が platform 依存になるため、test は全 platform 分を table-driven で固定する

## Alternatives Considered

- **terminal 設定(unbind)を必須にする**: 「必ず起動する・設定ゼロで壊れない」の製品原則(AGENTS.md 失敗モード)に反する。推奨 config は「より良くする」ための追加手段に留める
- **全 OS 同一 default**: 今回の誤動作の原因そのもの。OS が保証するキーと食い違う default は terminal 変換で必ず破綻する
- **Ghostty 変換の検出と動的リマップ**: terminal ごとの変換規則に依存し説明可能性を損なう。OS 慣行への準拠なら terminal 非依存で同じ結果が得られる
