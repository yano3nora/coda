# ADR-0006: Syntax Highlighting (syntect, presentation-only)

- Status: Proposed
- Date: 2026-07-05

## Context

初期案では syntax highlighting を deferred としていたが、これは採用リスクの読み違いである。

- ターゲット(GUI editor 派)にとって、モノクロ editor は「機能不足」ではなく「壊れている」に見え、第一印象で離脱される。核である keymap import 体験に到達してもらえない
- 競合基準でも nano / micro はハイライトを持つ。差別化点(import / report)を体験してもらう前に見た目で負ける
- 主戦場の「設定ファイルの 1 分編集」でも、YAML のインデントや閉じ忘れ文字列の発見に実用性がある

一方で、tree-sitter 統合は grammar 同梱・incremental parsing・バイナリサイズと統合コストが重く、fold / rename など syntax-aware 編集(ADR-0001 で拒否した方向)への構造的な誘惑を生む。

## Decision

### 1. MVP に syntax highlighting を含める。エンジンは syntect

- [syntect](https://github.com/trishume/syntect)(Sublime Text 文法定義ベース、`bat` 採用実績)を使う
- 行単位のパース状態を持つ設計のため、editor のライン単位キャッシュ・部分再パースと相性が良い
- 言語定義は同梱セットをそのまま使い、自作しない

### 2. Presentation-only の境界を守る

- highlighting は「buffer の行 → 色スパン列」の変換であり、renderer に渡すだけ
- `core/`(buffer / cursor / undo)は highlighting の存在を知らない
- **syntax 情報を編集操作・keymap 解決に使うことを禁止する**(fold / rename / syntax-aware selection への入口を塞ぐ。ADR-0001 Non-goals の防波堤)

### 3. Theme は同梱 dark / light の 2 つ、選択のみ可能

- MVP は同梱 theme(dark / light)から `config.toml` で選ぶだけとする
- theme 形式は syntect が読める既存形式(`.tmTheme`)に乗り、独自形式を作らない。将来のユーザー theme 追加(ファイル配置による拡張)への互換路線を確保する
- 将来拡張しても「theme = 配色ファイルの追加」以上のもの(plugin 化)にはしない

### 4. Color capability も「検出して明示的に degrade」

keyboard capability(ADR-0003)と同じ思想で扱う。

- truecolor / 256 色 / 16 色を検出し、theme 色を最も近い色へ丸める
- 検出不能・16 色未満の環境では highlighting を off にして起動は常に成功させる

### 5. Large file protection と連動

- 閾値超過ファイル(SPEC-0001)では highlighting を自動 off にする(性能リスクの主対策)

### 6. 実装順序は keybinding engine の後

keybinding engine(ADR-0004 step 1〜9)を遅らせない。find / replace の後、multi-buffer tabs / split view の前に組み込む。

## Alternatives Considered

- **MVP では highlighting なし(初期案)**: 第一印象での離脱により import 体験まで到達しない採用リスクが大きい。不採用。
- **tree-sitter**: 統合コストが keybinding engine より重くなりうる上、syntax-aware 編集への scope creep を構造的に誘発する。deferred(将来 highlighting engine の差し替え候補として検討余地は残す。presentation-only 境界はその場合も維持)。
- **自前の regex 定義**: 言語定義の作成・保守が無限に続く。syntect なら定義ごと付いてくる。不採用。

## Consequences

### 良くなること

- GUI editor 派への第一印象が「普通のエディタ」になり、核機能の体験まで到達する
- 数百言語の定義が同梱で手に入り、言語対応の保守を負わない
- capability 検出 → 明示的 degrade の設計思想が keyboard / color で一貫する

### リスク・コスト

- バイナリサイズが数 MB 増える(syntect + 文法定義)
- theme という設定面が増える(MVP は同梱 2 択に固定して抑える)
- syntect は正規表現ベースであり、巨大な 1 行(minified JS 等)でパースが重くなりうる。行長の上限で highlighting を打ち切る等の保護を入れる

## Migration Notes

SPEC-0001(deferred から MVP へ移動)、ADR-0004(module / 実装順序)、SPEC-0005(config.toml に theme 設定)を本 ADR と同時に更新。

## Open Questions

- 同梱 dark / light theme に何を選ぶか(既存 .tmTheme の流用候補選定)
- color capability 検出の詳細仕様(環境変数 `COLORTERM` / terminfo / query のどれを信じるか)を SPEC 化するタイミング
- 巨大単一行の打ち切り閾値

## Progress

- 2026-07-05: 初版作成(Proposed)。deferred 判断を覆して MVP scope に含める決定。
