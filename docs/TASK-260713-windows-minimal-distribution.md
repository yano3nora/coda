# TASK-260713: Windows 対応 (native 配布 + win32-input-mode)

## asis

- 配布 target は macOS (x64 / arm64) と Linux gnu (x64 / arm64) のみ (`.goreleaser.yaml`)
- 自分の Windows 機に `mise use github:yano3nora/coda@<version>` で導入したい需要が発生した
- 「goreleaser に target を足すだけ」では成立しない。`libc::termios` / `tcgetattr` / `poll` / `SIGWINCH` / `STDIN_FILENO` は windows target に存在せず、**そもそもコンパイルが通らない**
- unix 依存は設計どおり (ADR-0004) 隔離されており、移植面は小さい:
    - `src/input/raw_terminal.rs`: termios raw mode / poll / signal ベースの復元
    - `src/ui/terminal_size.rs`: TIOCGWINSZ ioctl / SIGWINCH
    - `src/input/capabilities.rs` / `src/app/import_cli.rs`: isatty
    - `src/app/event_loop.rs` / `src/app/verify_cli.rs`: STDIN_FILENO 直参照 + poll
    - `src/app/import_cli.rs` `config_base_dir`: HOME / XDG_CONFIG_HOME 前提
- decoder / keymap / core / highlight は環境非依存で変更不要。syntect は `default-fancy` (pure Rust regex) なので C 依存もない。CRLF は `core/buffer` 対応済み
- **配布の実現可能性は実証済み** (260713): macOS host から `cargo zigbuild --target x86_64-pc-windows-gnu` で、`windows-sys` の Console API (GetConsoleMode / SetConsoleMode) を呼ぶ PE32+ binary が link できることを確認した

## tobe

段階を分けず、一度で「Windows Terminal 上で修飾キーが正しく届く native `coda.exe`」まで作り、細かいバグは実機 dogfood で潰す (人間の方針判断 260713)。

- Windows Terminal (ConPTY) 上で起動・編集・保存でき、**win32-input-mode (`CSI ?9001h`) により Ctrl+Shift 区別・Shift+Enter 等の modifier が保持される**
- win32-input-mode が効かない環境では既存の capability 検出が fallback mode に落ち、届かない binding は disabled として明示される (ADR-0003 explicit degradation)
- GitHub Release に `coda-v{ver}-windows-x64.zip` が追加され、mise の GitHub backend で導入できる
- **非スコープ**: legacy conhost (VT 有効化に失敗したら明示 error で起動拒否)、バイナリ署名 (SmartScreen 警告は許容)、`ReadConsoleInput` backend (win32-input-mode で足りない事態が実測で判明した場合のみ ADR を起票して再検討)

## 設計

### 入力 protocol: win32-input-mode を採用 (ReadConsoleInput は不採用)

Windows Terminal は kitty keyboard protocol 未対応 (実測で要確認) だが、win32-input-mode という独自 protocol を持つ。`CSI ?9001h` で有効化すると、full key event (virtual key / modifier / up-down / repeat) が `CSI Vk;Sc;Uc;Kd;Cs;Rc _` という **VT sequence として** 届く。

採用理由 (vs `ReadConsoleInput`):

- 入力経路が既存の「raw bytes → decoder → normalized key event」のまま。第 2 入力経路が生えず、ADR-0004 の単方向 flow と `:inspect-key` (raw bytes 表示) がそのまま生きる
- decoder への追加なので、AGENTS.md が要求する raw bytes の table-driven test にそのまま乗る。**win32 sequence の decode は pure code として全 platform でテストされる**
- WezTerm / Alacritty (Windows 版) は kitty protocol 対応のため既存 decoder で救える。win32-input-mode は WT 用の追加 protocol という位置づけ

### decoder 拡張 (`src/input/decoder.rs`, 全 platform 共通の pure code)

- `decode_csi` に final byte `_` の分岐を追加し、`Vk;Sc;Uc;Kd;Cs;Rc` を解釈する:
    - Kd=0 (key up) と modifier 単独キー (VK_SHIFT / VK_CONTROL / VK_MENU / VK_LWIN 等) はイベントを出さない
    - modifiers は Cs (dwControlKeyState) から: ctrl=0x0004|0x0008, alt=0x0001|0x0002, shift=0x0010。**AltGr (RIGHT_ALT+LEFT_CTRL 同時) は修飾キーではなく文字入力として扱う** (非 US 配列の記号入力を壊さないため)
    - Uc が印字可能ならその文字 (ASCII 英字は lowercase + Shift modifier の既存 kitty 正規化に合わせる)。Ctrl で Uc が制御文字に潰れている場合は Vk から英数字を復元
    - Uc が UTF-16 high surrogate の場合は次の win32 sequence と結合 (揃うまで Incomplete)。孤立 surrogate は捨てる
    - Rc (repeat) はその回数ぶんイベントを複製 (暴走防止で上限あり)
- **注入イベントの unwrap pre-pass**: conhost / ConPTY は query 応答 (DA1 等) を「Vk=0 の key event」として input buffer に注入する仕様がある。`drain_input_events` の冒頭で、complete な win32 sequence のうち Vk=0 かつ Kd=1 のものを Uc の byte へ置換し、復元された `CSI ?...c` 等を通常の decode 経路に流す
- win32 sequence を初めて観測した chunk では `InputEvent::Win32InputMode` を 1 回発行し、capability 検出の証拠にする

### capability 検出 (`src/input/capabilities.rs`)

- `CapabilityDetection::Win32InputMode` を追加し、modern 相当の capabilities に解決する
- Windows build では DA1 応答を legacy の証拠にしない (win32-input-mode 有効時、DA1 応答自体が wrap されて届くか不明なため)。win32 証拠が来なければ timeout で legacy に落ちる (起動時 500ms 一度きりの代償)
- unix build の検出順序 (kitty flags → DA1 → timeout) は不変

### platform 分離

- `src/input/raw_terminal.rs` → `raw_terminal/` module に分割: 共通の cleanup sequence 定数・状態 static を `mod.rs` に、termios + signal 実装を `unix.rs`、以下の Windows 実装を `windows.rs` に置く。公開境界 (`RawModeGuard` / `poll_stdin_readable`) は共通
    - raw mode: `SetConsoleMode`。stdin は `ENABLE_VIRTUAL_TERMINAL_INPUT` を立てて LINE / ECHO / PROCESSED input を落とす (PROCESSED を落とすので Ctrl+C は 0x03 として届く)。stdout は `ENABLE_VIRTUAL_TERMINAL_PROCESSING` + `DISABLE_NEWLINE_AUTO_RETURN`。**VT 有効化に失敗したら明示 error** (conhost 起動拒否)
    - `poll_stdin_readable`: `WaitForSingleObject` + `PeekConsoleInput`。KEY_EVENT 以外の record (resize 等) は `ReadConsoleInput` で読み捨てる — ConPTY 配下では KEY_EVENT は必ず byte を生むため、「wait は成立したが ReadFile が block する」stall を防ぐ
    - 復元: signal handler の代わりに `SetConsoleCtrlHandler` (window close / Ctrl+Break)。cleanup sequence (`?9001l` 含む) の書き戻しと ConsoleMode 復元
- `poll_readable(fd, ms)` → `poll_stdin_readable(ms)` に改名 (呼び出し 3 箇所は全て stdin 固定で、fd という unix 概念を境界から消す)
- `src/ui/terminal_size.rs`: Windows は `GetConsoleScreenBufferInfo` (srWindow)。SIGWINCH が無いため `take_pending_resize` は「前回サイズとの比較」で判定 (event loop が毎 tick 呼ぶ前提)
- isatty: `src/input/tty.rs` を新設し `stdin_is_tty` / `stdout_is_tty` を提供 (unix: `libc::isatty`、windows: `GetConsoleMode` 成否)
- config path: `config_base_dir` に `USERPROFILE` fallback を追加 (`%USERPROFILE%\.config\coda`。`%APPDATA%` 準拠は使用感を見て判断)
- `Cargo.toml`: `libc` を `[target.'cfg(unix)'.dependencies]` へ移し、`[target.'cfg(windows)'.dependencies]` に `windows-sys` を追加

### 配布 (goreleaser / xtask)

- target は `x86_64-pc-windows-gnu` (zigbuild link 実証済み)。msvc は macOS host から link できない
- `.goreleaser.yaml` に windows build を追加。archive は `format_overrides` で zip、命名は mise の GitHub backend が判定できる `coda-v{ver}-windows-x64.zip`
- `xtask/src/toolchain.rs` の `RUST_TARGETS` へ追加、README の対応表を更新

### 実機実測で確認すること (blind 実装の前提を検証する)

- WT が `?9001h` を受理し win32 sequence が届くか。kitty protocol 対応状況
- DA1 応答が win32 event として wrap されて届くか (capability 検出の即時解決に影響)
- win32-input-mode 有効時の bracketed paste / SGR mouse の挙動
- IME (日本語入力) の確定文字列と surrogate pair の届き方
- WT 予約キー (`Ctrl+Shift+P` / `Ctrl+Shift+C/V` 等) の一覧 → quirks DB への追加 (ADR-0007 の Ghostty と同手順)

## todo

- [x] zigbuild で windows-gnu link 実証 (windows-sys の Console API 呼び出し込み)
- [x] `Cargo.toml` deps の platform 分離 (`libc` → unix / `windows-sys` → windows)
- [x] `input/tty.rs` 新設と isatty 呼び出し置換
- [x] `raw_terminal/` 分割 + Windows 実装 (raw mode / poll / ctrl handler 復元)
- [x] `terminal_size` の Windows 実装 (サイズ比較方式の resize 検出)
- [x] `config_base_dir` の USERPROFILE fallback
- [x] decoder: win32-input-mode decode + 注入 unwrap pre-pass + table-driven tests
- [x] capabilities: `Win32InputMode` detection + windows での DA1 保留 (timeout / win32 証拠のみで解決)
- [x] event loop / guards: windows で `?9001h/l` の発行・復元 (`KeyboardProtocolGuard`)
- [x] `.goreleaser.yaml` / `toolchain.rs` / README の target 追加
- [x] `cargo check --target x86_64-pc-windows-gnu` / `cargo zigbuild --target x86_64-pc-windows-gnu` を通す (coda.exe 5.5MB link 確認)

## testcases

- [x] macOS / Linux の既存挙動が不変 (`cargo fmt --check` / `cargo clippy --workspace -D warnings` / windows target clippy / `cargo test` 291+7 件通過)
- [x] decoder: win32 sequence の table-driven test (通常キー / Ctrl+Shift 区別 / Ctrl+I vs Tab / Ctrl+Enter / Shift+Enter / AltGr / key-up 無視 / repeat + 上限 / surrogate pair + 分割到着 / 注入 DA1 unwrap / paste 内不変)
- [x] capabilities: `Win32InputMode` が modern に解決される / DA1 保留 policy で legacy に即決しない
- [x] snapshot release の asset に `coda-v*-windows-x64.zip` と `.sha256` が含まれ、zip 直下に `coda.exe` (`goreleaser release --snapshot` で確認)
- [ ] (実機) Windows Terminal で起動 → `F1` palette → 編集 → 保存 → 終了後に console mode が復元される
- [ ] (実機) `Ctrl+Shift+J` 等が `:inspect-key` で区別されて届く
- [ ] (実機) `coda keymap verify` の結果と WT 予約キーの実測 → quirks DB 追加の要否判断
- [ ] (実機) `mise use github:yano3nora/coda@<version>` で導入・起動できる
- [ ] (実機) conhost 等 VT 有効化に失敗する環境で、黙って壊れず明示 error で終了する
- [ ] (実機) IME (日本語入力)・貼り付け・SGR mouse の挙動確認 (blind 実装のため要注意箇所)

## notes

- 段階配布 (fallback 品質で先に出す) 案は人間判断で棄却 (260713)。どうせ自分しか使わないので、一度で fidelity まで作って実機でバグ潰しする
- win32-input-mode の仕様参照: microsoft/terminal `doc/specs/#4999 - Improved keyboard handling in Conpty.md`
- Codex レビュー実施 (260713、7 指摘)。対応:
    - 修正: `vk=0` surrogate pair を注入 text として splice 消去してしまう問題 (IME の絵文字入力が消える) → surrogate は splice 対象から除外し key 経路の pairing に流す
    - 修正: stdout が console なのに VT output 有効化に失敗したケースを黙殺していた → stdin mode を巻き戻して明示 error
    - 修正: `poll_stdin_readable(0)` が readiness を一度も確認せず false を返していた → 期限超過でも 0ms wait を 1 回行う
    - 修正: restore で stdout の `SetConsoleMode` 失敗を握りつぶしていた → error を返し `restored` を立てない (Drop が再試行)
    - 修正: parse 不能な `CSI ..._` でも `Win32InputMode` capability 証拠にしていた → parse 成功時のみ
    - 見送り: poll の「KEY_EVENT record は必ず byte を生む」前提 — ConPTY が input record を byte stream から合成する仕様に依拠。実機検証項目に含める (key-up record を勝手に discard すると実入力を失うため、防御的 discard はできない)
    - 見送り: `SetConsoleCtrlHandler` の戻り値未検査と ctrl type 無差別 `ExitProcess` — unix 側の `libc::signal` 戻り値未検査・全 signal 共通 handler と同じ既存方針に合わせた
