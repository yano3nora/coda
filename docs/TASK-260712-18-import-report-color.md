# TASK-260712-18: import report の stdout 出力を bucket ごとに色分け

260712 import report color
===

## asis

- `coda keymap import vscode <path> --print-report` の stdout は単色テキストで、bucket(Imported / Ignored / Unsupported / ...)の区別が視覚的につかず、全件リスト化(TASK-260712-17)後は行数も多いため見づらい
- `src/keymap/report.rs` の `render_text()` / `render_summary()` は plain text のみ
- プロジェクトには色付け用の外部 crate はなく、`src/ui/` は raw ANSI escape を直書き、TTY 判定は `libc::isatty`(`src/input/capabilities.rs:189` 参照)で行う流儀

## tobe

- stdout が TTY かつ `NO_COLOR` 環境変数が未設定(または空)のときのみ、report / summary を ANSI 16色で色分けして出力する
- 保存ファイル `~/.config/coda/import-reports/latest-vscode-import.txt` には ANSI を一切含めない(常に plain)
- pipe / リダイレクト時(`isatty` false)や `NO_COLOR` 設定時は現行と完全に同一の plain 出力
- 配色(16色 SGR のみ。256色 / truecolor は使わない):
    - タイトル行: bold (`\x1b[1m`)
    - Imported: green (`\x1b[32m`)
    - Ignored: dim (`\x1b[2m`)
    - Unsupported commands / Unsupported conditions: yellow (`\x1b[33m`)
    - Invalid keys / Conflicts: red (`\x1b[31m`)
    - Disabled by terminal capability: magenta (`\x1b[35m`)
    - 色を付ける範囲: summary 行(`Imported: 22` など)と bucket 見出し行(`Imported (22):` など)を該当色に。見出しは bold も併用。entry 行自体はデフォルト色のまま、末尾の `[reason]` 部分だけ dim (`\x1b[2m`)
    - reset は `\x1b[0m`

### 設計

- `src/keymap/report.rs`:
    - `ReportStyle` struct を追加。フィールドは各 bucket / タイトル / reason 用の prefix と reset(いずれも `&'static str`)
    - `ReportStyle::plain()`(全フィールド空文字)と `ReportStyle::ansi()` を用意
    - `render_text_with(&self, style: &ReportStyle)` / `render_summary_with(&self, style: &ReportStyle)` を実装し、既存の `render_text()` / `render_summary()` は `plain()` を渡す薄い wrapper にする(既存呼び出し・既存テストは無変更で通ること)
    - keymap/ 層に terminal 依存(isatty / env 参照)を持ち込まない。ANSI 文字列定数は style 値としてのみ持つ
- `src/app/import_cli.rs`:
    - 色を使うかの判定関数(例: `stdout_supports_color()`)を追加: `libc::isatty(libc::STDOUT_FILENO)` かつ `NO_COLOR` が未設定または空
    - stdout 組み立て時のみ判定結果に応じた `ReportStyle` を渡す。ファイル保存用 `report_text` は従来通り `render_text()`(plain)を使う
    - `Report saved to:` 行は色なしのままでよい
    - テスト容易性のため、`run_vscode_import_in_base` 自体は style(または bool)を引数で受け取る形にして、isatty 判定は呼び出し側(`run_vscode_import` / main 経路)で行う構成を推奨

## todo

- [x] `src/keymap/report.rs`: `ReportStyle` + `render_text_with` / `render_summary_with` を実装
- [x] `src/app/import_cli.rs`: color 判定と style の受け渡しを実装(保存ファイルは plain 固定)
- [x] テスト追加(下記 testcases)
- [x] `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` を通す

## testcases

- [x] `render_text_with(ReportStyle::plain())` の出力が既存 `render_text()` と完全一致する
- [x] `render_text_with(ReportStyle::ansi())` の出力に各 bucket の SGR コード(`\x1b[32m` など)と reset が含まれ、bucket 見出し・summary 行が対応色で着色される
- [x] ansi 出力から SGR シーケンスを除去すると plain 出力と一致する(色は文言を変えない)
- [x] entry 行の `[reason]` が dim で着色され、key -> command 部分は着色されない
- [x] color 無効時(plain style)の CLI stdout に `\x1b` が含まれない
- [x] color 有効時でも保存された `latest-vscode-import.txt` に `\x1b` が含まれない
- [x] `NO_COLOR` 判定関数: 未設定 → true(TTY 前提)、`NO_COLOR=1` → false、`NO_COLOR=""`(空)→ true(NO_COLOR 規約は「存在し空でない」ときのみ無効化。https://no-color.org/ 準拠)

## notes

- 外部 crate は追加しない(プロジェクトの流儀: raw ANSI + libc)
- 256色 / truecolor を使わないのは、report は capability 検出前でも表示される可能性があるため最小公倍数に倒す判断
- TUI 本体(`ui/render.rs`)の色機構とは意図的に独立させる。report は CLI (非 alt-screen) 出力であり、ui/ に依存させると層違反になる(AGENTS.md 依存境界)
- 先行タスク: [TASK-260712-17](TASK-260712-17-import-report-full-listing.md)(全件リスト化)
