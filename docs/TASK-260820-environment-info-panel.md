# TASK-260820: 起動時通知の再設計 — environment info panel と F1 案内

260820 environment info panel
===

## asis

TASK-260820-startup-warning-panel で設定破損系は panel 化したが、起動時 status bar
には依然 `Ghostty intercepts 11 bindings: …` の 1 行 warning が毎回出る。dogfood
所感:

- 1 行は「長い・読めない・なんだかわからん」で情報として機能していない
- 毎起動同じ内容を言い続けるため、既知の定常状態がノイズ化している
- 一方で「ユーザーが使えると思っている keymap (例: palette に見える `cmd+t` →
  `buffer.new`) が terminal の都合で届かない」こと自体は keymap-first の核心情報。
  fix レシピ (`suggest_ghostty_fix` が生成済み) は inspector でしか見えない
- Terminal.app 等 `+list-keybinds` 相当の照会手段がない terminal では、そもそも
  検知できないことをユーザーに伝えていない

## tobe

境界は「設定ファイル起因か環境起因か」ではなく **「ユーザーがアクションを取れるか」**。
terminal 設定の見直しはアクションなので、intercept 情報は panel に昇格する。
ただし「terminal 設定を直さない」選択も正当なので acknowledge で沈黙できること。

1. **起動時 status bar は F1 案内 + file notice のみ**:
   `F1: command palette` を先頭に、`new file` / `mixed line endings` 等の短い
   per-file notice を続ける。intercept の 1 行 warning は廃止
2. **panel (旧 warnings panel) を environment info panel に拡張**:
   - 設定破損系 warning (従来どおり。存在する限り毎起動表示 = 直すまで nag してよい)
   - Ghostty intercept 詳細: `binding (action) — fix: <config 行>` を全件。
     fix がないものは reserved と明示。「あとこれだけ設定変更したらスムーズ」を
     1 画面で分からせる
   - 照会不能 terminal (TERM_PROGRAM != ghostty): 「この terminal の keybinding は
     照会できない。cmd 系 binding は黙って横取りされ得る。実測は
     `coda keymap verify`。modern terminal emulator (kitty protocol) 前提」と
     正直に 1 回伝える — 検知できないものを検知できるかのような UI にしない
3. **acknowledge**: 環境系 lines を terminal identity (TERM_PROGRAM + version) を
   key に state file へ保存。**内容が前回 acknowledge と同一なら panel を開かない**
   (設定破損系だけなら従来どおり開く)。dismiss = acknowledge 書き込み。
   Ghostty 更新・config 変更・別 terminal では内容が変わるので再表示される
4. **命名変更**: warning 以外も載るため `warnings.show` → `info.show`
   (`EditorAction::InfoShow`)。module も `warnings.rs` → `info.rs`。
   `info.show` は acknowledge 済みでも常に全文を再表示する

palette / which-key 上で届かない binding を dim + 注記する案は scope 外
(BACKLOG P2 に積む。露出すべき瞬間は palette で見た瞬間、が本丸のため段階実装)。

## todo

- [x] `warnings.rs` → `info.rs` rename (`InfoOverlay` / `draw_info` /
  `EditorAction::InfoShow` = `info.show`)
- [x] overlay を config 節 + environment 節の 2 節構成にし、起動時表示判定を
  「config 節あり || environment 節が未 acknowledge」にする
- [x] `ghostty_intercept_warning` (1 行要約) を廃止し、fix レシピ付きの複数行
  report に置き換え (`suggest_ghostty_fix` を集約)
- [x] 照会不能 terminal の notice 生成 (TERM_PROGRAM ベース)
- [x] acknowledge state の load/save (`environment-info.json`、terminal identity
  key の map。破損時は警告して無視 = 必ず起動する)
- [x] 起動時 message を `F1: command palette` + file notices に変更
- [x] BACKLOG に palette 側 dim + 注記の項を追加

## testcases

- [x] unit: intercept report が fix 行つきで全件列挙される (fixture quirks +
  macOS resolver。same-action 抑制・paste 委譲除外は従来挙動を維持)
- [x] unit: 照会不能 terminal の notice 文面
- [x] unit: acknowledge round-trip (同一内容 → panel 非表示 / 内容変化 → 表示 /
  state 破損 → 警告して表示)
- [x] unit: dismiss で acknowledge が保存される / `info.show` は acknowledge 後も
  全文表示する
- [x] unit: 起動時 message が F1 案内 + notices になる
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` (320 tests)

## notes

- 前提 dogfood 議論 (260820): 「ユーザーが割り当てた or OS 標準で使えると思って
  いる操作が terminal の都合で使えない → terminal or keybinding 設定を見直そうと
  思える情報が欲しい」。intercept 情報の価値は肯定しつつ、発火条件 (変化時のみ)
  と表現 (fix レシピ付き panel) を直すのが本タスク
- **例外: legacy capability warning は status bar のまま**。capability probe は起動
  ~500ms 後に非同期解決するため、起動時 surface である panel には載せられない
  (codex round 1 指摘への回答。将来 panel へ非同期挿入する場合は再設計)
- codex round 1 で P1 2 件を検出・修正:
  1. `quirks::detect()` が「照会失敗」と「照会成功・intercept なし」を区別せず、
     Ghostty で query が失敗すると環境クリーン扱いになる → 戻り値を
     `Option<Vec<_>>` にし、失敗時は「interception status unknown」notice を表示
  2. panel の行数上限を超えた未表示行まで dismiss で acknowledge される →
     ↑/↓/PageUp/PageDown での scroll を実装し、全行が読める状態にしてから
     acknowledge する設計に変更 (scroll キーは dismiss しない)
- codex round 1 P2: acknowledge 保存失敗が黙殺される → `save_environment_ack` を
  `io::Result` にし、失敗時は status bar に「再表示されるかも」と明示 (起動・編集
  は妨げない)
