# sapphire-journal → framework `main` migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move sapphire-journal onto sapphire-framework `main` by removing its use of the sync/git APIs deleted in framework #90, and repurposing its periodic git-sync into periodic re-indexing.

**Architecture:** This is a removal/refactor, not a feature — there are no new tests to write; each task's verification is that the crate still **compiles** (and existing tests pass). Removing the sync/git usage is forward-compatible (it builds against both the currently-pinned framework and `main`), so tasks 1–5 strip usage while the dep still points at the old branch, and task 6 flips the dep to `main` and rebuilds the whole workspace — the point where any unrelated API drift surfaces and is fixed. Task 7 is manual smoke verification.

**Tech Stack:** Rust workspace (journal-core, journal-cli, journal-desktop, journal-mcp), git deps on `sapphire-framework-*`, eframe/egui 0.34, tokio, rmcp (MCP).

## Global Constraints

- Work happens **only** in the `sapphire-journal` submodule, on branch `refactor/migrate-to-framework-main`. Commit there; no superproject commits.
- **Git deps only — no `cargo publish`.** Framework deps stay `git = "…", branch = "…"`.
- Do **not** adopt the remote server, the shared `WorkspaceRegistry`/`WorkspaceManager`, or vendor a git sync. Git-sync reintroduction is deferred to a future framework-provided option (framework #91).
- journal's git **repository** lifecycle (`registry.rs` `init_journal`, the clone dialog — both use `git2` directly) is **unchanged**.
- Keep `sync_interval_minutes` (now the re-index interval) and `JournalState::sync()` (cache re-index) and `sync_and_embed()`.
- After each task, the touched crate(s) must build against the **currently pinned** framework dep (tasks 1–5); the dep flip is task 6.

---

### Task 1: Strip sync/git from `journal-core` state + re-exports + feature

**Files:**
- Modify: `sapphire-journal-core/src/journal_state.rs`
- Modify: `sapphire-journal-core/src/lib.rs`
- Modify: `sapphire-journal-core/src/error.rs`
- Modify: `sapphire-journal-core/Cargo.toml`

- [ ] **Step 1: Remove the sync backend from `journal_state.rs`**
  - In the `use sapphire_workspace::{Embedder, RetrieveDb, SyncBackend};` import, drop `SyncBackend`.
  - Delete the field `sync_backend: Option<Box<dyn SyncBackend + Send + Sync>>,`.
  - In both `JournalState` constructors, delete the `sync_backend: None,` initializer **and** the following block (appears twice):
    ```rust
    #[cfg(feature = "git-sync")]
    if let Ok(git) = sapphire_workspace::GitSync::open(&state.journal.root) {
        state.sync_backend = Some(Box::new(git));
    }
    ```
  - Delete the methods `has_sync_backend()`, `stage_file()` (the one calling `sync.add_file`), and `git_sync()` (the one calling `sync.sync()`). Keep `sync()` and `sync_and_embed()`.

- [ ] **Step 2: Remove sync re-exports from `lib.rs`**
  - Delete `pub use sapphire_workspace::SyncBackend;` and the `#[cfg(feature = "git-sync")] pub use sapphire_workspace::GitSync;` lines.

- [ ] **Step 3: Remove the now-unused error variant**
  - In `error.rs`, if `Error::Sync(String)` is only referenced by the deleted `stage_file`/`git_sync`, delete that variant. Verify with:
    Run: `grep -rn "Error::Sync\|Sync(" sapphire-journal-core/src` — expect no remaining producers.

- [ ] **Step 4: Drop the `git-sync` feature**
  - In `sapphire-journal-core/Cargo.toml`, remove `"git-sync"` from `default = [...]` and delete the `git-sync = ["sapphire-workspace/git-sync"]` line.

- [ ] **Step 5: Verify it compiles (against the currently-pinned framework)**
  Run: `cargo build -p sapphire-journal-core`
  Expected: builds; no reference to `SyncBackend`/`GitSync`/`has_sync_backend`/`stage_file`/`git_sync`.

- [ ] **Step 6: Commit**
  ```bash
  git add sapphire-journal-core/src/journal_state.rs sapphire-journal-core/src/lib.rs sapphire-journal-core/src/error.rs sapphire-journal-core/Cargo.toml
  git commit -m "refactor(core): remove git/sync backend (framework #90)"
  ```

---

### Task 2: Remove sync config from `journal-core` user config

**Files:**
- Modify: `sapphire-journal-core/src/user_config.rs`

**Interfaces:**
- Produces: `UserConfig` **without** a `sync` field or `SyncConfig`/`SyncBackendKind` re-exports; `sync_interval_minutes` and `sync_interval()` remain (re-index interval, consumed by tasks 3–4).

- [ ] **Step 1: Remove the sync config surface**
  - In `pub use sapphire_workspace::{EmbeddingConfig, RetrieveConfig, SyncBackendKind, SyncConfig, VectorDb};`, drop `SyncBackendKind` and `SyncConfig`.
  - Delete the struct field `pub sync: SyncConfig,` and its `sync: SyncConfig::default(),` initializer in `Default`.
  - Delete the env-override branch that calls `parse_sync_backend(...)` and the `fn parse_sync_backend(...)` helper.
  - Keep `sync_interval_minutes`, its default, and `sync_interval()`.

- [ ] **Step 2: Verify**
  Run: `cargo build -p sapphire-journal-core` and `cargo test -p sapphire-journal-core`
  Expected: builds; existing config tests pass (adjust any test asserting a `sync` field — remove that assertion).

- [ ] **Step 3: Commit**
  ```bash
  git add sapphire-journal-core/src/user_config.rs
  git commit -m "refactor(core): drop SyncConfig from user config"
  ```

---

### Task 3: Repurpose the MCP periodic task to re-index; remove the git_sync tool

**Files:**
- Modify: `sapphire-journal-mcp/src/server.rs`
- Modify: `sapphire-journal-mcp/src/http.rs`

**Interfaces:**
- Produces: `spawn_periodic_reindex(state) -> Option<JoinHandle<()>>` (replaces `spawn_periodic_git_sync`), calling `guard.sync()`.

- [ ] **Step 1: Rename + repurpose the periodic task**
  - In `server.rs`, rename `spawn_periodic_git_sync` → `spawn_periodic_reindex`; inside the loop replace `guard.git_sync()` with `guard.sync()` and update the log message ("periodic re-index failed"). Keep the `sync_interval()` gate.
  - Update the internal call site (`let _sync_handle = spawn_periodic_git_sync(...)`) to the new name.

- [ ] **Step 2: Remove the `git_sync` MCP tool**
  - Delete the `fn git_sync(&self, _: Parameters<EmptyObject>) -> Result<String, String>` tool method and its registration/attribute (`#[tool(...)]` / router entry). Leave the other tools (which call `sync()`) intact.

- [ ] **Step 3: Rewire `http.rs`**
  - Update the `use crate::server::{…, spawn_periodic_git_sync, …}` import and the `let sync_handle = spawn_periodic_git_sync(shared_state);` call to `spawn_periodic_reindex`. Update the shutdown comment ("periodic re-index task").

- [ ] **Step 4: Verify**
  Run: `cargo build -p sapphire-journal-mcp`
  Expected: builds; no `git_sync`/`spawn_periodic_git_sync` remain (`grep -rn "git_sync" sapphire-journal-mcp/src`).

- [ ] **Step 5: Commit**
  ```bash
  git add sapphire-journal-mcp/src/server.rs sapphire-journal-mcp/src/http.rs
  git commit -m "refactor(mcp): periodic re-index instead of git sync; drop git_sync tool"
  ```

---

### Task 4: Desktop — remove sync-backend UI; sync affordances become re-index

**Files:**
- Modify: `sapphire-journal-desktop/src/screens/settings_panel.rs`
- Modify: `sapphire-journal-desktop/src/app.rs`
- Modify: `sapphire-journal-desktop/src/screens/journal_home.rs`
- Modify: `sapphire-journal-desktop/Cargo.toml`

- [ ] **Step 1: Remove the sync-backend picker from settings**
  - In `settings_panel.rs`, delete the `use …user_config::SyncBackendKind` import, the `sync_backend: String` field + its initialization, the picker widget, and the save-time `SyncBackendKind` mapping. Keep the `sync_interval_minutes` field and relabel it "Re-index interval (minutes)".

- [ ] **Step 2: Point the desktop "sync" at re-indexing**
  - In `app.rs`/`journal_home.rs`, wherever the periodic task or the "Sync" button invoked git sync, call the state's `sync()` (re-index) instead. Rename user-facing "Sync" labels to "Refresh". Keep `sync_in_progress` / `sync_interval()` wiring (now driving re-index). Remove any `git-sync`-gated code.

- [ ] **Step 3: Drop the `git-sync` feature wiring**
  - In `sapphire-journal-desktop/Cargo.toml`, remove any `git-sync` feature entry / passthrough to `sapphire-journal-core`.

- [ ] **Step 4: Verify**
  Run: `cargo build -p sapphire-journal-desktop`
  Expected: builds; `grep -rn "git-sync\|SyncBackendKind\|git_sync" sapphire-journal-desktop/src` is empty.

- [ ] **Step 5: Commit**
  ```bash
  git add sapphire-journal-desktop/src sapphire-journal-desktop/Cargo.toml
  git commit -m "refactor(desktop): remove sync-backend UI; Sync -> Refresh (re-index)"
  ```

---

### Task 5: CLI follow-through

**Files:**
- Modify: `sapphire-journal-cli/**` (only where needed)

- [ ] **Step 1: Find & remove any residual references**
  Run: `grep -rn "git-sync\|SyncConfig\|SyncBackendKind\|git_sync\|has_sync_backend\|stage_file" sapphire-journal-cli`
  Remove any `git-sync` feature entries in `sapphire-journal-cli/Cargo.toml` and drop/replace any command wired to git sync (cache commands already use `sync()` and need no change).

- [ ] **Step 2: Verify**
  Run: `cargo build -p sapphire-journal-cli`
  Expected: builds.

- [ ] **Step 3: Commit** (skip if nothing changed)
  ```bash
  git add sapphire-journal-cli
  git commit -m "refactor(cli): drop residual git-sync references"
  ```

---

### Task 6: Flip the framework dependency to `main` and rebuild everything

**Files:**
- Modify: `sapphire-journal-core/Cargo.toml`
- Modify: `Cargo.lock`

- [ ] **Step 1: Point framework deps at `main`**
  - In `sapphire-journal-core/Cargo.toml`, change both `sapphire-workspace` and `sapphire-track` from `branch = "feat/framework-migration"` to `branch = "main"`.
  - Run: `cargo update -p sapphire-framework-workspace -p sapphire-framework-track`

- [ ] **Step 2: Build the whole workspace against `main`**
  Run: `cargo build --workspace`
  Expected: builds. Fix any fallout from framework API drift between the old branch and `main`. The non-sync surface journal relies on is intact on `main` (confirm as needed):
  `sapphire_track::{TrackStore, RedbTrackStore, open_redb}`;
  `sapphire_workspace::{Embedder, RetrieveDb, build_embedder, FtsQuery, VectorQuery, Document, RetrieveError, EmbeddingConfig, RetrieveConfig, VectorDb, path_uuid, lancedb_store, AppContext (set_cache_dir/set_data_dir only)}`.

- [ ] **Step 3: Run the tests**
  Run: `cargo test --workspace`
  Expected: pass. Fix any test that asserted removed sync behavior.

- [ ] **Step 4: SQLite sanity (no *new* regressions from the framework side)**
  Run: `cargo tree -i libsqlite3-sys`
  Expected: any SQLite comes from journal's own `grain-id`/cache stack, not from a framework crate (framework `main` is SQLite-free).

- [ ] **Step 5: Commit**
  ```bash
  git add sapphire-journal-core/Cargo.toml Cargo.lock
  git commit -m "build: move framework deps to main"
  ```

---

### Task 7: Smoke verification

**Files:** none (verification only)

- [ ] **Step 1: Desktop** — `cargo run -p sapphire-journal-desktop`: open/create a journal; edit an entry `.md` in an external editor; confirm the periodic re-index (or Refresh button) picks it up and search finds the change. No git-sync UI present.
- [ ] **Step 2: MCP** — start the MCP server (stdio or `serve_http`); confirm it starts, the periodic re-index task runs, and tools work; the `git_sync` tool is gone.
- [ ] **Step 3: CLI** — run the cache/sync command; confirm re-index works.
- [ ] **Step 4:** If any smoke step required a fix, commit it with a clear message.

---

## Self-Review

- **Spec coverage:** journal-core state/lib/error/Cargo (Task 1–2, 6); mcp server/http (Task 3); desktop settings/app/home/Cargo (Task 4); cli (Task 5); dep flip + verify (Task 6); smoke (Task 7). Periodic-reindex repurpose (Task 3–4). Git deps only / no publish (Global Constraints, Task 6). ✅
- **Placeholder scan:** each task names the exact symbols/files to change and a concrete verify command. Task 6 Step 2 says "fix any fallout" — that's inherent to a dep bump; the confirm-list bounds it. ✅
- **Type consistency:** `spawn_periodic_git_sync` → `spawn_periodic_reindex` used consistently in server.rs and http.rs (Task 3). `sync()`/`sync_interval()`/`sync_and_embed()` names match the retained core API. ✅
