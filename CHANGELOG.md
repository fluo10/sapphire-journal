# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] - 2026-03-07

### Added

- Initial project structure with workspace crates: `archelon-core`, `archelon-cli`, `archelon-mcp`
- Entry management: create, list, edit, remove commands
- Journal initialization (`archelon init`) with `.archelon/` directory discovery (walks up from current directory)
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
- MCP server (`archelon-mcp`) for AI agent integration via stdio transport
- CI workflow for pull requests
- Release workflow

### Changed

- Unified title parameter and made frontmatter fields required
- Made `TaskMeta.status` and `EventMeta.start`/`end` required fields
- Minute-precision timestamp serialization
- Moved shared entry operations into `archelon-core::ops`
- Moved `body` into `EntryFields`
- Unmanaged files excluded from entry list output
