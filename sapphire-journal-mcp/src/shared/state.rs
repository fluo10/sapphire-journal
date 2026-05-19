use std::path::Path;

use anyhow::{Context, Result};
use sapphire_journal_core::{journal::Journal, user_config::UserConfig, JournalState};

/// Open a `JournalState` from an explicit directory or by walking up from the CWD.
///
/// Shared by CLI's entry commands and MCP's `journal_open` flow.
pub fn open_state(journal_dir: Option<&Path>) -> Result<JournalState> {
    let journal = match journal_dir {
        Some(dir) => Journal::from_root(dir.to_path_buf())
            .context("not a sapphire-journal — run `sapphire-journal init` to initialize one"),
        None => Journal::find()
            .context("not in a sapphire-journal — run `sapphire-journal init` to initialize one"),
    }?;
    JournalState::open(journal).map_err(Into::into)
}

/// Initialise the vector backend and embedder when configured.
///
/// This calls the sync variants on `JournalState`; the underlying lancedb store
/// uses `block_in_place` internally so this is also safe to call from inside a
/// tokio multi-thread runtime task (MCP wraps the call in `block_in_place` to
/// avoid blocking other tasks during the initial model download).
pub fn bootstrap_embedder(state: &JournalState, config: &UserConfig) -> Result<()> {
    state.load_retrieve_backend(config).map_err(anyhow::Error::msg)?;
    state.load_embedder(config).map_err(anyhow::Error::msg)?;
    Ok(())
}
