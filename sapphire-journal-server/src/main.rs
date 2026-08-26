use clap::Parser as _;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let cli = sapphire_journal_server::cli::Cli::parse();
    // 実際のディスパッチは Task 5 / Task 6 で埋める。
    let _ = cli;
    Ok(())
}
