# ADR-0005: Configuration Format and Layout

- Status: Proposed
- Date: 2026-07-05

## Context

本製品の設定は「手書きより import が主」(ADR-0001)である。import の出力・ユーザーの上書き・アプリ本体の設定をどう分離し、どの format で持つかを決める必要がある。

## Decision

### 1. Directory layout

```text
~/.config/<app>/
  config.toml                        # アプリ設定 (sequence timeout 等)
  bindings.json                      # user binding (手書き上書き用)
  imports/
    vscode-keybindings.json          # import 元のコピー
  generated/
    vscode-bindings.json             # import が生成した binding
  import-reports/
    latest-vscode-import.txt         # import report
```

### 2. Binding format は JSON

- VS Code users に馴染みがある
- import 元(`keybindings.json`)と近く、対応関係を目視確認しやすい
- conflict / generated output を機械処理しやすい

アプリ本体の設定(binding 以外)は TOML とする。binding の TOML 化は将来検討としても、MVP では format を増やさない。

### 3. Generated と user の分離

- import の出力は `generated/` に置き、直接編集させない
- ユーザー独自の上書きは `bindings.json` に記述する
- re-import 時は `generated/` のみ再生成され、user binding は保持される

設定ファイル上の優先順位は `rescue > user > generated(imported) > default`(解決規則の全体は ADR-0002 / SPEC-0002)。

## Alternatives Considered

- **すべて TOML**: アプリ設定との統一感はあるが、import 元 JSON との対応が読みにくくなり、配列中心の binding 定義に不向き。不採用。
- **単一ファイルに merge**: re-import で user 編集が破壊される。generated / user の分離は「import を主とする」設計の前提条件。不採用。
- **Lua 等の script 設定**: 表現力は高いが、plugin system を持たない製品(ADR-0001 Non-goals)には過剰。不採用。

## Consequences

### 良くなること

- re-import が安全になり、「import して育てる」workflow が成立する
- report・generated file が並ぶため、import 結果の追跡が容易

### リスク・コスト

- 設定箇所が複数ファイルに分かれ、初見の把握コストがある。`:which-key` で「どのファイル由来か(source)」を表示して緩和する(SPEC-0002)。
- JSON はコメント不可。VS Code 同様 JSONC を許容するかを実装時に決める。

## Migration Notes

Greenfield のため影響なし。

## Open Questions

- `bindings.json` を JSONC(コメント許容)としてパースするか。
- `~/.config` 以外(XDG_CONFIG_HOME 未設定の macOS 慣習)の扱い。

## Progress

- 2026-07-05: 初版作成(Proposed)。詳細仕様は SPEC-0005。
