mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "sapphire-journal", about = "Markdown-based task and note manager", version)]
struct Cli {
    /// Path to the journal root (the directory containing `.sapphire-journal/`).
    /// Overrides the automatic upward search from the current directory.
    /// Can also be set via the SAPPHIRE_JOURNAL_DIR environment variable.
    #[arg(long, env = "SAPPHIRE_JOURNAL_DIR", global = true, value_name = "DIR")]
    journal_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new journal in the given directory (defaults to current directory)
    Init {
        /// Directory to initialize (created if it does not exist)
        path: Option<PathBuf>,
    },
    /// Manage entries
    Entry {
        #[command(subcommand)]
        action: commands::entry::EntryCommand,
    },
    /// Manage the local SQLite cache
    Cache {
        #[command(subcommand)]
        action: commands::cache::CacheCommand,
    },
    /// Show current configuration (user-level and journal-level)
    Config {
        #[command(subcommand)]
        action: commands::config::ConfigCommand,
    },
    Mcp
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init { path } => commands::init::run(path)?,
        Command::Entry { action } => commands::entry::run(cli.journal_dir.as_deref(), action)?,
        Command::Cache { action } => commands::cache::run(cli.journal_dir.as_deref(), action)?,
        Command::Config { action } => commands::config::run(cli.journal_dir.as_deref(), action)?,
        Command::Mcp => commands::mcp::run(cli.journal_dir.as_deref())?,
    }

    Ok(())
}
