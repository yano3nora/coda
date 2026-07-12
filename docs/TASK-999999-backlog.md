# TASK-999999: Backlog

日付に依存しない deferred task の索引。v0.1 までの作業と release gate は
[v0.1 release readiness](TASK-260712-v0.1-release-readiness.md)を正とする。

## P1: v0.1 後に早めに欲しい

- 完了 (260712) → [TASK-260712 which-key / config / --cmd](TASK-260712-which-key-config-cmd.md)

## P2: v0.2 候補

- ~~split views: vertical / horizontal、pane focus、maximize~~
  - split は多機能 editor のやることであり、現時点の coda の責務範囲ではない
- 完了 (260712) → [TASK-260712 mouse / verify / inactive / SSH bootstrap](TASK-260712-mouse-verify-inactive-ssh.md)
- 完了 (260713) → [TASK-260713 mouse / palette / go to line](TASK-260713-mouse-palette-goto-line.md)
- [ ] OS 予約キー (`Cmd+Q` / `Cmd+Tab` 等) の `Unsupported: OS/terminal reserved` 分類 (ADR-0007 §4。SPEC-0004 に仕様だけ先行)
- [ ] keymap verify 結果の永続化と resolver への反映 (`TERM_PROGRAM` + version キー。ADR-0007 Open Question)

## Deferred: 着手前に再判断

- [ ] 他 editor profile import（Zed / Sublime / JetBrains / Helix）
- [ ] tree-sitter への highlighting engine 差し替え
- [ ] user theme / recent files / fuzzy file open / read-only mode / diff mode
- [ ] OSC 52 拒否環境の local fallback（pbcopy / xclip / wl-copy）
- [ ] Homebrew / crates.io / mise registry への配布拡大
- [ ] GitHub Actions による CI（macOS + Linux での fmt / clippy / test。260712 に一度作成したが「複数 platform test を常時回す段階ではない」ため撤去。contributor が増えた時点で再判断）
- [ ] GitHub Actions による tag 起点の自動 publish

## 運用

- 着手時に日付付き TASK を作り、この一覧から詳細を移す
- 同日の TASK が複数あっても `TASK-YYMMDD-topic.md` の内容名で区別する
- 機能追加前に「terminal での短時間編集を改善するか」「keymap import より優先か」を確認する
