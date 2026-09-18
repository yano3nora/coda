# TASK-260918: stdin が `/dev/tty` だと macOS で coda が固まる不具合の修正

260918 dev-tty poll freeze fix
===

## asis

gistan (fzf) → leaf → coda の順で起動すると、coda が一切の操作を受け付けない。
CPU は 98% 前後になり、status bar に `legacy terminal input` 警告が出る。
通常の `coda file` は問題ない。

- fzf の `execute` は、自身の stdin が tty でないと子プロセスの stdin に
  `/dev/tty` (cloning device) を渡す。leaf → coda はそれをそのまま継承する
- macOS の `poll(2)` は `/dev/tty` に対して即座に `POLLNVAL` を返す
- `input/raw_terminal/unix.rs` の `poll_fd_readable` は `POLLIN` が立たない
  結果を「入力なし」として `Ok(false)` を返していた
- event loop は「入力なし」を timeout 経過と解釈し、housekeeping と再描画だけを
  繰り返す。stdin は一度も read されない
- kitty protocol の応答 (`CSI ? u`) も読めないので capability probe が timeout し、
  legacy 警告が出る

最小再現 (Ghostty で直接):

```
coda README.md < /dev/tty
```

## tobe

stdin が `/dev/tty` でも通常どおり入力を受け付け、capability probe も成功する。
kernel に拒否された fd は「入力なし」ではなく error として表面化する。

## todo

- [x] macOS: `wait_readable_once` を `select(2)` で実装する
    - fzf と crossterm (`filedescriptor` crate) が同じ回避策を採っている
    - macOS の `select` は閉じた fd を EBADF にしないため `fcntl(F_GETFD)` で事前検証
- [x] 他 OS: `poll(2)` の `POLLNVAL` を `Err` にする
- [x] `poll_fd_readable` の EINTR retry (TASK-260820) はそのまま共通部に残す
- [x] 呼び出し側 (`event_loop.rs` / `verify_cli.rs` / `capabilities.rs`) は変更なし

## testcases

- [x] unit: fork した子プロセスで閉じた fd への wait が `Err` になる
  (`readiness_wait_on_closed_fd_errors_instead_of_reporting_no_input`。修正前は `Ok(false)`)
    - 子プロセスは single-thread なので fd 番号の再利用が起きない
      (親 process で高い fd 番号を仮定する方式は codex レビューで却下)
- [x] unit: `openpty` + `setsid` + `TIOCSCTTY` で専用 pty を controlling tty にした
  子プロセスで、`/dev/tty` への wait が timeout を守る
  (`readiness_wait_on_dev_tty_honors_timeout`。修正前は数µs で返っていた)
    - 開発者の実端末を使うと pending 入力で即時復帰が正常になり flaky。
      open 失敗の一律 skip も障害を隠す (codex レビュー指摘)。専用 pty なら skip 不要
    - 子プロセスは panic も heap 確保もせず exit code で報告する。multi-thread process の
      fork 後は他 thread の lock 状態を継承するため (codex レビュー指摘)。
      そのため `wait_readable_once` の error は `io::Error::from_raw_os_error`
      (POLLNVAL → EBADF、fd_set 範囲外 → EINVAL) にして確保なしにした
    - `waitpid` は EINTR で再試行、親の pty fd は `OwnedFd` で RAII (codex レビュー指摘)
- [x] `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` (349 tests)
- [x] `cargo check --target x86_64-unknown-linux-gnu --tests` (poll 経路のコンパイル確認)
- [x] E2E (pty harness): `target/debug/coda file < /dev/tty` でキー入力が届き、
  status が `F1: command palette` (modern) になる。released 0.8.1 は同条件で
  55KB/0.6s の再描画と CPU 97%、legacy 警告
- [ ] 手動: gistan → `ctrl-v` → leaf → `Ctrl+E` → coda で編集できること

## notes

- `/dev/tty` は `CHR 2,0` の cloning device で、実 pty slave (`/dev/ttysNNN`) とは
  別物。`poll` は後者では正常に動く
- 検証記録: `poll(/dev/tty)` → `rc=1 revents=0x20 (POLLNVAL)`、
  `poll(/dev/ttys010)` → `rc=0 revents=0x0`
- unit test は controlling tty 無しでも動く。ただし Claude Code の sandbox (Seatbelt) 内では
  `openpty` が EPERM になるため、sandbox 外で実行すること
- leaf 側の軽微バグ (config の `{$path}` 展開後に path を再度追加し `+N path path` になる)
  は coda に実害がないため対象外
