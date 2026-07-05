# SPEC-0005: CLI and Configuration Files

## Overview

CLI の起動・サブコマンド仕様と、設定ファイルの配置・形式を定義する。format 選定と分離方針の背景は ADR-0005。

## Goals

- `vim file.txt` と同じ気軽さで起動できる CLI
- import / user / default の設定が安全に共存する layout

## Non-Goals

- configuration UI(deferred)
- keybinding conflict UI(deferred)

## Terms

- `generated binding`
    - importer が生成する binding。直接編集させない
- `user binding`
    - 利用者が `bindings.json` に手書きする binding。generated より優先される

## Behavior

### CLI

```sh
# ファイルを開く
<app> path/to/file.ts

# 複数ファイル (tab / buffer として開く)
<app> file-a.ts file-b.ts

# VS Code keybindings の import
<app> keymap import vscode ~/.config/Code/User/keybindings.json
<app> keymap import vscode <path> --dry-run       # generated を書き換えず report のみ
<app> keymap import vscode <path> --replace       # 既存 generated を置き換える
<app> keymap import vscode <path> --print-report  # report を stdout にも出す

# raw input inspector (editor 内 :inspect-key と同等)
<app> inspect-key
```

editor 内 command(command palette 経由):

```text
:inspect-key
:which-key Ctrl+j
```

### Directory layout

```text
~/.config/<app>/
  config.toml                  # アプリ設定
  bindings.json                # user binding
  imports/
    vscode-keybindings.json    # import 元のコピー (再現・診断用)
  generated/
    vscode-bindings.json       # importer の出力
  import-reports/
    latest-vscode-import.txt
```

### config.toml

アプリ設定(binding 以外)。MVP で持つ項目の例:

```toml
[keymap]
sequence_timeout_ms = 800   # key sequence の待機時間 (SPEC-0002)

[terminal]
capability_warning = true   # 起動時の capability warning 表示

[appearance]
theme = "dark"              # 同梱 theme: "dark" | "light" (ADR-0006)
```

### bindings.json(user binding)

形式は VS Code `keybindings.json` に揃える。`command` には内部 action 名(SPEC-0004 の変換表)を書く。

```jsonc
[
  { "key": "ctrl+j", "command": "cursor.down", "when": "editorFocus" },
  { "key": "ctrl+shift+j", "command": "selection.down", "when": "editorFocus" }
]
```

### 優先順位

設定ファイル間の binding 優先順位(解決規則の全体は SPEC-0002):

```text
rescue > user (bindings.json) > generated (imported) > default
```

## Invariants

- `generated/` 配下は importer のみが書き換える(利用者の手書きは `bindings.json`)
- 設定ファイルが 1 つも存在しなくても起動できる(default + rescue)
- `--dry-run` はいかなるファイルも変更しない

## Edge Cases / Failure Modes

- `config.toml` / `bindings.json` が parse 不能 → 該当ファイルを無視して起動し、警告を表示する
- `generated/vscode-bindings.json` が手編集で壊れている → 無視して起動し、re-import を促す
- `XDG_CONFIG_HOME` 設定時はそちらを優先する
- 設定 directory が存在しない → 初回起動・初回 import 時に作成する

## API / Interface

- exit code: 正常終了 0、起動不能・import 失敗は非 0(値の割当は実装時に定義)
- `--print-report` の stdout 形式は report ファイル(SPEC-0004)と同一

## Trouble Shooting

- 設定がどれも効いていない → parse エラー警告を確認。`<app> keymap import ... --dry-run` と `:which-key` で切り分け
- import をやり直したい → `--replace` で generated を再生成(user binding は影響を受けない)

## Open Questions

- `bindings.json` を JSONC として許容するか(ADR-0005 Open Questions)
- `<app>` の正式名称(ADR-0001 Open Questions: `coda` の名称衝突)
- stdin からの読み込み(`git diff | <app> -`)を MVP に含めるか

## Progress

- 2026-07-05: 初版。
