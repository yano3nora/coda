//! 同梱 syntax 定義 (`src/highlight/assets/*.sublime-syntax`) を syntect の default set に
//! 足し、link 済みの dump として固める。実行時に YAML を parse すると TypeScript 定義だけで
//! 約 0.7 秒かかり「短時間編集」の起動が遅くなるため、parse は build 時に済ませる
//! (docs/TASK-260907-syntax-coverage-and-file-detection.md)。
//!
//! 定義が読めない場合は build を失敗させる。実行時に黙って言語が消えるより、追加時点で気づく方がよい。

use std::{env, fs, path::PathBuf};

use syntect::parsing::{SyntaxDefinition, SyntaxSet};

const ASSETS_DIR: &str = "src/highlight/assets";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ASSETS_DIR}");

    let mut paths: Vec<PathBuf> = fs::read_dir(ASSETS_DIR)
        .unwrap_or_else(|err| panic!("failed to read {ASSETS_DIR}: {err}"))
        .map(|entry| {
            entry
                .unwrap_or_else(|err| panic!("failed to read an entry of {ASSETS_DIR}: {err}"))
                .path()
        })
        .filter(|path| path.extension().is_some_and(|ext| ext == "sublime-syntax"))
        .collect();
    // 同名定義は後勝ち (Markdown が default を差し替える)。順序を決定的にして再現性を保つ
    paths.sort();

    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    for path in paths {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let syntax = SyntaxDefinition::load_from_str(&text, true, None)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()));
        builder.add(syntax);
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let dump_path = out_dir.join("syntaxes.packdump");
    syntect::dumps::dump_to_file(&builder.build(), &dump_path)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", dump_path.display()));
}
