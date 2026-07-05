# SPEC-0004: VS Code Keybinding Import

## Overview

VS Code `keybindings.json` を読み込み、内部 binding(`generated/vscode-bindings.json`)と import report を生成する機能の仕様を定義する。

## Goals

- MVP 対象の command family を内部 action へ変換する
- 変換できなかったものを分類し、利用者が「何を失ったか」を把握できるようにする

## Non-Goals

- VS Code command の完全互換
- `when` clause 式言語の完全再現
- VS Code 以外の editor profile import(deferred。Zed / Sublime / JetBrains / Helix)

## Terms

- `Imported`
    - 変換に成功し、有効な binding になったもの
- `Ignored`
    - editor の scope 外として意図的に対象外にしたもの(成功扱いにしない)
- `Unsupported command / condition`
    - scope 内だが MVP 未実装の command、または変換不能な `when` 条件
- `Conflict`
    - 変換後に同一 key・同一 context で衝突したもの
- `Disabled by terminal capability`
    - 現在の terminal で受信不能な key を使うため無効化されたもの(SPEC-0003)

## Behavior

### Input

VS Code の `keybindings.json`(JSONC)を読み込む。

```jsonc
[
  { "key": "ctrl+j", "command": "cursorDown", "when": "editorFocus" }
]
```

### Import scope(command 変換表)

#### Cursor movement

| VS Code command   | Internal action    |
| ----------------- | ------------------ |
| `cursorDown`      | `cursor.down`      |
| `cursorUp`        | `cursor.up`        |
| `cursorLeft`      | `cursor.left`      |
| `cursorRight`     | `cursor.right`     |
| `cursorWordLeft`  | `cursor.wordLeft`  |
| `cursorWordRight` | `cursor.wordRight` |
| `cursorLineStart` | `cursor.lineStart` |
| `cursorLineEnd`   | `cursor.lineEnd`   |
| `cursorPageDown`  | `cursor.pageDown`  |
| `cursorPageUp`    | `cursor.pageUp`    |

#### Selection movement

| VS Code command         | Internal action       |
| ----------------------- | --------------------- |
| `cursorDownSelect`      | `selection.down`      |
| `cursorUpSelect`        | `selection.up`        |
| `cursorLeftSelect`      | `selection.left`      |
| `cursorRightSelect`     | `selection.right`     |
| `cursorWordLeftSelect`  | `selection.wordLeft`  |
| `cursorWordRightSelect` | `selection.wordRight` |

#### Editing

| VS Code command                     | Internal action         |
| ----------------------------------- | ----------------------- |
| `editor.action.insertLineAfter`     | `edit.insertLineAfter`  |
| `editor.action.insertLineBefore`    | `edit.insertLineBefore` |
| `editor.action.moveLinesUpAction`   | `edit.moveLinesUp`      |
| `editor.action.moveLinesDownAction` | `edit.moveLinesDown`    |
| `editor.action.selectAll`           | `selection.all`         |
| `undo`                              | `edit.undo`             |
| `redo`                              | `edit.redo`             |

#### Search

| VS Code command                         | Internal action   |
| --------------------------------------- | ----------------- |
| `actions.find`                          | `search.open`     |
| `editor.action.startFindReplaceAction`  | `replace.open`    |
| `editor.action.nextMatchFindAction`     | `search.next`     |
| `editor.action.previousMatchFindAction` | `search.previous` |

#### Buffers / Views

| VS Code command                          | Internal action           |
| ---------------------------------------- | ------------------------- |
| `workbench.action.files.newUntitledFile` | `buffer.new`              |
| `workbench.action.splitEditor`           | `view.splitVertical`      |
| `workbench.action.focusNextGroup`        | `view.focusNextSplit`     |
| `workbench.action.focusPreviousGroup`    | `view.focusPreviousSplit` |

### Explicitly ignored commands

以下は `Ignored: outside editor scope` として報告する(成功扱いにしない)。

- `workbench.view.*`、`workbench.action.terminal.*`、`workbench.action.quickOpen*`
- extension 由来 command(`extension.*`、`chatgpt.*`、`projectManager.*`、`gistpad.*` 等)
- language server / refactor 系 command
- UI shell command、OS window command

### Unsupported commands

scope 内に見えるが MVP 未実装のものは `Unsupported: feature not implemented` として報告する。例:

- `editor.action.rename`、`editor.action.referenceSearch.trigger`
- `editor.toggleFold`
- suggestion / parameter hint 操作、code runner 操作

### `when` clause mapping

| VS Code `when`                | Internal context        |
| ----------------------------- | ----------------------- |
| `editorFocus`                 | `editorFocus`           |
| `editorTextFocus`             | `textInputFocus`        |
| `editorHasMultipleSelections` | `hasMultipleSelections` |
| `!editorReadonly`             | `!isReadonly`           |
| `suggestWidgetVisible`        | `suggestVisible`(※)   |
| `inQuickOpen`                 | `quickOpenVisible`(※) |
| `listFocus`                   | `listFocus`             |

- `&&` / `!` で構成され、全項が変換可能な式は変換する(例: `editorFocus && !editorReadonly`)
- 変換不能な項を含む式(例: `resourceLangId == markdown`)は binding ごと `Unsupported condition` とする
- (※)`suggestVisible` / `quickOpenVisible` は MVP では常に false(SPEC-0002)。変換自体は行うが、report に `imported (inactive in MVP)` と明示する

### Import report

import 後、必ず report を出力し、以下へ保存する。

```text
~/.config/<app>/import-reports/latest-vscode-import.txt
```

例:

```text
VS Code keybinding import completed.

Imported: 24
Ignored: 38
Unsupported commands: 5
Unsupported conditions: 3
Conflicts: 2
Disabled by terminal capability: 1

Examples:
- Imported Ctrl+j -> cursor.down [editorFocus]
- Ignored cmd+t -> terminal.newInActiveWorkspace [outside editor scope]
- Unsupported ctrl+r -> editor.action.rename [feature not implemented]
- Disabled Ctrl+Shift+j -> selection.down [terminal cannot distinguish Ctrl+Shift+j]
```

### Output

- 変換結果は `generated/vscode-bindings.json` に書き出す(ADR-0005)
- user の `bindings.json` には手を触れない
- `--replace` 時のみ既存 generated を置き換える(CLI は SPEC-0005)

## Invariants

- import 対象の全 entry が report のいずれかの分類に必ず現れる(黙って捨てない)
- Ignored / Unsupported を Imported に数えない
- re-import しても user binding は破壊されない

## Edge Cases / Failure Modes

- `keybindings.json` が JSONC として parse 不能 → import を中断し、エラー位置を表示する。generated は変更しない
- 同一 command の負値 binding(VS Code の `-command` 記法)→ MVP では `Unsupported command` として報告する
- `cmd`(macOS)modifier → terminal で受信不能な場合は `Disabled by terminal capability` に分類する
- 空の `keybindings.json` → 成功(Imported: 0)として report を出す

## API / Interface

CLI(詳細は SPEC-0005):

```sh
<app> keymap import vscode <path> [--dry-run] [--replace] [--print-report]
```

## Trouble Shooting

- import したのに key が効かない → report の分類(inactive / disabled / conflict)を確認 → `:which-key` → `:inspect-key`
- Imported 数が想定より少ない → Ignored の内訳を確認(extension command は scope 外)

## Open Questions

- VS Code の negative keybinding(`-command`)対応を MVP に含めるか
- default keybindings(VS Code 標準)も合成するか、user 定義分のみ import するか(MVP は user 定義分のみを想定)

## Progress

- 2026-07-05: 初版。draft からの変更: `suggestWidgetVisible` / `inQuickOpen` は「変換するが MVP では不活性」と明示する分類を追加。
