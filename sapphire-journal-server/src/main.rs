use std::sync::Arc;

use clap::Parser as _;
use sapphire_journal_server::cli::Cli;
use sapphire_journal_server::cli::Command;
use sapphire_journal_server::serve;

/// `--journal-dir` が要る理由は呼び出しごとに違うので、文言もそこで足す。
/// ここは共通部分だけ。
const JOURNAL_DIR_REQUIRED: &str =
    "--journal-dir is required (or set SAPPHIRE_JOURNAL_SERVER_DIR)";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `RUST_LOG` を読む。既定機能のままだと INFO 固定で、tick がおかしいときに
    // 運用者がログを上げる手段が無い。
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        // stdout は `device add` 等が生のトークンだけを出す契約になっている。
        // 既定の stdout writer のままだと、framework の `create_private` が
        // Windows で出す warn!（key file permissions are not restricted...）が
        // `device add > token.txt` のリダイレクト先に混ざり、トークンだけという
        // 約束を壊す。
        .with_writer(std::io::stderr)
        .init();

    let mut cli = Cli::parse();
    match cli.command.take() {
        Some(command) => run_registry_command(&cli, command),
        None => {
            let journal_dir = cli
                .journal_dir
                .clone()
                .ok_or_else(|| anyhow::anyhow!("{JOURNAL_DIR_REQUIRED} to serve"))?;
            let keys_path = match cli.keys.clone() {
                Some(p) => p,
                None => serve::default_keys_path(&journal_dir)?,
            };
            let journal_state = serve::open_journal_state(&journal_dir)?;
            let state = serve::build_state(&journal_dir, &keys_path, Arc::clone(&journal_state))?;
            serve::run(
                cli.addr,
                &journal_dir,
                &keys_path,
                state,
                journal_state,
                &cli.allowed_host,
            )
            .await
        }
    }
}

/// `main` の本体のうち、サブコマンドの分岐だけ。テストから `--journal-dir` の
/// 欠落を確かめられるように切り出してある。serve の腕は `async` なので含めない。
fn run_registry_command(cli: &Cli, command: Command) -> anyhow::Result<()> {
    // 名詞だけ先に取る。`command` はこの後 match で消費するので、クロージャに
    // 借用させたままにしない。
    let table = match command {
        Command::User { .. } => "user",
        Command::Device { .. } => "device",
    };
    let journal_dir = cli.journal_dir.clone().ok_or_else(|| {
        anyhow::anyhow!("{JOURNAL_DIR_REQUIRED} to locate the {table} table")
    })?;
    match command {
        Command::User { command } => {
            let users_path = serve::default_users_path(&journal_dir)?;
            sapphire_journal_server::cli_device::run_user(command, &users_path)
        }
        Command::Device { command } => {
            let devices_path = serve::default_devices_path(&journal_dir)?;
            let users_path = serve::default_users_path(&journal_dir)?;
            let keys_path = match cli.keys.clone() {
                Some(p) => p,
                None => serve::default_keys_path(&journal_dir)?,
            };
            sapphire_journal_server::cli_device::run_device(
                command,
                &devices_path,
                &users_path,
                &keys_path,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("sapphire-journal-server").chain(args.iter().copied()))
            .unwrap()
    }

    fn run_command_for_test(mut cli: Cli) -> anyhow::Result<()> {
        let command = cli.command.take().expect("サブコマンドが要る");
        run_registry_command(&cli, command)
    }

    /// `--journal-dir` の欠落は、そのコマンド自身の言葉で説明すること。
    /// serve の話（「serve するには要る」）にしない。
    #[test]
    fn a_registry_command_without_a_journal_dir_explains_itself_in_its_own_terms() {
        for args in [
            vec!["user", "list"],
            vec!["--keys", "somewhere/keys.toml", "device", "list"],
        ] {
            let err = run_command_for_test(cli(&args)).unwrap_err().to_string();

            assert!(!err.contains("to serve"), "{args:?}: {err}");
            assert!(
                err.contains("table"),
                "{args:?}: 台帳が要る話になっていない: {err}"
            );
        }
    }
}
