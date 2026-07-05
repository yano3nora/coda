# ADR-0007: Modifier Delivery Strategy (Cmd / per-terminal quirks)

- Status: Proposed
- Date: 2026-07-05

## Context

macOS の VS Code 筋肉記憶は Cmd 中心であり、Cmd が terminal に届くかどうかは製品価値に直結する。Ghostty 1.3.1 での実測(2026-07-05)で以下が判明した。

- `Cmd+S` は kitty keyboard protocol の super modifier として届く(CSI u, modifier bit 8)
- `Cmd+Shift+P` は届かない(Ghostty 自身の command palette `toggle_command_palette` が消費)
- `Cmd+Shift+J` も届かない(Ghostty の `write_screen_file:paste` が消費し、一時ファイルのパスが貼り付けられる)
- `Ctrl+Shift+J` は正しく区別されて届く
- `Shift+<文字>` は修飾情報が文字に畳み込まれて届く(protocol の仕様どおり。損失ではない)

`ghostty +list-keybinds` で予約キー一覧を照会した結果、**Ghostty は binding されていない super combo を透過し、binding 済みのものだけを消費する**(per-key 予約であり、Cmd+Shift 帯全体の問題ではない)と確定した。default 予約には VS Code 筋肉記憶の主要キーが含まれる点に注意:

- `super+c` / `super+v`(copy / paste — 意味的には整合するが editor には届かない)
- `super+z` / `super+shift+z`(undo / redo)
- `super+a`(select all)
- `super+k`(clear screen)、`super+q`(quit)、`super+1..9`(tab 切替)

ここから得られた設計上の重要な事実:

> **「届かない組み合わせ」は protocol 照会では検出できない。** terminal は予約キーを申告せず、ただ送ってこないだけである。よって capability 検出の自動化には原理的な限界がある。

また、副次的な原則も得られた:

> **GUI アプリで有名なキーほど、terminal を内包する GUI shell に消費されやすい。**`Ctrl+Shift+P` / `Cmd+Shift+P` は Ghostty・VS Code integrated terminal 自身の palette キーであり、TUI アプリの default にしてはならない(SPEC-0002 の palette キーを `Ctrl+Space` に変更した根拠)。

## Decision

### 1. super(Cmd / Win)は first-class modifier とする

normalized key event は ctrl / alt / shift / super を保持する(実装済み。SPEC-0003)。

### 2. Deliverability の判定は三層で行う

| 層 | 手段 | 分かること |
| --- | --- | --- |
| (a) 自動 | protocol negotiation(CSI ?u 照会) | protocol 対応の有無、有効 flags |
| (b) 知識 | 既知 terminal の quirk 情報(`TERM_PROGRAM` ベース) | 予約キーの警告(例: Ghostty の `Cmd+Shift+P`) |
| (c) 実測 | `keymap verify`(対話的検証) | **個々の chord が実際に届くか(真実はここ)** |

- (b) は警告のみに使い、小さく保つ(quirk DB の網羅を目指さない)。ただし Ghostty は `ghostty +list-keybinds` で**ユーザーの実設定を含む予約キー一覧を CLI 照会できる**ため、静的 DB ではなく実行時照会で正確な quirk 情報を得られる(照会可能な terminal はこの方式を優先する)
- (c) は import した binding の chord を利用者に実際に押してもらい、届いたかを記録する。inspect-key の仕組みを binding 検証に転用する

### 3. Import に cmd 戦略オプションを設ける

```sh
<app> keymap import vscode <path> --cmd=keep|ctrl|both
```

- `keep`(default): `cmd+s` をそのまま super binding として取り込む。verify を推奨する
- `ctrl`: `cmd+*` を `ctrl+*` に変換して取り込む。VS Code 自身が公式に持つ macOS / Windows keymap 対応に基づく変換とし、変換後の衝突は conflict として report する
- `both`: 両方登録する(衝突は report)

super が届かない terminal では report で `ctrl` 変換を提案する。

### 4. 原理的に取り返せないキーは最初から「移植不能」枠

`Cmd+Q`(アプリ終了)、`Cmd+Tab`(OS)等は capability に関わらず `Unsupported: OS/terminal reserved` として report する。

### 5. Terminal 側設定の生成支援(将来)

Ghostty の `keybind` 等、予約キーを明け渡す設定 snippet の生成は deferred(SPEC-0001)。MVP では report と手順ドキュメントで案内する。

## Alternatives Considered

- **quirk DB を主軸にする**: terminal × バージョン × 設定の組み合わせは網羅不能で、保守が破綻する。DB は警告用の補助に留め、実測(verify)を真実とする。
- **cmd を常に ctrl へ変換する**: Ghostty 実測で super が届くと分かった以上、届く環境で筋肉記憶を捨てさせる理由がない。戦略はユーザー選択にする。
- **verify を作らず「押しても反応しない」に任せる**: 「キーが黙って効かない」は本製品が最も否定する体験(ADR-0001 Explicit incompatibility)。不採用。

## Consequences

### 良くなること

- 「あなたの環境でこの binding は届かない」を、推測ではなく実測で言える
- モダン terminal ユーザー(主要ターゲット)は Cmd 筋肉記憶をほぼ持ち込める
- 非対応環境にも `--cmd=ctrl` という確立された退路がある

### リスク・コスト

- `keymap verify` という新しい対話フローの実装・UX コスト
- verify 結果の保存(chord 単位の deliverability)という状態が増える
- quirk 情報は少数でも陳腐化する(バージョン明記で緩和)

## Migration Notes

SPEC-0003(deliverability の非可査性)、SPEC-0004(--cmd オプション、reserved 分類)、SPEC-0005(`keymap verify` コマンド)を本 ADR に合わせて更新する。

## Open Questions

- ~~Ghostty で Cmd+Shift 全般が届かないのか、予約キー個別問題か~~ → 解決(2026-07-05): per-key 予約。未 binding の super combo は透過される
- verify 結果の保存形式・場所(`~/.config/<app>/` 配下)と、terminal が変わったときの無効化条件(`TERM_PROGRAM` + version をキーにする等)
- verify を import フローに組み込むか(import 直後に「5 個の binding が未検証です。今すぐ verify しますか」)
- `super+c/v/z/a` 等、Ghostty default に消費される主要キーの案内方針: Ghostty 側の keybind 解除を案内するか、これらに限り `--cmd=ctrl` 的な部分変換を提案するか

## Progress

- 2026-07-05: Ghostty 1.3.1 実測に基づき初版作成(Proposed)。super modifier の decode 保持は実装済み。
