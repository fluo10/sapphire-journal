# Changelog (VSCode Extension)

All notable changes to `sapphire-journal-vscode` are documented here.

## [0.8.0] - 2026-03-30

### Added

- VSCode settings (`sapphire-journal.cache.vectorDb`, `sapphire-journal.cache.embedding.*`) can now override `config.toml` values, allowing per-workspace embedding and vector DB configuration from the Settings UI

### Changed

- Extension renamed from `archelon-vscode` to `sapphire-journal-vscode`; extension ID changed to `fluo10.sapphire-journal`
- CLI binary renamed from `archelon` to `sajo`

## [0.7.0] - 2026-03-22

### Added

- Search Entries command powered by `entry_search` MCP tool (full-text and vector search)

### Changed

- Migrated to local MCP server via stdio; no longer requires a separately running MCP process

### Fixed

- Workspace root is now used as the journal root instead of the active file's directory
- `getChildren` errors are now surfaced in the tree view
- Entry tools now receive `EntryRef` objects instead of plain strings

## [0.6.0] - 2026-03-17

### Changed

- `New Entry` command now creates a sibling entry relative to the currently selected entry; task-related icons unified

## [0.5.0] - 2026-03-16

### Added

- `sapphire-journal init` command to initialize a new journal from the editor
- Cache rebuild command (`sapphire-journal cache rebuild`) accessible from the Command Palette

## [0.4.0] - 2026-03-15

### Added

- ThemeIcon decorations in tree view replacing emoji prefixes
- Entry type icons, period filter, and view improvements
- Context menu, rich tooltips, and tree/list toggle in the sidebar
- Drag-and-drop reparenting in the tree view
- `new-child-entry` command
- Published to VS Code Marketplace and Open VSX Registry

### Changed

- `--active` flag used by default in `listEntries` and `treeEntries`

### Fixed

- Installed extension is disabled in the dev host to prevent conflicts

## [0.2.0] - 2026-03-08

### Added

- Initial release: auto-fix on save, New Entry, Open Entry, Remove Entry, and List Entries commands; CLI binary bundled in platform-specific VSIX
