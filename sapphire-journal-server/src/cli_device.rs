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
/// 発行し、**行には一切触れない**（他ホストから同期されてきた行かもしれない）。
/// この分岐が無いと中断状態から抜ける手段が無い（`rotate` は既存の鍵を要求する）。
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

            // 名前の**完全一致**だけを見る。`resolve` は名前で見つからないと
            // セレクタを grain-id として読み直すので、`--name` に他のデバイスの
            // id 文字列を渡されると、無関係な行が「同名の既存行」に見えてしまう。
            // ここで問いたいのは `Devices::add` が拒否するもの —— 名前の重複 ——
            // ただ 1 つ。
            //
            // なお `devices` は別の腕で可変借用するので、参照を match をまたいで
            // 持てない。所有したコピーを先に取る。
            let existing = devices.entries().iter().find(|d| d.name == name).cloned();
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
                    // 再開は**鍵だけ**発行する。行には触れない。
                    //
                    // 「このホストに鍵が無い」は「この行は一度も使われていない」
                    // ではない。`devices.toml` は同期される一方、`keys.toml` は
                    // ホストローカル —— 他のホストで作られた行は、鍵を持たない
                    // 姿でここに同期されてくる。その行を作り直すと、向こうの
                    // ホストの鍵を孤児にし（401。そのホストの唯一の鍵なら
                    // サーバが起動しなくなる）、content に焼かれた
                    // `updated_by: <旧 id>` を解決不能にする。`retire` が既定で
                    // purge しないのと同じ理由がここにも効く。
                    //
                    // だから `--description` / `--user` は反映しない。エラーに
                    // もしない —— この分岐は「中断状態から抜ける唯一の道」で、
                    // ここを塞ぐと行き止まりが戻ってくる。効かなかったことを
                    // 警告で名指しして、鍵は出す。
                    let mut ignored: Vec<&str> = Vec::new();
                    if description.is_some() && description != existing.description {
                        ignored.push("--description");
                    }
                    if user_id.is_some() && user_id != existing.user_id {
                        ignored.push("--user");
                    }
                    if !ignored.is_empty() {
                        eprintln!(
                            "warning: {} not applied — device {name:?} already exists, and \
                             resuming `device add` only mints the missing key; it never \
                             rewrites the row, whose id may already be written into content. \
                             Edit {} by hand to change it, or run \
                             `sapphire-journal-server device retire {name} --purge` and add \
                             it again to start over with a new id",
                            ignored.join(" and "),
                            devices_file.display(),
                        );
                    }
                    existing
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
            //
            // そして**該当する鍵を全部**集める。`add` は 1 デバイスに 1 本しか
            // 出さないが、鍵ファイルの手編集や `add` の競合で 2 本になり得る。
            // 1 本目だけ消すと、「引退させた」と報告しながら生きた鍵が残り、
            // `--purge` はそれを孤児にする。
            let key_ids: Vec<String> = keys
                .entries()
                .iter()
                .filter(|k| k.device_id == Some(device.id))
                .map(|k| k.id.to_string())
                .collect();
            let had_key = !key_ids.is_empty();
            // 鍵を先に失効させる。引退の目的は「今すぐ止める」ことなので、
            // 2 つの書き込みの間で落ちても生きた鍵を残さない。
            for key_id in &key_ids {
                keys.revoke(key_id)?;
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

    /// 鍵の無い行に `--description` / `--user` を付けて `add` を再実行しても、
    /// **行には触れない**。`devices.toml` は同期される一方で `keys.toml` は
    /// ホストローカルなので、「このホストに鍵が無い」＝「一度も使われていない」
    /// ではない —— 他ホストから同期されてきた行を作り直すと、向こうの鍵を
    /// 孤児にし、content に焼かれた `updated_by` を解決不能にする。
    ///
    /// エラーにもしない。この分岐は中断状態から抜ける唯一の道で、塞ぐと
    /// 行き止まりが戻ってくる。
    #[test]
    fn device_add_resuming_a_keyless_row_mints_only_the_key() {
        let f = files();
        let old = Devices::load(&f.devices)
            .unwrap()
            .add("laptop", Some("the original note".into()), None)
            .unwrap();
        run_user(
            UserCommand::Add { name: "fluo".into(), description: None },
            &f.users,
        )
        .unwrap();

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
        assert_eq!(devices.entries().len(), 1, "行が増えている");
        let device = devices.resolve("laptop").unwrap();
        assert_eq!(device.id, old.id, "行の id が変わった");
        assert_eq!(device.created_at, old.created_at, "行が作り直されている");
        assert_eq!(
            device.description.as_deref(),
            Some("the original note"),
            "description が上書きされた"
        );
        assert_eq!(device.user_id, None, "user が上書きされた");
        assert!(!device.is_retired());

        let keys = KeyStore::load(&f.keys).unwrap();
        assert_eq!(keys.entries().len(), 1, "鍵が複数発行されている");
        assert_eq!(
            keys.entries()[0].device_id,
            Some(old.id),
            "鍵が既存の行を指していない"
        );
    }

    /// 空の行（description も user も無い）に `--user` を付けて再開しても、
    /// 同じく行は据え置き。`--user` は解決だけされて捨てられる —— ただし
    /// 存在しないユーザーは（何も書く前に）エラーのままであること。
    #[test]
    fn device_add_resuming_still_rejects_an_unknown_user() {
        let f = files();
        Devices::load(&f.devices).unwrap().add("laptop", None, None).unwrap();

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

        assert!(result.is_err(), "存在しないユーザーが受理された");
        assert!(
            KeyStore::load(&f.keys).unwrap().entries().is_empty(),
            "失敗したのに鍵が残っている"
        );
    }

    /// `Devices::resolve` は名前で見つからないとセレクタを grain-id として
    /// 読み直す。既存判定をそれに任せると、`--name` に他のデバイスの id 文字列
    /// を渡したときに、無関係な行が「同名の既存行」として拾われる。
    /// `Devices::add` が拒否するのは名前の重複だけなので、判定も名前の完全一致
    /// で行う。
    #[test]
    fn device_add_does_not_mistake_another_devices_id_for_an_existing_name() {
        let f = files();
        add(&f, "laptop").unwrap();
        let laptop = Devices::load(&f.devices).unwrap().resolve("laptop").unwrap().clone();

        add(&f, &laptop.id.to_string()).unwrap();

        let devices = Devices::load(&f.devices).unwrap();
        assert_eq!(devices.entries().len(), 2, "既存の行と取り違えている");
        let keys = KeyStore::load(&f.keys).unwrap();
        assert_eq!(keys.entries().len(), 2);
        assert_eq!(
            keys.entries()
                .iter()
                .filter(|k| k.device_id == Some(laptop.id))
                .count(),
            1,
            "無関係なデバイスに 2 本目の鍵が出ている"
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

    /// `add` は 1 デバイス 1 鍵にするが、鍵ファイルの手編集や `add` の競合で
    /// 2 本になり得る。1 本しか失効させないと、「引退させた」と報告しながら
    /// 生きた鍵が残る。
    #[test]
    fn device_retire_revokes_every_key_bound_to_the_device() {
        let f = files();
        add(&f, "laptop").unwrap();
        let device_id = Devices::load(&f.devices).unwrap().resolve("laptop").unwrap().id;
        // 同じデバイスを指す 2 本目（手編集や競合した `add` の再現）。
        KeyStore::load(&f.keys)
            .unwrap()
            .generate(
                TOKEN_PREFIX,
                None,
                Some(device_id),
                Some("laptop".into()),
                None,
            )
            .unwrap();
        assert_eq!(KeyStore::load(&f.keys).unwrap().entries().len(), 2);

        run_device(
            DeviceCommand::Retire { selector: "laptop".into(), purge: false },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();

        assert!(
            KeyStore::load(&f.keys).unwrap().entries().is_empty(),
            "2 本目の鍵が生きたまま残っている"
        );
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
