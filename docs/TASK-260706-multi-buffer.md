# TASK-260706: 複数 buffer / tabs

260706 multi buffer tabs
===

## asis

- editor は単一 buffer 前提(`EventLoop` が path / editor / view / saved_snapshot / highlight を直接保持)
- `coda a.txt b.txt` は先頭のみ開いて警告する
- `buffer.next/previous/close/new` は action として存在するが not implemented
- MVP 受け入れ基準(SPEC-0001 4.3)の最後の editor 系ブロッカー

## tobe

- 複数ファイルを tab として開き、切替・close・個別 save ができる
- 未保存 buffer の close / quit で警告が出る(既存 quit guard の一般化)
- tab bar が最上段に常時表示される

## todo

- [x] `src/app/document.rs`: 単一 buffer の状態を `Document` に集約する
    - `path` / `editor: EditorCore` / `view: EditorView` / `saved_snapshot` / highlight cache・syntax
    - `is_modified()` / `save()` / `display_name()`(file 名。同名 file が複数あるときだけ親 dir を付ける、は不要 — file 名のみでよい)
    - `EventLoop` の該当フィールドを `documents: Vec<Document>` + `active: usize` に置き換える(warning message・palette・overlay・pending 等の app 全域状態は EventLoop に残す)
- [x] CLI: `coda a.txt b.txt` で全引数を buffer として開く(先頭を active に。同一 path の重複指定は 1 つに)
- [x] dispatch 結線:
    - `buffer.next` / `buffer.previous`: 巡回切替(1 個なら no-op)。切替時に search overlay を閉じる
    - `buffer.close`: 未保存なら quit guard と同じ 2 段警告(`unsaved changes; close again to discard`)。最後の 1 個を close したら app 終了(こちらも未保存警告)
    - `buffer.new`: 無名 buffer(`[No Name]`)を作って active に。save 時に path が無ければ status bar にエラー(`file.saveAs` は今回も未実装のままでよい: `save as: not implemented yet`)
    - `file.save` / `app.quit`(全 buffer の未保存を対象に警告)を Document 構造に追従させる
- [x] tab bar: 最上段 1 行に常時表示
    - `1:main.rs  2:notes.md [+]  3:[No Name]` 形式(active は reverse、modified は `[+]`)
    - 幅超過は末尾を `…` で切る(スクロールは不要)
    - editor 領域は 1 行下にずれる(rows 計算・cursor y・resize の追従)
- [x] context: `tabFocus` は今回も常に false のままでよい(tab への focus 移動は作らない。切替は action のみ)
- [x] default bindings: `ctrl+tab` / `ctrl+shift+tab` → `buffer.next/previous`、`ctrl+w` と `cmd+w` → `buffer.close`(cmd+w は terminal に食われる環境では単に届かない)

## testcases

- [x] Document 化後の既存全テストが通る(regression)
- [x] 複数 path 指定で documents が引数順に並び、active = 0。重複 path は 1 つ
- [x] `buffer.next/previous` の巡回(3 buffer で next×3 が一周)
- [x] `buffer.close`: clean buffer は即 close、modified は 1 回目警告 → 2 回目 close
- [x] 最後の buffer の close が quit 判定になる(modified なら警告経由)
- [x] buffer ごとに undo / saved_snapshot / cursor が独立している(A で編集 → B へ切替 → A に戻ると状態が残っている)
- [x] tab bar 描画: active の reverse、modified の `[+]`、幅超過の `…`(pure な描画関数として test)
- [x] `app.quit` が「いずれかの buffer が modified」で警告する
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

手動(main agent PTY):

- [x] 2 ファイルで起動 → ctrl+tab 切替 → 双方編集 → 個別 save → 内容が正しい

## notes

- レビュー指摘なし。main agent の PTY E2E 済み (2026-07-06): 2 buffer 起動 → ctrl+tab 切替 → 双方編集 → 個別 save で両ファイル正しく保存、tab bar に active/modified 表示

- 新規依存 crate の追加は禁止
- highlight cache / syntax は Document ごとに保持する(切替のたびに全再計算しない)
- tab bar により editor 領域の原点が (0,1) になる。EditorView と cursor 描画・palette / search overlay の描画位置の整合に注意(overlay は tab bar より手前に置いてよい = 従来座標のままで上書きされて構わない)
- buffer 切替時に view(top_line / left_col)も Document 側に付いて回るため自然に保存される
- commit は人間または main agent が行う(AGENTS.md)
