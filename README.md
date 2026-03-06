# archelon

Markdown-based task and note manager for humans and AI agents — your data lives in plain text, timeless like fossils.

## Concept

- **Markdown as source of truth** — all data lives in plain `.md` files you can read and edit with any tool
- **SQLite as cache** — fast querying and indexing on top of the Markdown files (planned)
- **Bullet-journal inspired** — tasks, events, and notes are equal peers, each living as its own entry; just as a bullet journal treats every bullet (task, event, or note) uniformly, archelon treats every entry the same way regardless of type
- **Text-editor / IDE compatible** — plain `.md` files with YAML frontmatter; readable and editable in any editor without special tooling
- **Human–AI collaborative editing** — designed to work alongside AI agents (Claude, etc.) that can read, create, and edit entries in the same journal via git or Syncthing sync

## Design decisions

### Entry IDs: caretta-id instead of sequential numbers

Each entry filename is prefixed with a [caretta-id](https://github.com/fluo10/caretta-id) — a 7-character BASE32 identifier with decisecond precision (e.g. `123abcd_my_note.md`).

Sequential IDs would collide when a human and an AI agent add entries at the same time in a shared journal synced via git or Syncthing.
caretta-id uses the current Unix time in deciseconds as its value, so two entries created more than 0.1 seconds apart are guaranteed to have different IDs — a collision-free guarantee without any central coordinator.

### File layout: `{year}/{id}_{slug}.md`

Entries are grouped into year directories (e.g. `2026/`) to prevent the journal root from filling up over time, while keeping the hierarchy shallow enough to stay navigable.
The slug derived from the entry title keeps filenames readable even without opening archelon.

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

## CLI usage

See [archelon-cli/README.md](archelon-cli/README.md) for the full command reference.

A **journal** is any directory tree that contains a `.archelon/` directory.
`archelon` locates it by walking up from the current directory, the same way `git` finds `.git/`.
Use `archelon init` to create one.

## Project structure

```
archelon/
├── archelon-core/   # Data model, Markdown parser/serializer, (future) SQLite cache
├── archelon-cli/    # CLI binary built with clap
└── archelon-mcp/    # MCP server for AI agent integration
```

## Status

Early development — CLI is functional for basic entry management.
SQLite caching is planned.

## License

MIT OR Apache-2.0
