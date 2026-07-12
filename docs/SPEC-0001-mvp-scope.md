# SPEC-0001: MVP Scope and Acceptance Criteria

## Overview

MVP で提供する editor 機能の範囲と、MVP 完了の判定基準を定義する。keybinding 関連の詳細は SPEC-0002〜0005 に委譲する。

## Goals

- 既存 editor の keymap を import し、利用者がすでに持っている筋肉記憶でファイル編集できること
- terminal 内での短時間のテキスト編集(SSH 先、git rebase、設定ファイル修正)が完結すること

本製品は「Vim の代替」でも「VS Code の terminal 版」でもない。「既存 keymap を持ち込める plain text editor」である。

### Target User

- VS Code、Zed、Sublime Text、JetBrains 系などを普段使っている
- terminal 内の編集だけ Vim / Emacs に切り替えることが苦痛
- SSH、Git 操作、設定編集などで terminal editor が必要
- plugin や高度な IDE 機能ではなく、慣れた入力操作を求めている

## Non-Goals

ADR-0001 の Non-goals に加え、MVP では以下を実装しない。

- plugin system、LSP、file tree、Git UI
- syntax-aware な編集機能(fold / rename / syntax-aware selection)。highlighting は表示専用(ADR-0006)
- mouse support、persistent session、recent files、fuzzy file open(deferred)
- VS Code 全 command の互換

## Terms

- `buffer`
    - 開いているファイル 1 つ分の編集状態。tab として表示される
- `overlay`
    - editor 本文の上に表示される一時 UI(search / replace / command palette 等)。独自の keybinding context を持つ
- `command palette`
    - 全 EditorAction をインクリメンタルサーチして実行できる overlay。rescue の唯一の入口でもある(SPEC-0002)
- `rescue`
    - 設定破損時にも操作不能にならない保証。command palette 単一入口(`F1`)で提供する(SPEC-0002)

## Behavior

### File Operations

- 単一 / 複数ファイルを開く(複数は tab / buffer として)
- 保存、Save As
- 未保存変更がある場合の警告
- ファイルが外部変更された場合の警告
- UTF-8 テキストファイルの読み書き、改行コード(LF / CRLF)の維持
- large file protection(閾値超過時は警告または read-only で開く。閾値は Open Questions)

### Editing

- character / word / line start / line end / page 単位の cursor movement
- 行番号 gutter の常時表示(buffer 行数で桁揃え。2026-07-06 dogfood フィードバックにより deferred から昇格)
- selection movement、basic multi-line selection、select all
- copy / cut / paste
    - paste は terminal の bracketed paste を第一とする(`Cmd+V` 等は terminal に委譲。ADR-0008)
    - copy / cut は OSC 52 で OS clipboard へ書き込み、拒否時は内部 clipboard に fallback して明示する
- undo / redo
- insert line before / after
- delete character / word / line

### Buffers and Tabs

- 複数 buffer を開ける、次 / 前の buffer に移動できる
- buffer を閉じられる。未保存 buffer の close 時に警告する
- tab UI は最小限でよい

### Command Palette

- `F1`(常時有効)/ `Ctrl+Space`(default。config で変更可)で開く
- 全 EditorAction をインクリメンタルサーチして実行できる
- 各 command に現在 bind されている key を併記する
- save / quit / help / inspect-key を含む全操作の代替経路であり、shortcut を覚えていなくても操作が完結する

### Syntax Highlighting(ADR-0006)

- syntect による行ベースの highlighting。表示専用であり、編集操作・keymap 解決には使わない
- theme は同梱の dark / light から `config.toml` で選択する
- terminal の color capability(truecolor / 256 / 16 色)を検出し、色を丸める。検出不能時は highlighting off で起動する
- large file protection の閾値超過時は自動 off
- 実装は keybinding engine 完成後(ADR-0004 実装順序)

### Search and Replace

- current buffer search、next / previous match
- case sensitivity toggle(regex は MVP では optional)
- current buffer replace、replace all
- search overlay は editor keymap と異なる context を持つ(SPEC-0002)

### Split Views(MVP 後半または v0.2)

- vertical / horizontal split、pane focus next / previous、pane close、pane maximize toggle
- split より先に keymap import / editor interaction を完成させる

### Mouse Support(MVP 後半または v0.2。ADR-0008)

- click = カーソル移動、drag = selection、wheel = スクロール(SGR mouse protocol)
- Shift+ドラッグは terminal がアプリへ送らず native 選択に使う慣習に依存する。Shift付きSGR eventを受信した場合は無視するが、terminal 選択へ戻すことはできない

## Invariants

- `F1` による command palette open はいかなる設定状態でも有効である
- 未保存変更が警告なしに失われることはない
- import できない binding が黙って捨てられることはない(SPEC-0004)

## Edge Cases / Failure Modes

- 設定ファイル(config.toml / bindings.json)が破損していても、default binding + command palette で起動できる
- terminal が modern keyboard protocol 非対応でも安全に起動できる(SPEC-0003)
- 巨大ファイル・バイナリファイルを開いた場合に freeze しない(protection の詳細は Open Questions)

## API / Interface

CLI は SPEC-0005、設定ファイルは ADR-0005 / SPEC-0005 を参照。

## Acceptance Criteria

以下をすべて満たしたら MVP とする。

### Editor

- [ ] UTF-8 テキストファイルを開ける
- [ ] 編集して保存できる
- [ ] undo / redo が動く
- [ ] selection / copy / cut / paste が動く
- [ ] find / replace が動く
- [ ] 複数 buffer を開ける
- [ ] 主要な形式(例: YAML / TOML / JSON / shell / Rust)で syntax highlighting が表示される
- [ ] dark / light theme を切り替えられる

### Keymap

- [ ] VS Code `keybindings.json` を import できる
- [ ] `cursorDown` / `cursorUp` / word movement / selection movement を変換できる
- [ ] `editorFocus` を含む基本的な `when` を解決できる
- [ ] user binding が imported binding を上書きできる
- [ ] conflict を検出できる
- [ ] unsupported command / condition を report できる

### Terminal Compatibility

- [ ] modern keyboard protocol が使える terminal で modifier を可能な限り区別できる
- [ ] protocol 非対応 terminal で安全に起動できる
- [ ] 区別不能な binding を明示できる
- [ ] raw input inspector を使える

### Scope Control

- [ ] plugin system / LSP / file tree / Git UI を実装していない
- [ ] VS Code 全 command の互換を目指していない

## Trouble Shooting

- キーが効かない → `:inspect-key` で raw input と解決結果を確認する(SPEC-0003)
- binding がどこ由来か分からない → `:which-key <key>` で source を確認する(SPEC-0002)
- import 結果が想定と違う → `~/.config/<app>/import-reports/` の report を確認する(SPEC-0004)

## Open Questions

- large file protection の閾値(サイズ / 行数)と挙動(警告 / read-only / 拒否)
- clipboard 統合の優先順位(OSC 52 / OS clipboard コマンド / 内部 clipboard の fallback 順)
- Deferred(MVP 完了後に判断): split view 正式サポート、tree-sitter(highlighting engine の差し替え候補として)、ユーザー theme 追加、persistent session、recent files、fuzzy file open、minimap 相当、read-only mode、diff mode、他 editor profile import(Zed / Sublime / JetBrains / Helix)、remote clipboard、SSH 向け bootstrap、configuration UI、keybinding conflict UI
