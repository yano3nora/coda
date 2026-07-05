# TASK-260706-06: User bindings.json loader

260706 user bindings loader
===

## asis

- keymap resolver と key 文字列 parser は完成(TASK-260705-05)
- `bindings.json`(SPEC-0005 の user binding ファイル)を `Vec<Binding>` に変換する層がない
- serde / serde_json は導入済み(Cargo.toml)

## tobe

- JSON 文字列から `Source::User` の binding 列を構築できる
- 壊れた entry は**ファイル全体を殺さず**、entry 単位の issue として報告される(SPEC-0002 Edge Cases: 設定破損でも起動する)
- ファイル IO は含まない pure logic(fs 統合は app 層タスク)

## todo

- [ ] `src/keymap/user_bindings.rs` を実装する
    - 入力形式(SPEC-0005。VS Code keybindings.json と同形):
      ```jsonc
      [
        { "key": "ctrl+j", "command": "cursor.down", "when": "editorFocus" }
      ]
      ```
    - `load_user_bindings(text: &str) -> Result<UserBindingsLoad, UserBindingsError>`
        - `UserBindingsLoad { bindings: Vec<Binding>, issues: Vec<BindingIssue> }`
        - `UserBindingsError`: JSON 全体が parse 不能 / root が配列でない(この場合のみ全体エラー)
    - **JSONC 対応**: `//` 行コメントと `/* */` ブロックコメントを除去してから parse する(VS Code からのコピペを受け入れる。ADR-0005 Open Question の解決)。文字列リテラル内の `//` を壊さないこと
    - entry 単位の検証(失敗しても他 entry の処理を続行):
        - `key` の parse 失敗 → `BindingIssue { index, key, reason: InvalidKey(詳細) }`
        - `command` が未知 action → `UnknownCommand(名前)`
        - `when` の parse 失敗(未知 context 含む)→ `InvalidWhen(詳細)`
        - `key` / `command` フィールド欠落 → `MissingField(名前)`
        - 未知フィールドは無視する(将来互換)
    - 成功 entry は `Binding { source: Source::User }` として**定義順を保持**して返す(resolver の後勝ち規則に効くため)
- [ ] `src/keymap/mod.rs` で公開する
- [ ] table-driven unit test(AGENTS.md Testing 方針)

## testcases

- [ ] 正常系: 2 entry(when あり / なし)が定義順で `Source::User` の binding になる
- [ ] JSONC: `//` 行コメント・`/* */` ブロックコメント付きが parse できる
- [ ] 文字列内の `//`(例: `"when": "editorFocus // not-a-comment"` は when parse エラーになるが、JSON としては壊れない)がコメント扱いされない
- [ ] `"key": "ctrl+banana"` → 該当 entry のみ `InvalidKey`、他 entry は生きる
- [ ] `"command": "editor.action.rename"` → `UnknownCommand`
- [ ] `"when": "resourceLangId == markdown"` → `InvalidWhen`
- [ ] `key` 欠落 → `MissingField("key")`
- [ ] 空配列 `[]` → bindings も issues も空で成功
- [ ] root がオブジェクト / 壊れた JSON → `UserBindingsError`
- [ ] issue の `index` が元 JSON の entry 位置を指す
- [ ] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

## notes

- 依存は導入済みの serde / serde_json を使う。**それ以外の新規依存は追加禁止**(JSONC 除去は自前の小関数でよい。json5 等の crate を足さない)
- `BindingIssue` は将来 import report(SPEC-0004)と同じ表示経路に乗る。人間可読な `Display` を実装しておく
- `sequence`(`"ctrl+x ctrl+s"`)も `key` に書ける(parse_key_sequence をそのまま使う)
- commit は人間または main agent が行う(AGENTS.md)
