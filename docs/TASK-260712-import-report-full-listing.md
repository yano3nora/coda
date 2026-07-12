# TASK-260712: import report の全件リスト化と summary 重複出力の解消

260712 import report full listing
===

## asis

`coda keymap import vscode <path> --print-report` の出力に2つの問題がある。

1. **全件リストがどこにも出ない**: `ImportReport` は全 entry を bucket 分類して保持しているが、`render_text()` (`src/keymap/report.rs`) が各 bucket の先頭1件だけを "Examples:" として出力する。この出力がそのまま `~/.config/coda/import-reports/latest-vscode-import.txt` に保存されるため、保存版 report ですら「Ignored 33件の内訳」が見えない。SPEC-0004 Invariants「import 対象の全 entry が report のいずれかの分類に必ず現れる(黙って捨てない)」に実質反している
2. **summary が2回出る**: CLI 側 (`src/app/import_cli.rs` の `run_vscode_import_in_base`) が自前で summary 文字列を組み立てて stdout に出した後、`--print-report` 時に `render_text()`(冒頭に同じ summary を含む)を丸ごと追記しているため
3. **保存先パスが stdout に表示されない**: report がどこに保存されたか利用者に分からない
4. SPEC-0004 の出力例自体が "Examples:" 形式で書かれており、Invariants と矛盾している(実装がこの例に忠実だったのが問題1の遠因)

## tobe

- `render_text()` は「summary + bucket ごとの全件リスト」を出力する
    - 各 bucket は見出し行(例: `Ignored (33):`)+ 全 entry の行(既存の `format_entry` 形式 `{key} -> {command} [{when}] [{reason}]` を流用)
    - 0件の bucket はセクションごと省略する
    - 既存の "Examples:" セクションは廃止
- CLI stdout はデフォルトで「summary + `Report saved to: <report_path>`」のみ
    - `--dry-run` 時は report ファイルを書かないので `Report saved to:` 行は出さない
    - summary 部分は `render_text()` と重複実装せず、`ImportReport` 側に summary だけを render する関数を切り出して共用する
- `--print-report` 時は summary を2回出さない。「full report(summary 含む)+ `Report saved to:` 行」だけを出力する
- SPEC-0004 (`docs/SPEC-0004-vscode-import.md`) の出力例を全件リスト形式に更新し、Invariants と整合させる

## todo

- [ ] `src/keymap/report.rs`: `render_text()` を全件リスト形式に変更し、summary 部分を切り出した関数(例: `render_summary()`)を追加する
- [ ] `src/app/import_cli.rs`: 自前 summary 組み立てをやめ `render_summary()` を使う。`--print-report` 時の重複を解消し、非 dry-run 時に `Report saved to: <path>` を出力する
- [ ] `docs/SPEC-0004-vscode-import.md`: Import report 節の出力例を全件リスト形式に更新する
- [ ] `report.rs` / `import_cli.rs` の既存テストを新形式にあわせて更新し、全件リスト・重複解消・保存先表示のテストを追加する
- [ ] `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` を通す

## testcases

- [ ] 複数 entry を含む bucket(例: ignored 2件以上)の全 entry が `render_text()` の出力に現れる
- [ ] 0件の bucket のセクション見出しが出力に現れない
- [ ] `--print-report` 時の stdout に summary ブロック(`Imported:` 行)が1回しか現れない
- [ ] 非 dry-run 時の stdout に `Report saved to:` と実際の report path が含まれる
- [ ] `--dry-run` 時は `Report saved to:` 行が出ない(ファイルも書かれない — 既存テスト維持)
- [ ] 保存された `latest-vscode-import.txt` に全 entry が含まれる

## notes

- `ReportEntry` / bucket 分類のデータモデルは変更不要。表示層のみの修正
- SPEC-0004 の「利用者が『何を失ったか』を把握できる」が本製品の import 機能の最優先要件(import 成功率より優先)
- terminal capability 連携(`disabled_by_terminal_capability` の実データ化)は [keyboard capability detection](TASK-260712-keyboard-capability-detection.md)で扱う。本タスクでは扱わない
