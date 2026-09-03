use std::path::PathBuf;
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
        // stdout は `gen-key` 等が生のトークンだけを出す契約になっている。
        // 既定の stdout writer のままだと、framework の `create_private` が
        // Windows で出す warn!（key file permissions are not restricted...）が
        // `gen-key > token.txt` のリダイレクト先に混ざり、トークンだけという
        // 約束を壊す。
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let (keys, journal) = (cli.keys.clone(), cli.journal_dir.clone());
    match cli.command {
        // 鍵ファイルの位置は、サブコマンドごとに要求が違う。`gen-key` は
        // journal を開かないので、`--keys` を明示してあれば `--journal-dir`
        // は要らない —— 以前はこの解決を match の前で一度に済ませていたため、
        // `gen-key` に `--journal-dir` も `--keys` も無いと「serve するには
        // --journal-dir が要る」という、そのコマンドについて偽の説明が出ていた。
        // `user` は鍵に触らないので、鍵ファイルの解決自体をしない。台帳の
        // 位置は journal ルートからしか決まらないため `--journal-dir` は必須
        // ——`--keys` を渡しても代わりにはならない。
        Some(Command::User { command }) => {
            let journal_dir = journal.ok_or_else(|| {
                anyhow::anyhow!("{JOURNAL_DIR_REQUIRED} to locate the user table")
            })?;
            let users_path = serve::default_users_path(&journal_dir)?;
            sapphire_journal_server::cli_device::run_user(command, &users_path)
        }
        // `device` は台帳と鍵ファイルの両方に書くので、`--journal-dir`（台帳の
        // 位置）が必須。鍵ファイルだけは `--keys` で上書きできる。
        Some(Command::Device { command }) => {
            let journal_dir = journal.ok_or_else(|| {
                anyhow::anyhow!("{JOURNAL_DIR_REQUIRED} to locate the device table")
            })?;
            let devices_path = serve::default_devices_path(&journal_dir)?;
            let users_path = serve::default_users_path(&journal_dir)?;
            let keys_path = match keys {
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
        Some(command) => {
            let keys_path = keys_path_for_key_command(keys.as_deref(), journal.as_deref())?;
            sapphire_journal_server::keys::run(command, &keys_path)
        }
        None => {
            let journal_dir =
                journal.ok_or_else(|| anyhow::anyhow!("{JOURNAL_DIR_REQUIRED} to serve"))?;
            let keys_path = match keys {
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

/// 鍵サブコマンド（`gen-key` / `list-keys` / `rotate-key` / `revoke-key`）が
/// 使う鍵ファイル。
///
/// `--keys` があればそれ。無ければ journal のキャッシュディレクトリ既定を使う
/// ので、そのときだけ `--journal-dir` が要る。
fn keys_path_for_key_command(
    keys: Option<&std::path::Path>,
    journal_dir: Option<&std::path::Path>,
) -> anyhow::Result<PathBuf> {
    if let Some(p) = keys {
        return Ok(p.to_path_buf());
    }
    let journal_dir = journal_dir.ok_or_else(|| {
        anyhow::anyhow!("{JOURNAL_DIR_REQUIRED} to locate the key file, unless you pass --keys")
    })?;
    serve::default_keys_path(journal_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("sapphire-journal-server").chain(args.iter().copied()))
            .unwrap()
    }

    #[test]
    fn a_key_command_with_explicit_keys_needs_no_journal_dir() {
        let cli = cli(&["--keys", "somewhere/keys.toml", "list-keys"]);
        assert_eq!(
            keys_path_for_key_command(cli.keys.as_deref(), cli.journal_dir.as_deref()).unwrap(),
            PathBuf::from("somewhere/keys.toml")
        );
    }

    #[test]
    fn a_key_command_without_either_explains_itself_in_its_own_terms() {
        let cli = cli(&["gen-key"]);

        let err = keys_path_for_key_command(cli.keys.as_deref(), cli.journal_dir.as_deref())
            .unwrap_err()
            .to_string();

        assert!(
            !err.contains("to serve"),
            "gen-key の失敗が serve の話になっている: {err}"
        );
        assert!(err.contains("--keys"), "逃げ道を示していない: {err}");
    }

    #[test]
    fn the_command_is_a_key_command_only_when_one_was_given() {
        assert!(cli(&[]).command.is_none());
        assert!(matches!(cli(&["list-keys"]).command, Some(Command::ListKeys)));
    }
}
