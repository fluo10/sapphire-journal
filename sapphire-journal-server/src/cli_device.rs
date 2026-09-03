//! `device` と `user` サブコマンド。
//!
//! このアプリで鍵を発行する場所はここだけ。`device add` は**別の場所にある
//! 2 つのファイル**に書く —— デバイス行は同期されるワークスペースの台帳へ、
//! 鍵はホストローカルの鍵ファイルへ。順序が意味を持つ（行が先）。

use std::path::Path;

use anyhow::{Context as _, Result};
use clap::Subcommand;
use sapphire_framework::registry::Users;

#[derive(Subcommand, Debug)]
pub enum UserCommand {
    /// Register a user.
    Add {
        #[arg(long, value_name = "USER_NAME")]
        name: String,
        /// A note for you — who this is.
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
    },
    /// List users.
    List,
}

pub fn run_user(command: UserCommand, users_file: &Path) -> Result<()> {
    let mut users = Users::load(users_file)
        .with_context(|| format!("loading user table {}", users_file.display()))?;
    match command {
        UserCommand::Add { name, description } => {
            let user = users.add(&name, description)?;
            // id は stdout。`--user` に渡す値なので、パイプで拾えること。
            println!("{}", user.id);
            eprintln!("added {} ({})", user.id, user.name);
        }
        UserCommand::List => {
            for u in users.entries() {
                println!(
                    "{}  {}  {}  {}",
                    u.id,
                    u.name,
                    if u.is_retired() { "retired" } else { "active" },
                    u.description.as_deref().unwrap_or("-"),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    // Task 2 で `devices` / `keys` を足す。読まないフィールドを先に置くと
    // `dead_code` で clippy が鳴るので、ここでは `users` だけ。
    struct Files {
        _dir: tempfile::TempDir,
        users: PathBuf,
    }

    fn files() -> Files {
        let dir = tempfile::tempdir().unwrap();
        Files {
            users: dir.path().join("users.toml"),
            _dir: dir,
        }
    }

    #[test]
    fn user_add_writes_a_row() {
        let f = files();

        run_user(
            UserCommand::Add {
                name: "fluo".into(),
                description: Some("me".into()),
            },
            &f.users,
        )
        .unwrap();

        let users = Users::load(&f.users).unwrap();
        let user = users.resolve("fluo").unwrap();
        assert_eq!(user.description.as_deref(), Some("me"));
        assert!(!user.is_retired());
    }

    #[test]
    fn user_add_refuses_a_duplicate_name() {
        let f = files();
        run_user(
            UserCommand::Add { name: "fluo".into(), description: None },
            &f.users,
        )
        .unwrap();

        let result = run_user(
            UserCommand::Add { name: "fluo".into(), description: None },
            &f.users,
        );

        assert!(result.is_err(), "同名のユーザーが 2 行できた");
    }

    #[test]
    fn user_list_runs_on_an_empty_table() {
        // 台帳ファイルが存在しない状態。`Users::load` は空を返す（作らない）
        // ので、初回の `user list` はここを通る。
        let f = files();

        run_user(UserCommand::List, &f.users).unwrap();
    }
}
