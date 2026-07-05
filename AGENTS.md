# AGENTS - Development Guide
## Overview
- TODO: この repo は何をするものか
- TODO: 主要な技術スタック / 実行環境
- TODO: 最も重要な仕様・設計ドキュメント
- TODO: 関連 repo / 移植元 / 参照実装があれば書く

### 🎯 Role & Objective
あなたはエキスパートソフトウェアエンジニアとして、この repo の設計・実装・テストを行うこと。

### 🚨 CRITICAL: Architecture
- **TODO: 最重要な設計原則**: なぜ重要かを書く
- **TODO: 依存境界**: どの層が何に依存してよいかを書く
- **TODO: 状態管理の原則**: global state / singleton / cache などの扱いを書く
- **TODO: 失敗モード**: 前提にすべき失敗や edge case を書く
- **TODO: YAGNI / 過剰設計禁止**: 広げてはいけない方向を書く

### 📂 Code Organization Constraints
- **`TODO/`**: TODO
- **`TODO/`**: TODO
- **`TODO/`**: TODO
- **型 / 境界**: TODO

### 🛠️ Workflow & Development Rules
- **Secrets**: 企業名・製品名・機密情報などがあった場合、コード上に残らないように汎用・一般名称に差し替えること。
- **Commit**: `git commit` は基本的には人間判断で行うため、指示されたとき以外はコミットせず人間に判断を委ねること。
- **Push / Publish**: `github push` や `npm publish` など、外部へ公開・配布する操作は Agent が実行しない。人間が判断して実行する。
- **Testing**: タスク完了前に実行する検証を書く
    - TODO: unit test 方針
    - TODO: integration / e2e 方針
    - TODO: bugfix 時の再現テスト方針
- **Documentation**:
    - 技術的な意思決定や検討は `docs/ADR-XXXX-*.md` に記録し、大きな変更の前には既存 ADR を確認する
    - 設計・仕様の検討・決定事項は `docs/SPEC-XXXX-*.md` に記録する
    - 原則、全開発タスクが適切な粒度で `docs/TASK-YYMMDD-*.md` に残るようにする
    - 画像などは `docs/assets/` へ配置してリンクする
- **Versioning / Release**: TODO

## Domains
- `TODO`
    - TODO: 重要なドメイン用語の説明
- `TODO`
    - TODO
- `TODO`
    - TODO
