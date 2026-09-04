# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.0](https://github.com/fluo10/sapphire-journal/compare/core-v0.12.0...core-v0.13.0) - 2026-09-04

### Added

- [**breaking**] depend on sapphire-framework and drop the SQLite retrieve backend

### Fixed

- remove leftover lancedb artifacts and add missing comment
- *(server)* stop letting a stale push delete the entry it collided with
- *(core)* guard EntryRef::parse against an upstream grain-id panic on non-ASCII
- *(core)* don't mint a new id when re-upserting a renamed entry
- *(core)* accept CRLF frontmatter when parsing entries

### Other

- *(core)* [**breaking**] remove remaining lancedb references
- *(features)* [**breaking**] drop the lancedb-store feature
- *(core)* cover increment_until_free's id-collision check; use try_exists
- drop stale git-sync references (removed tool + comment)
- move framework deps to main
- *(core)* drop SyncConfig from user config
- *(core)* remove git/sync backend (framework #90)

## [0.12.0](https://github.com/fluo10/sapphire-journal/compare/core-v0.11.1...core-v0.12.0) - 2026-05-24

### Added

- *(sync)* enable periodic sync by default and surface settings to VS Code
- *(deps)* upgrade sapphire-workspace to 0.9.0

### Fixed

- *(core)* [**breaking**] initialise AppContext at startup, bump sapphire-workspace to 0.11

### Other

- *(deps)* bump sapphire-workspace to 0.12.1
- *(deps)* bump sapphire-workspace to 0.12.0
- scrub leftover Archelon identifiers in active code
- extract shared frontend helpers from mcp to core
- adopt release-plz for per-crate release cycles
- *(deps)* bump grain-id from 0.14 to 0.15 (closes #188)
- Merge pull request #189 from fluo10/dependabot/cargo/sapphire-workspace-0.10.1
- Merge pull request #185 from fluo10/docs/config-examples
