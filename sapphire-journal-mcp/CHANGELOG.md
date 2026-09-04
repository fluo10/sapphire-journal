# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/fluo10/sapphire-journal/compare/mcp-v0.1.0...mcp-v0.2.0) - 2026-09-04

### Added

- *(server)* default the server to port 3172 ([#285](https://github.com/fluo10/sapphire-journal/pull/285))
- *(mcp)* expose entry_new/entry_modify for driving outside the crate
- *(mcp)* expose the MCP service as a Router
- *(mcp)* notify an observer after a tool writes
- [**breaking**] depend on sapphire-framework and drop the SQLite retrieve backend

### Fixed

- *(server)* fold in the whole-branch review's minor findings
- *(server)* wire shutdown, survive a poisoned lock, and stop testing fixtures that never parse
- *(mcp)* widen the Host allowlist so /mcp answers non-loopback clients
- *(mcp)* use EmptyObject for zero-arg tool input schemas

### Other

- *(features)* [**breaking**] drop the lancedb-store feature
- *(mcp)* keep entry_new/entry_modify and their params private again
- *(mcp)* cover entry_fix's rename notification
- *(mcp)* drive real handlers, use tempfile for test journals
- *(mcp)* periodic re-index instead of git sync; drop git_sync tool
- *(mcp)* add LICENSE-MIT and LICENSE-APACHE files
- *(sapphire-journal-mcp)* release v0.1.0

## [0.1.0](https://github.com/fluo10/sapphire-journal/releases/tag/mcp-v0.1.0) - 2026-05-24

### Added

- *(desktop)* expose journal to AI agents via in-process HTTP MCP server

### Other

- *(release-plz)* enable mcp and reset to 0.1.0 for initial release
- *(sapphire-journal-core)* release v0.12.0
- scrub leftover Archelon identifiers in active code
- *(mcp)* convert mcp crate to lib-only and expose via cli `mcp` subcommand
- *(mcp)* drop journal open/close modes for a static tool list
- extract shared frontend helpers from mcp to core
- adopt release-plz for per-crate release cycles
- extract MCP server into sapphire-journal-mcp crate
