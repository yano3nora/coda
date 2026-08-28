# Backlogs — 未解決／積み残しタスク

> **Status: 常設 (クローズしない)**。未着手・保留・トリガー待ちのタスクを一元管理する唯一の置き場。
> 各 TASK の残項目はここに集約済みなので、過去 TASK を漁る必要はない。

## 運用ルール

1. 次の作業を始めるときは、ここから 1 件 pick して `TASK-YYMMDD-<slug>.md` を新規作成する
2. pick した項目は本ファイルから削除し、新 TASK へのリンクに差し替える
3. 新しい未解決事項が出たら、他の TASK には「BACKLOGへ追加」だけ書いてここへ追記する
4. ADR, SPEC の Open Questions と重複する項目は、決着時に ADR, SPEC 側も更新すること

## Backlogs
- [ ] 他 editor profile import（Zed / Sublime / JetBrains）
- [ ] (着手前に要検討) tree-sitter への highlighting engine 差し替え
- [x] .editorconfig の indent 設定読み込み → [TASK-260828-editorconfig-indent](TASK-260828-editorconfig-indent.md)
- [x] whitespace 可視化 → [TASK-260828-render-whitespace](TASK-260828-render-whitespace.md)

