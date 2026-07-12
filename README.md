coda
===

**Keymap-first TUI text editor** — GUI editor(VS Code 等)で育てた keybinding を import して、terminal の中でも自分の筋肉記憶のままテキストを編集するためのエディタ。

> Vim の代替でも VS Code の terminal 版でもない。「既存 keymap を持ち込める plain text editor」(詳細: [ADR-0001](docs/ADR-0001-keymap-first-tui-editor.md))

現在は **v0.1 release candidate 前**。MVP の中核は動作するが、配布 asset と Linux 実機検証は未完了。

## Features

- **VS Code `keybindings.json` の import** と、成功 / 無視 / 未対応 / 衝突を明示する import report
- `Key + Context -> Action` の context-aware keybinding 解決(`rescue > user > imported > default`)
- kitty keyboard protocol 対応(`Ctrl+J` / `Ctrl+Shift+J` / `Cmd+S` を区別)+ 非対応 terminal への安全な fallback
- command palette(`F1` は設定が壊れていても常に有効な rescue 入口)
- 編集基本機能: undo/redo(グルーピング)、find/replace、複数 buffer/tabs、行番号、grapheme 単位の Unicode 処理(日本語・絵文字)
- syntax highlighting(syntect、dark/light、表示専用)
- clipboard: OSC 52 書込(SSH 先から手元の OS clipboard へ)+ bracketed paste
- raw input inspector(`coda inspect-key`)で terminal の入力問題を自己診断

## Installation

現状は**ソースからの build のみ**。

```sh
# 要 Rust toolchain (rustup または mise install)
cargo install --path .        # ~/.cargo/bin/coda に入る

# または手動配置
cargo build --release         # target/release/coda を PATH の通った場所へ
```

v0.1 以降は GitHub Release の単体 binary を、mise の GitHub backend から導入できる形にする予定:

```sh
# release asset 公開後に利用可能（現時点ではまだ使えない）
mise use -g github:yano3nora/coda@0.1.0
```

SSH 先で使う場合は、同一 OS / arch のホストへ release binary をコピーすればよい(単体 binary。ただし Linux は glibc 動的リンクのため、極端に古い distro では要再ビルド)。

```sh
scp target/release/coda remote:~/bin/
```

## Usage

```sh
# ファイルを開く (複数可)
coda file.ts other.md

# VS Code keybindings を import (再実行で上書き)
coda keymap import vscode "~/Library/Application Support/Code/User/keybindings.json" --print-report

# terminal が何を送ってくるかの診断
coda inspect-key
```

エディタ内: `F1` / `Ctrl+Space` で command palette(全操作をインクリメンタルサーチ、bind 済み key 併記)。

### Terminal setup(macOS)

**設定ゼロでも動く**のが原則。default keymap は host OS の text 編集慣行に従う([ADR-0011](docs/ADR-0011-os-convention-default-keymap.md))ため、terminal が `Cmd+←` → `Ctrl+A` のような「macOS 標準キーへの翻訳」を行っても期待どおり動作する(`Cmd+←/→` = 行頭/行末、`Opt+←/→` = 単語移動、`Ctrl+N/P` = 上下)。

さらに `Cmd` キーを**本物の modifier として**届けたい場合(`Cmd+C` copy / `Cmd+A` select all 等)は、terminal 側の予約 keybind の解除が必要:

```ini
# Ghostty (~/.config/ghostty/config)
keybind = super+arrow_left=unbind
keybind = super+arrow_right=unbind
keybind = super+a=unbind
keybind = super+c=performable:copy_to_clipboard   # terminal 選択がない時だけ coda へ透過
```

- 解除の tradeoff: shell(zsh 等)でも `Cmd+←` の行頭ジャンプ翻訳が失われる
- どのキーが届いているかの診断は `coda inspect-key`
- terminal が key を消費・変換する理屈の詳細: [ADR-0007](docs/ADR-0007-modifier-delivery-strategy.md) / [TASK-260711-17](docs/TASK-260711-17-dogfood-2-ghostty-key-interception.md)

### 設定

```text
~/.config/coda/
  config.toml                # [appearance] theme = "dark" | "light" 等
  bindings.json              # user binding (VS Code 形式 + 内部 action 名)
  generated/                 # import の出力 (直接編集しない)
  import-reports/            # import report
```

```jsonc
// bindings.json の例 (JSONC 可)
[
  { "key": "ctrl+j", "command": "cursor.down", "when": "editorFocus" }
]
```

## Development

```sh
mise install          # rust toolchain
cargo run -- <file>   # 起動
cargo test            # unit tests
mise run pre-commit   # fmt --check / clippy -D warnings / test
```

- 設計ドキュメント: `docs/ADR-*.md`(意思決定)、`docs/SPEC-*.md`(仕様)、`docs/TASK-*.md`(開発ログ)
- 開発規約: [AGENTS.md](AGENTS.md)(依存境界・Testing 方針・scope 制御)
- 入口になる doc: [ADR-0001 製品方針](docs/ADR-0001-keymap-first-tui-editor.md) / [SPEC-0001 MVP スコープ](docs/SPEC-0001-mvp-scope.md) / [SPEC-0002 keybinding システム](docs/SPEC-0002-keybinding-system.md)

## Deployment / Distribution

v0.1 は **GitHub Releases の macOS / Linux 向け単体 binary** に限定する。crates.io / Homebrew / 自動 publish は、手動 release で asset 命名と導入 UX が安定してから判断する。

予定する release flow:

1. `mise run release:prepare -- <version>` で version bump・test・全 target の snapshot asset / checksum を生成
2. 人間が差分を review し、commit と `v<version>` tag を作成
3. 人間だけが `release:publish` を実行し、push と GitHub Release 作成を行う
4. 公開 asset を `mise use -g github:yano3nora/coda@<version>` で smoke test

Agent は 1 の検証までしか行わず、commit / tag / push / publish は行わない。実装状況と release gate は [TASK-260712-19](docs/TASK-260712-19-v0.1-release-readiness.md)を参照。

## Status

MVP 受け入れ基準(SPEC-0001)は全項目達成済み。v0.1 までに長行表示・小さな編集 gap・ファイル保護・配布再現性・Linux 検証を仕上げる。

- [v0.1 release readiness](docs/TASK-260712-19-v0.1-release-readiness.md)
- [v0.1 後の backlog](docs/TASK-999999-backlog.md)

macOS / Linux 対応(Windows は対象外)。動作確認は主に Ghostty / kitty 系 terminal。
