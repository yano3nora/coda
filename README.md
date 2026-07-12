coda
===

**Keymap-first TUI text editor** — GUI editor(VS Code 等)で育てた keybinding を import して、terminal の中でも自分の筋肉記憶のままテキストを編集するためのエディタ。

> Vim の代替でも VS Code の terminal 版でもない。「既存 keymap を持ち込める plain text editor」(詳細: [ADR-0001](docs/ADR-0001-keymap-first-tui-editor.md))

現在は **v0.1 release candidate 前**。MVP の中核は動作するが、配布 asset と Linux 実機検証は未完了。

## Features

- **VS Code `keybindings.json` の import** と、成功 / 無視 / 未対応 / 衝突 / 不活性を明示する import report。`--cmd=keep|ctrl|both` で Cmd キーの取り込み戦略を選べる
- `Key + Context -> Action` の context-aware keybinding 解決(`rescue > user > imported > default`)。sequence 入力中は which-key overlay が続きの候補を表示
- kitty keyboard protocol 対応(`Ctrl+J` / `Ctrl+Shift+J` / `Cmd+S` を区別)+ 非対応 terminal への安全な fallback
- command palette(`F1` は設定が壊れていても常に有効な rescue 入口)
- 編集基本機能: undo/redo(グルーピング)、find/replace、複数 buffer/tabs、行番号、grapheme 単位の Unicode 処理(日本語・絵文字)
- mouse support: click = カーソル移動、drag = 選択、wheel = スクロール(SGR)。**Shift+drag は terminal ネイティブ選択に素通し**するので、terminal のマウス選択コピーはそのまま使える
- syntax highlighting(syntect、dark/light、表示専用)
- clipboard: OSC 52 書込(SSH 先から手元の OS clipboard へ)+ bracketed paste
- 入力の自己診断: `coda inspect-key`(raw input inspector)と `coda keymap verify`(import した binding が実際に届くかの対話実測)

## Installation

### GitHub Release から (v0.1 以降)

[mise](https://mise.jdx.dev/) の GitHub backend で version 指定して導入する:

```sh
mise use -g github:yano3nora/coda@0.1.0
```

または [Releases](https://github.com/yano3nora/coda/releases) から手動で。asset は `coda-v<version>-<os>-<arch>.tar.gz`(展開直下に単体 binary `coda`)+ 個別の `.sha256`:

```sh
curl -LO https://github.com/yano3nora/coda/releases/download/v0.1.0/coda-v0.1.0-macos-arm64.tar.gz
tar -xzf coda-v0.1.0-macos-arm64.tar.gz   # ./coda を PATH の通った場所へ
```

対応 platform:

| asset | 対象 |
| --- | --- |
| `macos-arm64` | macOS (Apple Silicon) |
| `macos-x64` | macOS (Intel) |
| `linux-arm64` | Linux aarch64 (glibc 2.17+) |
| `linux-x64` | Linux x86_64 (glibc 2.17+) |

Linux binary は glibc 2.17 を baseline に build している(zig linker)。musl 静的 binary は未提供のため、glibc 系 distro(Ubuntu / Debian / RHEL 7+ 等)が対象。

### SSH / container 先への bootstrap

SSH 先のサーバーや Docker container など、`mise` はおろか `jq` すら無い最小環境向けに、OS/arch 判定・asset 取得・checksum 検証まで行う POSIX sh script を用意している(`curl` か `wget` があれば動く。dash / busybox sh 互換):

```sh
# 最新版を ~/.local/bin へ導入
curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | sh

# version を固定する場合 ("v" 付き/なしどちらも可)
curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | sh -s -- 0.1.0

# 導入先を変える場合
curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | CODA_INSTALL_DIR=/usr/local/bin sh
```

導入後 `coda --version` で疎通確認まで行い、`$PATH` に導入先が無ければ `export` 例を案内する。詳細は [`scripts/bootstrap.sh`](scripts/bootstrap.sh) を参照。

### ソースから build

```sh
# 要 Rust toolchain (rustup または mise install)
cargo install --path .        # ~/.cargo/bin/coda に入る

# または手動配置
cargo build --release         # target/release/coda を PATH の通った場所へ
```

SSH 先で使う場合は、同一 OS / arch 向けの release binary をコピーすればよい:

```sh
scp coda remote:~/bin/
```

## Usage

```sh
# ファイルを開く (複数可)
coda file.ts other.md

# VS Code keybindings を import (再実行で上書き)
coda keymap import vscode "~/Library/Application Support/Code/User/keybindings.json" --print-report

# Cmd が terminal に届かない環境向け: cmd+* を ctrl+* に変換して import
coda keymap import vscode <path> --cmd=ctrl   # keep (default) | ctrl | both

# import した binding が実際に届くかを 1 つずつ押して実測 (Esc: skip, Ctrl+C: 中断)
coda keymap verify

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
- terminal が key を消費・変換する理屈の詳細: [ADR-0007](docs/ADR-0007-modifier-delivery-strategy.md) / [TASK-260711-17](docs/TASK-260711-dogfood-2-ghostty-key-interception.md)

### 設定

```text
~/.config/coda/
  config.toml                # アプリ設定 (下記)
  bindings.json              # user binding (VS Code 形式 + 内部 action 名)
  generated/                 # import の出力 (直接編集しない)
  import-reports/            # import / verify report
```

```toml
# config.toml (SPEC-0005)
[appearance]
theme = "dark"                # "dark" | "light"

[editor]
wrap = false                  # 起動時の visual line wrap (alt+z で切替)

[keymap]
sequence_timeout_ms = 800     # key sequence の待機時間
palette_key = "ctrl+space"    # palette の便宜キー (F1 は常に有効)

[terminal]
capability_warning = true     # 起動時の legacy terminal 警告
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

release は goreleaser(compile / archive / checksum / Release 作成)+ `cargo xtask`(`xtask/`。version bump / 検証 / 人間 publish ゲート)の二段構成。cross build は `cargo zigbuild` を使い、macOS host から 4 target(macOS / Linux × x64 / arm64)を build する。toolchain は `mise install` で揃う(zig / cargo-zigbuild / goreleaser)。

手動 release 手順:

```sh
# 1. Cargo.toml の version bump・pre-commit (fmt / clippy / test)・toolchain 検査ののち、
#    goreleaser pipeline を丸ごとドライラン (tag を打つ前に実行できる snapshot mode。
#    全 target の asset と checksum を dist/ に生成し、push / publish は一切しない)。
mise run release:prepare -- 0.1.0

# 2. 差分を review し、version bump を commit して tag を打つ (人間)。
# git diff
# git add Cargo.toml Cargo.lock
# git commit -m "Release v0.1.0"
# git tag v0.1.0

# 3. 人間専用の publish。version / clean tree / tag=HEAD を検証してから commit + tag を
#    push し、goreleaser が tag 済み commit から rebuild して GitHub Release を作成する
#    (release notes は GitHub 生成、token は `gh auth token` を再利用)。
mise run release:publish -- 0.1.0 --i-understand-this-pushes-and-publishes

# 4. 公開 asset の導入 smoke test。
mise use -g github:yano3nora/coda@0.1.0
```

Agent は 1 の検証までしか行わず、commit / tag / push / publish は行わない(`release:publish` は確認 flag なしでは常に失敗する)。実装状況と release gate は [v0.1 release readiness](docs/TASK-260712-v0.1-release-readiness.md)を参照。

## Status

MVP 受け入れ基準(SPEC-0001)は全項目達成済み。v0.1 までに長行表示・小さな編集 gap・ファイル保護・配布再現性・Linux 検証を仕上げる。

- [v0.1 release readiness](docs/TASK-260712-v0.1-release-readiness.md)
- [v0.1 後の backlog](docs/TASK-999999-backlog.md)

macOS / Linux 対応(Windows は対象外)。動作確認は主に Ghostty / kitty 系 terminal。
