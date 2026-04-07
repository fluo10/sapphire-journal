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
pub mod user_config;

pub use journal_state::JournalState;
pub use sapphire_workspace::RetrieveDb;

/// Shared application context for sapphire-journal.
///
/// Provides the app name (`"sapphire-journal"`) and the platform cache base
/// directory used by all [`Journal`](journal::Journal) instances.
///
/// On Android, call [`AppContext::set_cache_base`](sapphire_workspace::AppContext::set_cache_base)
/// on this value at app startup (before opening any journal) with the path
/// obtained from `Context.getCacheDir()`.
pub static JOURNAL_CTX: sapphire_workspace::AppContext =
    sapphire_workspace::AppContext::new("sapphire-journal");

#[cfg(feature = "lancedb-store")]
pub use sapphire_workspace::lancedb_store;
