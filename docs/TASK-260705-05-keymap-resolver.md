# TASK-260705-05: Context-aware keymap resolver

260705 keymap resolver
===

## asis

- `input/` は normalized `KeyEvent` を生成でき、`core/` は `EditorCore` facade を持つ(TASK-260705-01〜04)
- `keymap/` は空。key event を editor action に解決する中核(SPEC-0002)が存在しない

## tobe

- `keymap/` が「`KeyEvent` 列 + `EditorContext` → `EditorAction`」を SPEC-0002 の規則どおり解決できる
- key 文字列(`"ctrl+shift+j"` / `"ctrl+x ctrl+s"`)を parse できる(後続の bindings.json loader と import の共通基盤)
- UI・terminal 非依存の pure logic として unit test 済み(ADR-0004)

## todo

- [x] `src/keymap/action.rs`: `EditorAction` enum + `FromStr` / `Display`(action 名は SPEC-0004 の変換表を正とする)
    - `cursor.{up,down,left,right,wordLeft,wordRight,lineStart,lineEnd,pageUp,pageDown}`
    - `selection.{up,down,left,right,wordLeft,wordRight,all}`
    - `edit.{insertLineAfter,insertLineBefore,moveLinesUp,moveLinesDown,undo,redo}`
    - `search.{open,next,previous}` / `replace.open`
    - `buffer.{new,next,previous,close}` / `file.{save,saveAs}`
    - `view.{splitVertical,focusNextSplit,focusPreviousSplit}`
    - `palette.open` / `app.quit` / `inspector.open`
    - 未知の action 名は `FromStr` でエラー(黙って捨てない。import report の材料になる)
- [x] `src/keymap/context.rs`: `EditorContext`(SPEC-0002 のフィールド全部。reserved の `suggestVisible` / `quickOpenVisible` 含む)。`Default` は全 false + `editorFocus`/`textInputFocus` = true
- [x] `src/keymap/predicate.rs`: `ContextPredicate`
    - 文法: `term ("&&" term)*`、`term = "!"? identifier`(`||` や括弧は対象外。SPEC-0004)
    - identifier は `EditorContext` のフィールド名。未知の identifier は parse エラー(`UnknownContext(String)`)
    - `eval(&EditorContext) -> bool` と `term_count() -> usize`(限定性の比較に使う)
- [x] `src/keymap/key_parse.rs`: key 文字列 parser
    - chord: `"ctrl+shift+j"` → `KeyEvent`。modifier 名: `ctrl` / `alt`(`opt`/`option` 別名)/ `shift` / `super`(`cmd`/`win`/`meta` 別名)
    - key 名: 英数字 1 文字、`f1`..`f12`、`enter` `escape`(`esc`)`tab` `space` `backspace` `delete` `up` `down` `left` `right` `home` `end` `pageup` `pagedown`
    - sequence: 空白区切り `"ctrl+x ctrl+s"` → `Vec<KeyEvent>`
    - 大文字小文字は区別しない。未知トークンはエラー
- [x] `src/keymap/binding.rs`: `Binding { keys: Vec<KeyEvent>, action: EditorAction, when: Option<ContextPredicate>, source: Source }`、`Source { Rescue, User, Imported, Default }`
- [x] `src/keymap/resolver.rs`: `Resolver`
    - `new(bindings: Vec<Binding>)`(定義順を保持する)
    - `resolve(&self, pending: &[KeyEvent], ctx: &EditorContext) -> ResolveResult`
    - ```rust
      enum ResolveResult {
          /// 一意に確定
          Matched(EditorAction),
          /// より長い sequence の prefix に一致。exact は「今確定できる完全一致」(timeout 時に app 層が発火する)
          Pending { exact: Option<EditorAction>, candidates: Vec<(Vec<KeyEvent>, EditorAction)> },
          NoMatch,
      }
      ```
    - 解決規則(SPEC-0002 を厳密に):
        1. `pending` と完全一致 or prefix 一致する binding を集める
        2. `when` が `ctx` で真のものだけ残す(`when` なしは常に真)
        3. 完全一致候補から: source 優先度 `Rescue > User > Imported > Default` → 同 source なら `term_count` が多い方 → なお同点なら**後に定義された方**
        4. prefix 一致(より長い sequence の途中)が 1 つでも残る場合は `Pending`(規則 3 の勝者を `exact` に入れる)。なければ `Matched` / `NoMatch`
- [x] `src/keymap/mod.rs` で公開する
- [x] table-driven unit test(AGENTS.md Testing 方針)

## testcases

key_parse:

- [x] `"ctrl+shift+j"` → Ctrl+Shift+`j`、`"cmd+s"` → Super+`s`、`"f1"` → F1、`"ctrl+space"` → Ctrl+Space
- [x] `"ctrl+x ctrl+s"` → 2 chord の sequence
- [x] `"CTRL+J"` が小文字と同じ結果になる
- [x] `"ctrl+unknown"` / `"foo+j"` がエラーになる

predicate:

- [x] `"editorFocus"` / `"!isReadonly"` / `"editorFocus && !isReadonly"` の parse と eval
- [x] `"resourceLangId"` など未知 identifier が `UnknownContext` エラーになる
- [x] `term_count` が項数を返す

resolver(source 優先度・限定性・定義順):

- [x] 同じ key に Default と User がある → User が勝つ
- [x] 同じ key に Imported と Rescue がある → Rescue が勝つ
- [x] 同 source で `when` なし vs `editorFocus`(真)→ 限定的な方が勝つ
- [x] 同 source・同 term 数 → 後に定義された方が勝つ
- [x] `when` が偽の binding は候補にならない(`searchVisible` 偽のとき search overlay 用 binding が無効)
- [x] context が変わると同じ key が別 action に解決される(`Ctrl+j`: editor では `cursor.down`、`searchVisible` では `search.next` のような 2 binding)

sequence:

- [x] `"ctrl+x ctrl+s"` 登録時、`ctrl+x` 単発 → `Pending`(candidates に続きが入る)、続けて `ctrl+s` → `Matched`
- [x] `ctrl+x` 単発 binding と `"ctrl+x ctrl+s"` が並ぶ → `ctrl+x` で `Pending { exact: Some(単発の action) }`
- [x] 無関係 key → `NoMatch`

品質:

- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

## notes

- レビュー指摘なし (resolver の優先度規則・sequence Pending・action/context の仕様一致を確認済み)

- 新規依存 crate の追加は禁止(必要と判断した場合は実装せず報告する)
- `keymap/` は `input/` の `KeyEvent` と `core/` に依存してよいが、`ui/` に依存してはならない(ADR-0004)
- `F1` の palette open は resolver を**経由しない**(input decoder 直後の hardcode。SPEC-0002)。resolver に F1 の特別扱いを入れないこと
- sequence timeout・状態保持(pending バッファ)は app 層 event loop の責務。resolver は毎回 stateless に `&[KeyEvent]` を受ける
- bindings.json の読み込み(ファイル IO・JSON parse)は次タスク。本タスクは in-memory の binding 構築まで
- commit は人間または main agent が行う(AGENTS.md)
