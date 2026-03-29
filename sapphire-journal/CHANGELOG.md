# Changelog (CLI)

All notable changes to `sapphire-journal` and `sapphire-journal-core` are documented here.

## [0.8.0] - 2026-03-29

### Changed

- Project renamed from `archelon` to `sapphire-journal`; all crate names, config/cache directories (`~/.config/sapphire-journal/`, `~/.cache/sapphire-journal/`), and environment variables (`SAPPHIRE_JOURNAL_*`) updated accordingly
- CLI binary renamed from `sapphire-journal` to `sajo`
- ID type renamed from `CarettaId` to `GrainId` (`caretta-id` dependency replaced with `grain-id` v0.14)
- `rusqlite` bumped from 0.37 to 0.39 (required by `grain-id` v0.14)

## [0.7.0] - 2026-03-22

### Added

- `entry_search` MCP tool and VS Code search command for full-text and vector search
- LanceDB vector search backend (`lancedb-store` optional feature)
- sqlite-vec vector search and FTS5 full-text search
- fastembed provider for server-free local embedding (`fastembed-embed` optional feature)
- Versioned cache paths and `cache clean` command
- User-level config and `config show` command
- `JournalState` in `sapphire-journal-core` — unified state holding `Journal`, SQLite connection, vector store, and embedder

### Changed

- Ops API unified to accept `JournalState`; vector store and embedder cached for MCP/GUI reuse
- `VectorStore` trait abstraction with paragraph-level chunking
- Release workflow split into independent CLI and VSCode tracks

### Fixed

- `entry_modify`: parent field now included in "nothing to update" guard
- `VectorDb::LanceDb` serde repr renamed to `"lancedb"`
- fastembed model cache stored under `~/.cache/sapphire-journal/fastembed/`
- lancedb_v1/ placed directly under cache dir (not `lancedb/lancedb_v1/`)
- sqlite-vec loaded via `sqlite3_auto_extension` instead of direct call
- Chunk vectors invalidated when chunk text changes on upsert

## [0.6.0] - 2026-03-17

### Added

- `EntryFlag` and `MatchFlag` enums for structured flag handling; `event_closed` flag detection

### Changed

- MCP: entry references now use structured `EntryRef` instead of a flat string

### Fixed

- `slugify`: unicode characters and hyphens are now preserved in slug generation
- Entry: `slug` and `tags` are always serialized in frontmatter output

## [0.5.0] - 2026-03-16

### Added

- `entries_dir` config field to customize the root directory for entry storage (default: journal root)

### Changed

- Config: `timezone` field removed
- Config: unknown fields are now preserved across read/write (prevents data loss when config was written by a newer version)

## [0.4.0] - 2026-03-15

### Added

- Unified `sapphire-journal` binary combining CLI and MCP server; `sapphire-journal-cli` and `sapphire-journal-mcp` crates removed
- SQLite cache with schema versioning and `cache` subcommands (`info`, `sync`, `rebuild`); FTS5 full-text search index is built but UI-side search is not yet implemented (planned)
- Entry hierarchy: `parent_id` frontmatter field, `--parent` flag for `entry new` / `entry modify`, `--no-parent` flag to clear parent
- `entry tree` command and `entry_tree` MCP tool — displays entries in a parent-child hierarchy (supports same filters as `entry list`)
- Title-based entry lookup: `@title` syntax as an alternative to ID prefix
- Duplicate title detection with configurable `duplicate_title` policy (`warn` | `allow` | `deny`)
- `--task-overdue`, `--task-in-progress`, `--task-unstarted` field selectors for `entry list` / MCP `entry_list`
- `--active` composite flag: matches tasks that are overdue or in-progress
- `--all-periods` flag for `entry list`; period is now a positional argument; extended period keywords
- Symbol system for entry list output: freshness and overdue indicators in a 2-slot prefix

### Changed

- `entry set` renamed to `entry modify` (CLI and MCP tool `entry_set` → `entry_modify`)
- JSON output fields: `status_labels` → `flags`, `match_labels` → `match_flags`
- Period selector no longer falls back to all-fields when any specific selector is explicitly set
- Default `duplicate_title` policy changed from `allow` to `warn`

### Fixed

- `entry fix`: entry is moved to the correct year directory when the year derived from `created_at` differs from its current location
- ID collision retry is now applied to `sync_cache` as well as `create_entry`
- Fullwidth middle dot used instead of fullwidth space as placeholder in `symbols_text`

### Removed

- `sapphire-journal-cli` and `sapphire-journal-mcp` crates (functionality merged into `sapphire-journal`)
- `entry fix --touch` flag

## [0.3.0] - 2026-03-09

### Added

- `task.started_at` timestamp field: auto-set when task status transitions to `in_progress`; supports manual override via `--task-started-at DATETIME` in `entry new` / `entry modify`
- `--task-started` filter for `entry list` (CLI and MCP): matches in-progress tasks (with optional `--period` overlap check)
- Preserve unknown frontmatter fields across round-trips — unknown YAML keys in `Frontmatter`, `TaskMeta`, and `EventMeta` are now retained on read/write, preventing data loss when entries were created by a newer version of sapphire-journal

## [0.2.1] - 2026-03-08

### Fixed

- `entry fix`: repeated calls no longer accumulate blank lines between the frontmatter fence and the body ([#30](https://github.com/fluo10/sapphire-journal/pull/30))

## [0.2.0] - 2026-03-08

### Added

- Install scripts for Linux/macOS (`install.sh`) and Windows (`install.ps1`) — installs pre-built binaries to `~/.local/bin`
- `cargo-binstall` support for `sapphire-journal-cli` and `sapphire-journal-mcp`
- `--version` flag for `sapphire-journal-cli` and `sapphire-journal-mcp`
- `--journal-dir` global option for `sapphire-journal-mcp`
- `entry fix --touch` flag to optionally refresh `updated_at` when fixing an entry
- `entry fix` now syncs `closed_at` based on task status when fixing
- Crate-level documentation for `sapphire-journal-core`

### Changed

- `entry list`: per-field `--FIELD PERIOD` arguments replaced with `--FIELD` boolean selectors that apply a shared `--period` value — simplifies the interface and avoids redundant argument pairs
- `entry list`: `--event-start` / `--event-end` filters replaced with `--event-span`, which matches entries whose event `[start, end]` interval overlaps the given period (in-progress events are included)

### Fixed

- `period`: `overlaps_event` now correctly returns `false` when an entry has no event instead of always matching

## [0.1.1] - 2026-03-07

### Fixed

- `TaskMeta.due` and `TaskMeta.closed_at`: add `#[serde(default)]` to prevent "missing field" errors when these optional fields are absent from the YAML frontmatter

## [0.1.0] - 2026-03-07

### Added

- Initial project structure with workspace crates: `sapphire-journal-core`, `sapphire-journal-cli`, `sapphire-journal-mcp`
- Entry management: create, list, edit, remove commands
- Journal initialization (`sapphire-journal init`) with `.sapphire-journal/` directory discovery (walks up from current directory)
- Entry types: note, task (with status/due), event (with start/end)
- YAML frontmatter with required fields: `id`, `title`, `created_at`
- `id` field using [caretta-id](https://github.com/fluo10/caretta-id) — decisecond-precision BASE32 identifiers for collision-free concurrent editing
- File layout: `{year}/{id}_{slug}.md` for shallow, readable hierarchy
- Auto-rename entry file on create/update to keep filename in sync with title slug
- Entry list filtering: by period (per-field), tags, overdue status
- Entry list sorting
- Entry check/fix/remove commands with shared `EntryRef` type
- `--journal-dir` global option to override journal root
- `--new` flag for `edit` command to create entries in editor
- MCP server (`sapphire-journal-mcp`) for AI agent integration via stdio transport
- CI workflow for pull requests
- Release workflow

### Changed

- Unified title parameter and made frontmatter fields required
- Made `TaskMeta.status` and `EventMeta.start`/`end` required fields
- Minute-precision timestamp serialization
- Moved shared entry operations into `sapphire-journal-core::ops`
- Moved `body` into `EntryFields`
- Unmanaged files excluded from entry list output
