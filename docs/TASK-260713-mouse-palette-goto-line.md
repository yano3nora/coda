# TASK-260713: mouse 追補 / palette navigation / go to line

## 目的

terminal での短時間編集で頻繁に使う mouse 操作と移動操作の欠落を埋める。
keymap-first の境界を守り、`cursor.goToLine` は palette・user binding・VS Code
import のすべてから同じ `EditorAction` として実行する。

## 対応

- SGR horizontal wheel を正規化し、wrap off の viewport 横スクロールへ反映
- tab bar click で buffer 切替
- double click で単語、triple click で論理行を選択
- command palette の `Ctrl+N` / `Ctrl+P` navigation と `Ctrl+U` query clear
- `cursor.goToLine` と入力 prompt（範囲外は末尾行へ clamp）
- VS Code `workbench.action.gotoLine` import と既定 `Ctrl+G`

## 検証

- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test`

## 非対応・リスク

- multi-click の単語分類は Unicode grapheme を使うが、言語別 word-break 辞書は持たない
- verify 結果の resolver 反映と OS 予約キーの import 分類は別の永続形式設計が必要なため backlog に残す
