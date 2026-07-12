# TASK-260711: 起動・編集の小改善(2nd dogfood 残項目)

260711 small ux gaps
===

## asis

2nd dogfood の残項目。いずれも独立した小改善で、根因調査済み。

1. **ファイル指定なしで起動できない**: `app::run_editor` が空 paths を usage error(exit 2)にしている。内部は `Document::unnamed()` / `buffer.new` action で unnamed buffer 対応済みで、CLI 入口だけが塞いでいる
2. **palette から設定ファイルを開けない**: `config.toml` / `bindings.json` の場所を知らないと編集導線がない
3. **`Shift+Tab` でインデントを戻せない**: decoder は `CSI Z` → `shift+tab` を正しく decode 済みだが、`EditIndent` / `EditOutdent` action 自体が存在しない(`Tab` は文字挿入のみ。event_loop 直処理)

## tobe

- `coda` 単体起動で unnamed buffer が開き、保存時に [v0.1 release readiness](TASK-260712-v0.1-release-readiness.md) の Save As 導線へ繋がる
- palette の `config.openSettings` / `config.openKeybindings` で設定ファイルを buffer に開ける
- `Tab` / `Shift+Tab` が VS Code 同等に動く: selection なし `Tab` = 文字挿入(現状維持)、selection あり `Tab` = 行インデント、`Shift+Tab` = 常に outdent

## todo

- [x] **CLI: 空 paths 許可**: `run_editor` の usage error を除去し unnamed buffer で起動(usage 表示は `--help` 相当に残す)
- [x] **EditorAction 追加**: `config.openSettings` / `config.openKeybindings`(config dir 解決は `app/config` の既存ロジックを再利用。ファイル未作成なら雛形つき新規 buffer)
- [x] **EditorAction 追加**: `edit.indent` / `edit.outdent`(default: `tab`(selection あり時)/ `shift+tab`。indent 幅は当面 4 spaces 固定とし、config 化は将来)
- [x] undo 単位: indent / outdent は 1 操作 = 1 undo にまとめる

## testcases

- [x] unit: 空 paths で EventLoop が unnamed buffer 1 枚で起動する
- [x] unit: indent / outdent の table-driven test(selection 複数行・行頭空白なし・タブ混在・undo 1 発で戻る)
- [x] unit: `shift+tab`(CSI Z 経由)が resolver で `edit.outdent` に解決される
- [ ] PTY: `coda` 引数なし起動 → 即 quit が正常終了する(PTY test 基盤が repo に存在しないため省略。`timeout 5 coda < /dev/null` による手動確認で、旧来の usage error(exit 2)ではなく raw mode 有効化まで到達することのみ確認した)
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`

## notes

- `ctrl+n` / `ctrl+p` のカーソル上下は本タスクではなく [Ghostty key interception](TASK-260711-dogfood-2-ghostty-key-interception.md) の「macOS 慣行 default」で扱う
- unnamed buffer の保存は Save As(P1)が前提。本タスク時点では「保存不可の旨を status bar 表示」でよい(黙って壊れない原則)
- **実装時の判断**:
    - `tab` の default binding は when 句 `textInputFocus && hasSelection` のみを追加し、selection なし Tab は従来どおり `event_loop::handle_text_input` の直接処理(文字挿入)に委ねた。resolver 側で when 不成立時は binding ごと除外される(`NoMatch`)ので、既存の挿入経路に自然にフォールバックする
    - `config.openSettings` / `config.openKeybindings` に default keybinding は割り当てなかった(仕様上 palette 経由のみが要件で、任意キーを潰すリスクを避けた)
    - 新規作成 config file のテンプレートは buffer に挿入するのみで、保存されるまでディスクに書かない(unnamed buffer / `buffer.new` と同じ「黙って壊れない」方針)。既存ファイルを開く場合はテンプレートを挿入しない
    - `indent`/`outdent` は `core::editor::EditorCore` に実装し、`move_lines_up`/`move_lines_down` と同じ `ReplaceLines` 1-group undo 機構(`record_line_replace_group`)を再利用した。`EditKind::Other` を使うため連続 Tab 押下でも undo が 1 回ずつに分かれる
    - `outdent` は行ごとに「先頭タブ1つ」または「先頭スペース最大4つ」のどちらか実際に存在する方だけを剥がすため、tab/space混在ブロックでも行ごとに正しく1レベルだけ戻る
    - 作業のついでに、事前から存在した `search_overlay.rs` の clippy 警告(`derivable_impls`, 本タスクと無関係)を `cargo clippy --all-targets -- -D warnings` の完了条件を満たすために修正した
