# TASK-260706-10: VS Code keybindings importer と import report

260706 vscode importer + report
===

## asis

- VS Code command 名 → 内部 action の変換表は `keymap/vscode_commands.rs` に実装済み(TASK-09)
- JSONC 除去・key parse・predicate parse・user bindings loader は完成済み
- import 機能(SPEC-0004 の中核)と、その結果を起動時に読む経路が存在しない

## tobe

- `coda keymap import vscode <path>` が動き、`generated/vscode-bindings.json` と import report を出力する
- editor 起動時に generated bindings が `Source::Imported` として読み込まれ、実際にキーが効く
- 変換できなかった binding が**全て** report に分類されて現れる(SPEC-0004 Invariants)

## todo

### keymap 層(pure logic)

- [x] `src/keymap/vscode_when.rs`: VS Code `when` 式 → 内部 predicate 文字列への変換
    - identifier 置換表(SPEC-0004): `editorFocus→editorFocus` / `editorTextFocus→textInputFocus` / `editorHasMultipleSelections→hasMultipleSelections` / `editorReadonly→isReadonly`(`!` を保持)/ `suggestWidgetVisible→suggestVisible` / `inQuickOpen→quickOpenVisible` / `listFocus→listFocus`
    - `&&` と `!` のみ対応。`||`・括弧・`==` 等の演算子、置換表にない identifier を含む式は `UnsupportedCondition(詳細)` エラー
- [x] `src/keymap/vscode_import.rs`: importer 本体
    - 入力: VS Code `keybindings.json` の JSONC テキスト(`strip_jsonc_comments` を再利用。必要なら pub(crate) 化)
    - entry 分類(この順で判定。**全 entry が必ずいずれかに落ちる**):
        1. `command` が `-` 始まり(negative binding)→ `Unsupported command`(理由: `negative binding is not supported in MVP`)
        2. `command` が変換表にある → key / when の変換へ(4 以降)
        3. `command` が `workbench.` / `extension.` / その他 dot 区切り拡張名で変換表にない → **Ignored**(`outside editor scope`)。ただし `editor.` / `cursor` 始まりは **Unsupported command**(`feature not implemented`)
        4. key parse 失敗(`ctrl+[IntlBackslash]` 等)→ `Invalid key`
        5. `when` 変換失敗 → `Unsupported condition`
        6. 変換成功 → **Imported**。ただし既に Imported 済みの entry と「key 列と when が完全一致」する場合は **Conflict**(先行 entry を報告付きで上書き = VS Code の後勝ちに合わせる)
    - 出力: `VsCodeImport { bindings: Vec<Binding /* Source::Imported */>, report: ImportReport }`
- [x] `src/keymap/report.rs`: `ImportReport`
    - 分類ごとの entry リスト(key / command / 理由)を保持
    - `summary()` と `render_text()`: SPEC-0004 の report 形式に従う(件数一覧 + Examples 行。`Invalid keys: N` 行を追加拡張)
    - `Disabled by terminal capability` は capability 検出が未実装のため常に 0(行は出す。TODO コメントで capability task に繋ぐ)
- [x] key 列の設定ファイル向け直列化: `format_key_for_config(&[KeyEvent]) -> String`(`"ctrl+shift+j"` / `"ctrl+k ctrl+s"` 形式。`parse_key_sequence` との round-trip をテストで保証)

### app 層

- [x] `src/app/import_cli.rs`: `coda keymap import vscode <path> [--dry-run] [--replace] [--print-report]`(SPEC-0005)
    - 変換結果を `$XDG_CONFIG_HOME/coda/generated/vscode-bindings.json` に書く(内部 action 名の配列。ディレクトリは無ければ作成)
    - 既存 generated がある場合、`--replace` なしなら書き込まずエラー終了(メッセージで `--replace` を案内)
    - report を `import-reports/latest-vscode-import.txt` に保存し、summary を stdout に出す。`--print-report` で全文 stdout、`--dry-run` は一切書き込まない
    - exit code: 成功 0、入力ファイル不存在・JSON 破損は非 0
- [x] `src/app/config.rs`: 起動時に `generated/vscode-bindings.json` を `Source::Imported` として読み込む(`bindings.json` の user loader を流用し、source を Imported に差し替え。優先順位は resolver が処理する)。壊れていても起動は止めず警告
- [x] `src/app/mod.rs`: `keymap import vscode` サブコマンドの routing 追加

## testcases

importer(table-driven。全分類を含む fixture JSON を使う):

- [x] `cursorDown`(when: `editorFocus`)→ Imported、内部で `cursor.down` / `editorFocus`
- [x] `cursorDownSelect` → `selection.down`
- [x] `workbench.action.terminal.new` → Ignored(outside editor scope)
- [x] `extension.foo` / `projectManager.list` → Ignored
- [x] `editor.action.rename` → Unsupported command(feature not implemented)
- [x] `-cursorDown` → Unsupported command(negative binding)
- [x] when `resourceLangId == markdown` → Unsupported condition
- [x] when `editorTextFocus && !editorReadonly` → Imported(`textInputFocus && !isReadonly` に変換)
- [x] key `ctrl+[IntlBackslash]` 相当の parse 不能 key → Invalid key
- [x] 同一 key・同一 when の 2 entry → 後勝ちで 1 binding + Conflict 1 件
- [x] 全 entry 数 = 各分類の合計(黙って消えた entry がない)
- [x] report の `render_text()` に SPEC-0004 の件数行と Examples 行が含まれる(golden 比較)
- [x] `format_key_for_config` → `parse_key_sequence` の round-trip(`ctrl+shift+j` / `cmd+s` / `ctrl+k ctrl+s` / `f1` / `ctrl+space`)

app:

- [x] generated/vscode-bindings.json を書いた後の config::load が Imported source の binding を返す
- [x] `--dry-run` がファイルを書かない(unit で fs 副作用を検証)
- [x] 品質: `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

手動(main agent が実施):

- [x] 実際の VS Code keybindings.json サンプルで import → report 確認 → editor 起動して imported binding が効く

## notes

- レビュー指摘なし。main agent の E2E 検証済み (2026-07-06): 9 entry fixture で全分類の合計一致 (Imported 3 / Ignored 2 / Unsupported 3 / Conflict 1)、generated 書出し、起動時 Imported 読込、imported ctrl+k (cursor.up) の実動作を PTY で確認

- 新規依存 crate の追加は禁止(serde / serde_json 導入済み)
- `Disabled by terminal capability` の実分類は capability 検出タスク(SPEC-0003)実装後に importer へ結線する。本タスクでは行だけ出す
- report の保存パス・generated のレイアウトは SPEC-0005 / ADR-0005 に従う
- import 対象の `keybindings.json` は VS Code の「user 定義分」であり、VS Code default keymap 全体の合成はしない(SPEC-0004 Open Question は「user 定義分のみ」で確定)
- commit は人間または main agent が行う(AGENTS.md)
