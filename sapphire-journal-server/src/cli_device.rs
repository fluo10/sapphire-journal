//! `device` と `user` サブコマンド。
//!
//! このアプリで鍵を発行する場所はここだけ。`device add` は**別の場所にある
//! 2 つのファイル**に書く —— デバイス行は同期されるワークスペースの台帳へ、
//! 鍵はホストローカルの鍵ファイルへ。順序が意味を持つ（行が先）。

use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::Subcommand;
use sapphire_framework::registry::{Devices, Users};
use sapphire_framework::remote_server::KeyStore;

use crate::keys::{TOKEN_PREFIX, absolute_expiry};

#[derive(Subcommand, Debug)]
pub enum DeviceCommand {
    /// Register a device and mint the key it authenticates with.
    Add {
        #[arg(long, value_name = "DEVICE_NAME")]
        name: String,
        /// A note for you — what this device is.
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
        /// Whose device this is: a user id or name from users.toml.
        #[arg(long, value_name = "SELECTOR")]
        user: Option<String>,
        /// Expire the key after this long, e.g. `90d`, `12h`.
        #[arg(long, value_name = "DURATION")]
        expires_in: Option<String>,
    },
    /// List devices, their users, and whether they hold a key on this host.
    List,
    /// Re-issue a device's token, keeping its id and its row.
    ///
    /// The old token stops working immediately in this process, but a running
    /// server only picks the change up when it next reloads the files (e.g. on
    /// restart).
    ///
    /// `--expires-in` REPLACES the expiry rather than keeping it: omitting the
    /// flag makes the key non-expiring, it does not carry the old expiry over.
    Rotate {
        /// The device's name, or its id.
        selector: String,
        #[arg(long, value_name = "DURATION")]
        expires_in: Option<String>,
    },
    /// Stop a device: revoke its key, and mark the row retired.
    ///
    /// A running server keeps accepting the old token until it restarts.
    Retire {
        /// The device's name, or its id.
        selector: String,
        /// Delete the row outright instead of retiring it. Device ids get
        /// written into content, so this makes those references unresolvable.
        /// Retiring is the default for that reason.
        #[arg(long)]
        purge: bool,
    },
}

/// デバイスサブコマンドを実行する。
///
/// `add` は**デバイス行を先に、鍵を後に**書く。鍵の無いデバイス行は完全に
/// 不活性（誰も認証できない）だが、逆順で中断すると、誰も掃除しない孤児の鍵が
/// 残る。その上で `add` を再開可能にしてある —— 鍵の無い行を見つけたら鍵だけ
/// 発行する。この分岐が無いと中断状態から抜ける手段が無い（`rotate` は既存の
/// 鍵を要求する）。
pub fn run_device(
    command: DeviceCommand,
    devices_file: &Path,
    users_file: &Path,
    keys_file: &Path,
) -> Result<()> {
    let mut devices = Devices::load(devices_file)
        .with_context(|| format!("loading device table {}", devices_file.display()))?;
    let mut keys = KeyStore::load(keys_file)
        .with_context(|| format!("loading key file {}", keys_file.display()))?;

    match command {
        DeviceCommand::Add {
            name,
            description,
            user,
            expires_in,
        } => {
            // 失敗しうる解決を、何かを書く前に全部済ませる。
            let expires_at = absolute_expiry(expires_in.as_deref())?;
            let user_id = match user {
                Some(selector) => {
                    let users = Users::load(users_file)
                        .with_context(|| format!("loading user table {}", users_file.display()))?;
                    Some(users.resolve(&selector)?.id)
                }
                None => None,
            };

            // `resolve` は `devices` を不変借用し、別の腕は可変借用するので、
            // 参照を match をまたいで持てない。所有したコピーを先に取る。
            let existing = devices.resolve(&name).ok().cloned();
            let device = match existing {
                Some(existing) if existing.is_retired() => {
                    // retired なデバイスは、どのトークンを持っていても認証で
                    // 弾かれる。鍵を出せば「成功したのに何も通らないトークン」
                    // ができる。framework に `retired_at` を消す API は無い。
                    bail!(
                        "device {name:?} is retired; minting it a new key would not help — a \
                         retired device is rejected by every authenticated route regardless of \
                         which token it holds. Either add it under a different name, or accept \
                         a new id for it: `sapphire-journal-server device retire {name} \
                         --purge` and then `device add --name {name}` again"
                    );
                }
                Some(existing) => {
                    if keys.entries().iter().any(|k| k.device_id == Some(existing.id)) {
                        bail!(
                            "device {name:?} already exists and already holds a key on this \
                             host; use `sapphire-journal-server device rotate {name}` to \
                             re-issue its token"
                        );
                    }
                    // 鍵の無い行は一度も使えていない（誰も認証できていない）
                    // ので、content に id が焼かれている心配が無い。だから
                    // `--description` / `--user` をここで反映してよい ——
                    // `Devices` に in-place 更新は無いので、purge して足し直す。
                    if description.is_some() || user_id.is_some() {
                        devices.purge(&name)?;
                        devices.add(
                            &name,
                            description.or(existing.description),
                            user_id.or(existing.user_id),
                        )?
                    } else {
                        existing
                    }
                }
                None => devices.add(&name, description, user_id)?,
            };

            let entry = keys.generate(
                TOKEN_PREFIX,
                None,
                Some(device.id),
                Some(device.name.clone()),
                expires_at,
            )?;

            println!("{}", entry.token);
            eprintln!(
                "id {}  created {}{}",
                device.id,
                device.created_at.to_rfc3339(),
                entry
                    .expires_at
                    .map(|e| format!("  expires {}", e.to_rfc3339()))
                    .unwrap_or_default()
            );
        }
        DeviceCommand::List => {
            for d in devices.entries() {
                let has_key = keys.entries().iter().any(|k| k.device_id == Some(d.id));
                println!(
                    "{}  {}  {}  {}  {}  {}",
                    d.id,
                    d.name,
                    d.user_id
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| "-".to_owned()),
                    if has_key { "key" } else { "no-key" },
                    if d.is_retired() { "retired" } else { "active" },
                    d.description.as_deref().unwrap_or("-"),
                );
            }
        }
        DeviceCommand::Rotate {
            selector,
            expires_in,
        } => {
            let expires_at = absolute_expiry(expires_in.as_deref())?;
            let device = devices.resolve(&selector)?.clone();
            if device.is_retired() {
                bail!(
                    "device {selector:?} ({}) is retired; rotating it would print a token that \
                     authenticates to nothing. Purge the row \
                     (`sapphire-journal-server device retire {selector} --purge`) and add it \
                     again, or add a replacement under a different name",
                    device.name
                );
            }
            // 鍵は `device_id` で引く。`add` は `label = 名前` を入れるが、
            // `devices.toml` は手編集され得て、リネームは label に伝播しない
            // —— 名前で引くと別の鍵を掴むか、何も見つけられなくなる。
            let key_id = keys
                .entries()
                .iter()
                .find(|k| k.device_id == Some(device.id))
                .map(|k| k.id.to_string())
                .ok_or_else(|| {
                    anyhow!(
                        "device {selector:?} ({}) has no key on this host; use \
                         `sapphire-journal-server device add --name {}` to mint one instead of \
                         rotating a key that does not exist",
                        device.name,
                        device.name
                    )
                })?;
            let entry = keys.rotate(TOKEN_PREFIX, &key_id, expires_at)?;
            println!("{}", entry.token);
            eprintln!(
                "rotated {} ({}){}",
                device.id,
                device.name,
                entry
                    .expires_at
                    .map(|e| format!("  expires {}", e.to_rfc3339()))
                    .unwrap_or_else(|| "  no expiry".to_owned())
            );
            eprintln!("a running server keeps accepting the old token until it restarts");
        }
        DeviceCommand::Retire { selector, purge } => {
            let device = devices.resolve(&selector)?.clone();
            // `Rotate` と同じ理由で `device_id` から引く。名前で引くと、
            // リネーム済みのデバイスを「引退させた」と報告しながら鍵を
            // 生かしたまま残す。
            let key_id = keys
                .entries()
                .iter()
                .find(|k| k.device_id == Some(device.id))
                .map(|k| k.id.to_string());
            let had_key = key_id.is_some();
            // 鍵を先に失効させる。引退の目的は「今すぐ止める」ことなので、
            // 2 つの書き込みの間で落ちても生きた鍵を残さない。
            if let Some(key_id) = key_id {
                keys.revoke(&key_id)?;
            }
            if purge {
                devices.purge(&selector)?;
                eprintln!("purged {} ({})", device.id, device.name);
            } else {
                devices.retire(&selector)?;
                eprintln!("retired {} ({})", device.id, device.name);
            }
            if had_key {
                eprintln!("a running server keeps accepting the old token until it restarts");
            }
        }
    }
    Ok(())
}

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

    struct Files {
        _dir: tempfile::TempDir,
        devices: PathBuf,
        users: PathBuf,
        keys: PathBuf,
    }

    fn files() -> Files {
        let dir = tempfile::tempdir().unwrap();
        Files {
            devices: dir.path().join("devices.toml"),
            users: dir.path().join("users.toml"),
            keys: dir.path().join("keys.toml"),
            _dir: dir,
        }
    }

    fn add(f: &Files, name: &str) -> anyhow::Result<()> {
        run_device(
            DeviceCommand::Add {
                name: name.into(),
                description: None,
                user: None,
                expires_in: None,
            },
            &f.devices,
            &f.users,
            &f.keys,
        )
    }

    #[test]
    fn device_add_writes_both_the_row_and_a_key_bound_to_it() {
        let f = files();

        add(&f, "laptop").unwrap();

        let devices = Devices::load(&f.devices).unwrap();
        let device = devices.resolve("laptop").unwrap();
        let keys = KeyStore::load(&f.keys).unwrap();
        assert_eq!(keys.entries().len(), 1);
        let key = &keys.entries()[0];
        assert_eq!(key.device_id, Some(device.id), "鍵がデバイスを指していない");
        assert!(key.token.starts_with("sjt_"), "接頭辞が違う: {}", key.token);
        assert_eq!(key.label.as_deref(), Some("laptop"));
    }

    #[test]
    fn device_add_attaches_the_user() {
        let f = files();
        run_user(
            UserCommand::Add { name: "fluo".into(), description: None },
            &f.users,
        )
        .unwrap();

        run_device(
            DeviceCommand::Add {
                name: "laptop".into(),
                description: None,
                user: Some("fluo".into()),
                expires_in: None,
            },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        let users = Users::load(&f.users).unwrap();
        let user_id = users.resolve("fluo").unwrap().id;
        let devices = Devices::load(&f.devices).unwrap();
        assert_eq!(devices.resolve("laptop").unwrap().user_id, Some(user_id));
    }

    #[test]
    fn device_add_errors_on_an_unknown_user_before_writing_anything() {
        let f = files();

        let result = run_device(
            DeviceCommand::Add {
                name: "laptop".into(),
                description: None,
                user: Some("nobody".into()),
                expires_in: None,
            },
            &f.devices,
            &f.users,
            &f.keys,
        );

        assert!(result.is_err());
        assert!(
            Devices::load(&f.devices).unwrap().entries().is_empty(),
            "失敗したのにデバイス行が残っている"
        );
        assert!(
            KeyStore::load(&f.keys).unwrap().entries().is_empty(),
            "失敗したのに鍵が残っている"
        );
    }

    /// `add` は行を先に書くので、中断すると鍵の無い行が残る。再実行が
    /// 「名前が重複」で行き止まりになると、そこから抜ける手段が無い
    /// （`rotate` は既存の鍵を要求する）。
    #[test]
    fn device_add_finishes_a_row_that_has_no_key_yet() {
        let f = files();
        let id = Devices::load(&f.devices)
            .unwrap()
            .add("laptop", None, None)
            .unwrap()
            .id;

        add(&f, "laptop").unwrap();

        let keys = KeyStore::load(&f.keys).unwrap();
        assert_eq!(keys.entries().len(), 1);
        assert_eq!(keys.entries()[0].device_id, Some(id), "行の id が変わった");
    }

    /// 鍵の無い行に `--description` / `--user` を付けて `add` を再実行すると、
    /// `devices.purge` → `devices.add` の 2 回書きで行を作り直す。反映される
    /// ことと、鍵が 1 本だけ発行されることをここで固定する。
    #[test]
    fn device_add_resuming_a_keyless_row_applies_description_and_user() {
        let f = files();
        let old_id = Devices::load(&f.devices)
            .unwrap()
            .add("laptop", None, None)
            .unwrap()
            .id;
        run_user(
            UserCommand::Add { name: "fluo".into(), description: None },
            &f.users,
        )
        .unwrap();
        let user_id = Users::load(&f.users).unwrap().resolve("fluo").unwrap().id;

        run_device(
            DeviceCommand::Add {
                name: "laptop".into(),
                description: Some("work laptop".into()),
                user: Some("fluo".into()),
                expires_in: None,
            },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        let devices = Devices::load(&f.devices).unwrap();
        let device = devices.resolve("laptop").unwrap();
        assert_eq!(
            device.description.as_deref(),
            Some("work laptop"),
            "description が反映されていない"
        );
        assert_eq!(device.user_id, Some(user_id), "user が反映されていない");
        assert_ne!(device.id, old_id, "purge + add で行の id は変わるはず");

        let keys = KeyStore::load(&f.keys).unwrap();
        assert_eq!(keys.entries().len(), 1, "鍵が複数発行されている");
        assert_eq!(
            keys.entries()[0].device_id,
            Some(device.id),
            "鍵が作り直した行を指していない"
        );
    }

    #[test]
    fn device_add_refuses_a_name_that_already_holds_a_key() {
        let f = files();
        add(&f, "laptop").unwrap();

        let err = add(&f, "laptop").unwrap_err().to_string();

        assert!(err.contains("rotate"), "逃げ道を示していない: {err}");
        assert_eq!(KeyStore::load(&f.keys).unwrap().entries().len(), 1);
    }

    #[test]
    fn device_add_refuses_a_retired_row() {
        let f = files();
        add(&f, "laptop").unwrap();
        run_device(
            DeviceCommand::Retire { selector: "laptop".into(), purge: false },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        let err = add(&f, "laptop").unwrap_err().to_string();

        // retired なデバイスは認証で必ず弾かれるので、鍵を出しても意味が
        // ない。「成功したのに何も通らないトークン」を作らせない。
        assert!(err.contains("retired"), "{err}");
        assert!(err.contains("--purge"), "逃げ道を示していない: {err}");
    }

    #[test]
    fn device_rotate_replaces_the_token_and_keeps_the_row() {
        let f = files();
        add(&f, "laptop").unwrap();
        let before = KeyStore::load(&f.keys).unwrap().entries()[0].clone();
        let device_id = Devices::load(&f.devices).unwrap().resolve("laptop").unwrap().id;

        run_device(
            DeviceCommand::Rotate { selector: "laptop".into(), expires_in: None },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        let keys = KeyStore::load(&f.keys).unwrap();
        assert_eq!(keys.entries().len(), 1);
        let after = &keys.entries()[0];
        assert_ne!(after.token, before.token, "トークンが差し替わっていない");
        assert_eq!(after.id, before.id, "鍵の id は保たれる");
        assert_eq!(after.device_id, Some(device_id), "鍵がデバイスを指さなくなった");
        assert!(after.rotated_at.is_some());
        assert_eq!(
            Devices::load(&f.devices).unwrap().resolve("laptop").unwrap().id,
            device_id,
            "デバイス行の id が変わった"
        );
    }

    /// `add` は `label = 名前` で鍵を発行するが、`devices.toml` は手編集され得て、
    /// リネームは鍵ファイルの label に伝播しない。名前で鍵を引くと、リネーム後の
    /// `rotate` が別の鍵を掴むか、何も見つけられなくなる。
    #[test]
    fn device_rotate_finds_the_key_by_device_id_not_by_label() {
        let f = files();
        add(&f, "laptop").unwrap();
        let key_id = KeyStore::load(&f.keys).unwrap().entries()[0].id;
        // 台帳側だけ名前を変える（手編集の再現）。
        let text = std::fs::read_to_string(&f.devices).unwrap();
        std::fs::write(&f.devices, text.replace("\"laptop\"", "\"desktop\"")).unwrap();

        run_device(
            DeviceCommand::Rotate { selector: "desktop".into(), expires_in: None },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        let keys = KeyStore::load(&f.keys).unwrap();
        assert_eq!(keys.entries().len(), 1);
        assert_eq!(keys.entries()[0].id, key_id, "別の鍵を作ってしまっている");
        assert!(keys.entries()[0].rotated_at.is_some(), "鍵が差し替わっていない");
    }

    #[test]
    fn device_rotate_refuses_a_retired_row() {
        let f = files();
        add(&f, "laptop").unwrap();
        run_device(
            DeviceCommand::Retire { selector: "laptop".into(), purge: false },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        let err = run_device(
            DeviceCommand::Rotate { selector: "laptop".into(), expires_in: None },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap_err()
        .to_string();

        // retired なデバイスは認証で必ず弾かれるので、rotate しても意味が
        // ない。「成功したのに何も通らないトークン」を作らせない。
        assert!(err.contains("retired"), "{err}");
        assert!(err.contains("--purge"), "逃げ道を示していない: {err}");
    }

    #[test]
    fn device_rotate_errors_when_the_device_has_no_key_here() {
        let f = files();
        Devices::load(&f.devices).unwrap().add("laptop", None, None).unwrap();

        let err = run_device(
            DeviceCommand::Rotate { selector: "laptop".into(), expires_in: None },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("device add"), "逃げ道を示していない: {err}");
    }

    #[test]
    fn device_rotate_sets_a_new_expiry_and_drops_the_old_one_when_omitted() {
        let f = files();
        run_device(
            DeviceCommand::Add {
                name: "laptop".into(),
                description: None,
                user: None,
                expires_in: Some("90d".into()),
            },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        run_device(
            DeviceCommand::Rotate { selector: "laptop".into(), expires_in: None },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        // framework の `rotate` は expires_at を保持ではなく置き換える。
        // 意図した挙動であることをここで固定する。
        assert!(
            KeyStore::load(&f.keys).unwrap().entries()[0].expires_at.is_none(),
            "期限が引き継がれてしまっている"
        );
    }

    #[test]
    fn device_retire_revokes_the_key_and_keeps_the_row() {
        let f = files();
        add(&f, "laptop").unwrap();

        run_device(
            DeviceCommand::Retire { selector: "laptop".into(), purge: false },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        assert!(
            KeyStore::load(&f.keys).unwrap().entries().is_empty(),
            "鍵が生きたまま残っている"
        );
        // 行は残す —— content に焼かれた device_id が解決し続けるように。
        assert!(Devices::load(&f.devices).unwrap().resolve("laptop").unwrap().is_retired());
    }

    #[test]
    fn device_retire_with_purge_removes_the_row() {
        let f = files();
        add(&f, "laptop").unwrap();

        run_device(
            DeviceCommand::Retire { selector: "laptop".into(), purge: true },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        assert!(Devices::load(&f.devices).unwrap().entries().is_empty());
        assert!(KeyStore::load(&f.keys).unwrap().entries().is_empty());
    }

    #[test]
    fn device_list_runs_with_and_without_rows() {
        let f = files();
        run_device(DeviceCommand::List, &f.devices, &f.users, &f.keys).unwrap();

        add(&f, "laptop").unwrap();

        run_device(DeviceCommand::List, &f.devices, &f.users, &f.keys).unwrap();
    }

    #[test]
    fn device_add_errors_instead_of_panicking_on_an_absurd_expiry() {
        let f = files();

        let result = run_device(
            DeviceCommand::Add {
                name: "laptop".into(),
                description: None,
                user: None,
                expires_in: Some("99999999999d".into()),
            },
            &f.devices,
            &f.users,
            &f.keys,
        );

        assert!(result.is_err(), "表現できない期限が受理された");
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
