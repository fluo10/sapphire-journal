use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "sapphire-journal-mcp",
    about = "MCP server for sapphire-journal",
    version
)]
struct Cli {
    /// Path to the journal root (the directory containing `.sapphire-journal/`).
    /// Defaults to the current directory (with upward search when `--init` is not set).
    /// Can also be set via the SAPPHIRE_JOURNAL_DIR environment variable.
    #[arg(long, env = "SAPPHIRE_JOURNAL_DIR", value_name = "DIR")]
    journal_dir: Option<PathBuf>,

    /// Initialize the target directory as a sapphire-journal if it isn't one already.
    /// Creates the directory itself if missing. No-op when a journal already exists there.
    #[arg(long)]
    init: bool,
}

fn main() -> Result<()> {
    sapphire_journal_core::init_app_context();
    let cli = Cli::parse();
    sapphire_journal_mcp::run(cli.journal_dir.as_deref(), cli.init)
}
