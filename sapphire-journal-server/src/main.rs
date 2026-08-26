use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser as _;
use sapphire_journal_server::cli::Cli;
use sapphire_journal_server::serve;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let cli = Cli::parse();
    let keys_path = resolve_keys_path(&cli)?;
    match cli.command {
        Some(command) => sapphire_journal_server::keys::run(command, &keys_path),
        None => {
            let journal_dir = cli.journal_dir.clone().ok_or_else(|| {
                anyhow::anyhow!("--journal-dir is required to serve")
            })?;
            let journal_state = serve::open_journal_state(&journal_dir)?;
            let state = serve::build_state(&journal_dir, &keys_path, Arc::clone(&journal_state))?;
            serve::run(cli.addr, &journal_dir, &keys_path, state, journal_state).await
        }
    }
}

/// 鍵ファイルの位置を決める。
///
/// `--keys` が無ければ journal のキャッシュディレクトリ既定を使う。
fn resolve_keys_path(cli: &Cli) -> anyhow::Result<PathBuf> {
    match &cli.keys {
        Some(p) => Ok(p.clone()),
        None => {
            let journal_dir = cli
                .journal_dir
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("--journal-dir is required to serve"))?;
            serve::default_keys_path(journal_dir)
        }
    }
}
