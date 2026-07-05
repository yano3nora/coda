# SPEC-0002: Keybinding System

## Overview

keybinding の内部モデル、context モデル、解決規則、rescue binding、key sequence の仕様を定義する。設計判断の背景は ADR-0002。

## Goals

- VS Code 由来の context 依存 binding を構造を保ったまま表現・解決する
- 設定破損時にも操作不能にならない
- 「なぜこのキーがこの動作になるのか」を利用者が追跡できる

## Non-Goals

- VS Code `when` clause の完全再現(対応 predicate は限定し、未対応は import report へ。SPEC-0004)
- modal editing

## Terms

- `KeyChord`
    - 単一の修飾キー付きキー入力(例: `Ctrl+Shift+j`)
- `KeySequence`
    - 複数 chord の連続(例: `Ctrl+x Ctrl+s`)
- `ContextPredicate`
    - `EditorContext` に対する条件式(例: `editorFocus && !isReadonly`)
- `source`
    - binding の由来: `rescue` / `user` / `imported` / `default`

## Behavior

### Binding model

```ts
type Binding = {
  key: KeyChord | KeySequence;
  action: EditorAction;
  when?: ContextPredicate;
  source?: "default" | "rescue" | "imported" | "user";
  priority?: number; // MVP で必要性を再評価する (ADR-0002 Open Questions)
};
```

### Context model

MVP の `EditorContext`:

```ts
type EditorContext = {
  editorFocus: boolean;
  textInputFocus: boolean;
  hasSelection: boolean;
  hasMultipleSelections: boolean; // when 変換対象 (SPEC-0004) のため保持
  isReadonly: boolean;

  searchVisible: boolean;
  replaceVisible: boolean;
  commandPaletteVisible: boolean;
  listFocus: boolean;

  // reserved: 対応 feature が MVP に存在しないため常に false。
  // これらを参照する imported binding は不活性になる (import report に明示する)。
  suggestVisible: boolean;
  quickOpenVisible: boolean;

  tabFocus: boolean;
  splitFocus: boolean;
};
```

### Resolution

1. 入力 key(chord / sequence)が一致する binding を集める
2. `when` が現在の context に一致するものだけを候補に残す
3. source 優先度で選ぶ: `rescue > user > imported > default`
4. 同一 source 内では predicate の条件数が多い(より限定的な)binding を優先する
5. なお同点なら、後から定義された binding が勝つ

overlay 表示中に overlay 用 binding が優先されるのは、規則 4 の限定性による(`searchVisible` 等を predicate に持つため)。

### Rescue = command palette(single entry point)

rescue は「例外ショートカットの集合」ではなく、**command palette という単一の入口**で提供する。

| Key            | Action                          | 保証                                     |
| -------------- | ------------------------------- | ---------------------------------------- |
| `F1`           | command palette open            | 常に有効(keymap resolver より前で処理) |
| `Ctrl+Shift+p` | command palette open            | modifier 区別可能な terminal のみ        |
| `Esc`          | close overlay / cancel sequence | overlay 表示中・sequence 待機中のみ有効  |

- palette からは save / save as / quit / buffer close / help / inspect-key / which-key を含む**全 EditorAction** をインクリメンタルサーチして実行できる
- `F1` は legacy escape sequence として全 terminal で受信可能なため、**保証された rescue 入口**とする。keymap resolver を経由せず input decoder 直後に処理し、resolver や設定が壊れていても必ず開く
- `Ctrl+Shift+p` は fallback terminal では `Ctrl+p` と区別できないため(SPEC-0003)、capability がある場合のみ rescue として登録する
- palette 内の各 command には、現在 bind されている key を併記する(発見性と「palette で覚えて shortcut に昇格する」導線)
- これにより `Ctrl+c` / `Ctrl+s` / `Ctrl+q` / `Ctrl+g` は rescue から解放され、user / imported binding が自由に使える(`Ctrl+c` = copy 等)
- `Ctrl+c` の SIGINT は raw mode で無効化し、通常の key として扱う

### Key sequences

- `Ctrl+x Ctrl+s` のような複数 chord の sequence をサポートする
- sequence timeout は設定可能。default: 800ms
- 待機中は status bar に候補(続きの chord と action)を表示する
- 待機中の `Esc` / `Ctrl+c` は sequence をキャンセルする

### Binding inspection

command palette から以下を実行できる(`:` 表記は palette 内 command の表記であり、modal な command line ではない)。

```text
:which-key Ctrl+j
```

```text
Key: Ctrl+j
Context: editorFocus=true, suggestVisible=false
Resolved action: cursor.down
Source: imported:vscode
```

## Invariants

- `F1` による command palette open は、設定・import・keymap resolver の状態に関わらず常に機能する
- 全 EditorAction は command palette から実行可能である(shortcut は palette command への別経路にすぎない)
- 単純な `Map<Key, Action>` に縮退させない(context を無視した解決をしない)
- 同一 key・同一 context で複数 binding が同点になった場合も、解決は決定的である(規則 5)

## Edge Cases / Failure Modes

- `bindings.json` が parse 不能 → default + rescue のみで起動し、警告を表示する
- sequence 待機中に unmapped key → sequence を破棄し、key を通常解決に回す
- terminal capability 不足で受信不能な key を持つ binding → disabled 扱い(SPEC-0003 / SPEC-0004)

## API / Interface

- user binding ファイル形式は SPEC-0005
- `EditorAction` の一覧(`cursor.*` / `selection.*` / `edit.*` / `search.*` / `buffer.*` / `view.*`)は SPEC-0004 の変換表を正とする

## Trouble Shooting

- 期待した action にならない → `:which-key <key>` で source と context を確認
- key 自体が届いていない疑い → `:inspect-key`(SPEC-0003)

## Open Questions

- `F1` すら受信できない・食われる環境(一部 multiplexer / OS shortcut)への最終手段。候補: `Ctrl+c` 連打(3 回)で force-quit prompt。過剰設計になるなら見送る
- palette open key(`F1` / `Ctrl+Shift+p`)を config で変更可能にするか(rescue の「常に有効」保証と矛盾しない範囲で)
- 限定性(条件数)による優先は近似にすぎない。実運用で直感に反したら specificity の定義を見直す

## Progress

- 2026-07-05: 初版。draft の優先度 5 層(rescue > overlay > user > imported > default)を「context filter + source 優先度 + 限定性」に整理(ADR-0002)。
- 2026-07-05: rescue を「複数の例外ショートカット」から「command palette 単一入口(`F1` 保証)」に変更。`Ctrl+c` / `Ctrl+s` / `Ctrl+q` / `Ctrl+g` を user keymap に開放。
