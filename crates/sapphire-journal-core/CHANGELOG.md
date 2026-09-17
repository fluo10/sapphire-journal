# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.0](https://github.com/fluo10/sapphire-journal/compare/core-v0.12.0...core-v0.13.0) - 2026-09-17

### Added

- *(core,cli,mcp)* add stale/hidden filter with opt-in restoration (issue #288)

### Fixed

- adapt cache change-detection to framework FileStamp API
- *(core)* update include_str docs/config path after crate move

### Other

- *(core)* centralize stale threshold in STALE_AFTER_DAYS_DEFAULT and default EntryFilter to 30
- *(core)* add unit tests for stale/hidden filter gate
- *(deps)* switch sapphire-framework from git/branch to crates.io version
- adopt repo-layout and language conventions; add CONTRIBUTING and README.ja

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
