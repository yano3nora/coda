# Ghostty + coda: passing Cmd keys through (macOS)

A worked example for delivering `Cmd`-modified keys to coda inside [Ghostty](https://ghostty.org). Verified on Ghostty 1.2+ / macOS (2026-07).

## Why this is needed

Ghostty (like every macOS terminal) handles `Cmd` keys in the AppKit layer before they can reach a terminal program. Three different things can eat a chord:

1. **Ghostty's own keybinds** — e.g. `super+z=undo` (undo closed tab), `super+arrow_up=jump_to_prompt`.
2. **macOS menu shortcuts** — e.g. `Cmd+H` = Hide, `Cmd+M` = Minimize. These exist even when Ghostty binds nothing.
3. **Byte translations** — e.g. `super+arrow_left=text:\x01` rewrites `Cmd+←` into `Ctrl+A` before delivery.

coda negotiates the [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/), so once a chord *reaches* the pty, `Cmd` arrives as a real `super` modifier. The whole game is stopping Ghostty/macOS from consuming it first.

## The decision rule

| The chord is… | Fix | Why |
| --- | --- | --- |
| Bound by Ghostty, **no** macOS menu reservation (`Cmd+Z`, `Cmd+↑`, …) | `keybind = <trigger>=unbind` | Once unbound the key falls through and Ghostty encodes it via the kitty protocol ([ghostty#9868](https://github.com/ghostty-org/ghostty/discussions/9868)). |
| Reserved by a macOS menu item (`Cmd+H` Hide, `Cmd+M` Minimize) | `keybind = <trigger>=text:<kitty bytes>` | `unbind` lets the OS menu take the key back, and `ignore` consumes it in the AppKit layer even with the `unconsumed:` prefix ([ghostty#7339](https://github.com/ghostty-org/ghostty/discussions/7339), [#8181](https://github.com/ghostty-org/ghostty/discussions/8181)). Sending the kitty-protocol encoding yourself is the only reliable path. |
| Copy (`Cmd+C`) | `keybind = super+c=performable:copy_to_clipboard` | Fires only while the terminal has a selection; otherwise passes through to coda. |
| `Cmd+Q`, `Cmd+Tab` | give up | Reserved by Ghostty/macOS at a level you cannot reclaim. coda classifies these as non-portable on import. |

## Example config

`~/.config/ghostty/config` (or `~/Library/Application Support/com.mitchellh.ghostty/config`):

```ini
# --- undo / redo -> coda (tradeoff: lose Ghostty's "undo close tab") ---
keybind = super+z=unbind
keybind = super+shift+z=unbind

# --- Cmd+Up/Down = buffer top/bottom in coda (tradeoff: lose jump_to_prompt) ---
keybind = super+arrow_up=unbind
keybind = super+arrow_down=unbind
keybind = super+shift+arrow_up=unbind
keybind = super+shift+arrow_down=unbind

# --- Cmd+H = cursor left (macOS reserves Cmd+H for Hide, so send the
#     kitty encoding explicitly: 104 = 'h', modifier 9 = 1 + super) ---
keybind = super+h=text:\x1b[104;9u

# --- optional: real Cmd+Left/Right and Cmd+A/C as super chords ---
# Not strictly needed: coda's default keymap already follows the macOS
# text-editing conventions Ghostty translates into (Ctrl+A/E etc.),
# so Cmd+Left/Right work with zero config. Unbind only if you want the
# raw super chords (e.g. your imported keymap rebinds them).
#keybind = super+arrow_left=unbind
#keybind = super+arrow_right=unbind
#keybind = super+a=unbind
keybind = super+c=performable:copy_to_clipboard
```

After editing, **restart Ghostty** — `reload_config` alone can leave stale menu key equivalents behind.

## Tradeoffs to accept

- The config is terminal-wide: `Cmd+Z` etc. stop doing anything in your shell and other TUI apps (unless they also speak the kitty protocol).
- The `text:` line for `Cmd+H` sends its bytes unconditionally, so pressing `Cmd+H` in a program that does not parse kitty escapes (plain zsh) inserts garbage. Harmless, but visible.
- Unbinding `super+arrow_left/right` (optional block) also removes the line-start/line-end jump in zsh that Ghostty's translation provided.

## Verify

```sh
coda keymap verify   # press each imported chord, see Delivered / Mismatch
coda inspect-key     # watch the raw bytes / decoded key for one keypress
```

Inside the editor, `F1` → `inspector.open` shows the same diagnosis live, including which chords Ghostty is still intercepting.
