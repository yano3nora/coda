# TASK-260705-03: TextBuffer core とファイル入出力

260705 text buffer core + file io
===

## asis

- `src/core/mod.rs` は空の skeleton のみ
- buffer データ構造の設計は ADR-0009 で決定済み(行ベース `Vec<String>`、grapheme index 正準、EOL / 末尾改行の往復保証)

## tobe

- `core/buffer` が pure logic として存在し、position 指定の編集・ファイル往復が unit test で保証されている
- UI・terminal に一切依存しない(ADR-0004 依存境界)

## todo

- [x] 依存追加: `unicode-segmentation`、`unicode-width`(ADR-0009)
- [x] `src/core/position.rs`: `Position { line: usize, grapheme: usize }` を定義する(Ord 実装含む。selection 範囲の正規化に使う)
- [x] `src/core/buffer.rs`: `TextBuffer` を実装する
    - fields: `lines: Vec<String>`(EOL を含まない)、`line_ending: LineEnding { Lf, CrLf }`、`trailing_newline: bool`
    - `from_bytes(&[u8]) -> Result<(TextBuffer, LoadInfo), LoadError>`
        - UTF-8 不正は `LoadError::InvalidUtf8` で拒否する(lossy 変換しない。ADR-0009)
        - EOL 検出: CRLF / LF の多数派を `line_ending` に採用。混在は `LoadInfo::mixed_line_endings = true` で報告
        - 末尾改行の有無を `trailing_newline` に保持
        - 空入力は 1 つの空行を持つ buffer とする(cursor が常に置ける)
    - `to_bytes() -> Vec<u8>`: `line_ending` と `trailing_newline` を復元して直列化する
    - 編集 API(すべて grapheme 単位。byte offset を公開しない):
        - `insert(pos: Position, text: &str) -> Position`(挿入後の cursor 位置を返す。text 内の改行は行分割として処理)
        - `delete_range(start: Position, end: Position) -> String`(削除したテキストを返す。undo の逆操作用)
        - `line(&self, index) -> Option<&str>` / `line_count()` / `grapheme_count(line_index) -> usize`
    - position の正規化: 行末超過の grapheme index は行末に clamp する helper を提供する
- [x] `src/core/mod.rs` で `buffer` / `position` を公開する
- [x] table-driven unit test(AGENTS.md Testing 方針)

## testcases

round-trip(`from_bytes` → `to_bytes` が入力と一致すること):

- [x] LF・末尾改行あり
- [x] LF・末尾改行なし
- [x] CRLF・末尾改行あり
- [x] 空ファイル(0 bytes)
- [x] 日本語・絵文字(👨‍👩‍👧‍👦 等の ZWJ sequence)・結合文字を含むファイル

load:

- [x] CRLF 多数 + LF 少数の混在 → `line_ending = CrLf` かつ `mixed_line_endings = true`
- [x] 不正 UTF-8 bytes → `LoadError::InvalidUtf8`(panic しない)

編集:

- [x] ASCII 行への `insert` / `delete_range` が期待通り動く
- [x] `"あa👍"` の grapheme index 1 への挿入が「あ」と「a」の間に入る(byte 境界でなく grapheme 境界)
- [x] ZWJ 絵文字(👨‍👩‍👧‍👦)を 1 grapheme として `delete_range` で丸ごと消せる
- [x] 改行を含む text の `insert` で行が分割される
- [x] 複数行にまたがる `delete_range` で行が結合され、削除テキストが改行込みで返る
- [x] `delete_range` の返り値を同じ位置に `insert` すると元に戻る(undo の前提となる往復性)
- [x] 行末超過 position の clamp が働く

品質:

- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がすべて通る

## notes

- レビューでの修正 1 件: codex sandbox が crates.io に接続できず、unicode-segmentation / unicode-width の**不完全な自作 shim** を vendor/ に置いて通していた。Unicode 処理の偽物実装は grapheme 境界バグの温床になるため、本物の crates.io 依存 (unicode-segmentation 1.13.3 / unicode-width 0.2.2) に差し替えて vendor/ を削除。全 15 テストは本物の crate で通過を確認済み

- undo スタック・cursor 移動(word 単位等)・selection は次タスク(TASK-260705-04 予定)。本タスクは buffer 本体と往復保証まで
- ファイルの read/write(std::fs)は本タスクでは扱わない。`from_bytes` / `to_bytes` の pure なレイヤーまで(fs 統合は app 層のタスクで行う)
- display column(CJK 幅、Tab 展開)は renderer タスクで実装する。本タスクでは `unicode-width` を依存に入れるだけでよい(使わないなら追加自体を次タスクに送ってもよい)
- commit は人間または main agent が行う(AGENTS.md)
