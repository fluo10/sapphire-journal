//! Core library for **archelon** — a Markdown-based task and note manager that
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
//!   accepts either a filesystem path, an exact CarettaId, or a title.
//! - [`error`] — Library-wide [`error::Error`] and [`error::Result`] types.
//! - [`journal`] — Journal directory discovery, entry collection, and
//!   configuration loading.
//! - [`labels`] — Status label classification for entry types and freshness.
//! - [`ops`] — High-level entry operations (list, create, update, delete)
//!   shared across frontends.
//! - [`parser`] — Markdown + YAML frontmatter parsing and serialization.
//! - [`period`] — Time period types used to filter entries by timestamp ranges.

pub mod cache;
pub mod chunker;
pub mod embed;
pub mod labels;
pub mod lancedb_store;
pub mod vector_store;
pub mod entry;
pub mod entry_ref;
pub mod error;
pub mod journal;
pub mod ops;
pub mod parser;
pub mod period;
pub mod user_config;
