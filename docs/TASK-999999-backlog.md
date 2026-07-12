# TASK-999999: Backlog

日付に依存しない deferred task の索引。v0.1 までの作業と release gate は
[TASK-260712-19](TASK-260712-19-v0.1-release-readiness.md)を正とする。

## P1: v0.1 後に早めに欲しい

- [ ] `:which-key`: 入力中の sequence prefix に続く候補一覧を表示する
- [ ] `config.toml` の残項目結線: `sequence_timeout_ms` / `palette_key` / `capability_warning`
- [ ] import の `--cmd=keep|ctrl|both`: super が届かない terminal の退路

## P2: v0.2 候補

- [ ] split views: vertical / horizontal、pane focus、maximize
- [ ] mouse support: SGR protocol、click / drag / wheel、Shift+drag の terminal 素通し
- [ ] keymap verify: binding の deliverability を対話的に実測する
- [ ] `suggestVisible` / `quickOpenVisible` を使う imported binding の inactive 表示を明確化する

## Deferred: 着手前に再判断

- [ ] 他 editor profile import（Zed / Sublime / JetBrains / Helix）
- [ ] tree-sitter への highlighting engine 差し替え
- [ ] user theme / recent files / fuzzy file open / read-only mode / diff mode
- [ ] OSC 52 拒否環境の local fallback（pbcopy / xclip / wl-copy）
- [ ] SSH bootstrap script
- [ ] Homebrew / crates.io / mise registry への配布拡大
- [ ] GitHub Actions による tag 起点の自動 publish

## 運用

- 着手時に日付付き TASK を作り、この一覧から詳細を移す
- 同日に複数 TASK がある場合だけ `TASK-YYMMDD-NN-topic.md` とする
- 機能追加前に「terminal での短時間編集を改善するか」「keymap import より優先か」を確認する
