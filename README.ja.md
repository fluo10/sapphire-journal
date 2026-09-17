# sapphire-journal（日本語）

> 言語: [English](README.md) | **日本語**

sapphire-framework上に構築された、タスク・イベント・ノート管理アプリ。ファイルベース・ローカルファースト・人間とAIエージェントの協働のために作られました。

## コンセプト

- **Markdownをソース・オブ・トゥルースに** — すべてのデータはプレーンな`.md`ファイルに保存され、どんなツールでも読み書きできます
- **SQLiteをキャッシュに** — Markdownファイルの上に素早いクエリとインデックスを実現（予定）
- **バレットジャーナル着想** — タスク・イベント・ノートは対等なPeerとして、それぞれが独立したentryとして存在します。バレットジャーナルがすべてのバレット（タスク・イベント・ノート）を等しく扱うように、sapphire-journalもタイプを問わずすべてのentryを同じように扱います
- **テキストエディタ／IDE互換** — YAMLフロントマター付きのプレーンな`.md`ファイル。特別なツールなしにどんなエディタでも読み書きできます
- **人間とAIの共同編集** — gitやSyncthing同期を介してAIエージェント（Claudeなど）が同じジャーナルを・作成・することを想定して設計されています

## 設計判断

### エントリID：連番ではなくcaretta-id

各entryのファイル名は[caretta-id](https://github.com/fluo10/caretta-id)（0.1秒精度の7文字BASE32識別子、例 `123abcd_my_note.md`）が前置きされます。

連番IDだと、gitやSyncthingで同期された共有ジャーナルで人間とAIエージェントが同時にentryを追加したときに衝突します。caretta-idは現在のUnix時間を0.1秒単位で値に使うため、0.1秒以上離れて作られた2つのentryは必ず異なるIDになります — 中央調停者なしで衝突なしを保証します。

### ファイル配置：`{year}/{id}_{slug}.md`

entryは年ディレクトリ（例 `2026/`）にグループ化され、ジャーナルルートが長期間で埋まりすぎるのを防ぎつつ、階層が浅すぎてナビゲートしづらくならないようにしています。タイトルから作ったスラッグにより、sapphire-journalを開かなくてもファイル名が読みやすくなります。

## データモデル

各ファイルは最も重要なデータ単位である**Entry**です。entryにはフリーフォームのノート、タスクのチェックボックス（`- [ ]`）、またはその両方を含められます。

ノートentry：

```markdown
---
id: '1a2b3c4'
title: Meeting notes
created_at: '2026-03-06T14:00:00'
tags: [work]
---

Discussion points from the team meeting.
```

task entryは`task`ブロックを追加します：

```markdown
---
id: '2b3c4d5'
title: Fix login bug
created_at: '2026-03-06T10:00:00'
tags: [work, backend]
task:
  status: open
  due: '2026-03-08T18:00:00'
---

Reproduction steps and notes here.
```

event entryは`event`ブロックを追加します：

```markdown
---
id: '3c4d5e6'
title: Team sync
created_at: '2026-03-06T09:00:00'
event:
  start: '2026-03-07T15:00:00'
  end: '2026-03-07T16:00:00'
---
```

## インストール

### Linux / macOS

```sh
curl -fsSL https://raw.githubusercontent.com/fluo10/sapphire-journal/main/install.sh | sh
```

### Windows

```powershell
irm https://raw.githubusercontent.com/fluo10/sapphire-journal/main/desktop/install.ps1 | iex
```

### cargo-binstall

```sh
cargo binstall sapphire-journal-cli
```

### ソースから

```sh
cargo install sapphire-journal-cli
```

## CLIの使い方

コマンドリファレンス全体は[cli/README.md](cli/README.md)を参照してください。

**ジャーナル**とは、`.sapphire-journal/`ディレクトリを含むディレクトリツリーのことです。`sapphire-journal`はカレントディレクトリから上へ辿ってそれを特定します。`git`が`.git/`を見つけるのと同じ方法です。`sapphire-journal init`で作成します。

## プロジェクト構成

```
sapphire-journal/
├── cli/                     # CLIバイナリ（sapphire-journal）
├── desktop/                 # デスクトップGUI（egui）
├── server/                  # セルフホストのリモートワークスペース＋MCPサーバー
└── crates/
│   ├── sapphire-journal-core/   # データモデル、Markdownパーサ/シリアライザ、SQLiteキャッシュ
│   └── sapphire-journal-mcp/    # MCPサーバーライブラリ（cliとserverが組み込み）
```

## ステータス

開発初期段階 — CLIとMCPサーバーは、階層と全文検索を含むentry管理について動作します。

## ライセンス

このリポジトリは異なるライセンスのコンポーネントを含みます：

| コンポーネント | ライセンス |
|-----------|---------|
| `sapphire-journal-core` | MIT OR Apache-2.0 |
| `sapphire-journal-cli` | MIT OR Apache-2.0 |
| `sapphire-journal-mcp` | MIT OR Apache-2.0 |
| `sapphire-journal-desktop` | GPL-3.0-or-later、App Storeマーケットプレイス例外付き |

各コンポーネントのディレクトリにある`LICENSE`（または`LICENSE-MIT` / `LICENSE-APACHE`）ファイルに完全なライセンス文書があります。
