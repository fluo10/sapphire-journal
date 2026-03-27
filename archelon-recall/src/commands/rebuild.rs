use std::path::Path;

use anyhow::Result;

use crate::{state::WorkspaceState, workspace::Workspace};

pub fn run(workspace_dir: Option<&Path>) -> Result<()> {
    let workspace = Workspace::resolve(workspace_dir)?;
    let state = WorkspaceState::rebuild(workspace)?;
    let (upserted, _removed) = state.sync()?;
    println!("rebuilt: {upserted} files indexed");
    Ok(())
}
