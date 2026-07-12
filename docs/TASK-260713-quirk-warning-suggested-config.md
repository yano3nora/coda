# TASK-260713: quirk 警告に「推奨 Ghostty config 行」を機械生成して提示する

260713 quirk warning: suggested terminal config
===

## asis

- 起動時警告は `Ghostty intercepts N bindings: Cmd+Z, … — run inspector.open for details` の 1 行のみ (`event_loop.rs` の `ghostty_intercept_warning` / `format_intercept_warning`)
- inspector live mode は「terminal が横取りしている可能性」を注記するが、**どう直せばよいかは示さない**。README の Terminal setup 節に一般例があるだけで、キーごとの正解 (unbind か `text:` か) はユーザーが upstream discussion を掘らないと分からない
- 260713 の実測 (dogfood 3) で、修正内容が trigger の種類ごとに機械的に導出できることが確定した。診断材料 (quirk の trigger / effect) は既に `input/quirks.rs` が持っている

## tobe

- quirk ごとに「Ghostty config に足すべき 1 行」を機械生成し、`:inspect-key` の quirk 注記と `coda keymap verify` の Mismatch 結果に併記する
- 提案には「なぜその行か」の理由を 1 行添える (説明可能性の原則。ADR-0001 / SPEC-0002)
- 起動時警告は 1 行のまま変えない (導線は従来どおり inspector)

### 提案の決定則 (260713 実測で確定)

| trigger の分類 | 提案 | 理由 |
| --- | --- | --- |
| macOS システム予約のない super combo (`super+z`, `super+arrow_up` 等) | `keybind = <trigger>=unbind` | unbind すれば素通りし、kitty protocol でエンコードされて届く (ghostty#9868 の cmd+backspace と同パターン) |
| macOS メニュー予約あり (`super+h`=Hide, `super+alt+h`=Hide Others, `super+m`=Minimize) | `keybind = <trigger>=text:<KKP bytes>` (例: `super+h=text:\x1b[104;9u`) | unbind は OS メニュー定義が復活し、`ignore` は `unconsumed:` を付けても AppKit 層で消費され pty へ流れない (ghostty#7339 / #8181)。同義の KKP バイト列を明示送信するしかない |
| copy/paste 系 (`super+c` 等) | `performable:copy_to_clipboard` 案内 (README 既載) | terminal 選択があるときだけ terminal 側が動く両立解 |
| 移植不能 (`super+q`, `super+tab`) | 提案しない (P2 key delivery で reserved 分類済み) | OS/terminal を奪えない |
| Translated だが same-action (ADR-0011 ケース) | 提案しない (現行どおり警告自体を抑制) | 意図どおり動いている |

## todo

- [x] `input/quirks.rs` に pure 関数 `suggest_ghostty_fix(&TerminalQuirk) -> Option<Suggestion>` を追加 (`Suggestion { config_line, reason }`)
    - `KeyEvent` → Ghostty trigger 記法 (`super+shift+arrow_up` 等) の formatter (既存 `map_key_name` / `split_trigger` の逆写像) — `format_ghostty_trigger`
    - `KeyEvent` → KKP CSI u バイト列 (`\x1b[<codepoint>;<1+modifiers bitmask>u`) の formatter — `kitty_csi_u_bytes`。検証済みの macOS メニュー予約 table は char key (`h`/`m`) のみなので、functional key の legacy-with-modifier 形式は実装せず `None` を返す (関数doc に明記)
    - macOS メニュー予約 chord の静的 table (`super+h` / `super+alt+h` / `super+m`。検証済みのものだけ載せ、増えたらここに足す) — `MACOS_MENU_RESERVED`
- [x] `:inspect-key` の quirk 注記に `config_line` + `reason` を併記 (`app/inspector.rs`)
- [x] `coda keymap verify` の Mismatch 出力で、chord が検出済み quirk と一致する場合に同じ提案を併記 (`app/verify_cli.rs`)
- [x] 提案文言の末尾に「適用後は Ghostty を再起動して `coda keymap verify`」を含める (reload だけでは menu equivalent が残ることがある)
- [x] `docs/examples/ghostty.md` (260713 新設) と文言・決定則を一致させる (実装後に再確認: 追加修正不要、文言一致済み)
- [x] README の Terminal setup 節に「coda が fix 行を提示する」段落を追加 (出力例は**実装後の実出力**を貼る。想像上の出力を先に書かない)
- [x] fmt / clippy / test

## testcases

- [x] table-driven: `super+z=undo` (Consumed) → `unbind` 提案 / `super+h=ignore` (Consumed, メニュー予約) → `text:\x1b[104;9u` 提案 / `super+c=copy_to_clipboard:mixed` → `performable:copy_to_clipboard` 提案 / `super+q=quit` → None / Translated quirk → `unbind` 提案
    - 実装確定時の補足: `suggest_ghostty_fix` は `input/` の pure 関数で resolver を持たないため、「Translated だが same-action (ADR-0011 ケース)」の抑制は本関数の責務にしない — その抑制は従来どおり `event_loop.rs::ghostty_intercept_warning` (起動時警告の対象選定) 側で行われ、変更していない。`suggest_ghostty_fix` 自体は quirk 単体から機械的に決定則を適用するだけで、呼び出し側 (inspector / verify) が「表示するに値する quirk か」を選ぶ
- [x] formatter: `KeyEvent` → Ghostty trigger 記法と KKP バイト列 (modifier 9 = super, 10 = shift+super) の往復
- [x] 起動時警告の 1 行形式は既存 test のまま不変
- [x] verify の Mismatch report に提案行が含まれる (quirk 一致時のみ)

## notes

- 根拠 (260713 実測 + upstream):
    - `unconsumed:super+h=ignore` は hide を止めるが pty へ流れない (実測)。macOS では cmd 系が AppKit `performKeyEquivalent` 層で処理されるため ([ghostty#7339](https://github.com/ghostty-org/ghostty/discussions/7339) / [#8181](https://github.com/ghostty-org/ghostty/discussions/8181))
    - Ghostty keybind はメニューショートカットより優先 ([#3187](https://github.com/ghostty-org/ghostty/discussions/3187) → #4590 で修正済み、2025-01)
    - OS 予約のない super combo は unbind で KKP エンコードが届く ([#9868](https://github.com/ghostty-org/ghostty/discussions/9868))
- `text:` 方式は KKP 非対応 app (素の zsh 等) にゴミバイトが飛ぶ tradeoff がある。提案の `reason` にも明記する
- suggestion 生成は Ghostty 専用でよい (quirk 照会自体が `ghostty +list-keybinds` 依存)。kitty 等への一般化は scope 外 — 「terminal での短時間編集を改善するか」で評価して、必要になったら別 TASK
- 表示専用の機能であり resolver の挙動は変えない (verify による自動無効化は P2 で実装済みの範囲のまま)
