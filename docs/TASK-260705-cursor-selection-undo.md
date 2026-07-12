# TASK-260705: Cursor 移動・Selection・Undo と EditorCore facade

260705 cursor / selection / undo
===

## asis

- `TextBuffer` の grapheme 単位編集とファイル往復は完成(TASK-260705-03)
- cursor 移動・selection・undo が存在せず、editor として動作する土台がない

## tobe

- `core/` に cursor 移動(word 単位含む)・selection・undo スタックが pure logic として存在する
- `EditorCore` facade が「buffer + cursor + selection + undo」を束ね、後続タスク(event loop / keymap resolver)が EditorAction をこの facade の呼び出しに変換できる状態になる
- すべて UI / terminal 非依存で unit test 済み(ADR-0004)

## todo

- [x] `src/core/movement.rs`: `TextBuffer` と `Position` を受け取る pure な移動関数群
    - `left` / `right`: grapheme 単位。行頭で left → 前行末、行末で right → 次行頭
    - `up` / `down`: 行移動。`preferred_grapheme`(引数)への clamp 付き
    - `line_start` / `line_end` / `buffer_start` / `buffer_end`
    - `page_up` / `page_down`: `rows: usize` 引数で行数分移動(clamp 付き)
    - `word_left` / `word_right`: `unicode-segmentation` の word 境界を使う
        - `word_right`: 行末なら次行頭へ。それ以外は空白を読み飛ばし、次の word 片の**末尾**へ
        - `word_left`: 行頭なら前行末へ。それ以外は直前の空白を読み飛ばし、word 片の**先頭**へ
- [x] `src/core/selection.rs`: `Selection { anchor: Position, head: Position }`
    - `is_empty()` / `range() -> (Position, Position)`(Ord で正規化)
- [x] `src/core/undo.rs`: 逆操作スタック(ADR-0009)
    - 編集記録は「forward 操作 + inverse 操作 + 前後の cursor 位置」を持つ
    - **グルーピング**: 改行を含まない連続した `insert_text` で、挿入位置が直前グループの終端に連続している場合は同一グループに併合する。連続 backspace も同様に併合する
    - グループ境界: 改行の挿入、編集種別の切替(insert ↔ delete)、非連続位置への編集、明示的な `commit_group()`
    - `redo` スタックは新規編集で破棄する(線形 undo)
- [x] `src/core/editor.rs`: `EditorCore { buffer, cursor: Position, preferred_grapheme, selection: Option<Selection>, undo }`
    - `move_cursor(motion, extend: bool)`
        - `extend = true`: anchor 未設定なら現 cursor を anchor に。head を移動
        - `extend = false` で selection がある場合: `left` は範囲先頭へ、`right` は範囲末尾へ collapse。その他の motion は selection 解除後 head から移動
        - `up` / `down` / `page_*` は `preferred_grapheme` を維持し、水平移動はそれを更新する
    - 編集操作(すべて undo 記録付き。selection があれば置換 = 削除+挿入を 1 グループで):
        - `insert_text(&str)`
        - `backspace`(selection 削除 / 前 grapheme 削除 / 行頭では行結合)
        - `delete_forward`(対称)
        - `delete_word_left` / `delete_word_right`(word 境界まで削除)
        - `delete_line`(現在行を丸ごと。最終行では内容のみ削除で空行維持)
        - `select_all`
    - `undo()` / `redo()`: buffer を復元し cursor をグループ記録位置へ戻す。返り値で成否を返す
- [x] `src/core/mod.rs` で公開する
- [x] table-driven unit test(AGENTS.md Testing 方針)

## testcases

movement:

- [x] 行頭 `left` → 前行末、行末 `right` → 次行頭、buffer 先頭 / 末尾では動かない
- [x] `up` / `down` で短い行を跨いでも `preferred_grapheme` が維持される(長い行に戻ると元の列に復帰)
- [x] `word_right` が `"foo  bar"` の先頭から `foo` 末尾 → `bar` 末尾と進む
- [x] `word_left` が対称に戻る
- [x] 日本語文(`"これはtestです"`)で word 境界が panic せず単調に進む
- [x] 行末の `word_right` → 次行頭、行頭の `word_left` → 前行末

selection:

- [x] `extend = true` の移動で anchor が固定され `range()` が正規化される(逆方向選択も同じ範囲)
- [x] selection 中の `extend = false` `left` / `right` が範囲の先頭 / 末尾に collapse する

編集 + undo:

- [x] `insert_text` の連続 1 文字入力(`"a"` `"b"` `"c"`)が 1 回の `undo()` でまとめて消える
- [x] 改行入力でグループが切れる(`undo()` 1 回で改行以降のみ戻る)
- [x] 連続 `backspace` が 1 グループに併合される
- [x] selection 置換(選択して `insert_text`)が 1 回の `undo()` で元に戻る(削除+挿入がアトミック)
- [x] `undo()` → `redo()` で buffer と cursor が完全に往復する
- [x] 新規編集後に `redo()` が失敗する(redo スタック破棄)
- [x] `backspace` の行頭実行で行結合され、`undo()` で分割が戻る
- [x] `delete_line` が最終行で空行を維持する(buffer が 0 行にならない)
- [x] `delete_word_left` が `"foo bar|"` から `"foo |"` にする(`|` は cursor)

品質:

- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

## notes

- レビューでの修正 2 件:
    - `word_left` が単語の途中から実行すると自単語の先頭を飛び越えて前の単語へ移動するバグを修正 (mid-word テストを追加)
    - `delete_line` が複数行 buffer の最終行で行を消さず空行を残す挙動を VS Code 同等 (直前の改行ごと削除) に修正

- 依存 crate の追加は不要(word 境界は導入済みの `unicode-segmentation`)。**新規依存を追加しないこと**(必要と思った場合は実装せず報告する)
- `preferred_grapheme` は MVP では grapheme index ベース。display column(全角幅)ベースの stickiness は renderer タスクで再検討(ADR-0009 Open Questions)
- undo グルーピングの時間境界(無操作 N 秒)は入れない(pure logic に時計を持ち込まない。必要なら app 層で `commit_group()` を呼ぶ)
- `EditorAction`(`cursor.down` 等の action 名)との対応付けは keymap resolver タスクで行う。本タスクは facade メソッドまで
- commit は人間または main agent が行う(AGENTS.md)
