use anyhow::Result;
use std::path::Path;

/// Transitional shim for `sajo mcp`.
///
/// The MCP server now lives in its own `sapphire-journal-mcp` crate, installed
/// as the `sapphire-journal-mcp` binary. This subcommand stays in `sajo` for at
/// least one release so existing setups keep working; new integrations should
/// invoke `sapphire-journal-mcp` directly.
pub fn run(journal_dir: Option<&Path>) -> Result<()> {
    sapphire_journal_mcp::run(journal_dir)
}
