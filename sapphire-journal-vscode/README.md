# Sapphire Journal for VS Code

VS Code extension for the [Sapphire Journal](https://github.com/fluo10/sapphire-journal) journal.

## Features

### Entries Panel

An **Entries** panel in the Activity Bar shows all journal entries as a tree or flat list.

- **Tree / List view** — toggle between hierarchical and flat view with the toolbar buttons.
- **Filter** (`Filter Entries`) — filter entries by title, tag (`#work`), or ID.
- **Period filter** (`Filter by Period`) — filter entries by date: today, this week, this month, custom range, etc.
- **Sort** (`Sort Entries`) — sort by ID, title, updated/created date, task status, task due, or event start.
- **Refresh** (`Refresh`) — manually refresh the entry list.
- **Drag and drop** — reparent entries by dragging them in the tree.

### Commands

| Command | Description |
|---|---|
| `Sapphire Journal: New Entry` | Create a new journal entry and open it in the editor. |
| `Sapphire Journal: New Child Entry` | Create a child entry under the selected entry. |
| `Sapphire Journal: Open Entry by ID` | Open an entry by ID or ID prefix. |
| `Sapphire Journal: Remove Entry` | Remove the active entry (or one specified by ID) after confirmation. |
| `Sapphire Journal: List Entries` | Show all entries in a Quick Pick and open the selected one. |
| `Sapphire Journal: Initialize Journal` | Run `sapphire-journal init` in the workspace root to set up a new journal. |
| `Sapphire Journal: Rebuild Cache` | Run `sapphire-journal cache rebuild` to drop and reconstruct the local SQLite cache. |

### Auto-fix on Save

When saving a managed entry file, `entry fix` is run automatically: timestamps are updated and the filename is normalized. If the file is renamed, the new file is opened and the old tab is closed. Can be disabled via `sapphire-journal.autoFixOnSave`.

## Requirements

The `sapphire-journal` CLI binary must be available. When installing from a platform-specific VSIX, the binary is bundled automatically. Otherwise, install it separately and set `sapphire-journal.binaryPath` if needed.

## Extension Settings

| Setting | Default | Description |
|---|---|---|
| `sapphire-journal.binaryPath` | `""` | Path to the sapphire-journal binary. Leave empty to use the bundled binary or `sapphire-journal` on `$PATH`. |
| `sapphire-journal.autoFixOnSave` | `true` | Automatically run `entry fix` when saving a managed entry file. |
| `sapphire-journal.defaultPeriod` | `"today"` | Default period filter for the Entries panel. Accepted values: `today`, `this_week`, `this_month`, `YYYY-MM-DD`, `YYYY-MM-DD,YYYY-MM-DD`, or empty to disable. |
| `sapphire-journal.defaultSortField` | `"updated_at"` | Default sort field for the Entries panel. |
| `sapphire-journal.defaultSortOrder` | `"desc"` | Default sort direction for the Entries panel (`asc` or `desc`). |
