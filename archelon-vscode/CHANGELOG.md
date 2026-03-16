# Change Log

All notable changes to the "archelon-vscode" extension will be documented in this file.

Check [Keep a Changelog](http://keepachangelog.com/) for recommendations on how to structure this file.

## [0.6.0] - 2026-03-17

### Changed

- `New Entry` command now creates a sibling entry relative to the currently selected entry
- Bundled CLI binary updated to v0.6.0: `EntryFlag`/`MatchFlag` enums, structured `EntryRef` in MCP, unicode/hyphen-preserving slugify, and always-serialized `slug`/`tags` in frontmatter

## [0.3.0] - 2026-03-09

### Added

- Bundled CLI binary updated to v0.3.0: `task.started_at` timestamp with `--task-started` filter, and preservation of unknown frontmatter fields

## [0.2.1] - 2026-03-08

### Fixed

- Bundled CLI binary updated to v0.2.1: repeated `entry fix` calls no longer accumulate blank lines between the frontmatter fence and the body

## [0.2.0] - 2026-03-08

- Initial release