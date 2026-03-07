# archelon-cli

Command-line interface for [archelon](https://github.com/fluo10/archelon) — a Markdown-based task and note manager.

## Installation

```bash
cargo install --path .
```

## Usage

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

## License

MIT OR Apache-2.0
