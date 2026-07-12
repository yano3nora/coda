# TASK-260713: P2 key delivery completion

## asis

- OS / terminal 予約キーが通常の import として扱われる
- `keymap verify` はテキスト report のみ保存し、起動時の resolver に反映されない
- Cmd 系の基本編集キーと `Ctrl+C` の明示的な終了設定がない

## tobe

- `Cmd+Q` / `Cmd+Tab` を移植不能として明示する
- verify 結果を `TERM_PROGRAM` と version ごとに保存し、mismatch 済み chord を無効化する
- Cmd copy / undo / redo / save を default binding にし、`keymap.ctrl_c = "quit"` を選択可能にする

## todo

- [x] reserved key の import 分類
- [x] terminal identity 付き verify state と resolver 反映
- [x] Cmd default と Ctrl+C 設定
- [x] docs と backlog を更新
- [x] fmt / clippy / test

## testcases

- [x] reserved key が generated binding に入らない
- [x] terminal identity が一致する場合だけ mismatch chord が無効になる
- [x] `ctrl_c` の既定は copy、明示設定時のみ quit

## notes

`Ctrl+C` は SIGTERM ではなく慣例上 SIGINT。ただし coda 内では OS signal を発生させず、
未保存確認を維持する `app.quit` action に割り当てる。verify による action の自動変更は、
terminal 更新時に copy のつもりで終了する危険があるため行わない。
