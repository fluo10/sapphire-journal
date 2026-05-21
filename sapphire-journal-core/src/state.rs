//! Frontend-neutral helpers for opening a journal and bootstrapping its
//! optional vector/embedding backends.
//!
//! These wrap [`Journal`] and [`JournalState`] operations that are repeated
//! verbatim by every frontend (CLI, MCP, GUI), so they live in core to avoid
//! cross-frontend dependencies.

use std::path::Path;

use crate::{error::Result, journal::Journal, user_config::UserConfig, JournalState};

/// Open a [`JournalState`] from an explicit directory, or by walking up from
/// the current working directory when `journal_dir` is `None`.
pub fn open_state(journal_dir: Option<&Path>) -> Result<JournalState> {
    let journal = match journal_dir {
        Some(dir) => Journal::from_root(dir.to_path_buf())?,
        None => Journal::find()?,
    };
    JournalState::open(journal)
}

/// Initialise the vector backend and embedder when configured.
///
/// Calls the sync variants on [`JournalState`]; the underlying lancedb store
/// uses `block_in_place` internally so this is safe to call from inside a
/// tokio multi-thread runtime task (MCP wraps the call in `block_in_place`
/// to avoid blocking other tasks during the initial model download).
pub fn bootstrap_embedder(state: &JournalState, config: &UserConfig) -> Result<()> {
    state.load_retrieve_backend(config)?;
    state.load_embedder(config)?;
    Ok(())
}
