coda
===

**Keymap-first TUI text editor** — import the keybindings you have built up in a GUI editor (such as VS Code) and edit text in the terminal with the muscle memory you already have.

> Not a Vim alternative, nor a terminal port of VS Code. A plain text editor you can bring your own keymap to.

## Features

- **Imports VS Code `keybindings.json`**, with an import report that classifies each binding as imported / ignored / unsupported / conflict / disabled. `--cmd=keep|ctrl|both` selects how Cmd-key bindings are brought in
- Context-aware keybinding resolution (`Key + Context -> Action`, with `rescue > user > imported > default` precedence). While typing a key sequence, a which-key overlay shows the remaining candidates
- kitty keyboard protocol support (distinguishes `Ctrl+J` / `Ctrl+Shift+J` / `Cmd+S`), with a safe fallback for terminals that lack it
- Command palette (`F1` is a rescue entry point that always works, even with a broken config)
- Editing basics: undo/redo (with grouping), find/replace, multiple buffers/tabs, line numbers, grapheme-aware Unicode handling (CJK, emoji)
- Mouse support: click to move the cursor, drag to select, wheel to scroll (SGR). In most terminals **Shift+drag is left to the terminal's own selection** instead of being sent to the app. In terminals that do send Shift-modified SGR events, coda ignores them, but cannot hand them back to the terminal selection
- Syntax highlighting (syntect, dark/light themes, display-only)
- Clipboard: OSC 52 write (copy from an SSH session to your local OS clipboard) + bracketed paste
- Input self-diagnosis: `coda inspect-key` (raw input inspector) and `coda keymap verify` (interactively checks whether imported bindings actually reach the app)

### Supported platforms

macOS and Linux (Windows is not supported). Tested mainly on Ghostty and kitty-family terminals.

## Installation

### From GitHub Releases

Install a pinned version with the GitHub backend of [mise](https://mise.jdx.dev/):

```sh
mise use -g github:yano3nora/coda
```

Or manually from [Releases](https://github.com/yano3nora/coda/releases). Assets are named `coda-v<version>-<os>-<arch>.tar.gz` (a single `coda` binary at the archive root) with a separate `.sha256` each:

```sh
curl -LO https://github.com/yano3nora/coda/releases/download/v0.1.0/coda-v0.1.0-macos-arm64.tar.gz
tar -xzf coda-v0.1.0-macos-arm64.tar.gz   # put ./coda somewhere on your PATH
```

Available assets:

| asset | target |
| --- | --- |
| `macos-arm64` | macOS (Apple Silicon) |
| `macos-x64` | macOS (Intel) |
| `linux-arm64` | Linux aarch64 (glibc 2.17+) |
| `linux-x64` | Linux x86_64 (glibc 2.17+) |

Linux binaries are built against glibc 2.17 (zig linker). No musl static binary is provided, so glibc-based distros (Ubuntu / Debian / RHEL 7+ etc.) are the target.

### Bootstrap on SSH / container hosts

For minimal environments without `mise` or even `jq` — an SSH server, a Docker container — a POSIX sh script handles OS/arch detection, asset download, and checksum verification (works with `curl` or `wget`; dash / busybox sh compatible):

```sh
# Install the latest version into ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | sh

# Pin a version (with or without the "v" prefix)
curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | sh -s -- 0.1.0

# Change the install destination
curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | CODA_INSTALL_DIR=/usr/local/bin sh
```

After installing, the script runs `coda --version` as a smoke test and prints an `export` example if the destination is not on `$PATH`. See [`scripts/bootstrap.sh`](scripts/bootstrap.sh) for details.

### Build from source

```sh
# Requires a Rust toolchain (rustup or mise install)
cargo install --path .        # installs to ~/.cargo/bin/coda

# Or place it manually
cargo build --release         # put target/release/coda somewhere on your PATH
```

To use it on an SSH host, just copy a release binary built for the same OS / arch:

```sh
scp coda remote:~/bin/
```

## Usage

```sh
# Open files (multiple allowed)
coda file.ts other.md

# Import VS Code keybindings (re-running overwrites)
coda keymap import vscode "~/Library/Application Support/Code/User/keybindings.json" --print-report

# For environments where Cmd never reaches the terminal: import cmd+* as ctrl+*
coda keymap import vscode <path> --cmd=ctrl   # keep (default) | ctrl | both

# Press each imported binding to check it actually arrives (Esc: skip, Ctrl+C: abort)
coda keymap verify

# Inspect what your terminal actually sends
coda inspect-key
```

Inside the editor: `F1` / `Ctrl+Space` opens the command palette (incremental search over every action, bound keys shown alongside).

### Terminal setup (macOS)

**Zero configuration works** by design. The default keymap follows the host OS text-editing conventions, so even when the terminal translates keys like `Cmd+←` into `Ctrl+A`, things behave as expected (`Cmd+←/→` = line start/end, `Opt+←/→` = word movement, `Ctrl+N/P` = up/down).

If you additionally want the `Cmd` key delivered **as a real modifier** (`Cmd+C` copy, `Cmd+A` select all, etc.), you need to unbind the terminal's reserved keybindings:

```ini
# Ghostty (~/.config/ghostty/config)
keybind = super+arrow_left=unbind
keybind = super+arrow_right=unbind
keybind = super+a=unbind
keybind = super+c=performable:copy_to_clipboard   # pass through to coda only when there is no terminal selection
```

- Tradeoff: the shell (zsh etc.) also loses the `Cmd+←` line-start translation
- Use `coda inspect-key` to diagnose which keys actually arrive

### Configuration

```text
~/.config/coda/
  config.toml                # app settings (below)
  bindings.json              # user bindings (VS Code format + internal action names)
  generated/                 # import output (do not edit directly)
  import-reports/            # import / verify reports
```

```toml
# config.toml
[appearance]
theme = "dark"                # "dark" | "light"

[editor]
wrap = false                  # visual line wrap at startup (toggle with alt+z)

[keymap]
sequence_timeout_ms = 800     # wait time for key sequences
palette_key = "ctrl+space"    # convenience key for the palette (F1 always works)
ctrl_c = "copy"               # set to "quit" to exit with Ctrl+C (with unsaved-changes prompt)

[terminal]
capability_warning = true     # legacy terminal warning at startup
```

```jsonc
// bindings.json example (JSONC allowed)
[
  { "key": "ctrl+j", "command": "cursor.down", "when": "editorFocus" }
]
```

## Development

```sh
mise install          # rust toolchain
cargo run -- <file>   # run
cargo test            # unit tests
mise run pre-commit   # fmt --check / clippy -D warnings / test
```

- Design docs: `docs/ADR-*.md` (decisions), `docs/SPEC-*.md` (specs), `docs/TASK-*.md` (dev log)
- Conventions: [AGENTS.md](AGENTS.md) (dependency boundaries, testing policy, scope control)
- Entry-point docs: [ADR-0001 product direction](docs/ADR-0001-keymap-first-tui-editor.md) / [SPEC-0001 MVP scope](docs/SPEC-0001-mvp-scope.md) / [SPEC-0002 keybinding system](docs/SPEC-0002-keybinding-system.md)

## Deployment / Distribution

v0.1 ships as **single binaries for macOS / Linux on GitHub Releases** only. crates.io / Homebrew / automated publishing will be considered after manual releases stabilize the asset naming and install UX.

Releases use goreleaser (compile / archive / checksum / Release creation) plus `cargo xtask` (`xtask/`; version bump / validation / human-only publish gate). Cross builds use `cargo zigbuild` to produce all 4 targets (macOS / Linux × x64 / arm64) from a macOS host. `mise install` provides the toolchain (zig / cargo-zigbuild / goreleaser).

Manual release procedure:

```sh
# 1. Bump the version in Cargo.toml, run pre-commit (fmt / clippy / test) and toolchain
#    checks, then dry-run the entire goreleaser pipeline (snapshot mode, runnable before
#    tagging: generates all target assets and checksums into dist/ without pushing or
#    publishing anything).
mise run release:prepare -- 0.0.0

# 2. Review the diff, commit the version bump, and tag (human).
# git diff
# git add Cargo.toml Cargo.lock
# git commit -m "Release v0.0.0"
# git tag v0.0.0

# 3. Human-only publish. Validates version / clean tree / tag=HEAD, then pushes the
#    commit + tag; goreleaser rebuilds from the tagged commit and creates the GitHub
#    Release (release notes generated by GitHub, token reused from `gh auth token`).
mise run release:publish -- 0.0.0 --i-understand-this-pushes-and-publishes

# 4. Smoke-test installing the published asset.
mise use -g github:yano3nora/coda@0.0.0
```

Agents only go as far as the validation in step 1; commit / tag / push / publish are human-only (`release:publish` always fails without the confirmation flag). See [v0.1 release readiness](docs/TASK-260712-v0.1-release-readiness.md) for implementation status and release gates.
