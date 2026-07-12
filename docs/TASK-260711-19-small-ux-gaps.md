# TASK-260711-19: 起動・編集の小改善(2nd dogfood 残項目)

260711 small ux gaps
===

## asis

2nd dogfood の残項目。いずれも独立した小改善で、根因調査済み。

1. **ファイル指定なしで起動できない**: `app::run_editor` が空 paths を usage error(exit 2)にしている。内部は `Document::unnamed()` / `buffer.new` action で unnamed buffer 対応済みで、CLI 入口だけが塞いでいる
2. **palette から設定ファイルを開けない**: `config.toml` / `bindings.json` の場所を知らないと編集導線がない
3. **`Shift+Tab` でインデントを戻せない**: decoder は `CSI Z` → `shift+tab` を正しく decode 済みだが、`EditIndent` / `EditOutdent` action 自体が存在しない(`Tab` は文字挿入のみ。event_loop 直処理)

## tobe

- `coda` 単体起動で unnamed buffer が開き、保存時に Save As 導線(TASK P1「Save As」)へ繋がる
- palette の `config.openSettings` / `config.openKeybindings` で設定ファイルを buffer に開ける
- `Tab` / `Shift+Tab` が VS Code 同等に動く: selection なし `Tab` = 文字挿入(現状維持)、selection あり `Tab` = 行インデント、`Shift+Tab` = 常に outdent

## todo

- [ ] **CLI: 空 paths 許可**: `run_editor` の usage error を除去し unnamed buffer で起動(usage 表示は `--help` 相当に残す)
- [ ] **EditorAction 追加**: `config.openSettings` / `config.openKeybindings`(config dir 解決は `app/config` の既存ロジックを再利用。ファイル未作成なら雛形つき新規 buffer)
- [ ] **EditorAction 追加**: `edit.indent` / `edit.outdent`(default: `tab`(selection あり時)/ `shift+tab`。indent 幅は当面 4 spaces 固定とし、config 化は将来)
- [ ] undo 単位: indent / outdent は 1 操作 = 1 undo にまとめる

## testcases

- [ ] unit: 空 paths で EventLoop が unnamed buffer 1 枚で起動する
- [ ] unit: indent / outdent の table-driven test(selection 複数行・行頭空白なし・タブ混在・undo 1 発で戻る)
- [ ] unit: `shift+tab`(CSI Z 経由)が resolver で `edit.outdent` に解決される
- [ ] PTY: `coda` 引数なし起動 → 即 quit が正常終了する
- [ ] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`

## notes

- `ctrl+n` / `ctrl+p` のカーソル上下は本タスクではなく TASK-17 の「macOS 慣行 default」で扱う
- unnamed buffer の保存は Save As(P1)が前提。本タスク時点では「保存不可の旨を status bar 表示」でよい(黙って壊れない原則)
