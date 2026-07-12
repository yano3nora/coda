# ADR-0008: Clipboard and Mouse Strategy (terminal delegation boundary)

- Status: Accepted
- Date: 2026-07-05

## Context

ADR-0007 の実測で、Ghostty は `super+c/v/z/a` を default で消費すると判明した。これらを「editor に届ける」方向で戦うか、「terminal に任せる」方向で設計するかを決める必要がある。また、GUI editor 出身の対象ユーザーにとってマウス操作(クリックでカーソル移動・ドラッグで選択)は期待値であり、scope の再判断が必要になった。

## Decision

### 1. 委譲境界の原則

> **screen / clipboard レベルに意味がある操作は terminal に委譲する。buffer / selection の状態を必要とする操作は editor 内部で実装する。**

| 操作 | 方針 | 理由 |
| --- | --- | --- |
| paste(`Cmd+V`) | **terminal に委譲** | terminal の paste は bracketed paste としてアプリに届く。意味が完全に一致する |
| copy(マウス選択) | **terminal に委譲** | terminal 画面上の選択コピーはそのまま成立する(Shift+ドラッグ経路。下記 3) |
| copy(editor 内 selection) | **editor 内部 + OSC 52** | editor の selection は terminal から見えない。OSC 52 で terminal 経由の clipboard 書込を行う(SSH 越しでも動作) |
| select all(`Cmd+A`) | **editor 内部のみ** | terminal の select_all は screen/scrollback 選択であり、buffer 選択と意味が異なる |
| undo / redo(`Cmd+Z`) | **editor 内部のみ** | terminal の undo は tab/split 操作の取り消しであり、buffer 編集と無関係 |

- editor 内部操作(select all / undo / copy)には配達可能なキーが必要。super が消費される環境では import report で代替(`--cmd=ctrl` 部分変換、または terminal 側の keybind 解除)を案内する(ADR-0007)

### 2. Clipboard 実装

- **書込(copy / cut)**: OSC 52 を第一とする。terminal が拒否(許可制含む)する場合は内部 clipboard に fallback し、その旨を status bar で明示する
- **読出(paste)**: bracketed paste の受信を第一とする(paste 内容を key 入力として解釈しない。SPEC-0003)。OSC 52 read は多くの terminal で制限されるため MVP では使わない
- 内部 clipboard は常に保持する(OSC 52 の成否に関わらず editor 内 copy/paste は成立させる)

### 3. Mouse support を scope に入れる(MVP 後半または v0.2)

- SGR mouse protocol(DECSET 1000/1002/1006)で click / drag / wheel を受信する
- click = カーソル移動、drag = selection、wheel = スクロール
- mouse reporting 有効中は terminal ネイティブのマウス選択が奪われるため、**Shift+ドラッグをアプリへ送らず terminal 側選択に使う**慣習に乗る。Shift付きSGR eventが届いた場合は app 側で無視するが、受信済みbytesを terminal 選択へ戻すことはできない(help / ドキュメントに明記)
- split view と同じ優先度帯とし、keybinding engine より先行させない

## Alternatives Considered

- **`super+c/v/z/a` を Ghostty の keybind 解除で全部取り返す**: ユーザーに terminal 側の設定変更を強いる範囲が広すぎ、Ghostty 以外では再現しない。委譲で済む paste/copy まで戦うのは過剰。
- **OSC 52 を使わず OS clipboard コマンド(pbcopy 等)を叩く**: SSH 先で動かない(本製品の主戦場で死ぬ)。local 専用 fallback としてなら将来検討可。
- **mouse support を deferred のまま維持**: 対象ユーザー(GUI editor 派)の期待値であり、「terminal での短時間編集を改善するか」基準を満たすと判断して scope に入れる。

## Consequences

### 良くなること

- `Cmd+V` / マウス選択コピーが「設定なしで期待どおり動く」体験になる
- OSC 52 により SSH 先でも editor 内 copy が OS clipboard に届く
- 「terminal と奪い合わない」方針が palette キー(SPEC-0002)と一貫する

### リスク・コスト

- OSC 52 は terminal の許可設定(Ghostty `clipboard-write` 等)に依存し、初回に確認 prompt が出る場合がある
- mouse protocol の decode(SGR sequence)が input decoder に追加される
- 「アプリ内ドラッグ選択」と「Shift+ドラッグ terminal 選択」の使い分けはユーザー学習が必要

## Migration Notes

SPEC-0001(mouse を deferred から v0.2 帯へ、clipboard 挙動の明記)、SPEC-0003(SGR mouse decode、Shift+ドラッグの terminal override 依存)を本 ADR に合わせて更新する。

## Open Questions

- wheel スクロールの単位(行数)と加速の扱い
- ダブルクリック(単語選択)/ トリプルクリック(行選択)を v0.2 に含めるか
- OSC 52 拒否時の fallback として pbcopy / xclip / wl-copy を検出するか(local 実行時のみ)

## Progress

- 2026-07-05: 初版作成(Proposed)。ADR-0007 の Ghostty 実測を受けて委譲境界を定義。
- 2026-07-12: 決定 3 の SGR mouse support (DECSET 1002/1006、click / drag / wheel、
  Shift+drag の terminal override 依存) を実装し Accepted へ
  ([TASK](TASK-260712-mouse-verify-inactive-ssh.md))。wheel 単位は 3 行固定・加速なし
  (Open Question の回答)。double / triple click と OSC 52 拒否時の local fallback は
  引き続き Open。wheel scroll は cursor 非追従の free scroll とし、次の keystroke で
  viewport が cursor に再追従する。
