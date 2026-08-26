//! コマンドライン引数。
//!
//! `serve` は既定動作なのでサブコマンドを持たない。鍵の管理だけがサブコマンド。

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "sapphire-journal-server",
    about = "Remote workspace + MCP server for sapphire-journal",
    version
)]
pub struct Cli {
    /// Journal root (the directory containing `.sapphire-journal/`). Required to serve.
    #[arg(long, env = "SAPPHIRE_JOURNAL_SERVER_DIR", value_name = "DIR")]
    pub journal_dir: Option<PathBuf>,

    /// Address to bind.
    #[arg(
        long,
        env = "SAPPHIRE_JOURNAL_SERVER_ADDR",
        default_value = "127.0.0.1:8080"
    )]
    pub addr: SocketAddr,

    /// Path to the API key file. Defaults to `<journal cache dir>/keys.toml`.
    #[arg(long, env = "SAPPHIRE_JOURNAL_SERVER_KEYS", value_name = "FILE")]
    pub keys: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Generate a new API key and print it.
    GenKey {
        /// A note for you — which host or person this key is for.
        label: Option<String>,
        /// Expire the key after this long, e.g. `90d`, `12h`.
        #[arg(long, value_name = "DURATION")]
        expires_in: Option<String>,
    },
    /// List the keys, with tokens masked.
    ListKeys,
    /// Remove a key by id or label.
    RevokeKey {
        /// The key's UUID, or its label when that is unambiguous.
        selector: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn no_subcommand_means_serve() {
        let cli = Cli::try_parse_from(["sapphire-journal-server"]).unwrap();
        assert!(cli.command.is_none(), "サブコマンド無しは serve");
    }

    #[test]
    fn addr_is_a_single_socket_addr() {
        let cli = Cli::try_parse_from(["sapphire-journal-server", "--addr", "0.0.0.0:9000"]).unwrap();
        assert_eq!(cli.addr.to_string(), "0.0.0.0:9000");
    }

    #[test]
    fn gen_key_takes_an_optional_label() {
        let cli = Cli::try_parse_from(["sapphire-journal-server", "gen-key"]).unwrap();
        assert!(matches!(cli.command, Some(Command::GenKey { label: None, .. })));

        let cli = Cli::try_parse_from(["sapphire-journal-server", "gen-key", "laptop"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::GenKey { label: Some(ref l), .. }) if l == "laptop"
        ));
    }

    #[test]
    fn revoke_key_requires_a_selector() {
        assert!(Cli::try_parse_from(["sapphire-journal-server", "revoke-key"]).is_err());
    }
}
