# Archelon for VS Code

VS Code extension for the [Archelon](https://github.com/fluo10/archelon) journal.

## Features

- **Auto-fix on save** — automatically runs `entry fix --touch` when saving a managed entry, updating timestamps and normalizing the filename.
- **New Entry** (`Archelon: New Entry`) — creates a new journal entry and opens it in the editor.
- **Open Entry** (`Archelon: Open Entry by ID`) — opens an entry by ID or ID prefix.
- **Remove Entry** (`Archelon: Remove Entry`) — removes the active entry (or one specified by ID) after confirmation.
- **List Entries** (`Archelon: List Entries`) — shows all entries in a Quick Pick and opens the selected one.

## Requirements

The `archelon` CLI binary must be available. When installing from a platform-specific VSIX, the binary is bundled automatically. Otherwise, install it separately and set `archelon.binaryPath` if needed.

## Extension Settings

| Setting | Default | Description |
|---|---|---|
| `archelon.binaryPath` | `"archelon"` | Path to the archelon binary. Defaults to `archelon` on `$PATH`. |
| `archelon.autoFixOnSave` | `true` | Automatically run `entry fix --touch` when saving a managed entry file. |
