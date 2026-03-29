# sapphire-journal

Markdown-based task and note manager for humans and AI agents — your data lives in plain text, timeless like fossils.

## Concept

- **Markdown as source of truth** — all data lives in plain `.md` files you can read and edit with any tool
- **SQLite as cache** — fast querying and indexing on top of the Markdown files (planned)
- **Bullet-journal inspired** — tasks, events, and notes are equal peers, each living as its own entry; just as a bullet journal treats every bullet (task, event, or note) uniformly, sapphire-journal treats every entry the same way regardless of type
- **Text-editor / IDE compatible** — plain `.md` files with YAML frontmatter; readable and editable in any editor without special tooling
- **Human–AI collaborative editing** — designed to work alongside AI agents (Claude, etc.) that can read, create, and edit entries in the same journal via git or Syncthing sync

## Design decisions

### Entry IDs: caretta-id instead of sequential numbers

Each entry filename is prefixed with a [caretta-id](https://github.com/fluo10/caretta-id) — a 7-character BASE32 identifier with decisecond precision (e.g. `123abcd_my_note.md`).

Sequential IDs would collide when a human and an AI agent add entries at the same time in a shared journal synced via git or Syncthing.
caretta-id uses the current Unix time in deciseconds as its value, so two entries created more than 0.1 seconds apart are guaranteed to have different IDs — a collision-free guarantee without any central coordinator.

### File layout: `{year}/{id}_{slug}.md`

Entries are grouped into year directories (e.g. `2026/`) to prevent the journal root from filling up over time, while keeping the hierarchy shallow enough to stay navigable.
The slug derived from the entry title keeps filenames readable even without opening sapphire-journal.

## Data model

Each file is an **Entry** — the primary unit of data. An entry can contain free-form notes, task checkboxes (`- [ ]`), or both.

A note entry:

```markdown
---
id: '1a2b3c4'
title: Meeting notes
created_at: '2026-03-06T14:00:00'
tags: [work]
---

Discussion points from the team meeting.
```

A task entry adds a `task` block:

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

An event entry adds an `event` block:

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

## Installation

### Linux / macOS

```sh
curl -fsSL https://raw.githubusercontent.com/fluo10/sapphire-journal/main/install.sh | sh
```

### Windows

```powershell
irm https://raw.githubusercontent.com/fluo10/sapphire-journal/main/install.ps1 | iex
```

### cargo-binstall

```sh
cargo binstall sapphire-journal
```

### From source

```sh
cargo install sapphire-journal
```

## CLI usage

See [sapphire-journal/README.md](sapphire-journal/README.md) for the full command reference.

A **journal** is any directory tree that contains a `.sapphire-journal/` directory.
`sapphire-journal` locates it by walking up from the current directory, the same way `git` finds `.git/`.
Use `sapphire-journal init` to create one.

## VS Code extension

The `sapphire-journal-vscode` extension integrates with the editor:

- **Auto-fix on save** — runs `entry fix` automatically, keeping filenames and year directories in sync
- **Hierarchical tree view** — entries displayed as a parent-child tree with type icons, ThemeIcon status decorations, period filter, and tree/list toggle
- **Drag-and-drop reparenting** — drag entries in the tree to reassign parent
- **New Entry**, **New Child Entry**, **Open Entry by ID**, **Remove Entry**, **List Entries** commands available from the Command Palette and context menu
- **Rich tooltips** — hover over tree items to see full entry details

Available on the [VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=fluo10.sapphire-journal-vscode) and [Open VSX Registry](https://open-vsx.org/extension/fluo10/sapphire-journal-vscode).
Platform-specific VSIX files (with the CLI binary bundled) are also on the [Releases](https://github.com/fluo10/sapphire-journal/releases) page.

## Project structure

```
sapphire-journal/
├── sapphire-journal-core/    # Data model, Markdown parser/serializer, SQLite cache
├── sapphire-journal/         # Unified binary: CLI + MCP server (sapphire-journal mcp)
├── sapphire-journal-dioxus/  # GUI app (desktop / mobile)
└── sapphire-journal-vscode/  # VS Code extension
```

## Status

Early development — CLI and MCP server are functional for entry management including hierarchy and full-text search. VS Code extension available on the Marketplace and Open VSX.

## License

This repository contains components under different licenses:

| Component | License |
|-----------|---------|
| `sapphire-journal-core` | MIT OR Apache-2.0 |
| `sapphire-journal` | MIT OR Apache-2.0 |
| `sapphire-journal-dioxus` | GPL-3.0-or-later |
| `sapphire-journal-vscode` | MIT |

See the `LICENSE` (or `LICENSE-MIT` / `LICENSE-APACHE`) file in each component's directory for the full license text.
