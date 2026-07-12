# TASK-260712-16: keyboard capability 検出の結線

260712 keyboard capability detection wiring
===

## asis

- 起動時に `CSI >1u`(kitty protocol push)と `CSI ?u`(flags 照会)は送っているが、**応答 `CSI ? <flags> u` は decoder で `Key::Unknown` に落ちて捨てられている**(`decoder.rs` の `decode_kitty_csi_u` が `?` 付き params を parse できない)
- `KeyboardCapabilities`(SPEC-0003 で型定義済み)が実装に存在しない
- importer の `Disabled by terminal capability` は常に 0(`report.rs` に TODO コメント)
- fallback terminal での起動時 warning がない
- `:inspect-key` に protocol 判定結果が出ない
- これらが SPEC-0001 受け入れ基準「区別不能な binding の明示」の最後の未達項目

## tobe

- terminal の keyboard protocol 対応状況が `KeyboardCapabilities` として確定し、次の 4 箇所に露出する:
    1. 起動時 warning(legacy terminal のとき 1 行)
    2. import report の `Disabled by terminal capability` 実分類
    3. `:inspect-key`(in-editor overlay)の protocol 行
    4. `coda inspect-key`(CLI)の protocol 行
- SPEC-0001 の Terminal Compatibility 受け入れ基準が全て埋まる

## 設計判断(260712)

### 検出方式: `CSI ?u` 応答 + DA1 fallback(timeout は最後の砦)

- push(`CSI >1u`)と照会(`CSI ?u`)の直後に **DA1(`CSI c`, Primary Device Attributes)も送る**。DA1 はほぼ全ての terminal(Terminal.app / tmux / screen 含む)が応答するため:
    - `CSI ? <flags> u` 応答あり → **modern**(kitty protocol 有効)
    - `?u` 応答なしで DA1 応答(`CSI ? ... c`)が先に届く → **legacy 確定**(timeout を待たない = legacy terminal でも起動が遅くならない)
    - どちらも来ない → **timeout 500ms で legacy**(SPEC-0003 Open Question「negotiation timeout」への回答。idle poll が 100ms なので粒度は十分)
- flags の解釈: `flags & 1`(disambiguate escape codes)が立っていれば modern。push 済みなので対応 terminal は必ず立てて返す。立っていない応答は保守的に legacy 扱い

### 型と配置(SPEC-0003 / ADR-0003 / ADR-0004)

- 新 module `input/capabilities.rs`:
    - `KeyboardCapabilities { supports_modified_keys, supports_shift_ctrl_distinction, supports_shift_enter, supports_ctrl_enter }`(SPEC-0003 の 4 bool)+ `modern()` / `legacy()` constructor
    - `CapabilityDetection`(判定根拠の explainability 用): `KittyFlags(u16)` / `LegacyDeviceAttributes` / `LegacyTimeout` / `AssumedModern`(非 TTY import 用)。`capabilities()` accessor と、inspector 表示用の短い説明文 `description()` を持つ
    - `CapabilityProbe`(**pure な状態機械**。terminal I/O を持たず unit test 可能): `on_event(&InputEvent) -> Option<CapabilityDetection>` と `on_tick(now: Instant) -> Option<CapabilityDetection>`。一度 resolve したら以後 None
    - `probe_blocking(timeout) -> CapabilityDetection`(import CLI 用): stdin / stdout のどちらかが TTY でなければ何も送らず `AssumedModern`(stdout redirect 時に query の escape bytes がファイルへ漏れて偽 legacy になるのを防ぐ)。両方 TTY なら RawModeGuard + push/照会/DA1 送信 → poll+read → `CapabilityProbe` に食わせ、終了時に `CSI <u` で pop
- keymap resolver / importer は capability のみを見る(protocol 名・raw bytes を見ない。ADR-0003 の境界を維持)

### decoder: 応答を InputEvent に昇格

- `InputEvent` に `CapabilityReply(u16)` と `DeviceAttributes` を追加。`CSI ? <digits> u` → `CapabilityReply(flags)`、`CSI ? ... c` → `DeviceAttributes`
- `drain_key_events` / `decode_key_events` はこれらを key として漏らさない(`Key::Unknown` に落とすのをやめる)。parse 不能な `?` 付き CSI は従来どおり `Key::Unknown`
- CLI `inspect-key` の `format_inspect_chunk` は応答 chunk を `Protocol: kitty keyboard protocol supported (flags=1)` のような行で表示する(現状は Unknown 表示)

### event loop 結線

- `run()` で `KeyboardProtocolGuard::push` の直後に `CSI c` を書いて probe を arm(deadline = now + 500ms)
- `handle_input_event` に `CapabilityReply` / `DeviceAttributes` の arm を追加し probe へ。メインループ毎周で `on_tick`。resolve 時:
    - `capabilities` / `capability_detection` を保存
    - legacy なら warning を `self.message` の**先頭に** prepend(TASK-17 の Ghostty warning と同じ理由)。文言: `legacy terminal input: Ctrl+Shift+J / Shift+Enter etc. cannot be distinguished — run inspector.open for details`
    - `capability_warning` config(SPEC-0005)の結線は P1 のまま(常に表示。gate は backlog 済み)
- `poll_timeout_ms` は idle 100ms のままで良い(on_tick が拾う)
- resolve 前に editor を quit した場合も何も壊れない(probe は単に捨てられる)

### importer 結線(SPEC-0004)

- `import_vscode_keybindings(text, &KeyboardCapabilities)` に引数追加。key parse / when 変換を通った entry に対して chord 単位で判定し、該当したら `disabled_by_terminal_capability` へ(bindings には**含めない**。bucket 排他と `total_classified` の debug_assert を維持):
    1. super を含む && `!supports_modified_keys` → `terminal cannot deliver Cmd/Super`
    2. `Key::Char` + ctrl + shift && `!supports_shift_ctrl_distinction` → `terminal cannot distinguish Ctrl+Shift+<K> from Ctrl+<K>`
    3. Enter + shift && `!supports_shift_enter` → `terminal cannot receive Shift+Enter`
    4. Enter + ctrl && `!supports_ctrl_enter` → `terminal cannot receive Ctrl+Enter`
    - **arrow / Home / End 等の special key は legacy CSI(`CSI 1;6C` 等)で modifier が届くため対象外**(Ctrl+Shift+Right は legacy でも disabled にしない)
- `--cmd=ctrl` の提案文言は flag 未実装(P1)のため reason に入れない(存在しない flag を案内しない)
- import CLI: `probe_blocking(500ms)` で判定し、stdout と report file の先頭に `Terminal capability: modern (kitty CSI u, flags=1)` / `legacy (no CSI ?u reply)` / `not detected (not an interactive terminal); assuming modern` を出す。非 TTY(pipe / CI)は modern 仮定 = 誤 disable で binding を失わせない(runtime 側の真実は editor 起動時検出が持つ)

### inspector 表示

- in-editor overlay: body 先頭に 1 行 `protocol: detecting…` / `protocol: kitty CSI u (flags=1)` / `protocol: legacy (DA1 answered, no CSI ?u reply)` / `protocol: legacy (query timed out)`

## todo

- [x] `input/capabilities.rs`: `KeyboardCapabilities` / `CapabilityDetection` / `CapabilityProbe`(pure)/ `probe_blocking`
- [x] `input/decoder.rs`: `CapabilityReply` / `DeviceAttributes` の decode(key として漏らさない)
- [x] `app/event_loop.rs`: probe 結線・legacy warning・inspector への状態受け渡し
- [x] `keymap/vscode_import.rs` + `report.rs`: capability 引数と Disabled 分類・reason 文言
- [x] `app/import_cli.rs` + `app/mod.rs`: probe_blocking 結線と capability 行の出力
- [x] `app/inspector.rs`: protocol 行の表示
- [x] `input/mod.rs`(CLI inspect-key): 応答 chunk の friendly 表示

## testcases

- [x] unit(decoder, table-driven): `\x1b[?1u` → `CapabilityReply(1)` / `\x1b[?0u` → `CapabilityReply(0)` / `\x1b[?62;22c` → `DeviceAttributes` / key と混在 chunk / `drain_key_events` がこれらを漏らさない / 分割到着(Incomplete)
- [x] unit(probe, table-driven): reply(flags=1)→ modern / DA1 先着 → legacy / timeout → legacy / resolve 後のイベントは無視
- [x] unit(importer, table-driven): legacy capability で cmd+s → Disabled(reason 固定)/ ctrl+shift+j → Disabled / shift+enter・ctrl+enter → Disabled / **ctrl+shift+right は Disabled にならない** / modern capability では全て Imported / summary 件数と render_text
- [x] unit(inspector / event loop): protocol 行の 4 状態、legacy warning の prepend
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`
- [x] PTY E2E: (1) 素の PTY(応答なし)→ timeout 経由で legacy warning 表示・正常起動 / (2) PTY 側から `\x1b[?1u` を返す模擬 modern → warning なし / (3) import CLI を pipe で叩き `not detected ... assuming modern` 行と Disabled: 0 を確認(+ 実 Ghostty での手動確認は人間 dogfood に委ねる)

## notes

- 優先度 P0(MVP 最終タスク)。完了で SPEC-0001 受け入れ基準が全て埋まる
- ADR-0003 の Open Questions(timeout 値・tmux 検出)への回答: timeout は 500ms + DA1 fallback。`$TMUX` の特別扱いはしない(tmux も DA1 に応答し、extended-keys 透過は inspector で確認可能 = SPEC-0003 Trouble Shooting の導線どおり)
- runtime resolver での「fallback mode における個別 binding の disabled 化・conflict 表示」は本タスクの scope 外(importer 分類と起動時 warning で「明示」の受け入れ基準は満たす。必要になれば backlog へ)
- 実装フロー: 設計・TASK 化(main agent)→ Sonnet 委任 → main agent レビュー(省略不可。TASK-17 で毎回実バグ検出の実績)→ PTY E2E → commit は人間判断
- 260712 実装記録: Sonnet 委任(170 tests green で納品)→ main agent レビューで**実害のある edge case 1 件検出・修正**: `probe_blocking` が stdin の TTY 判定しかしておらず、`coda keymap import vscode k.json > report.txt` のような **stdout redirect 時に query の escape bytes が redirect 先ファイルへ混入 + 応答が来ないので 500ms ブロック → 偽 legacy 判定で bindings を誤 disable** する。stdout も TTY であることを要求する形に修正(`AssumedModern` の文言も `not an interactive terminal` へ変更)。fmt / clippy / test(170)green、PTY E2E 4 本(legacy timeout / 模擬 modern 応答 / **DA1 先着 legacy(timeout 不要の即確定)** / 非 TTY import)実測 green。実 Ghostty(modern 環境)での warning 非表示は人間 dogfood で最終確認のこと。commit は人間判断待ち
