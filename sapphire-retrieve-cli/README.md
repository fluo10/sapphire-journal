# sapphire-retrieve-cli

CLI tool and MCP server for indexing and searching arbitrary text files, built on [sapphire-retrieve](../sapphire-retrieve).

## Commands

| Command | Description |
|---|---|
| `sync` | Walk the workspace directory and upsert changed files into the FTS index |
| `rebuild` | Drop and recreate the entire index from scratch |
| `embed` | Embed any unembedded documents (requires embedding configured) |
| `info` | Show index statistics and embedding status |
| `clean` | Remove stale cache databases for workspaces that no longer exist |
| `mcp` | Start the MCP server (stdin/stdout transport) |

## MCP tools

| Tool | Description |
|---|---|
| `workspace_info` | Return index statistics for the current workspace |
| `workspace_sync` | Sync the workspace and return a summary |
| `workspace_rebuild` | Rebuild the index from scratch |
| `search` | Full-text and/or semantic search over indexed files |

## Options

```
--workspace-dir <DIR>   Directory to index [env: SAPPHIRE_RECALL_WORKSPACE]
```

When `--workspace-dir` is omitted and stdin is a TTY, you will be asked to confirm using the current directory.

## Configuration

`$XDG_CONFIG_HOME/sapphire-retrieve-cli/config.toml`

```toml
[embedding]
enabled   = true
vector_db = "lancedb"   # "none" | "sqlite_vec" | "lancedb"
provider  = "fastembed"
model     = "BGESmallENV15"
```

## License

MIT OR Apache-2.0
