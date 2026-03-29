mod commands;
mod config;
mod indexer;
mod mcp;
mod state;
mod workspace;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "sapphire-retrieve", about = "Index and search text files with FTS and semantic search")]
struct Cli {
    /// Workspace directory to index (env: SAPPHIRE_RETRIEVE_WORKSPACE).
    ///
    /// When omitted and running in a terminal, you will be prompted to confirm
    /// using the current directory.
    #[arg(long, env = "SAPPHIRE_RETRIEVE_WORKSPACE", global = true, value_name = "DIR")]
    workspace_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Incrementally sync the workspace into the index
    Sync,

    /// Delete the current index and rebuild it from scratch
    Rebuild,

    /// Show index location, schema version, and document count
    Info,

    /// Generate embeddings for documents that do not yet have a vector
    Embed,

    /// Remove stale index files from previous schema versions
    Clean,

    /// Start the MCP server (stdio transport)
    Mcp,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let workspace_dir = cli.workspace_dir.as_deref();

    match cli.command {
        Command::Sync => commands::sync::run(workspace_dir)?,
        Command::Rebuild => commands::rebuild::run(workspace_dir)?,
        Command::Info => commands::info::run(workspace_dir)?,
        Command::Embed => commands::embed::run(workspace_dir)?,
        Command::Clean => commands::clean::run(workspace_dir)?,
        Command::Mcp => mcp::run(workspace_dir)?,
    }
    Ok(())
}
