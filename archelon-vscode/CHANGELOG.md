# Changelog (VSCode Extension)

All notable changes to `archelon-vscode` are documented here.

## [0.6.0] - 2026-03-17

### Changed

- `New Entry` command now creates a sibling entry relative to the currently selected entry; task-related icons unified

## [0.5.0] - 2026-03-16

### Added

- `archelon init` command to initialize a new journal from the editor
- Cache rebuild command (`archelon cache rebuild`) accessible from the Command Palette

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
