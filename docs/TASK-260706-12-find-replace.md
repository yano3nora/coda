# TASK-260706-12: Find / Replace

260706 find and replace
===

## asis

- editor は編集・保存・palette・import が動くが、検索・置換(SPEC-0001 4.5、実装順序 step 10)が無い
- `search.open` / `replace.open` / `search.next` / `search.previous` は action として存在するが dispatch で not implemented

## tobe

- current buffer の incremental search、next / previous(wrap around)、case 切替、置換(1 件 / 全件)が動く
- search overlay は独自 context(`searchVisible` / `replaceVisible`)を持ち、editor keymap と衝突しない(SPEC-0002)
- replace all が 1 回の undo で戻る

## todo

### core 層(pure logic)

- [x] `src/core/search.rs`
    - `find_matches(buffer, query, case_sensitive) -> Vec<(Position, Position)>`(grapheme 位置の半開区間。行ごとの走査でよい。query に改行は含まない前提で、含まれたら空を返す)
    - case_insensitive は Unicode lowercase 比較(`to_lowercase`)。**grapheme 境界を跨ぐ match を返さない**こと(`"か"` を検索して `"が"` の一部に当てない等は Unicode 的に自然に満たされるが、テストで固定する)
    - `next_match_from(matches, cursor) -> Option<usize>` / `previous_match_from(...)`: cursor 位置から wrap around で選ぶ
- [x] `EditorCore::replace_range_all(replacements: &[(Position, Position, &str)])` 相当の一括置換(名前は任せる)
    - 複数 match の置換を**後ろから順に適用**して位置ズレを防ぎ、**全体を 1 つの EditGroup** に記録(undo 1 回で全部戻る)

### app 層

- [x] `src/app/search_overlay.rs`: overlay 状態と key 処理(palette と同じ「visible 中は直接処理」パターン)
    - state: `query` / `replace_text` / `case_sensitive` / `replace_mode`(replace 欄の有無)/ `focus`(Search 欄 / Replace 欄)/ `current`(現在 match index)
    - 文字 / backspace → focus 中の欄を編集し、query 変更のたびに match を再計算して「cursor 以降の最初の match」へジャンプ(incremental)
    - `enter` → 次の match、`shift+enter` → 前の match(wrap around)
    - `tab` → replace_mode のとき Search / Replace 欄の focus を切替
    - `alt+c` → case_sensitive 切替(再検索)
    - `esc` → overlay close(editor へ focus 復帰)
    - replace 実行: `ctrl+enter`(または `cmd+enter`)→ 現在 match を置換して次へ、`ctrl+alt+enter` → replace all
    - match 移動時は editor の selection を match 範囲に設定する(現在 match のハイライトは selection 描画を再利用)
- [x] 描画: 画面上部に 1〜2 行のバー(palette と同様に下のテキストが透けない塗り)
    - `Find: {query} [Aa:{on/off}] {current}/{total}`、replace_mode なら 2 行目に `Replace: {replace_text}`
    - focus 中の欄に hardware cursor を置く(`Screen::set_cursor`)
    - match 0 件のときは `no matches` 表示
- [x] context 結線: overlay 表示中は `searchVisible = true`(replace_mode なら `replaceVisible` も)、`editorFocus = false`
- [x] dispatch 結線: `search.open`(search のみ)/ `replace.open`(replace_mode で開く)/ `search.next` / `search.previous`(overlay 非表示時は「直前の query で next/prev」。query が空なら overlay を開く)
- [x] default bindings 追加(OS / GUI 標準。SPEC-0002 の default 方針):
    - `cmd+f` → `search.open`、`cmd+alt+f` → `replace.open`
    - `cmd+g` / `cmd+shift+g` → `search.next` / `search.previous`(mac 標準)
    - `f3` / `shift+f3` → `search.next` / `search.previous`(cross-platform 標準)

## testcases

core(table-driven):

- [x] 1 行内の複数 match、複数行にまたがる match 一覧(位置が grapheme 単位)
- [x] case_sensitive on/off で件数が変わる(`"Foo foo"` / query `foo`)
- [x] 日本語・絵文字を含む行での match 位置が正しい(`"あ👍あ"` から `"👍"`)
- [x] `next_match_from` が cursor 直後の match を選び、末尾を越えたら先頭へ wrap する(previous は逆)
- [x] 一括置換が後ろから適用され、置換後の内容が正しい(同一行複数 match 含む)
- [x] replace all が 1 回の undo で完全に戻る(cursor / selection 含む)

app(pure な部分):

- [x] overlay の文字入力 / backspace / tab focus 切替 / case 切替の状態遷移
- [x] query 変更で current が「cursor 以降の最初の match」になる
- [x] context が searchVisible / replaceVisible / editorFocus を正しく反映する
- [x] 新 default bindings が全て parse できる
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

## notes

- レビューでの修正 1 件: find_matches が重複 match を返していた ("aaa" で "aa" が 2 件)。重複 range は replace all を壊すため、match 後は match 長ぶん進める非重複方式 (VS Code 同等) に修正しテスト追加。main agent の PTY E2E 済み (2026-07-06): cmd+alt+f → foo→XX replace all → 保存でファイル内容と overlay 表示 (3/3 → no matches) を確認

- 新規依存 crate の追加は禁止。**regex は MVP では対象外**(SPEC-0001。将来 optional)
- 検索は素朴な全行走査でよい(large file protection 圏内では十分)。性能最適化はしない
- overlay 表示中に palette(`F1` / `Ctrl+Space`)が開いた場合は search overlay を閉じてよい(同時表示しない)
- `search.close` のような専用 action は不要(`esc` は overlay 直接処理。palette と同じ)。ただし palette から `search.open` 等を実行できることは維持する
- 置換 UI の replace 欄 key(`ctrl+enter` 等)は kitty protocol 環境でのみ区別可能なものを含む。fallback 環境では palette 経由(`replace.all` を action 化して palette に載せる)が逃げ道になるため、`replace.next` / `replace.all` を EditorAction に追加して dispatch へ結線すること(`Display`/`FromStr`/`ALL` の 3 箇所)
- commit は人間または main agent が行う(AGENTS.md)
