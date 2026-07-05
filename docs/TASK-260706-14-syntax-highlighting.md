# TASK-260706-14: Syntax highlighting (syntect + dark/light theme)

260706 syntax highlighting
===

## asis

- 方針は ADR-0006 で決定済み(syntect、表示専用、同梱 dark/light、色 capability の明示的 degrade)
- `src/highlight/` は空 skeleton。`ui::Style` は reverse / dim のみで色を持たない
- `config.toml` の読込が存在しない(theme 選択に必要)
- syntect(default-fancy)と toml crate は導入済み(Cargo.toml)

## tobe

- `.rs` / `.toml` / `.json` / `.sh` / `.md` 等を開くと色付きで表示される
- `config.toml` の `[appearance] theme = "dark" | "light"` で切替できる(default: dark)
- truecolor 非対応 terminal では 256 色に丸め、色不明環境では白黒で安全に動く
- **highlighting は表示専用**: `core/` / `keymap/` への依存追加ゼロ(ADR-0006 の防波堤)

## todo

### ui 層(色の土台)

- [ ] `ui::Style` に `fg: Option<(u8, u8, u8)>` を追加する(bg は追加しない: MVP は fg のみで terminal の背景色を尊重する。ADR-0006)
- [ ] `ColorMode { TrueColor, Ansi256, Mono }` を追加し、renderer が Style 直列化時に参照する
    - TrueColor: `CSI 38;2;{r};{g};{b}m`、Ansi256: `CSI 38;5;{n}m`(RGB→256 の cube 変換関数 + grayscale 対応。変換はテスト可能な pure 関数)、Mono: fg を出さない
    - `ColorMode` の検出: `COLORTERM` に `truecolor`/`24bit` → TrueColor、`TERM` に `256color` → Ansi256、それ以外 → Mono(検出関数は env 値を引数で受けて pure にする)
    - SGR run 判定(既存の「run 切替時のみ SGR」)に fg も含める

### highlight 層(表示専用)

- [ ] `src/highlight/engine.rs`
    - `HighlightEngine::new(theme_choice)`: `SyntaxSet::load_defaults_newlines()` + `ThemeSet::load_defaults()`。dark = `base16-ocean.dark`、light = `InspiredGitHub`
    - `syntax_for_path(path) -> Option<..>`: 拡張子で判定。不明なら None(= 無色)
- [ ] `src/highlight/cache.rs`: 行単位の incremental cache
    - 保持: 行ごとの `(行テキストのコピー, span 列, 行末 ParseState/HighlightState)`
    - `spans_for(buffer, viewport_range) -> Vec<Vec<(grapheme_range, (r,g,b))>>`:
        - cache と行テキストを比較し、**最初に異なる行から** viewport 末尾まで再計算(それ以前は cache を返す)
        - span は byte range から **grapheme range に変換**して返す(view 層が grapheme 単位で cell を塗るため)
    - 保護: 1 行が 2,000 bytes 超の行は無色(ADR-0006 の巨大単一行対策。閾値は定数)。buffer 全体が 20,000 行超なら highlighting 自体を off
- [ ] `src/app/editor_view.rs`: 行描画時に span の色を `Style.fg` へ反映(selection の reverse が優先。Tab 展開部分は無色)
- [ ] `src/app/config.rs`: `config.toml` 読込を追加
    - `[appearance] theme = "dark" | "light"`(未指定・ファイル無し → dark。parse 失敗 → 警告して default。起動は止めない)
    - 既存 `AppConfig` に `theme` を追加し、EventLoop → HighlightEngine へ渡す
- [ ] event loop / view の結線: ファイルの path から syntax を決め、編集のたびに cache 経由で再計算(cache が差分再計算を担う)

## testcases

- [ ] RGB→256 変換: 純色・グレー・白黒の既知ベクタ
- [ ] ColorMode 検出: COLORTERM=truecolor / TERM=xterm-256color / TERM=dumb の 3 系
- [ ] renderer: fg 付き Style の run が TrueColor / Ansi256 / Mono で期待どおりの SGR になる
- [ ] engine: `.rs` で syntax が見つかり、拡張子不明で None
- [ ] cache: 初回計算 → 中間行を編集 → **編集行より前は再計算されない**(計算回数 or 状態比較で検証)+ 編集行以降が新しい span になる
- [ ] cache: span が grapheme range で返る(日本語コメント行で桁ズレしない)
- [ ] 2,000 bytes 超の行が無色 / 20,000 行超で全体 off
- [ ] config.toml: theme=light / 未指定 / 壊れた TOML(警告 + default)
- [ ] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

手動(main agent PTY + 人間):

- [ ] PTY: .rs ファイルを開いた出力に `38;2;`(truecolor SGR)が含まれる(COLORTERM=truecolor 指定時)
- [ ] 人間: Ghostty で Rust / TOML ファイルが色付き表示され、編集してもハイライトが追従する。theme = "light" 切替

## notes

- **依存境界(最重要)**: `highlight/` は `core/`(行テキスト読取)と `ui/`(Style)にのみ依存する。`core/` / `keymap/` から highlight を参照してはならない。syntax 情報を編集・keymap に使う API を作らない(ADR-0006)
- syntect の Theme 背景色は使わない(fg のみ)。選択範囲は既存 reverse が優先
- 性能: syntect の行 parse は状態依存のため、cache の「最初に異なる行から再計算」が本質。viewport より下の行は計算しない
- `config.toml` の他の設定項目(sequence_timeout 等)は今回結線しなくてよい(theme のみ)
- commit は人間または main agent が行う(AGENTS.md)
