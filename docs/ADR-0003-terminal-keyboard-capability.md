# ADR-0003: Terminal Keyboard Capability Abstraction

- Status: Proposed
- Date: 2026-07-05

## Context

従来の terminal 入力では modifier 情報が失われる。

- `Ctrl+j` と `Ctrl+Shift+j` が区別できない
- `Tab` と `Ctrl+i`、`Enter` と `Ctrl+j` が同じ byte 列になる
- `Shift+Enter` / `Ctrl+Enter` が受信できない

kitty keyboard protocol(CSI u)や xterm `modifyOtherKeys` などの modern keyboard protocol はこれを解決するが、対応状況は terminal emulator ごとに異なる。keymap-first 製品(ADR-0001)にとって、この差異の扱いは中核の設計問題である。

## Decision

### 1. Capability layer による抽象化

特定 protocol 名に依存せず、terminal capability を抽象化した layer を設ける。

```ts
type KeyboardCapabilities = {
  supportsModifiedKeys: boolean;
  supportsShiftCtrlDistinction: boolean;
  supportsShiftEnter: boolean;
  supportsCtrlEnter: boolean;
};
```

keymap resolver はこの capability だけを見て動作し、protocol 検出・negotiation の詳細は input layer に閉じ込める。

### 2. Progressive enhancement

- 利用可能なら modern keyboard protocol を有効化して modifier を保持する
- 利用できない場合は fallback mode で動作する(起動は常に成功させる)

### 3. Explicit degradation

fallback mode では「黙って壊さない」。

- 受信できない modifier に依存する binding は disabled とし、import report と起動時 warning で明示する
- 同じ input sequence に解決される binding は conflict として report する

### 4. Raw input inspector を最初から提供

keybinding 問題の切り分け用に、raw bytes / 認識した protocol / 解決された action を表示する inspector を MVP の初期から実装する(SPEC-0003)。terminal input 問題を debug できない editor は keymap-first 製品として成立しないため。

## Alternatives Considered

- **modern protocol 対応 terminal を動作要件にする**: SSH 先や古い環境での利用が主用途(ADR-0001)なのに、その環境を切り捨てることになる。不採用。
- **黙って fallback する(silent degradation)**: 「import した binding が効かない理由が分からない」体験は本製品の価値を直接毀損する。不採用。
- **terminfo のみに依存する**: terminfo は modern keyboard protocol の capability を表現できない。実測(query / negotiation)ベースの検出を採用する。

## Consequences

### 良くなること

- terminal ごとの挙動差を input layer に隔離でき、keymap resolver を環境非依存で test できる
- 利用者は「何が使えて何が使えないか」を起動時・import 時に把握できる

### リスク・コスト

- capability 検出(protocol negotiation)自体が terminal ごとに癖を持ち、検証コストが高い
- tmux / screen 経由では protocol が透過されない場合があり、検出結果が実態とずれる可能性がある

## Migration Notes

Greenfield のため影響なし。

## Open Questions

- capability 検出に失敗した(応答がない)場合の timeout と default 値。
- tmux 配下での検出戦略(`$TMUX` 検出時に保守的な capability に倒すか)。

## Progress

- 2026-07-05: 初版作成(Proposed)。詳細仕様は SPEC-0003。
