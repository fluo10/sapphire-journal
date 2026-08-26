use std::path::PathBuf;

use clap::Parser as _;
use sapphire_journal_server::cli::Cli;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let cli = Cli::parse();
    let keys_path = resolve_keys_path(&cli)?;
    match cli.command {
        Some(command) => sapphire_journal_server::keys::run(command, &keys_path),
        None => todo!("serve — Task 6"),
    }
}

/// 鍵ファイルの位置を決める。
///
/// このタスクの時点では `--keys` が必須。Task 6 で `None` の分岐を journal の
/// キャッシュディレクトリ既定に差し替える（`Journal` をここで引き込みたくない）。
fn resolve_keys_path(cli: &Cli) -> anyhow::Result<PathBuf> {
    cli.keys
        .clone()
        .ok_or_else(|| anyhow::anyhow!("--keys is required (a default arrives with the serve path)"))
}
