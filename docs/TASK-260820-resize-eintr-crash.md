# TASK-260820: terminal resize で editor が終了する crash の修正

260820 resize EINTR crash fix
===

## asis

coda を開いた状態で terminal window を resize すると editor が閉じてしまう。

- resize 通知は SIGWINCH handler (`ui/terminal_size.rs`) で flag を立て、
  event loop が `take_pending_resize` を見る設計
- しかし SIGWINCH が `libc::poll` (`input/raw_terminal/unix.rs`) を EINTR で
  中断すると `poll_stdin_readable` が `Err` を返し、event loop の `?` で
  そのまま伝播して editor が落ちていた
- `stdin.read` も同様に `ErrorKind::Interrupted` で `?` 伝播する経路があった

## tobe

EINTR を error として伝播させず、event loop が生き残って
`take_pending_resize` を拾い再描画する(resize で落ちない)。

## todo

- [x] `poll_fd_readable`: EINTR なら残り時間で retry(fd を引数化して
  `poll_stdin_readable` はその wrapper に)
    - 「入力なし (`Ok(false)`)」で即返すと caller が escape-flush timeout 経過と
      誤解して pending ESC を誤発火しうるため retry 方式(codex レビュー指摘)
    - retry は `Instant` の絶対 deadline から残り時間を計算。full timeout での
      retry だと resize drag 中の連続 SIGWINCH で poll から戻れず housekeeping が
      飢餓する(codex レビュー指摘)
- [x] event loop の `stdin.read`: `Interrupted` は入力処理を skip して loop 末尾の
  housekeeping(resize 処理を含む)へ fall through(`continue` だと housekeeping を
  飛ばすため。codex レビュー指摘)

## testcases

- [x] unit: pipe を poll 中の thread へ `pthread_kill` で signal を送り、error に
  ならず timeout まで retry して `Ok(false)` が返る
  (`poll_interrupted_by_signal_retries_instead_of_erroring`。修正前は `Err(EINTR)` で fail)
    - signal は SIGUSR1 を使用。SIGWINCH だと同一 process 内の production handler
      を test が書き換えてしまうため(codex レビュー指摘)。EINTR の機構は同一
    - `sigaction` / `pthread_kill` の戻り値を assert し、test 終了時に旧 handler を
      復元(送信失敗時に自然 timeout で偽陽性になるのを防ぐ。codex レビュー指摘)
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`(312 tests)
- [ ] 手動: coda 起動中に window resize して継続・再描画されること

## notes

- macOS の `signal(3)` は SA_RESTART 付きだが、poll は restart 対象外の
  ことがあるため EINTR を syscall wrapper 側で吸収するのが確実
- SIGWINCH handler 自体 (`ui/terminal_size.rs`) は変更なし
- 対象外: `input/capabilities.rs` の probe read と `app/verify_cli.rs` の read にも
  EINTR 経路はあるが、resize crash(常駐 event loop)とは影響度が異なるため本 TASK
  では触らない。問題が観測されたら同じ方針で対応する
