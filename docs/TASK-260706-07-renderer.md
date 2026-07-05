# TASK-260706-07: Screen buffer と差分 renderer

260706 renderer
===

## asis

- ADR-0010 で「TUI framework 不採用・自前 renderer」を決定済み
- `ui/` は空 skeleton。画面描画の仕組みが存在しない

## tobe

- 「styled cell の screen buffer + 前面/背面 diff + ANSI 直列化」が `ui/` に存在する
- 出力先は `impl Write` に抽象化され、in-memory の `Vec<u8>` に対して unit test できる
- alternate screen の RAII guard が raw mode / kitty protocol と同じ作法(Drop + signal 復元)で入る

## todo

- [x] `src/ui/screen.rs`: `Screen` buffer
    - `Cell { symbol: String, width: u8, style: Style }`(symbol は grapheme 1 個。全角は width=2 で直後 cell を continuation 扱いにする)
    - `Style { reverse: bool, dim: bool }`(色は highlight タスクで拡張する。MVP は属性のみ)
    - `Screen::new(width, height)` / `resize(width, height)`(resize で全 cell クリア)
    - `put_str(x, y, text, style) -> usize`: grapheme 分割(`unicode-segmentation`)と表示幅(`unicode-width`)で cell を埋め、右端で clip する。次の x を返す。**全角文字が右端 1 column しか残っていない場合は書かない**(はみ出し禁止)
    - Tab 文字は `put_str` に渡す前に呼び出し側で展開する想定(この module では制御文字を受け取ったら空白 1 個に置換して安全側に倒す)
    - `set_cursor(x, y)` / `cursor: Option<(u16, u16)>`(None = cursor 非表示)
- [x] `src/ui/render.rs`: 差分 renderer
    - `render_diff(prev: &Screen, next: &Screen, out: &mut impl Write) -> io::Result<()>`
        - 変更のあった**行だけ**を再描画する(行内は先頭から行末まで書き直しでよい。cell 単位 diff は不要)
        - 行描画: `CSI {row};1H` で移動 → style run ごとに SGR(reverse=`CSI 7m`、dim=`CSI 2m`、解除=`CSI 0m`)→ 行末で `CSI 0m` と `CSI K`(clear to EOL)
        - SGR は run の切り替わり時のみ出力する(cell ごとに出さない)
        - 描画後: cursor が Some なら `CSI {row};{col}H` + `CSI ?25h`(表示)、None なら `CSI ?25l`(非表示)
        - `prev.size != next.size` の場合は全行再描画
    - `render_full(next: &Screen, out) -> io::Result<()>`: 初回・resize 後用(全行 + `CSI 2J` 相当)
- [x] `src/ui/alt_screen.rs`: `AltScreenGuard`
    - enter: `CSI ?1049h`(alternate screen)+ `CSI ?25l`(cursor 非表示)
    - drop: `CSI ?25h` + `CSI ?1049l`
    - `src/input/raw_terminal.rs` の signal 復元に alt screen leave を追加する(`KEYBOARD_PROTOCOL_PUSHED` と同じ AtomicBool パターン。順序: alt screen leave → kitty pop → termios 復元)
    - guard は `Write` を受け取る構成にし、test では `Vec<u8>` に出力できること(実利用は stdout)
- [x] `src/ui/terminal_size.rs`: `terminal_size() -> Option<(u16, u16)>`(`libc::ioctl` + `TIOCGWINSZ`)と、SIGWINCH で立つ resize フラグ(`take_pending_resize() -> bool`。AtomicBool、signal handler は flag を立てるだけ)
- [x] `src/ui/mod.rs` で公開する
- [x] table-driven unit test(AGENTS.md Testing 方針)

## testcases

screen:

- [x] `put_str` が ASCII / 日本語(width 2)/ ZWJ 絵文字(1 grapheme)を正しい cell 数で配置する
- [x] 右端 clip: 幅 4 の screen に `"あいう"` → 「あい」まで(3 grapheme 目は入らない)
- [x] 右端 1 column 残りに全角 → 書かれない(cell がはみ出さない)
- [x] 制御文字(`\t` 等)が空白に置換される
- [x] `resize` で全 cell がクリアされる

render:

- [x] 初回 `render_full` の出力に全行と `CSI 2J` が含まれる
- [x] 1 行だけ変えた `render_diff` の出力に、その行の `CSI {row};1H` **のみ**が含まれる(他行の移動 sequence が無い)
- [x] 変更なしの `render_diff` が cursor 制御以外ほぼ空
- [x] reverse style の run が `CSI 7m` ... `CSI 0m` で 1 回ずつ出る(cell ごとに SGR が出ない)
- [x] cursor Some → `CSI {row};{col}H` + `?25h`、None → `?25l`
- [x] size 変更時に全行再描画される

alt screen / size:

- [x] `AltScreenGuard` が enter/drop で正しい sequence を writer に出す
- [x] (自動確認)`terminal_size()` が TTY 以外で None を返し panic しない

品質:

- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

## notes

- 新規依存 crate の追加は禁止(unicode-segmentation / unicode-width / libc は導入済み)
- ANSI の row/col は **1-origin**。Screen の座標は 0-origin。変換ミスに注意
- synchronized output(`CSI ?2026`)は今回入れない(ADR-0010 Open Question。必要になったら render_diff の前後に足すだけの構造にしておく)
- 色(16/256/truecolor)は highlighting タスクで `Style` を拡張して入れる。今の `Style` に色フィールドを先取りで作らないこと(YAGNI)
- event loop・editor viewport・palette は次タスク(TASK-260706-08)。この renderer は「Screen を渡されたら描く」だけに徹する
- commit は人間または main agent が行う(AGENTS.md)
