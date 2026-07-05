# ADR-0009: Buffer Data Structure and Text Model

- Status: Proposed
- Date: 2026-07-05

## Context

editor 本体(実装順序 step 3〜4。ADR-0004)に入るにあたり、text buffer のデータ構造と Unicode の扱いを決める必要がある。

前提条件:

- 主用途は設定ファイル・ソースコード・git rebase todo 等の**短時間編集**(ADR-0001)
- large file protection が仕様として存在する(SPEC-0001)。巨大ファイルは警告 / read-only であり、巨大ファイルの快適編集は要件ではない
- 日本語等の CJK(全角幅)、絵文字、結合文字を正しく扱う必要がある(利用者層的に必須)
- `core/` は pure logic として unit test 可能でなければならない(ADR-0004)

## Decision

### 1. MVP の buffer は行ベース(`Vec<String>`)とする

```rust
struct TextBuffer {
    lines: Vec<String>,        // 各行は EOL を含まない UTF-8
    line_ending: LineEnding,   // Lf | CrLf (ファイル単位で保持)
    trailing_newline: bool,    // 末尾改行の有無を保持
}
```

- rope / piece table は採用しない。large file protection がある以上、rope の主な利点(巨大ファイルでの編集性能)は MVP の要件に無い
- 行単位の操作(insert line / delete line / move lines)が仕様の中心であり、行ベースが最も素直
- buffer の公開 API は position 指定の insert / delete / 行アクセスに限定し、内部表現に依存させない。将来 rope へ差し替える余地を API 境界で残す(trait 抽象は今はしない。YAGNI)

### 2. Position は (行 index, grapheme index) とする

3 つの「列」概念を明確に区別する:

| 概念 | 用途 |
| --- | --- |
| byte offset | String 操作の内部実装のみ |
| **grapheme index** | **cursor / selection の位置(正準表現)** |
| display column | 描画・クリック位置解決(CJK=2 幅、Tab 展開) |

- cursor 移動は grapheme cluster 単位(`unicode-segmentation` crate)。絵文字・結合文字を 1 移動 = 1 単位で扱う
- 表示幅は `unicode-width` crate(CJK 全角 = 2 columns)
- byte offset を `core/` の公開 API に出さない(grapheme 境界以外での文字列破壊を型レベルで防ぐ)

### 3. Undo は逆操作スタック + グルーピング

- 編集を `EditOp`(位置付き insert / delete)として記録し、逆操作を undo スタックに積む(スナップショット方式は採らない)
- 連続する文字入力は 1 つの undo 単位にまとめる。グループ境界: 改行入力、cursor の編集位置以外への移動、delete 系との切替
- redo スタックは新規編集で破棄する(一般的な線形 undo。undo tree は non-goal)

### 4. 改行・末尾改行の扱い

- 読込時に EOL(LF / CRLF)を検出してファイル単位で保持し、保存時に復元する
- 混在ファイルは多数派に正規化して保存し、読込時に警告を出す
- 末尾改行の有無を保持して往復させる(diff 汚染を防ぐ)
- 内部表現は常に「EOL を含まない行の列」とし、EOL は metadata に隔離する

### 5. UTF-8 以外・巨大ファイル

- UTF-8 として不正なファイルは開かない(エラー表示。lossy 変換で開かない — 保存時のデータ破壊を防ぐ)
- サイズ閾値(初期値 10MB、`config.toml` で変更可)超過時は read-only で開き、警告を表示する

## Alternatives Considered

- **rope(ropey crate)**: helix 採用の実績があり巨大ファイルに強いが、grapheme 処理は結局 ropey の外(unicode-segmentation)で行うため複雑さは減らない。MVP の要件(protection 済みの中小ファイル)に対して過剰。行ベース API の背後に隠せる設計にしておき、必要が実証されたら差し替える。
- **gap buffer / piece table**: 単一挿入点の連続編集には強いが、行単位操作・複数 selection との相性が悪く、実装・テストコストが高い。不採用。
- **`String` 一本(行分割なし)**: 行アクセスのたびに走査が必要になり、行ベースの仕様群と噛み合わない。不採用。
- **lossy UTF-8 読込(不正バイトを置換して開く)**: 保存時に元データを破壊する。「開けない」ことを明示する方が本製品の思想(黙って壊さない)に合う。不採用。

## Consequences

### 良くなること

- buffer / cursor / undo が std + 小さな Unicode crate 2 つだけで pure logic として test できる
- 行ベース仕様(insert line / move lines / 行単位描画)と表現が一致し、実装が素直になる
- EOL / 末尾改行の往復保証を invariant として test で固定できる

### リスク・コスト

- 数万行超の行挿入 / 削除は O(行数) の memmove になる(large file protection の閾値内では実害なし)
- 将来 rope に差し替える場合、position 型は保てるが内部実装の書き直しコストはかかる
- grapheme 境界の扱いは `unicode-segmentation` のバージョン(= Unicode バージョン)に依存する

## Migration Notes

新規実装。依存 crate として `unicode-segmentation` と `unicode-width` を追加する。

## Open Questions

- Tab 文字の display column 幅(4 / 8 / 設定可)
- undo グルーピングの時間境界(無操作 N 秒で区切るか)を入れるか
- 巨大な単一行(minified JS 等)への保護(highlighting は ADR-0006 で打ち切り済み。編集側の閾値を設けるか)

## Progress

- 2026-07-05: 初版作成(Proposed)。
