# TASK-260813: Cmd cut / paste の default binding と clipboard command import

## asis

- `cmd+c` は default binding にあるが `cmd+x` / `cmd+v` がない
    - TASK-260713 で「Cmd copy / undo / redo / save」だけを cmd 化した際の漏れ
    - paste は Ghostty の `super+v` (bracketed paste) が拾うため気づかず、cut だけ「壊れている」ように見えていた
    - 未解決 chord は text input に落ちるが super 付き文字は挿入拒否されるため、無反応 (silent no-op) になる
- vscode-importer の command 変換表に clipboard 系がなく、`editor.action.clipboardCutAction` 等が Unsupported になる
    - import 経由でも `cmd+x` を供給できない

## tobe

- `cmd+x` = `edit.cut`、`cmd+v` = `edit.paste` を default binding (COMMON / textInputFocus) に追加する
- vscode-importer が `editor.action.clipboardCopyAction / clipboardCutAction / clipboardPasteAction` を
  `edit.copy / edit.cut / edit.paste` へ変換する (SPEC-0004 の変換表も更新)
- Ghostty では `super+v` の default bind が先に勝ち bracketed paste になる (それで正常動作) ことを docs に補足する

## todo

- [x] `default_bindings.rs` に `cmd+x` / `cmd+v` を追加
- [x] `vscode_commands.rs` に clipboard command 変換を追加
- [x] SPEC-0004 の Editing 変換表を更新
- [x] `docs/examples/ghostty.md` に `Cmd+V` / `Cmd+X` の挙動を補足
- [x] codex review 反映: `paste_from_clipboard` を正常委譲として扱う
    - `ghostty_intercept_warning` は「Consumed = paste_from_clipboard かつ trigger が `edit.paste`」を警告しない
    - `suggest_ghostty_fix` は paste_from_clipboard に `unbind` を提案しない (提案なし)
    - ADR-0008 に「terminal 消費が正常系 / 届いたら内部 clipboard fallback」を追記
- [x] fmt / clippy / test

## testcases

- [x] `cmd+x` / `cmd+v` (および `cmd+c`) が MacOs / Other 両 platform で resolver 経由で解決される
- [x] clipboard 系 VS Code command 3 種が対応する internal action に変換される
- [x] clipboard 系 3 command の import が `Imported` として binding を生成する
- [x] Ghostty fixture の `super+v=paste_from_clipboard` が intercept 警告に含まれない
- [x] `suggest_ghostty_fix` が paste_from_clipboard に対して提案を返さない

## notes

- `cmd+v` は Ghostty default (`super+v=paste_from_clipboard`) 環境では terminal 側が消費して
  bracketed paste として届くため、この binding が実際に発火するのは `super+v` を unbind した場合のみ。
  その場合は coda 内部 clipboard からの paste になる (OS clipboard は読めない。ADR-0008)
- `super+x` は Ghostty が default で bind しておらず macOS menu 予約もないため、追加設定なしで pty に届く
