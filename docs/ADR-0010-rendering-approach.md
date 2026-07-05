# ADR-0010: Rendering Approach (custom minimal renderer, no TUI framework)

- Status: Proposed
- Date: 2026-07-05

## Context

実装順序 step 5(minimal command palette)以降で初めて画面描画が必要になる。ADR-0004 で保留していた「TUI framework(ratatui 等)を使うか、renderer を自前実装するか」を決める必要がある。

前提:

- terminal との対話はすでに自前で行っている: raw mode(termios)、kitty keyboard protocol の push/pop、独自 input decoder。今後 bracketed paste・SGR mouse・OSC 52(ADR-0008)も自前 sequence で扱う
- 描画対象は「テキストグリッド + status bar + tab bar + overlay(palette / search)」で、一般的な TUI アプリのような複雑な widget / layout 要求がない
- ADR-0004 の原則: 「TUI framework に製品の設計を引っ張られないこと」

## Decision

### 1. TUI framework は採用せず、`ui/renderer` を自前実装する

- ANSI escape sequence を直接出力する最小 renderer を書く
- 画面は「styled cell の 2 次元 buffer」を前面 / 背面の 2 枚持ち、差分行だけ再描画する(flicker 防止)
- 表示幅は `unicode-width`(導入済み)で計算する(CJK = 2 columns。ADR-0009 の display column 概念)
- alternate screen(`CSI ?1049h/l`)の enter/leave は RawModeGuard と同様の RAII guard + signal 復元で扱う
- resize は `SIGWINCH` を event loop へのイベントとして流す

### 2. 採用理由

- **terminal 状態の所有者を一元化する**: keyboard protocol / paste / mouse / clipboard をすべて自前 sequence で扱う以上、出力側だけ framework に渡すと terminal 状態の所有者が 2 つになり、干渉バグ(mode の push/pop 順序、restore 漏れ)の温床になる
- editor の主画面は「行の並び + ハイライト span」であり、TUI framework の widget 抽象がほぼ役に立たない領域
- highlight(ADR-0006 の色 span)を renderer に直結でき、変換層が要らない

### 3. 撤退条件

以下のいずれかが現実になったら ratatui(custom backend)への移行を再検討する:

- split view 実装で layout 計算が renderer の複雑さの主因になった場合
- 差分描画の品質問題(flicker / 性能)が 2 スプリント以上解決しない場合

## Alternatives Considered

- **ratatui + crossterm backend**: input を crossterm に奪わせない構成が不自然になり、raw mode / protocol 管理が二重化する。不採用。
- **ratatui + 自前 backend**: terminal 所有権の問題は緩和されるが、widget / layout 抽象は editor 主画面にほぼ寄与せず、依存とフレームワーク流儀だけが残る。UI 複雑化時の再検討候補として保留。
- **毎フレーム全画面再描画(diff なし)**: 実装は最小だが SSH 越しの帯域で flicker と遅延が出る。主戦場が SSH である以上、行 diff は最初から入れる。

## Consequences

### 良くなること

- terminal との全対話が `input/` と `ui/renderer` の 2 箇所に閉じ、挙動を完全に把握できる
- 依存が増えない(既存の libc / unicode-width で足りる)
- inspect-key で培った sequence の知識がそのまま描画側にも通用する

### リスク・コスト

- diff 描画・style run の直列化・resize 対応を自作する実装コスト
- terminal ごとの描画差異(色数、`CSI ?2026`(synchronized output)対応有無)を自分で吸収する必要がある
- split view の layout 計算を将来自前で書くことになる(撤退条件参照)

## Migration Notes

新規実装。`ui/renderer` の詳細仕様は実装タスク(TASK)側で定義する。

## Open Questions

- synchronized output(`CSI ?2026`)を最初から使うか(対応 terminal では tearing が消える)
- 色出力の表現: 16 色 / 256 色 / truecolor の変換を renderer 層と highlight 層のどちらに置くか(ADR-0006 の ColorCapabilities と要整合)
- status bar / overlay の描画 API(cell 直書き vs 簡易 widget 関数)

## Progress

- 2026-07-05: 初版作成(Proposed)。ADR-0004 の保留事項を解決。
