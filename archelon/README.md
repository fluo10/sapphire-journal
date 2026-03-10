# archelon

Unified binary for [archelon](https://github.com/fluo10/archelon) — a Markdown-based task and note manager.
Provides both the CLI for humans and an MCP server for AI agents.

## Installation

### Install script (Linux / macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/fluo10/archelon/main/install.sh | sh
```

### Install script (Windows)

```powershell
irm https://raw.githubusercontent.com/fluo10/archelon/main/install.ps1 | iex
```

### From crates.io

```bash
cargo install archelon
```

### From source

```bash
cargo install --path .
```

---

## CLI usage

### Initialize a journal

```bash
archelon init [PATH]
```

Creates `.archelon/config.toml` with the detected local timezone and `.archelon/.gitignore`.

### Global options

```bash
# Override journal root (also settable via ARCHELON_JOURNAL_DIR env var)
archelon --journal-dir /path/to/journal <command>
```

### Entry commands

#### Create a new entry

```bash
archelon entry new --title <TITLE> [--body "body text"] [OPTIONS]
```

Options:
- `--slug SLUG` — override the filename slug
- `--tags tag1,tag2` — set tags
- `--task-due DATETIME` — set task due date
- `--task-status STATUS` — set task status (`open` | `in_progress` | `done` | `cancelled` | `archived`)
- `--event-start DATETIME`, `--event-end DATETIME`

The filename is auto-generated as `{year}/{caretta-id}_{slug}.md`.

#### Create and edit in $EDITOR

```bash
archelon entry edit --new
```

Opens `$EDITOR` (`$VISUAL` → `$EDITOR` → `vi`) with a pre-filled frontmatter template. On save, the filename is adjusted to match the title.

#### List entries

```bash
archelon entry list [PATH] [OPTIONS]
```

Timestamp filters (OR'd across fields):

```bash
--period PERIOD               # shorthand: applies to all timestamp fields
--task-due PERIOD             # filter by task due date
--event-span PERIOD           # filter by event span overlap (in-progress events included)
--created-at PERIOD           # filter by created_at
--updated-at PERIOD           # filter by updated_at
--overdue                     # include tasks whose due date is past and not closed
```

PERIOD formats: `today` | `this_week` | `this_month` | `YYYY-MM-DD` | `YYYY-MM-DD,YYYY-MM-DD` | `YYYY-MM-DDTHH:MM,YYYY-MM-DDTHH:MM`

Other filters (AND'd on top of timestamp filters):

```bash
--task-status open,in_progress   # comma-separated status values
--tags work,urgent               # entry must have ALL specified tags
```

Sort:

```bash
--sort-by FIELD    # id | title | task_status | created_at | updated_at | task_due | event_start | event_end
--sort-order asc   # or desc (default: asc)
```

Output:

```bash
--json   # output all matching entries as JSON (metadata + body)
```

#### Display entries as a tree

```bash
archelon entry tree [PATH] [OPTIONS]
```

Displays entries in a parent-child hierarchy based on `parent_id` in frontmatter.
Supports the same filter and sort options as `entry list`.

```bash
--json   # output the tree as JSON (nested children arrays)
```

#### Show an entry

```bash
archelon entry show <file-or-id>
```

#### Edit an entry

```bash
archelon entry edit <file-or-id>
```

Opens the entry in `$EDITOR`.

#### Update frontmatter fields

```bash
archelon entry set <file-or-id> --title "New title"
archelon entry set <file-or-id> --tags work,backend
archelon entry set <file-or-id> --tags          # clear all tags
archelon entry set <file-or-id> --task-status done
```

When `--task-status` is set to `done`, `cancelled`, or `archived`, `closed_at` is set automatically.

#### Check and fix filename

```bash
archelon entry check <file-or-id>   # report any filename/frontmatter mismatches
archelon entry fix <file-or-id>     # rename file to match frontmatter
```

#### Remove an entry

```bash
archelon entry remove <file-or-id>
```

### Cache commands

```bash
archelon cache info       # show cache status and statistics
archelon cache sync       # incrementally update the cache
archelon cache rebuild    # drop and rebuild the cache from scratch
```

### DATETIME format

`YYYY-MM-DD` or `YYYY-MM-DDTHH:MM`.

For deadline/end timestamps (`--task-due`, `--event-end`), date-only input is interpreted as `23:59`.
For start/close timestamps (`--event-start`, `--task-closed-at`), date-only input is interpreted as `00:00`.

### Journal configuration

`.archelon/config.toml`:

```toml
[journal]
timezone = "Asia/Tokyo"   # IANA timezone name
week_start = "monday"     # or "sunday" — used by this_week period
```

---

## MCP server

`archelon mcp` launches an MCP (Model Context Protocol) server over stdio, letting AI agents (Claude, etc.) read and write journal entries.

### Start the server

```bash
archelon mcp
```

### Environment variables

| Variable | Description |
|---|---|
| `ARCHELON_JOURNAL_DIR` | Path to the journal root. If not set, the server walks up from the current directory to find `.archelon/`. |

### Example: Claude Desktop

```json
{
  "mcpServers": {
    "archelon": {
      "command": "archelon",
      "args": ["mcp"],
      "env": {
        "ARCHELON_JOURNAL_DIR": "/path/to/your/journal"
      }
    }
  }
}
```

### Available tools

| Tool | Description |
|---|---|
| `journal_init` | Initialize a new archelon journal |
| `entry_list` | List entries as JSON with filtering and sorting |
| `entry_tree` | List entries as a nested JSON tree (parent-child hierarchy) |
| `entry_show` | Show the contents of an entry by ID prefix or file path |
| `entry_new` | Create a new journal entry |
| `entry_set` | Update frontmatter fields of an existing entry |
| `entry_check` | Validate an entry's frontmatter and filename |
| `entry_fix` | Rename an entry file to match its frontmatter |
| `entry_remove` | Delete an entry file |
| `cache_info` | Show cache status and statistics |
| `cache_sync` | Incrementally update the SQLite cache |
| `cache_rebuild` | Drop and rebuild the cache from scratch |

### `entry_list` parameters

Timestamp filters are **OR'd** across fields; `task_status` and `tags` are **AND'd** on top.

| Parameter | Description |
|---|---|
| `period` | Shorthand: applies to all timestamp fields |
| `task_due` | Filter by task due date |
| `event_span` | Filter by event span overlap: matches entries whose event [start, end] overlaps the period |
| `created_at` | Filter by created_at |
| `updated_at` | Filter by updated_at |
| `overdue` | Include tasks whose due date is past and not closed |
| `task_status` | Array of statuses to include, e.g. `["open", "in_progress"]` |
| `tags` | Array of tags; entry must have ALL specified tags |
| `sort_by` | Field to sort by: `id` \| `title` \| `task_status` \| `created_at` \| `updated_at` \| `task_due` \| `event_start` \| `event_end` |
| `sort_order` | `"asc"` (default) or `"desc"` |

PERIOD format: `today` \| `this_week` \| `this_month` \| `YYYY-MM-DD` \| `YYYY-MM-DD,YYYY-MM-DD` \| `YYYY-MM-DDTHH:MM,YYYY-MM-DDTHH:MM`

## License

MIT OR Apache-2.0
