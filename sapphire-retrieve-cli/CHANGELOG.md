# Changelog (sapphire-retrieve)

All notable changes to `sapphire-retrieve-cli` and `sapphire-retrieve` are documented here.

## [0.1.0] - 2026-03-29

### Added

- Initial release of `sapphire-retrieve` library and `sapphire-retrieve` binary
- FTS5 trigram full-text search and vector search (sqlite-vec) over arbitrary text files
- LanceDB vector search backend (`lancedb-store` optional feature)
- Pluggable embedder backends: OpenAI, Ollama, and fastembed (`fastembed-embed` optional feature)
- Text chunker for paragraph-level indexing
- `sync` command — incrementally index new and modified files in the workspace
- `rebuild` command — drop and rebuild the index from scratch
- `embed` command — generate embeddings for documents not yet indexed
- `info` command — display index location, schema version, and document count
- `clean` command — remove stale index files from previous schema versions
- `mcp` command — start an MCP server (stdio transport) for AI agent integration
- `--workspace-dir` global flag and `SAPPHIRE_RETRIEVE_WORKSPACE` environment variable to specify the target directory
