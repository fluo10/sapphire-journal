//! Core library for **sapphire-journal** — a Markdown-based task and note manager that
//! keeps your data alive as plain text.
//!
//! This crate provides the data models, parsing utilities, and high-level
//! operations that are shared across the CLI and MCP frontends.
//!
//! # Modules
//!
//! - [`entry`] — Core data structures: [`entry::Entry`], [`entry::Frontmatter`],
//!   [`entry::TaskMeta`], and [`entry::EventMeta`].
//! - [`entry_ref`] — [`entry_ref::EntryRef`]: a canonical input type that
//!   accepts either a filesystem path, an exact GrainId, or a title.
//! - [`error`] — Library-wide [`error::Error`] and [`error::Result`] types.
//! - [`journal`] — Journal directory discovery, entry collection, and
//!   configuration loading.
//! - [`labels`] — Status label classification for entry types and freshness.
//! - [`ops`] — High-level entry operations (list, create, update, delete)
//!   shared across frontends.
//! - [`parser`] — Markdown + YAML frontmatter parsing and serialization.
//! - [`period`] — Time period types used to filter entries by timestamp ranges.

pub mod cache;
pub mod labels;
pub mod entry;
pub mod entry_ref;
pub mod error;
pub mod journal;
pub mod journal_state;
pub mod ops;
pub mod parser;
pub mod period;
pub mod state;
#[cfg(feature = "text-input")]
pub mod text_input;
pub mod user_config;

pub use journal_state::JournalState;
#[allow(deprecated)]
pub use sapphire_workspace::RetrieveDb;
pub use sapphire_workspace::SyncBackend;
pub use sapphire_workspace::{FtsQuery, VectorQuery};
#[cfg(feature = "git-sync")]
pub use sapphire_workspace::GitSync;

/// Shared application context for sapphire-journal.
///
/// Provides the app name (`"sapphire-journal"`) plus the cache + data
/// directories used by every [`Journal`](journal::Journal) instance.
///
/// Starting with `sapphire-workspace` 0.10 the library no longer ships its
/// own platform-directory resolver, so the *host app* is responsible for
/// injecting both directories at startup.  Desktop hosts (CLI, MCP server,
/// GUI) can simply call [`init_app_context`] once at the top of `main`;
/// mobile or sandboxed hosts that need to point at platform-supplied paths
/// (e.g. Android's `Context.getCacheDir()`, iOS's `$HOME/Library/Caches`)
/// should call
/// [`AppContext::set_cache_dir`](sapphire_workspace::AppContext::set_cache_dir)
/// and
/// [`AppContext::set_data_dir`](sapphire_workspace::AppContext::set_data_dir)
/// on this value directly instead.
///
/// Forgetting to set either directory makes the first call to
/// [`Journal::cache_dir`](journal::Journal::cache_dir) — and therefore
/// every cache, retrieve, and sync operation — panic.
pub static JOURNAL_CTX: sapphire_workspace::AppContext =
    sapphire_workspace::AppContext::new("sapphire-journal");

/// Initialise [`JOURNAL_CTX`] with platform-default cache and data
/// directories resolved via the `dirs` crate.
///
/// The directories are computed as:
///
/// - `cache_dir`: [`dirs::cache_dir`]`()`/`sapphire-journal/`
/// - `data_dir`:  [`dirs::data_dir`]`()`/`sapphire-journal/`
///
/// Both directories are created if they don't already exist.  This call is
/// idempotent — `AppContext` uses first-writer-wins semantics, so a second
/// call (or a host that already injected explicit paths beforehand) is a
/// silent no-op.
///
/// Call this once at the top of `main` in every desktop host binary
/// (CLI, GUI, MCP server) before any operation that opens a journal or
/// touches the cache.
pub fn init_app_context() {
    let cache = dirs::cache_dir()
        .unwrap_or_else(|| std::env::temp_dir().join(".cache"))
        .join("sapphire-journal");
    let data = dirs::data_dir()
        .unwrap_or_else(|| std::env::temp_dir().join(".local").join("share"))
        .join("sapphire-journal");
    let _ = std::fs::create_dir_all(&cache);
    let _ = std::fs::create_dir_all(&data);
    JOURNAL_CTX.set_cache_dir(cache);
    JOURNAL_CTX.set_data_dir(data);
}

#[cfg(feature = "lancedb-store")]
pub use sapphire_workspace::lancedb_store;
