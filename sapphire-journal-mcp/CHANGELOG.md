# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
