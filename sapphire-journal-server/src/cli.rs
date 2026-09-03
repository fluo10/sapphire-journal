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

    /// A hostname clients use to reach `/mcp`, e.g. `box.tailnet.ts.net` or
    /// `nas.local:8080`. Repeatable. Loopback and `--addr` are always allowed;
    /// anything else must be named here or MCP answers it with 403.
    #[arg(
        long = "allowed-host",
        env = "SAPPHIRE_JOURNAL_SERVER_ALLOWED_HOSTS",
        value_delimiter = ',',
        value_name = "HOST"
    )]
    pub allowed_host: Vec<String>,

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
    /// Re-issue a key's token, keeping its id, label and created_at.
    ///
    /// The old token stops working immediately in this process, but a
    /// running server only picks the change up when it next reloads the
    /// key file (e.g. on restart) — `ServerState` holds a snapshot taken
    /// at start-up and has no reload path.
    RotateKey {
        /// The key's UUID, or its label when that is unambiguous.
        selector: String,
        /// Expire the new token after this long, e.g. `90d`, `12h`.
        ///
        /// This REPLACES the expiry rather than keeping it: omitting the
        /// flag makes the key non-expiring, it does not carry the old
        /// expiry over. Re-issuing an expired key with its old expiry
        /// would produce a token that is already unusable.
        #[arg(long, value_name = "DURATION")]
        expires_in: Option<String>,
    },
    /// Remove a key by id or label.
    ///
    /// The key stops working immediately in this process, but a running
    /// server only picks the change up when it next reloads the key file
    /// (e.g. on restart) — `ServerState` holds a snapshot taken at
    /// start-up and has no reload path.
    RevokeKey {
        /// The key's UUID, or its label when that is unambiguous.
        selector: String,
    },
    /// Manage the users devices belong to.
    User {
        #[command(subcommand)]
        command: crate::cli_device::UserCommand,
    },
    /// Manage the devices that authenticate to this server.
    Device {
        #[command(subcommand)]
        command: crate::cli_device::DeviceCommand,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subcommand_means_serve() {
        let cli = Cli::try_parse_from(["sapphire-journal-server"]).unwrap();
        assert!(cli.command.is_none(), "サブコマンド無しは serve");
    }

    #[test]
    fn addr_is_a_single_socket_addr() {
        let cli =
            Cli::try_parse_from(["sapphire-journal-server", "--addr", "0.0.0.0:9000"]).unwrap();
        assert_eq!(cli.addr.to_string(), "0.0.0.0:9000");
    }

    #[test]
    fn allowed_host_is_repeatable_and_defaults_to_empty() {
        let cli = Cli::try_parse_from(["sapphire-journal-server"]).unwrap();
        assert!(cli.allowed_host.is_empty());

        let cli = Cli::try_parse_from([
            "sapphire-journal-server",
            "--allowed-host",
            "box.tailnet.ts.net",
            "--allowed-host",
            "nas.local:8080",
        ])
        .unwrap();
        assert_eq!(cli.allowed_host, ["box.tailnet.ts.net", "nas.local:8080"]);
    }

    #[test]
    fn allowed_host_also_splits_a_comma_separated_value() {
        // 環境変数（`SAPPHIRE_JOURNAL_SERVER_ALLOWED_HOSTS`）は 1 本の文字列
        // でしか渡せないので、区切り文字が効いていないと service unit から
        // 複数指定できない。
        let cli = Cli::try_parse_from([
            "sapphire-journal-server",
            "--allowed-host",
            "a.example,b.example",
        ])
        .unwrap();
        assert_eq!(cli.allowed_host, ["a.example", "b.example"]);
    }

    #[test]
    fn gen_key_takes_an_optional_label() {
        let cli = Cli::try_parse_from(["sapphire-journal-server", "gen-key"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::GenKey { label: None, .. })
        ));

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

    #[test]
    fn rotate_key_requires_a_selector() {
        assert!(Cli::try_parse_from(["sapphire-journal-server", "rotate-key"]).is_err());
    }

    #[test]
    fn rotate_key_takes_a_selector_and_an_optional_expiry() {
        let cli = Cli::try_parse_from(["sapphire-journal-server", "rotate-key", "laptop"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::RotateKey { ref selector, expires_in: None }) if selector == "laptop"
        ));

        let cli = Cli::try_parse_from([
            "sapphire-journal-server",
            "rotate-key",
            "laptop",
            "--expires-in",
            "90d",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::RotateKey { expires_in: Some(ref d), .. }) if d == "90d"
        ));
    }

    #[test]
    fn user_add_requires_a_name() {
        assert!(Cli::try_parse_from(["sapphire-journal-server", "user", "add"]).is_err());

        let cli =
            Cli::try_parse_from(["sapphire-journal-server", "user", "add", "--name", "fluo"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::User {
                command: crate::cli_device::UserCommand::Add { ref name, description: None }
            }) if name == "fluo"
        ));
    }

    #[test]
    fn user_list_parses() {
        let cli = Cli::try_parse_from(["sapphire-journal-server", "user", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::User { command: crate::cli_device::UserCommand::List })
        ));
    }

    #[test]
    fn device_add_requires_a_name() {
        assert!(Cli::try_parse_from(["sapphire-journal-server", "device", "add"]).is_err());

        let cli = Cli::try_parse_from([
            "sapphire-journal-server",
            "device",
            "add",
            "--name",
            "laptop",
            "--expires-in",
            "90d",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Device {
                command: crate::cli_device::DeviceCommand::Add {
                    ref name,
                    expires_in: Some(ref d),
                    ..
                }
            }) if name == "laptop" && d == "90d"
        ));
    }

    #[test]
    fn device_rotate_and_retire_require_a_selector() {
        assert!(Cli::try_parse_from(["sapphire-journal-server", "device", "rotate"]).is_err());
        assert!(Cli::try_parse_from(["sapphire-journal-server", "device", "retire"]).is_err());

        let cli =
            Cli::try_parse_from(["sapphire-journal-server", "device", "retire", "laptop"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Device {
                command: crate::cli_device::DeviceCommand::Retire { ref selector, purge: false }
            }) if selector == "laptop"
        ));
    }
}
