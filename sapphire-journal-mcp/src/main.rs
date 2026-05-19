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
    /// Overrides the automatic upward search from the current directory.
    /// Can also be set via the SAPPHIRE_JOURNAL_DIR environment variable.
    #[arg(long, env = "SAPPHIRE_JOURNAL_DIR", value_name = "DIR")]
    journal_dir: Option<PathBuf>,
}

fn main() -> Result<()> {
    sapphire_journal_core::init_app_context();
    let cli = Cli::parse();
    sapphire_journal_mcp::run(cli.journal_dir.as_deref())
}
