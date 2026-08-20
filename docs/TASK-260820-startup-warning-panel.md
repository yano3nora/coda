# TASK-260820: 設定破損 warning の起動時 panel 表示

260820 startup warning panel
===

## asis

warning は全て `"; "` join で 1 行 status bar に流すだけで、右端で切れて埋もれる。
trailing comma 事件 (TASK-260820-jsonc-trailing-comma) では「bindings.json 全体が
無効化された」warning が実質見えず、黙って壊れた状態になった。
「重要な warning は先頭に insert」という応急処置が 2 箇所あり、1 行方式の限界は
コード上も自認済み (event_loop.rs の interim measure コメント。full warning viewer
は backlog とされていたが BACKLOG.md には未記載だった)。

## tobe

vim の hit-enter prompt 方式: ユーザーが直すべき warning は起動時に複数行 panel で
blocking 表示し、キー入力で dismiss。palette の `warnings.show` で再閲覧できる。

- **panel 対象は「user の設定が失われた/読めなかった」系のみ** (`AppConfig::warnings`
  = bindings.json / config.toml / disabled-chords の load 失敗、HOME 未設定による
  設定 skip)。HOME 未設定は環境起因だが「設定が丸ごと失われた」通知なので panel 側
  (codex round 1 の指摘に対する判断)。Ghostty intercept や capability fallback 等の
  環境系は毎起動再現するため panel に入れると nag になる (「terminal での短時間編集」
  コンセプトと衝突)。従来どおり status bar + `inspector.open` に残す
- dismiss は任意キー (押したキーは飲み込む = buffer に流さない)。paste も同様に
  飲み込んで閉じる。F1 のみ例外で palette が panel の上に開く (rescue 常時有効の原則)
- **入力の優先順位は描画順と一致させる**: panel は prompt / inspector より上・
  palette より下に描画されるので、key / paste も palette の次・prompt / inspector の
  前に受ける (codex round 1: 「見えない overlay に入力が流れる」指摘の修正)
- **描画できない panel は入力を奪わない**: 極小画面 (`can_draw` = 高さ 6 未満 /
  幅 12 未満) では `blocks_input` が false になり、キーは透過する (codex round 1:
  「見えない blocking panel」指摘の修正)。panel は保持され、画面拡大で表示される
- **切り捨ては必ず明示する**: warning は表示幅ベース (grapheme 単位) で折り返し、
  行数超過時は末尾を `… N more line(s)` marker にする (codex round 1: 「viewer
  なのに全文閲覧できない」「全角文字が枠を破る」指摘の修正)

## todo

- [x] `app/warnings.rs`: `WarningsOverlay` + `draw_warnings` (status bar 直上に
  bottom-anchored の枠付き panel。inspector の `frame_line` / `clip_to_width` を
  pub(crate) 化して再利用)
- [x] `EditorAction::WarningsShow` (`warnings.show`) 追加、palette から実行可能に。
  warning ゼロ時は "no config warnings" を status bar に出す
- [x] 配線: `run_editor` が config warnings を `set_config_warnings` へ分離
  (status bar 行きの warnings には混ぜない)。key / paste / mouse の各 handler と
  draw に panel を追加 (palette より下に描画)

## testcases

- [x] unit (warnings.rs): panel が warning 行 + 件数 title + dismiss footer を
  status bar 直上に描画する / warning ゼロ・非表示・極小画面では何も描かない /
  長い warning が折り返される / 全角文字でも右枠が壊れない / 行数超過は
  `… N more line(s)` になる / `blocks_input` は draw guard と同値
- [x] unit (event_loop.rs): 任意キーで dismiss され打鍵が buffer に届かない /
  F1 は panel を消さず palette を開く / prompt が下にあっても panel が先にキーを
  取る / 極小画面ではキーが透過する / `warnings.show` で再表示 /
  warning ゼロ時の `warnings.show` は status bar 報告のみ
- [x] unit (action.rs): 既存 round-trip test が `ALL` 経由で `warnings.show` を検証
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` /
  `cargo test` (309 tests)

## notes

- 表示対象の severity 分けは実装ではなく「発生源」で行った: `AppConfig::warnings`
  に入るものは全て「user 設定の喪失」という既存の構造をそのまま境界に使い、warning
  への severity タグ付けは導入していない。環境系 warning を panel に入れたくなったら
  その時に型を分ける
- codex round 1 で P1 3 件を検出・修正: (1) 入力優先順位が描画順と食い違い、panel
  表示中に prompt / inspector へキーが流れる、(2) 極小画面で「見えない blocking
  panel」がキーを飲む、(3) 文字数基準 clip で全角が枠を破る + 切り捨てが無言。
  いずれも再現テストを追加してから修正
- TUI の PTY E2E は対象外 (AGENTS.md: MVP 期の e2e は importer snapshot のみ)。
  実機確認は次回 dogfood で行う
