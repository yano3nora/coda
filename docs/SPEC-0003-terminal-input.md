# SPEC-0003: Terminal Input and Capability Detection

## Overview

terminal raw input の decode、keyboard capability の検出、fallback mode、raw input inspector の仕様を定義する。設計判断の背景は ADR-0003。

## Goals

- `Ctrl+j` / `Ctrl+Shift+j`、`Tab` / `Ctrl+i`、`Enter` / `Ctrl+Enter` / `Shift+Enter` を可能な限り区別する
- protocol 非対応 terminal でも安全に起動し、失われる binding を明示する
- keybinding 問題を利用者自身が切り分けられるようにする

## Non-Goals

- 特定 protocol(kitty keyboard protocol 等)の全機能対応。必要な capability だけを抽象化して使う
- Windows(ConPTY)対応(ADR-0004 Open Questions)

## Terms

- `normalized key event`
    - raw bytes を decode した環境非依存の key 表現(key + modifiers)。keymap resolver への唯一の入力
- `KeyboardCapabilities`
    - 現在の terminal で何が区別できるかを表す抽象(ADR-0003)
- `fallback mode`
    - modern keyboard protocol が使えない場合の動作 mode

## Behavior

### Capability detection

- 起動時に modern keyboard protocol(kitty CSI u / modifyOtherKeys 等)の negotiation を試みる
- 結果を `KeyboardCapabilities` に反映する

```ts
type KeyboardCapabilities = {
  supportsModifiedKeys: boolean;
  supportsShiftCtrlDistinction: boolean;
  supportsShiftEnter: boolean;
  supportsCtrlEnter: boolean;
};
```

- keymap resolver / importer は protocol 名ではなく capability のみを参照する

### Super(Cmd / Win)modifier

- normalized key event は ctrl / alt / shift に加えて super を保持する。kitty CSI u の modifier bit 8 で受信でき、受信した場合に捨てると `Cmd+s` が素の `s` 入力に化けるため(Invariants 違反)、decode 段階では必ず保持する
- ただし多くの terminal は Cmd / Win を自身のショートカットとして消費し、アプリまで届けない。super に依存する binding の有効 / 無効は capability 層で判定し、受信不能な環境では `Disabled by terminal capability` として report する(SPEC-0004)

### Fallback mode

modern protocol が利用できない場合:

- 受信できない modifier は unavailable とする
- unavailable な modifier に依存する binding は disabled とし、import report に出す(SPEC-0004)
- 同じ input sequence に解決される複数 binding は conflict として扱い、report に出す
- 起動時に capability warning を表示できるようにする

例:

```text
Ctrl+j and Ctrl+Shift+j cannot be distinguished in this terminal.
Imported binding "Ctrl+Shift+j" was disabled.
```

### Raw input inspector

command palette の `:inspect-key`、または CLI の `<app> inspect-key` で起動する(SPEC-0005)。

キーを押すと以下を表示する:

```text
Pressed: Ctrl+Shift+j
Raw bytes: \x1b[106;6u
Protocol: modified-key protocol
Resolved action: selection.cursorDown
```

- inspector は MVP の最初期から提供する(実装順序 1 に対応。ADR-0004)

### Deliverability の限界と `keymap verify`(ADR-0007)

- terminal が予約・消費するキー(例: Ghostty の `Cmd+Shift+P`)は **protocol 照会では検出できない**。送られてこないだけであり、非対応と無入力を区別できない
- そのため chord 単位の deliverability は `keymap verify`(対話的実測)で確定する: 対象 binding のキーを利用者に押してもらい、届いたかを記録する
- quirk 情報(`TERM_PROGRAM` ベースの既知予約キー)は警告表示のみに使う

## Invariants

- capability 検出の成否に関わらず、起動は常に成功する
- unavailable な modifier に依存する binding が黙って別の action に化けることはない(disabled + 明示)
- keymap resolver は raw bytes を直接見ない(normalized key event のみ)

## Edge Cases / Failure Modes

- protocol negotiation に terminal が応答しない → timeout 後、保守的な capability(全て false)で続行
- tmux / screen 経由で protocol が透過されない → 検出結果が実態とずれる可能性がある。inspector で確認可能にする
- 貼り付け(bracketed paste)と key 入力の混同 → bracketed paste mode を有効化し、paste は key 解決を通さない(paste が editor への text 挿入の第一経路でもある。ADR-0008)
- mouse event(SGR protocol、v0.2 帯)→ mouse sequence は key 解決を通さず mouse event として decode する。mouse reporting 有効中も Shift+ドラッグは terminal 側選択として素通しされる(ADR-0008)

## API / Interface

- `input/` module(terminal-decoder / keyboard-capabilities / key-chord / key-sequence)の出力は normalized key event(ADR-0004)
- capability は `:inspect-key` 画面と起動時 warning で利用者に露出する

## Trouble Shooting

- binding が効かない → `:inspect-key` で「そもそも key が届いているか」「どの raw bytes か」を確認
- terminal を変えたら挙動が変わった → 起動時 warning と import report の "Disabled by terminal capability" を確認
- tmux 配下で modifier が失われる → tmux の `extended-keys` 設定を確認(ドキュメントに手順を用意する)

## Open Questions

- ~~negotiation timeout の値~~ → 500ms + DA1 fallback(TASK-260712-16。ADR-0003 Open Questions 参照)
- ~~`$TMUX` 検出時に保守的 capability へ自動で倒すか(ADR-0003 Open Questions)~~ → 特別扱いしない(同上)
- fallback mode で `Esc` 単押しと escape sequence 先頭の区別に使う待ち時間(ESC timeout)

## Progress

- 2026-07-05: 初版。
- 2026-07-12: capability detection を実装(TASK-260712-16)。検出方式は `CSI ?u` 応答 + DA1(`CSI c`)fallback。fallback mode の起動時 warning、importer の `Disabled by terminal capability` 分類、inspector(overlay / CLI)の protocol 表示まで結線。
- 2026-07-12: SGR mouse decode を実装 ([TASK](TASK-260712-mouse-verify-inactive-ssh.md))。
  `CSI < Cb;Cx;Cy M|m` を `InputEvent::Mouse` として key 解決と別 channel で emit し、
  DECSET 1002/1006 の有効化・シグナル時の解除も結線。Shift 付き mouse event はアプリ側で
  無視して terminal 選択に委ねる (ADR-0008)。
