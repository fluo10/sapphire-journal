# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
