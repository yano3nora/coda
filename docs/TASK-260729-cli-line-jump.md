# TASK-260729: CLI line jump (`+N`)

## 目的

lazygit の `editAtLine` や fzf + grep 連携など「行番号を知っている呼び出し元」から
EDITOR として起動されたとき、指定行へ直接ジャンプして開けるようにする。
vim 互換の `+N` 構文を採用することで、lazygit の editPreset 未検出時の
デフォルトテンプレート (`{{editor}} +{{line}} -- {{filename}}`) が設定なしで動く。

## 設計

- `coda [+N] [path...]`: `+N` は 1-based 行番号。複数ファイル指定時は最初の
  (= active な) ファイルに適用する (vim 準拠)
- `--` 以降の引数はすべてパスとして扱う (`+` 始まりのファイル名との曖昧さ解消。
  lazygit デフォルトテンプレートも `--` を含む)
- 範囲外の行番号は最終行へ clamp し、status bar に明示する
  (「必ず起動する・黙って壊れない」原則。既存 `cursor.goToLine` prompt と同挙動)
- `+0` / `+abc` / `+N` の重複は InvalidUsage として明示的に reject する
  (黙って別解釈しない)
- bare `file:line` の自動パースはしない (ファイル名に `:` が合法なため
  silent misparse のリスク。必要になったら `--goto` フラグを opt-in で足す)

## 対応

- `Command::parse` (`src/app/mod.rs`): `+N` / `--` のパース、usage 更新
- `EventLoop::set_initial_line` (`src/app/event_loop.rs`): 起動時ジャンプ。
  clamp 通知は status bar が 1 行で右端 truncate されるため message の先頭へ
  prepend する (Ghostty intercept summary と同じ理由。config warnings は後続に残す)
- prompt の Go to Line と cursor 移動ロジック (`jump_to_line`) を共有

## 検証

- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test` (parse の table-driven test / event loop のジャンプ・clamp test)

## 非対応・リスク

- カラム指定 (`+N:C`) は非対応 (lazygit は line しか渡さない)
- `+/pattern` (vim の検索ジャンプ) は非対応
- `file:line` / `--goto` は非対応。Helix 系ツールで必要になった時点で再判断
