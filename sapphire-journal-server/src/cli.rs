//! コマンドライン引数。
//!
//! `serve` は既定動作なのでサブコマンドを持たない。サブコマンドはデバイスと
//! ユーザーの台帳管理だけ。

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
    /// Journal root (the directory containing `.sapphire-journal/`). Required by
    /// every command: it locates the journal to serve, and the device and user
    /// tables the `device` and `user` subcommands read and write.
    #[arg(long, env = "SAPPHIRE_JOURNAL_SERVER_DIR", value_name = "DIR")]
    pub journal_dir: Option<PathBuf>,

    /// Address to bind.
    #[arg(
        long,
        env = "SAPPHIRE_JOURNAL_SERVER_ADDR",
        default_value = "127.0.0.1:8080"
    )]
    pub addr: SocketAddr,

    /// Path to the device key file. Defaults to `<journal cache dir>/keys.toml`.
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
