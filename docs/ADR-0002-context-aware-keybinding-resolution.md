# ADR-0002: Context-aware Keybinding Resolution

- Status: Proposed
- Date: 2026-07-05

## Context

単純な `Key -> Command` の map では VS Code 由来の keymap を再現できない。VS Code では同じ `Ctrl+j` が文脈によって異なる動作をする。

- editor: cursor down
- suggestion popup: next suggestion
- quick open: next item
- list: focus down
- terminal pane: next terminal

また、設定破損・import 失敗・terminal capability 不足時にも操作不能にならない仕組みが必要になる。

## Decision

### 1. Binding model

keybinding は `Key + Context predicate -> Action` として解決する。

```ts
type Binding = {
  key: KeyChord | KeySequence;
  action: EditorAction;
  when?: ContextPredicate;
  source?: "default" | "rescue" | "imported" | "user";
  priority?: number;
};
```

### 2. Resolution order

解決は「context filter → source 優先度」の 2 段階とする。

1. 現在の `EditorContext` に `when` が一致する binding だけを候補にする
2. 候補間の優先度は source で決める: `rescue > user > imported > default`
3. 同一 source 内では、より限定的な context(predicate の条件数が多い)を優先し、それでも同点なら後から定義されたものが勝つ

overlay(search / palette 等)表示中の binding が editor binding より優先されるのは、source 優先度ではなく context の限定性による(overlay 用 binding は `searchVisible` 等を predicate に持つため)。

### 3. Rescue = command palette 単一入口

user keymap が壊れても操作不能にならない保証は、複数の rescue ショートカットではなく **command palette を唯一の rescue 入口**とすることで実現する。

- palette open(`F1`。全 terminal で受信保証)は keymap resolver より前で処理し、常に有効とする
- save / quit / help を含む全 EditorAction は palette からインクリメンタルサーチで実行できる
- shortcut は「palette command への別経路」であり、palette 内に現在の binding を併記して発見・昇格の導線にする
- これにより `Ctrl+c` / `Ctrl+s` / `Ctrl+q` / `Ctrl+g` 等の一等地キーを rescue が占有せず、keymap-first の思想(筋肉記憶の持ち込み)と矛盾しない

具体仕様は SPEC-0002。

### 4. Key sequences

`Ctrl+x Ctrl+s` のような複数キー sequence をサポートする。timeout は設定可能(default 800ms)とし、待機中は status bar に候補を表示する。

## Alternatives Considered

- **単純な `Map<Key, Action>`**: VS Code の文脈依存 binding を import できず、overlay 操作と editor 操作が衝突する。不採用。
- **VS Code `when` clause engine の完全再現**: `when` の式言語・変数は膨大で、実装が終わらない。MVP は対応 predicate を限定し、未対応は import report で明示する方針とする(SPEC-0004)。
- **Modal editing(mode で context を代替)**: mode の習得コスト自体が本製品の否定する前提。不採用(ADR-0001 Non-goals)。
- **draft 案の「rescue > overlay > user > imported > default」の 5 層優先度**: overlay は binding の source ではなく context であり、source 優先度に混ぜると設定ファイル上の分類(SPEC-0005 の `rescue > user > generated > default`)と矛盾する。「context filter + source 優先度 + 限定性」に整理した。
- **draft 案の複数 rescue ショートカット(`Ctrl+c` / `Ctrl+s` / `Ctrl+q` / `Ctrl+g`)**: これらは VS Code ユーザーの筋肉記憶(copy / save / quit / go to line)と衝突し、「keymap を持ち込める」という製品思想と自己矛盾する。また rescue キーが増えるほど user keymap との調停ルールが複雑化する。command palette 単一入口方式に変更した。多くのユーザーが VS Code 等で「palette からインクリメンタルサーチして実行」を体験済みであり、導入障壁も低い。

## Consequences

### 良くなること

- VS Code の `when` 付き binding を構造を保ったまま import できる
- overlay / editor の binding 衝突を仕組みとして解決できる
- keymap resolver を UI から独立して unit test できる(ADR-0004 の境界と対応)

### リスク・コスト

- context の「限定性」比較は predicate 数という近似であり、直感に反する解決が起きうる。`:which-key` での可視化(SPEC-0002)を必須にして緩和する。
- command palette が MVP の必須 component になる(rescue の前提のため、実装順序の早い段階で最小版が必要。ADR-0004)。
- `F1` が terminal / multiplexer / OS に食われる環境では rescue 入口を失う。最終手段は SPEC-0002 Open Questions。

## Migration Notes

Greenfield のため影響なし。

## Open Questions

- `priority?: number` フィールドは source 優先度・限定性ルールと役割が重複する。MVP で本当に必要か、削除するかを実装時に判断する。

## Progress

- 2026-07-05: 初版作成(Proposed)。詳細仕様は SPEC-0002。
