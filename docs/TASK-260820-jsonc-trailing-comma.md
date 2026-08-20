# TASK-260820: bindings.json の trailing comma 許容

260820 jsonc trailing comma tolerance
===

## asis

dogfood で `bindings.json` に `{ ... },` と末尾カンマを書いたところ、file 全体が
`InvalidJson (trailing comma)` で捨てられ、user binding が 1 件も効かなくなった。

- JSONC 対応は `strip_jsonc_comments`(comment 除去)のみで、その後は strict な
  `serde_json` に渡していた
- VS Code の `keybindings.json` は trailing comma を許容するため、そこから entry を
  コピペする習慣と必ず衝突する。SPEC-0005 の「形式は VS Code に揃える」とも不整合
- 起動時 warning は出るが 1 行 status bar では見逃しやすく、実質「黙って壊れる」

## tobe

VS Code 同様、`]` / `}` 直前の trailing comma を許容する。

## todo

- [x] `strip_trailing_commas` を `keymap/user_bindings.rs` に追加し、
  `load_bindings_with_source` で comment 除去後に適用
    - comment 除去済み text が前提なので、保護対象は string literal のみ
    - 削除でなく空白置換にして serde_json の error 位置を保つ(`strip_jsonc_comments` と同じ方針)
- [x] scaffold(`KEYBINDINGS_TEMPLATE`)の案内文に trailing comma 許容を明記

## testcases

- [x] unit: trailing comma 入り(entry 内 `,}` / 配列末尾 `,]` + 行 comment 混在)が
  issues なしで load される(`accepts_trailing_commas_like_vscode`。修正前に fail を確認)
- [x] unit: string literal 内の `,]` / `,}` は除去されない
  (`comma_before_bracket_inside_string_is_not_stripped`)
- [x] unit: escaped quote / escaped backslash / multibyte 直後の comma で
  stripper が壊れない(`trailing_comma_stripper_handles_escapes_and_multibyte`。
  codex レビュー指摘で追加)
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`(298 tests)

## notes

- generated(imported)側も同じ loader を通るが、importer 出力は valid JSON なので無害
- lookahead は `text[index + 1..].trim_start()` だが、comma が走査を打ち切るため
  各 comma の whitespace 走査区間は重複せず、全体では O(n)(codex レビュー指摘で訂正)
