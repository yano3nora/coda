# TASK-260706-99: Backlog(今後のタスク一覧)

260706 backlog
===

## asis

- TASK-01〜15 完了。MVP 受け入れ基準(SPEC-0001)のうち Editor / Keymap / Scope Control は全項目達成、Terminal Compatibility は「区別不能な binding の明示」のみ未達
- 累計 121 unit tests。実装は全て fmt / clippy -D warnings / test グリーン

## tobo(優先度順)

### P0: MVP 完成に必要

- [ ] ざっと動かしての違和感、諦めるかどうかの判定基準
    - cmd, option 系操作をどのように解決するか
        - option + ←→ で単語移動ができてない、cmd だけ無理ならまだしも「 tui, cui では使えないキーが多数ある」では、製品としてちょっと成り立たない
            - 結局、それぞれの開発者の「オレオレ使いやすいエディタ」にしかならない、別にそっちを目指して個人用にしたっていいけど、だったら lazyvim の無限カスタム地獄に行ったほうがまだまし
        - cmd が届かない問題は認識してるが、何らか回避策がないか
    - ファイル指定なしで開く (buffer) 機能がほしい
    - コマンドパレットから、設定ファイルを開きたい
    - Shift + Tab でインデントを戻せない
    - ctrl + n, p によるカーソル上下移動効かない
    - **判定(260711 調査。詳細は `docs/TASK-260711-17-*.md`)**: 「使えないキーが多数」は誤認で、実態は 3 層 — (1) terminal keybind による消費(設定で解除可能)、(2) 別キーへの変換(`cmd+←`→`^A` 等。**変換先は macOS 標準の emacs 系キーなので coda の default を macOS 慣行に合わせれば設定ゼロで吸収できる**)、(3) legacy terminal の protocol 表現力不足(super 不達。`--cmd=ctrl` の退路設計済み = ADR-0007)。原理的に回避不能なのは (3) の環境の super のみ → **製品は成立する(Go)。ただし TASK-17 の macOS 慣行 default + 明文化を P0 扱いで先行する**
        - 各項目の行き先: cmd/option/ctrl+n,p → TASK-17 / ファイルなし起動・config open・Shift+Tab → TASK-19(`docs/TASK-260711-19-*.md`)

- [ ] **TASK-17: Ghostty key 横取り対応と「届かない key」の明文化**(2nd dogfood。`docs/TASK-260711-17-*.md`。macOS 慣行 default(要 ADR)・alt+b/f 追加・quirk 実行時照会・inspect-key live mode。P1「inspect-key palette 統合」を吸収。上記判定により P0 昇格)

- [ ] **TASK-16: keyboard capability 検出の結線**(SPEC-0003 / ADR-0003)
    - 起動時の `CSI ?u` 応答を parse して `KeyboardCapabilities` を確定(現状は push して応答を Unknown 扱いで捨てている)
    - fallback terminal での起動時 warning(`Ctrl+J と Ctrl+Shift+J を区別できません` 等)
    - importer へ capability を渡し `Disabled by terminal capability` を実分類化(現状は常に 0)
    - `inspect-key` に判定結果を表示
    - これで SPEC-0001 受け入れ基準が全て埋まる

### P1: 実用上早めに欲しい

- [ ] **TASK-18: wrap toggle と長行の視認性**(2nd dogfood。`docs/TASK-260711-18-*.md`)
- [ ] **TASK-19: 起動・編集の小改善**(2nd dogfood。`docs/TASK-260711-19-*.md`。ファイルなし起動・palette から config open・Tab/Shift+Tab indent)
- [ ] **Save As**(`file.saveAs`。palette からのパス入力 UI が必要 → 汎用の 1 行入力 prompt を作る)
- [ ] **外部変更検知**(SPEC-0001 File Operations: save 時に mtime 比較で警告。watch は不要)
- [ ] **large file protection の app 結線**(SPEC-0001 / ADR-0009: 10MB 超を read-only で開く + `isReadonly` context の実運用)
- [ ] **`:which-key` / `:inspect-key` の palette 統合**(SPEC-0002 Binding inspection。editor 内から binding の由来を確認する導線)
- [ ] **config.toml の残項目結線**(`sequence_timeout_ms` / `palette_key` / `capability_warning`。SPEC-0005 に定義済みで未結線)
- [ ] **import の `--cmd=keep|ctrl|both` オプション**(ADR-0007 決定 3。super が届かない環境の退路)

### P2: v0.2 スコープ(SPEC-0001 で予告済み)

- [ ] **split views**(vertical / horizontal、pane focus、maximize。ADR-0010 の撤退条件の試金石)
- [ ] **mouse support**(SGR protocol、click/drag/wheel、Shift+ドラッグ素通し。ADR-0008)
- [ ] **keymap verify**(ADR-0007 決定 2(c): 対話的 deliverability 実測。quirk 警告 = Ghostty は `+list-keybinds` の実行時照会)
- [ ] **suggest/quickOpen reserved context の扱い明示**(SPEC-0004 の `imported (inactive in MVP)` 表示)

### P3: Deferred(SPEC-0001 Open Questions 由来。着手前に要判断)

- [ ] 他 editor profile import(Zed / Sublime / JetBrains / Helix)— 2 つ目の importer で抽象の妥当性が試される
- [ ] tree-sitter への highlighting engine 差し替え検討(ADR-0006 撤退条件)
- [ ] ユーザー theme 追加(.tmTheme 配置)/ recent files / fuzzy file open / read-only mode / diff mode
- [ ] OSC 52 拒否環境の fallback(local 時のみ pbcopy / xclip / wl-copy 検出)
- [ ] SSH 向け bootstrap(単体 binary 配布、install script)

### リリース前に必ず(機能ではない)

- [ ] **製品名の決定**(`coda` は Coda.io / 旧 Panic Coda と衝突。ADR-0001 Open Questions)
- [ ] CI(GitHub Actions: fmt / clippy / test。macOS + Linux matrix)— push は人間判断のまま、CI 整備のみ
- [ ] 配布形態(cargo install / homebrew / GitHub Releases の単体 binary)と versioning 運用(AGENTS.md の TODO)
- [ ] Linux 実機での動作確認(開発は macOS のみで進行中。termios / ioctl は libc 経由なので動くはずだが未検証)

## testcases

- [ ] 各項目の着手時に個別の TASK-YYMMDD doc を起こす(この backlog は索引として維持し、完了時にチェックする)

## notes

- 優先度は 2026-07-06 時点の判断。P1 の並びは「SSH 先での短時間編集を改善するか」(ADR-0001 の評価基準)による
- **scope 警告**: P2 以降を進める際は都度「keybinding engine の完成度・import 体験より優先か」を問うこと。ADR-0001 が最大リスクと名指しした scope creep はここから始まる
- 開発フロー: 設計・TASK 化 → `codex exec` 実装 → main agent レビュー(毎回 1〜3 件の実バグを検出してきた実績があるためレビュー省略は不可)→ PTY E2E → commit
- codex 運用の知見: 依存 crate は事前に main agent が追加しておく(codex sandbox は crates.io 不達で偽 shim を作った前科がある。TASK-03 notes)
