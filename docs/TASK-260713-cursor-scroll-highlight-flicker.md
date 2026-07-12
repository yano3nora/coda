# TASK-260713: Cursor scroll highlight flicker

## asis

- event loop は highlight 取得後に `EditorView::draw` を呼ぶ
- `draw` 内の cursor follow が `top_line` を変えるため、スクロールした1フレームだけ本文と highlight の viewport がずれる
- mouse scroll は描画前に viewport を更新し、cursor follow を無効化するため再現しない

## tobe

- cursor follow で viewport を確定してから highlight を取得する
- renderer は確定済み viewport の本文と同じ範囲の highlight を描く

## todo

- [x] viewport 準備処理を独立させる
- [x] highlight 取得前に呼び出す
- [x] regression test
- [x] fmt / clippy / test

## testcases

- [x] cursor が画面下端を越えた場合、highlight 取得前に `top_line` が更新される
- [x] mouse scroll の `follow_cursor = false` では viewport を変更しない

## notes

syntect の再計算性能ではなく、viewport 確定と highlight 取得の順序が原因。
tree-sitter 差し替えでは解決しない。
