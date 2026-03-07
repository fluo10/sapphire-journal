# archelon-mcp

MCP (Model Context Protocol) server for [archelon](https://github.com/fluo10/archelon) — lets AI agents read and write journal entries over stdio.

## Usage

The server communicates over stdin/stdout using the MCP protocol. Run it as a subprocess from your MCP host configuration.

### Environment variables

| Variable | Description |
|---|---|
| `ARCHELON_JOURNAL_DIR` | Path to the journal root. If not set, the server walks up from the current directory to find `.archelon/`. |

### Example: Claude Desktop

```json
{
  "mcpServers": {
    "archelon": {
      "command": "archelon-mcp",
      "env": {
        "ARCHELON_JOURNAL_DIR": "/path/to/your/journal"
      }
    }
  }
}
```

## Available tools

| Tool | Description |
|---|---|
| `journal_init` | Initialize a new archelon journal |
| `entry_list` | List entries as JSON with filtering and sorting |
| `entry_show` | Show the contents of an entry by ID prefix or file path |
| `entry_new` | Create a new journal entry |
| `entry_set` | Update frontmatter fields of an existing entry |
| `entry_check` | Validate an entry's frontmatter and filename |
| `entry_fix` | Rename an entry file to match its frontmatter |
| `entry_remove` | Delete an entry file |

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
