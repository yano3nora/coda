# TASK-260711: 2nd dogfood — Ghostty による key 横取りへの対応と「届かない key」の明文化

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
- [x] **Ghostty quirk 実行時照会**(ADR-0007 決定 2(b) の実装): `TERM_PROGRAM=ghostty` のとき `ghostty +list-keybinds` を parse し、有効 binding と衝突する `text:` / `esc:` 変換・super 消費を検出。起動時 warning(実機で `Ghostty intercepts 11 bindings: Cmd+Shift+Enter, Cmd+A, … — run inspector.open for details`)として表示
    - 設計判断(260712):
        - module は `input/quirks.rs`(terminal 依存の隔離先)。`parse_ghostty_keybinds(output: &str)` を pure 関数にして fixture test、subprocess 実行と env 判定は薄い wrapper に分離。**照会失敗・timeout・parse 不能行は黙って skip し、起動を絶対に妨げない**(失敗モード原則)
        - quirk は `trigger: KeyEvent` + `effect: Consumed { action } | Translated { events, raw }` で表現。`text:` payload の `\xNN` は unescape して既存 decoder(`decode_key_events`)で normalize、`esc:X` は `ESC + X` として同様
        - **警告対象は super modifier を含む trigger と、`text:`/`esc:` 変換のみ**。shift/ctrl のみの bind(`shift+arrow_left=adjust_selection` 等)は performable 系(terminal 選択があるときだけ発火し普段は透過)が多く、警告すると偽陽性になるため対象外
        - **same-action 抑制**: Translated quirk は「変換先 key の resolve 結果」と「trigger に bind された action」が一致するなら警告しない(ADR-0011 により `cmd+left→^A→cursor.lineStart` は意図どおり動くため)。判定 context は `editor_focus + text_input_focus` の代表値でよい
        - 起動時 warning は 1 行に要約(件数 + 例示 3 件まで + `inspector.open で詳細`)
- [x] **`:inspect-key` live mode**(backlog P1「inspect-key palette 統合」の前倒し・具体化): palette から起動し、次の打鍵について raw bytes / decoded KeyEvent / resolve 結果(+ quirk 照会に基づく横取り警告)を表示する。フィードバック 5 の直接回答
    - 設計判断(260712):
        - `inspector.open`(action 定義済み・dispatch 未実装)を SearchOverlay パターンで実装。**観測専用**: overlay 表示中の key は編集・action 実行に流さない(デバッグ中の事故防止)。`Esc` で閉じる。`F1` の palette rescue は inspector より優先(常に効く原則)
        - 表示内容: (1) raw bytes(hex + escaped。1 read chunk 単位の近似でよい)、(2) decoded `KeyEvent`、(3) resolve 結果 = matched action + 一致した binding の source(`resolver.bindings()` を first-chord 一致で走査。Resolver 本体の API は変えない)、(4) quirk 注記 = 受信 event が既知の Translated 先と一致するとき「terminal が cmd+left から変換した可能性」を表示
        - bracketed paste は `paste (N bytes)` とだけ表示(内容は流さない)
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
- [x] unit: `+list-keybinds` 出力サンプル(fixture)から `text:` / `esc:` / super 消費の衝突が分類される(**fixture は xxd で実出力に忠実化**: `text:` payload は wire 上 `\\xNN` = backslash 2 個)
- [x] unit: inspect-key の表示文字列(raw bytes → decoded → resolved → quirk note)を固定
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`(148 tests)
- [x] PTY E2E(クリーン `XDG_CONFIG_HOME` + 実 Ghostty CLI): 起動時 warning(11 bindings 検出、`cmd+left` は same-action 抑制で非表示)/ palette → `inspector.open` → `\x01` 打鍵で `raw: 0x01` / `cursor.lineStart` / `note: Ghostty rewrites Cmd+Left to this key` の実表示を確認

## notes

- `Cmd+←` 問題は coda 側では原理的に修正不能(`\x01` と本物の `Ctrl+A` は区別できない)。よって「terminal 設定の案内 + 明文化」が正解であり、これは ADR-0007 の「届かない組み合わせは protocol 照会では検出できない」の系
- `super+c` の `performable:` 化は「terminal に選択があるときは terminal コピー、なければ app へ透過」となり両立できる。実測確認をタスク内で行うこと
- quirk 照会は Ghostty 専用の最適化であり、静的 quirk DB の網羅はしない(ADR-0007)
- 260711 実装記録: ADR-0011 起票 → default 層の platform 分離(`bindings_for(Platform)`)を Sonnet へ委任 → main agent レビュー(指摘なし。diff が小さく仕様を書き切れたため)→ fmt / clippy / test(124)/ PTY E2E green。**残 todo は quirk 実行時照会と inspect-key live mode**。commit は人間判断待ち
- 260712 実装記録(quirk 照会): Sonnet 委任 → main agent レビューで**実バグ 1 件検出・修正**: `text:` payload の `\xNN` unescape が str slice(`&payload[i+2..i+4]`)を使っており、`\x` 直後に multibyte 文字が来る user config で char boundary panic = 起動クラッシュ(「必ず起動する」原則違反)。byte 単位の hex 変換に修正し、再現テスト(`unescape_survives_multibyte_text_payload_without_panicking`)を追加。レビュー省略不可の実績がまた 1 件積まれた
- 260712 実装記録(inspector): Sonnet 委任 → コードレビュー指摘なし → **PTY E2E で統合バグ 1 件検出・修正**: 実 `+list-keybinds` の `text:` payload は wire 上 `\\xNN`(backslash 2 個。xxd で確認)なのに fixture・unescape とも 1 個想定で、実環境では全 Translated quirk が `[0x5c, byte]` に誤 decode → quirk note 不発 + same-action 抑制が外れ `cmd+left` が偽陽性警告。**指示書の fixture が実出力と食い違っていたのが根因**(fixture は必ず実 bytes から採取すること)。両形式(`\xNN` / `\\xNN`)対応 + fixture 忠実化で修正
- 教訓: unit test が全部 green でも fixture が現実と違えば無意味。subprocess 連携は PTY E2E まで通して初めて「動いた」と言える
- 起動時 warning は warnings の**先頭**に挿入する(user config の per-binding 警告が多数あると 1 行 status bar から押し出されるため。全文閲覧手段は backlog の既存課題)
