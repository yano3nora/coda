260907 syntax highlight の言語カバレッジと file 判定
===

## asis

- `.ts` など主戦場のファイルでハイライトが効かない。原因は 2 つ
    1. syntect の `load_defaults_newlines()` に TypeScript / TSX / TOML / INI / Dockerfile / git 系 (commit message・rebase-todo・config・ignore) の定義がそもそも入っていない。TOML は SPEC-0001 の受け入れ基準 (YAML / TOML / JSON / shell / Rust) に入っているのに未達だった
    2. `HighlightEngine::syntax_for_path` が `path.extension()` だけで判定しており、`Makefile` / `Dockerfile` / `.bashrc` / `.gitconfig` / `COMMIT_EDITMSG` のような拡張子なし・dotfile は定義があっても色が付かない (syntect の `file_extensions` は file 名も持つのに参照していなかった)
- 同梱定義は Markdown 1 本だけで、実行時に `SyntaxDefinition::load_from_str` で YAML parse → `into_builder().build()` で再 link していた。TypeScript 定義 (3,400 行) を同じ方式で足すと release でも parse に約 670ms かかり、「短時間編集」の起動を壊す

## tobe

- file 名 → 拡張子 → 1 行目 (shebang / `FROM ...`) の順で syntax を解決する (syntect の `find_syntax_for_file` と同順。file 名の完全一致が最も限定的)。開いている buffer の 1 行目を使い、file I/O はしない
- terminal で短時間編集するファイル (設定ファイル・git 作業・コンテナ・SSH 先の dotfile) を優先して定義を同梱する: TOML / INI / Dockerfile / Git Commit / Git Rebase Todo / Git Config / Git Ignore / Git Attributes / TypeScript / TSX
- 同梱定義の parse と link は build.rs で済ませ、実行時は dump を `from_binary` で読むだけにする (起動コストを増やさない)
- 同梱定義の出典とライセンスを `src/highlight/assets/LICENSE-*.txt` に残し、`THIRD-PARTY-NOTICES.md` へ転記する

## todo

- [x] `syntax_for_path` を `syntax_for_file(path, first_line)` に変え、file 名 → 拡張子 → 1 行目で解決する。1 行目は highlight cache と同じ行長上限で切ってから regex にかける
- [x] `build.rs` で default set + `assets/*.sublime-syntax` を link 済み dump にし、`engine.rs` は `include_bytes!` + `from_binary` で読む
- [x] 実行時の syntect feature を `parsing` / `default-themes` / `dump-load` / `regex-fancy` に絞る (yaml-load / html / plist-load を落とす)
- [x] TOML / INI / Dockerfile / Git Formats (Commit / Rebase / Config / Ignore / Attributes / Common) / TypeScript / TypeScriptReact の定義を同梱する
- [x] 出典・ライセンス notice を assets に置き、xtask licenses の `BUNDLED_ASSET_NOTICES` に登録して `THIRD-PARTY-NOTICES.md` を再生成する
- [x] ADR-0006 に「同梱する言語の基準」と build-time dump の決定を追記する
- [x] 残りの候補 (nginx / systemd / ssh_config / .env / HCL / Nix / Kotlin / Swift 等) を BACKLOG に積む

## testcases

- [x] table-driven: 拡張子 (`main.rs` / `app.ts` / `app.tsx` / `config.toml` / `php.ini` / `setup.cfg`)、file 名 (`Makefile` / `Dockerfile` / `Cargo.lock` / `.bashrc` / `.gitconfig` / `.gitignore` / `.gitattributes` / `COMMIT_EDITMSG` / `git-rebase-todo`)、1 行目 (`#!/usr/bin/env bash` / `FROM ubuntu`) で期待の syntax 名に解決し、未知は `None`
- [x] 同梱した各言語で複数色のスパンが出る (定義が dump に入り link できている)
- [x] 既存の Markdown fence embed / Rust / 日本語 grapheme のテストが従来どおり通る
- [x] `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` が通る
- [x] release binary: 4.90MB → 4.56MB (yaml-load 等を落とした分)。engine 構築 (dump 読込) は release で 0.03 秒

## notes

- 取得元と固定 commit は各 `LICENSE-*.txt` に記載。TypeScript / TSX は bat が Microsoft の tmLanguage を sublime-syntax に変換したものを流用した (syntect は tmLanguage を読めない)。INI は bat が使う clintberry 版が無ライセンスだったため jwortmann/ini-syntax (Apache-2.0) を採用した。Git Formats は Markdown と同じ sublimehq/Packages v3211 (version 2 機能を使わない世代)
- syntect の `include: Git Common.sublime-syntax#ctx` は syntax **名** (`name: Git Common`) で link される。file 名ではないので assets 側の file 名は自由
- dump は同じ syntect version の bincode 形式に依存する。build-dependencies と dependencies を同じ version 指定にしてある (Cargo.lock で一致)
- `find_syntax_by_first_line` は毎フレーム呼ばれるが、file 名か拡張子で解決できたファイルは到達しない。到達するのは名前で決まらないファイルのみで、1 行目を `MAX_HIGHLIGHT_LINE_BYTES` で切った上での数十個の regex なので無視できる (Codex 指摘: 上限なしだと巨大 1 行で cache の保護を迂回する)
- Codex レビュー 2 往復。1 回目の指摘 3 件 (解決順序 / 1 行目の長さ保護 / build.rs の read_dir エラー黙殺) と 2 回目の指摘 (長さ制限テストが制限なしでも通る / 順序の回帰テスト不足 / doc comment の位置) を反映した。順序の回帰テストは、同梱セットで file 名一致と拡張子一致が別 syntax を指す唯一の組 (`js.erb`: JavaScript (Rails) vs erb -> HTML (Rails)) で行う
- `MAX_HIGHLIGHT_LINE_BYTES` は cache.rs のまま engine.rs から参照する (Codex は mod.rs への移動を提案)。行数上限 `MAX_HIGHLIGHT_LINES` と対で cache の保護値なので分離しない。相互参照は Rust 上問題なし
- `.gitignore` に `build/` があるが `build.rs` はファイルなので無視されない
- 生成した `THIRD-PARTY-NOTICES.md` に bincode の CRLF LICENSE が混ざり git が毎回 warning を出したので、generator 側で LF に正規化した
