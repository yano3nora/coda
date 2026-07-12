# TASK-260711-17: 2nd dogfood — Ghostty による key 横取りへの対応と「届かない key」の明文化

260711 dogfood 2: ghostty key interception & explainability
===

## asis

2 回目の dogfood(Ghostty 1.3.1 / macOS、Ghostty はユーザー config なしの default keybind)で以下のフィードバックを得た。

1. `Cmd+←` で全選択になる(期待: 行頭移動)
2. `Cmd+→` が無反応(期待: 行末移動)
3. `Option+←` / `Option+→` で単語移動できない
4. `Cmd+C` でコピーできない(lazyvim では「できる」ように見える)
5. **できないことが何なのか明文化されず、デバッグしづらい**

### 根因(`ghostty +list-keybinds` 実測で確定)

coda の default binding(`cmd+left` → `cursor.lineStart` 等)と decoder(super modifier / CSI u / legacy CSI)は正しい。**問題は全て Ghostty の default keybind が key を app に届く前に変換・消費していること**。

| 打鍵 | Ghostty default | coda に届くもの | 症状 |
| --- | --- | --- | --- |
| `Cmd+←` | `super+arrow_left=text:\x01` | `Ctrl+A` | `selection.all` 発火 = 「全選択になる」 |
| `Cmd+→` | `super+arrow_right=text:\x05` | `Ctrl+E` | 未 bind = 無反応 |
| `Opt+←` | `alt+arrow_left=esc:b` | `Alt+B` | 未 bind = 無反応 |
| `Opt+→` | `alt+arrow_right=esc:f` | `Alt+F` | 未 bind = 無反応 |
| `Cmd+C` | `super+c=copy_to_clipboard:mixed` | (届かない) | terminal の選択コピーが動くだけ |

- lazyvim で「できる」のは vim/readline が `\x01` / `ESC b` に偶然意味を持つため(同じ変換を受けている)。terminal が cmd/opt を届けているわけではない
- ADR-0007 が「binding 済み super combo は消費される」と記録済みだが、**`text:` / `esc:` による「別の key への変換」は消費より悪質**(誤動作として現れる)。今回それが実害として確認された

## tobe

- Ghostty default 環境でも opt 単語移動が動く(coda 側で吸収できるものは吸収)
- coda 側で吸収不能なもの(`\x01` と本物の `Ctrl+A` は区別不能)は、**起動時警告と inspect-key で「terminal が横取りしている」ことが利用者に見える**
- 推奨 terminal 設定が docs に明文化されている

## todo

- [x] **macOS text 編集慣行の default 採用(ADR-0011 として決定済み)**: Ghostty の変換先(`^A` / `^E` / `ESC b/f`)は macOS ネイティブ text field が標準サポートする emacs 系キーそのもの。coda の default をこれに合わせれば **terminal 設定ゼロで cmd+←/→・opt+←/→・ctrl+n/p が期待どおり動く**
    - `ctrl+a` → `cursor.lineStart`(現 `selection.all` から変更。**これが cmd+← 誤動作の真の根因**)
    - `ctrl+e` → `cursor.lineEnd` / `ctrl+n` → `cursor.down` / `ctrl+p` → `cursor.up`
    - `selection.all` の default は `cmd+a` へ(Ghostty default では消費されるため、palette / 推奨 config を導線に。ADR-0008 決定 1 と整合)
    - macOS の VS Code 筋肉記憶は `cmd+a` = select all なので `ctrl+a` 変更と衝突しない。Windows/Linux 向け default は従来どおり(platform 別 default)。user / imported 層はこれを上書きできる(source 優先度は不変)
- [x] **default binding 追加**: `alt+b` → `cursor.wordLeft` / `alt+f` → `cursor.wordRight`(+ shift 版で selection)。emacs 慣行と一致し、Ghostty の `esc:b/f` 変換がそのまま単語移動になる(フィードバック 3 を terminal 設定なしで解消)
- [ ] **Ghostty quirk 実行時照会**(ADR-0007 決定 2(b) の実装): `TERM_PROGRAM=ghostty` のとき `ghostty +list-keybinds` を parse し、有効 binding と衝突する `text:` / `esc:` 変換・super 消費を検出。起動時 warning(例: `cmd+left は Ghostty が Ctrl+A に変換します — 設定で unbind してください`)として表示
- [ ] **`:inspect-key` live mode**(backlog P1「inspect-key palette 統合」の前倒し・具体化): palette から起動し、次の打鍵について raw bytes / decoded KeyEvent / resolve 結果(+ quirk 照会に基づく横取り警告)を表示する。フィードバック 5 の直接回答
- [x] **docs: terminal setup guide**: Ghostty 向け推奨 config と、その shell 側 tradeoff(unbind すると zsh の行頭・行末ジャンプも失われる)を明記(README「Terminal setup(macOS)」節)

```
# Ghostty 推奨 config(coda 向け)
keybind = super+arrow_left=unbind
keybind = super+arrow_right=unbind
keybind = super+c=performable:copy_to_clipboard
```

## testcases

- [x] unit: `alt+b` / `alt+f` が default resolver で word motion に解決される(table-driven。macOS / Other 両 platform の default セットを `bindings_for(Platform)` で固定、計 124 tests green)
- [x] PTY E2E: Ghostty の変換 bytes そのもの(`\x01` / `\x05` / `ESC b`)で行頭・行末・単語移動が発火し、`\x01` が全選択にならないことを実測(`hello world` → `Xhello world` / `hello Zworld`)
- [ ] unit: `+list-keybinds` 出力サンプル(fixture)から `text:` / `esc:` / super 消費の衝突が分類される
- [ ] unit: inspect-key の表示文字列(raw bytes → decoded → resolved)の snapshot
- [ ] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`

## notes

- `Cmd+←` 問題は coda 側では原理的に修正不能(`\x01` と本物の `Ctrl+A` は区別できない)。よって「terminal 設定の案内 + 明文化」が正解であり、これは ADR-0007 の「届かない組み合わせは protocol 照会では検出できない」の系
- `super+c` の `performable:` 化は「terminal に選択があるときは terminal コピー、なければ app へ透過」となり両立できる。実測確認をタスク内で行うこと
- quirk 照会は Ghostty 専用の最適化であり、静的 quirk DB の網羅はしない(ADR-0007)
- 260711 実装記録: ADR-0011 起票 → default 層の platform 分離(`bindings_for(Platform)`)を Sonnet へ委任 → main agent レビュー(指摘なし。diff が小さく仕様を書き切れたため)→ fmt / clippy / test(124)/ PTY E2E green。**残 todo は quirk 実行時照会と inspect-key live mode**。commit は人間判断待ち
