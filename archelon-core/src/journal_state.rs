//! In-memory session state: an open journal paired with its SQLite cache connection.
//!
//! [`JournalState`] is the single object that frontends (CLI, MCP, GUI) hold while
//! a workspace is active. Passing it to `ops` functions avoids reopening the
//! journal directory and database on every call.

use crate::{cache, error::Result, journal::Journal};

/// An open journal paired with its SQLite cache connection.
///
/// Create with [`JournalState::open`] or [`JournalState::rebuild`], then pass
/// references to [`crate::ops`] functions.
pub struct JournalState {
    pub journal: Journal,
    pub conn: rusqlite::Connection,
}

impl JournalState {
    /// Open the cache for `journal`, creating it if it does not yet exist.
    pub fn open(journal: Journal) -> Result<Self> {
        let conn = cache::open_cache(&journal)?;
        Ok(Self { journal, conn })
    }

    /// Drop and recreate the cache from scratch, then return the new state.
    pub fn rebuild(journal: Journal) -> Result<Self> {
        let conn = cache::rebuild_cache(&journal)?;
        Ok(Self { journal, conn })
    }

    /// Incrementally sync the cache with the current on-disk journal state.
    pub fn sync(&self) -> Result<()> {
        cache::sync_cache(&self.journal, &self.conn)
    }

    /// Return cache statistics (path, schema version, entry count, etc.).
    pub fn cache_info(&self) -> Result<cache::CacheInfo> {
        cache::cache_info(&self.journal, &self.conn)
    }
}
