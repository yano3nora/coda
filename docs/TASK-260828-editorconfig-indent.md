# TASK-260828: .editorconfig の indent 設定読み込み

## 背景

BACKLOG 由来 (TASK-260828-indent-style-config の追記から)。SSH 先で他人のリポジトリを直すコア用途では、相手リポジトリの `.editorconfig` に indent 設定が書かれていることが多い。config.toml の editor-wide 設定だけだと buffer ごとの流儀に追従できない。

## 決定

- **優先順位**: palette の一時変更 > `.editorconfig` > config.toml。上位層が未指定の項目 (style / width 個別) は下位層へフォールスルーする
- **per-buffer 化**: `Document` が `.editorconfig` 由来の値と palette override を持つ。palette の `editor.toggleIndentStyle` / `editor.setIndentWidth` は **editor-wide → active buffer のみ** に挙動変更 (他 buffer の `.editorconfig` 値を壊さないため)。runtime-only 契約 (config.toml へ書き戻さない) は従来通り
- **パーサー**: `ec4rs` v1.2.0 を導入。glob (`{a,b}` / `**` / 文字クラス) や `root=true` の階層解決を仕様準拠で扱うため、自前実装はしない (ユーザー合意済み)
- **読む property**: `indent_style` / `indent_size` のみ (`indent_size = "tab"` は `tab_width` へのフォールバックを ec4rs の `use_fallbacks` に委ねる)。width は config.toml と同じ 1〜`IndentConfig::MAX_WIDTH` に制限し、範囲外・不正値は「その項目だけ未指定扱い」— editorconfig 仕様自体が不正値 ignore を要求しており、他人のリポジトリを開くたびに warning を出さない
- **解決タイミング**: `Document::open` と Save As での path 確定時。unnamed buffer は対象外
- **palette override の意味論**: 一度設定すると **buffer 生存中は固定** (Save As で別の `.editorconfig` chain に移っても override が勝ち続ける)。「palette 一時変更が最優先」の宣言をそのまま適用した挙動で、解除は buffer を閉じて開き直す。下位層と一致したら `None` に戻す暗黙解除は、ユーザーの明示操作を勝手に忘れる挙動になるため採らない (Codex レビューで意味論を確認)
- **壊れた `.editorconfig`** (チェーンの parse 失敗): 設定を黙って落とさず status bar に notice を出して config.toml 値で継続 (黙って壊れないルール)。注意: ec4rs は preamble (最初の section より前) が壊れたファイルを open 段階で丸ごと skip するため、warning が出るのは section 内の parse 失敗のみ

## 実装

- `Cargo.toml`: `ec4rs = "1.2.0"` 追加
- `app/editorconfig.rs` (新規): `IndentOverride { style: Option<IndentStyle>, width: Option<usize> }` と `resolve(path) -> Resolution { indent, warning }`。ec4rs 依存はこのモジュールに隔離
- `app/document.rs`: `editorconfig_indent` / `indent_override` フィールド + `apply_editorconfig()` (path があるとき resolve して warning を返す)。解決は `Document::open` 内で行い `LoadInfo.editorconfig_warning` で返す — 呼び出し側の手動呼び出しにすると将来の open 経路で漏れるため open の不変条件にした (Codex レビュー指摘)。`LoadInfo` は `Copy` を外した
- `app/event_loop.rs`:
    - `effective_indent()` — active buffer の 3 層を合成。Tab 挿入 / `edit.indent` / `edit.outdent` / prompt 表示が参照
    - `EditorToggleIndentStyle` / `IndentWidth` prompt 確定 — active buffer の `indent_override` へ書き込み
    - open (startup / runtime) と Save As の path 確定時に `apply_editorconfig()` を呼び、warning を notice へ
- `self.indent` は config.toml 由来の base 値として存続 (`set_indent` の契約不変)

## Codex レビュー対応

- [P2] Save As の書き込み失敗時に `.editorconfig` warning が消失 → 成否によらず message へ連結するよう修正
- [P2] open 時の解決が呼び出し側任せで将来の経路で漏れる → `Document::open` 内解決 + `LoadInfo.editorconfig_warning` へ移動 (上記)
- 意味論確認 (override の解除不能) → 「buffer 生存中は固定」を仕様として明文化 (上記)
- テスト不足 → broken chain warning / Save As での chain 移動 / buffer-local の双方向 isolation を追加

## 検証

- `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`
- 追加テスト: `app/editorconfig.rs` の resolve (style/width、`indent_size = tab`→`tab_width`、範囲外 ignore、glob セクション、broken file warning)、event_loop の effective 合成 (editorconfig あり buffer の Tab 挿入 / palette override が buffer-local であること / editorconfig なしは config 値)
