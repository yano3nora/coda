# TASK-260711: wrap toggle と長行の視認性

260711 wrap toggle & long-line visibility
===

## asis

- 長い行は viewport 幅で切れて表示され、`left_col` による cursor 追従の横スクロールのみ存在する([renderer task](TASK-260706-renderer.md))
- wrap 表示は未実装。SPEC-0001 / ADR にも言及なし
- 2nd dogfood で「wrap の toggle が欲しい」「長文で横スクロールしたい」の要望。横スクロール自体は実装済みだが、(1) 行が切れていることを示す indicator がなく、(2) `Cmd+→` / `End` 到達手段が [Ghostty key interception](TASK-260711-dogfood-2-ghostty-key-interception.md) の問題で死んでいたため、「スクロールできない」ように見えていた

## tobe

- `view.toggleWrap`(VS Code `editor.action.toggleWordWrap` 相当、default `alt+z`)で wrap on/off を切替できる
- wrap off 時、行が viewport 外へ続くことが右端 indicator(`…` 等)で分かる
- config.toml に `wrap` 初期値を持てる

## todo

- [x] **EditorAction 追加**: `view.toggleWrap`(palette から実行可能に。default binding `alt+z`)
- [x] **描画: visual line wrap**: `EditorView::draw` で 1 logical line を複数 screen row に折返す。継続行の gutter は空白。grapheme / display width(既存の `unicode_width` 基盤)単位で折る(単語境界 wrap は scope 外)
- [x] **cursor 表示位置と `ensure_cursor_visible` の wrap 対応**: wrap on では `left_col` を常に 0 とし、縦方向は visual row 単位で追従
- [x] **wrap off の truncation indicator**: 右端(および `left_col > 0` のとき左端)に切断 marker を表示
- [x] **config**: `wrap = false` を default に `config.toml` へ追加(SPEC-0005 に追記)

## testcases

- [x] unit: 折返し計算(ASCII / 全角 / 絵文字 / tab 相当)の table-driven test
- [x] unit: wrap on/off それぞれで cursor screen position と viewport 追従が正しい
- [x] unit: truncation indicator が「切れているときだけ」出る
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`

## notes

- 実装 (260712): 折返し計算は `editor_view::wrap_segments(line, cols) -> Vec<(start, end)>`(grapheme index 区間)に一元化し、描画・cursor 位置・viewport 追従が全てこれを参照する。scroll 位置は `(top_line, top_segment)` の visual row 単位で、1 logical line が viewport より高くても追従できる。wrap flag は EventLoop が所有し(全 buffer 共通)、`[editor] wrap` が起動時初期値。VS Code importer に `editor.action.toggleWordWrap -> view.toggleWrap` の変換も追加
- MVP 判断: `cursor.up/down` は logical line 移動のまま(VS Code は visual 移動だが、wrap 中の visual 移動は cursor model への影響が大きい)。dogfood で不満が出たら別タスクで検討
- ADR-0001 評価基準との整合: SSH 先での config / log 閲覧は長行が頻出であり「terminal での短時間編集を改善するか」= yes。scope creep ではない
- **trackpad / wheel での横スクロールは本タスクの scope 外**: mouse event は app が opt-in する protocol(DECSET 1000/1002/1006)であり、coda は未実装のため terminal から wheel event 自体が届かない。ADR-0008 決定 3(mouse support、backlog P2)で扱う。本タスクは keyboard 到達性(`End` / `cmd+→`)と表示(wrap / indicator)のみを対象とする
