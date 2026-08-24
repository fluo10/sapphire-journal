# Design: migrate sapphire-journal to sapphire-framework `main` (drop git auto-sync)

Date: 2026-08-10

## Context

`sapphire-journal` pins its framework dependencies (`sapphire-framework-workspace`,
`sapphire-framework-track`) to the stale `feat/framework-migration` branch. That
branch predates the framework's current `main`, which (via framework issue #90)
**removed local-workspace auto-sync**: `SyncBackend`, `GitSync`,
`SyncConfig`/`SyncBackendKind`, the `git-sync` feature, and `AppContext` device
tracking. Concurrent editing is now the central server's job; git is a manual,
files-as-origin concern, and a GUI-integrated git story is a separate future
rebuild (framework #91).

This migration moves journal onto framework `main` and removes its use of those
deleted APIs. journal keeps **git deps** (no crates.io release yet) — a
coordinated release of framework + journal + agent comes only after both
consumers are verified against the framework API.

### Decision: drop journal's git auto-sync now — a deferral, not a permanent stance

journal's multi-device sync today is git: `JournalState::git_sync()`
(commit → pull → push via the framework's `GitSync`), the MCP
`spawn_periodic_git_sync` task + `git_sync` tool, and the desktop periodic
sync + sync button; edits are staged via `stage_file`. This migration
**removes journal's automated git sync** to land on framework `main`; git stays
available manually (journals remain git repositories).

This is a deferral, not a verdict that git sync is gone for good. The intended
sync tiers are:

- **General users** — plain Markdown files in a folder synced by an external
  service (Google Drive, Dropbox, …). No git, no server; the lowest hurdle.
- **Middle tier** — git-based local-workspace sync: sturdier than a synced
  folder, still no server to run. We are deliberately **not** closing the door
  on this. The framework may reintroduce a git sync (framework #91,
  "GUI-integrated git, rebuilt from zero"), at which point journal can re-adopt
  a *framework-provided* git sync rather than vendoring its own.
- **Advanced / concurrent editing** — a self-hosted remote-workspace server.

The immediate priority is remote-workspace sync, but a framework-provided git
sync stays a planned middle-tier option; journal's removal here is temporary.

Note: journal's git **repository** lifecycle is unaffected — `registry.rs`
`init_journal` and the clone dialog use `git2` **directly**, not the framework's
`GitSync`. Only the automated commit/pull/push **cycle** (which went through the
framework `SyncBackend`) is removed.

### Decision: repurpose the periodic task to periodic re-indexing

The periodic git-sync task becomes a **periodic cache re-index**
(`JournalState::sync()`). Rationale: a core strength of journal is that entries
are **plain Markdown files** editable in any external editor; periodically
re-indexing keeps the search cache in step with edits made outside the app.
`sync_interval_minutes` is retained as the re-index interval.

## Scope / non-goals

**In scope**
- Bump framework git deps `feat/framework-migration` → `main`.
- Remove all use of the #90-deleted sync/git APIs.
- Repurpose periodic git sync → periodic re-index; drop the `git_sync` tool/UI.
- Keep git deps; build/test/run journal against framework `main`.

**Out of scope (later / separate)**
- Re-introducing git sync. It is deferred to a future **framework-provided**
  middle-tier option (framework #91), not vendored into journal now.
- Adopting the remote server / remote workspaces as journal's sync.
- Adopting the shared `WorkspaceRegistry` / `WorkspaceManager` (journal keeps
  its own `JournalRegistry` for now).
- crates.io release (framework + journal + agent, coordinated, later).
- agent's migration (separate spec/plan).

## Changes by crate

### `sapphire-journal-core`
- `journal_state.rs`: remove the `sync_backend` field, the two `GitSync::open`
  attach blocks (`#[cfg(feature = "git-sync")]`), and `has_sync_backend()` /
  `stage_file()` / `git_sync()`. Keep `sync()` (cache re-index) and
  `sync_and_embed()`.
- `lib.rs`: remove `pub use sapphire_workspace::{SyncBackend, GitSync}` and the
  `#[cfg(feature = "git-sync")]` re-export.
- `user_config.rs`: remove `SyncConfig`, `SyncBackendKind`, `parse_sync_backend`,
  and the `sync` field + its env override. Keep `sync_interval_minutes` (now the
  re-index interval) and `sync_interval()`.
- `error.rs`: remove `Error::Sync(String)` if it becomes unused.
- `Cargo.toml`: framework deps → `branch = "main"`; delete the `git-sync`
  feature and its default entry.

### `sapphire-journal-mcp`
- `server.rs`: replace `spawn_periodic_git_sync` with a periodic **re-index**
  task (calls `sync()`), renamed accordingly. **Remove the `git_sync` MCP tool**
  (git is now manual; there is no automated sync to trigger). Other tools that
  call `sync()` are unchanged.
- `http.rs`: rewire the handle to the renamed periodic re-index task.

### `sapphire-journal-desktop`
- `settings_panel.rs`: remove the sync-backend (`SyncBackendKind`) picker; keep
  the interval field (now "re-index interval").
- `app.rs` / `screens/journal_home.rs`: the periodic sync + "sync" affordance
  become **re-index** (drop the git leg). Rename user-facing "Sync" → "Refresh"
  where it referred to git.
- `Cargo.toml`: drop the `git-sync` feature wiring if present.

### `sapphire-journal-cli`
- Follow through on any `git-sync`/`SyncConfig` references; cache commands
  already use `sync()` (re-index) and should be unaffected beyond feature flags.

## Verification

- `cargo build --workspace` and `cargo test --workspace` in `sapphire-journal`
  against framework `main` (git dep). Fix compile fallout from the API removals.
- `cargo tree -i libsqlite3-sys` sanity (journal intentionally carries SQLite via
  grain-id/its own cache; confirm no *new* regressions from the framework side —
  framework main is SQLite-free).
- Smoke: launch the desktop app (opens a journal, edit externally → periodic
  re-index picks it up, search finds it); MCP server starts and its tools work;
  CLI cache commands run.
- Confirm the framework's non-sync surface journal relies on is intact on `main`:
  `sapphire_track::{TrackStore, RedbTrackStore, open_redb}`,
  `sapphire_workspace::{Embedder, RetrieveDb, build_embedder, FtsQuery,
  VectorQuery, Document, RetrieveError, EmbeddingConfig, RetrieveConfig, VectorDb,
  path_uuid, lancedb_store, AppContext (set_cache_dir/set_data_dir only)}`.

## Release

Git deps only. No `cargo publish`. A coordinated crates.io release of framework
(facade + internal crates) → journal → agent happens later, once both consumers
are proven stable against the framework API.
