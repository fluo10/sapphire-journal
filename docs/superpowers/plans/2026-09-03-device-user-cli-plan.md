# 鍵コマンドのデバイス／ユーザー管理コマンド移行 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `sapphire-journal-server` の `gen-key` / `list-keys` / `rotate-key` /
`revoke-key` を削除し、`device` / `user` サブコマンドと、デバイス台帳を経由する
fail-closed な認証に置き換える。

**Architecture:** 台帳は framework の `sapphire-framework-registry`
（`Devices` / `Users`）を使い、同期される `<root>/.sapphire-journal/{devices,users}.toml`
に置く。鍵は従来通りキャッシュディレクトリの `keys.toml`。`device add` が
台帳の行と鍵を 1 度に作り、鍵の `device_id` が行を指す。認証は
`src/device_auth.rs` の `DeviceAuth` が持ち、`/rpc` と `/mcp` を束ねた Router の
最外周に 1 枚レイヤとして被せる。

**Tech Stack:** Rust 2024 / clap 4 (derive) / axum 0.8 / anyhow / chrono /
`sapphire-framework`（`remote-server` と `registry` フィーチャ）/ tokio + tower（テスト）

**Spec:** `docs/superpowers/specs/2026-09-03-device-user-cli-design.md`

## Global Constraints

- **トークン接頭辞は `sjt`。** `keys::TOKEN_PREFIX` を変えない。
- **出力規約:** トークンだけ stdout、メタデータは stderr。
- **`--expires-in` の書式:** `90d` / `12h` / `30m`。単位必須、0 と負は拒否、範囲外は
  panic ではなくエラー。既存の `keys::parse_duration` をそのまま使い、書き直さない。
- **`rotate` は期限を保持ではなく置き換える。** ヘルプに明記する。
- **動いているサーバに効くのは再起動時。** `rotate` と `retire` の両方が stderr に
  この注意を出す。
- **fail-closed。** 「台帳に無い鍵を通す」トグルは作らない。
- **失敗理由をレスポンスに出さない。** 認証の失敗はすべて 401、区別は `debug!` のみ。
- **コメントは日本語**（このクレートの既存コードに合わせる）。テストの assert
  メッセージも日本語。
- **既存の逃げ道は残さない。** `gen-key` 等の別名や互換シムを作らない。
- 全タスクで `cargo test -p sapphire-journal-server` が緑であること。
  `cargo clippy -p sapphire-journal-server --all-targets` も警告なしにする。

## File Structure

| ファイル | 役割 |
|---|---|
| `sapphire-journal-server/Cargo.toml` | `sapphire-framework` に `registry` フィーチャを足す |
| `src/cli.rs` | トップレベルの CLI。`Command` を `Device` / `User` の 2 変種に置き換える |
| `src/cli_device.rs`（新規） | `DeviceCommand` / `UserCommand` と `run_device` / `run_user`。台帳と鍵ファイルへの書き込みはここだけ |
| `src/keys.rs` | CLI 実体を抜き、`TOKEN_PREFIX` / `parse_duration` / `absolute_expiry` のヘルパに縮小 |
| `src/device_auth.rs`（新規） | `DeviceAuth`（トークン → デバイスの解決、起動時ガードの判定材料）と axum レイヤ |
| `src/serve.rs` | `default_devices_path` / `default_users_path` を追加。`build_router` に `DeviceAuth` を受け取らせ、`run_until` の起動ガードを差し替える |
| `src/main.rs` | サブコマンド別のパス解決 |
| `tests/harness/mod.rs` | 鍵の発行を `device add` 経由にする |
| `tests/retire.rs`（`revoke.rs` を改名） | `device retire` 後に 401 になる鎖 |
| `tests/device_auth.rs`（新規） | fail-closed の各条件が 401 になること |
| `tests/no_keys.rs` | 起動ガードの新しい条件 |
| `sapphire-journal-server/README.md` | 手順と認証の説明 |

---

### Task 1: `user` サブコマンドと台帳のパス

**Files:**
- Modify: `sapphire-journal-server/Cargo.toml`
- Create: `sapphire-journal-server/src/cli_device.rs`
- Modify: `sapphire-journal-server/src/lib.rs`
- Modify: `sapphire-journal-server/src/serve.rs`（`default_keys_path` の直後に追記）
- Modify: `sapphire-journal-server/src/cli.rs`
- Modify: `sapphire-journal-server/src/main.rs`

**Interfaces:**
- Consumes: `sapphire_journal_core::journal::Journal::journal_dir()`
- Produces:
  - `serve::default_devices_path(journal_dir: &Path) -> anyhow::Result<PathBuf>`
  - `serve::default_users_path(journal_dir: &Path) -> anyhow::Result<PathBuf>`
  - `cli_device::UserCommand`（`Add { name: String, description: Option<String> }`, `List`）
  - `cli_device::run_user(command: UserCommand, users_file: &Path) -> anyhow::Result<()>`
  - `cli::Command::User { command: cli_device::UserCommand }`

このタスクは追加だけで、既存の `gen-key` 等には触らない。

- [ ] **Step 1: `registry` フィーチャを足す**

`sapphire-journal-server/Cargo.toml` の該当行を差し替える:

```toml
sapphire-framework = { version = "0.1", git = "https://github.com/fluo10/sapphire-framework", branch = "main", default-features = false, features = ["remote-server", "registry"] }
```

- [ ] **Step 2: 台帳パスの失敗するテストを書く**

`src/serve.rs` の `mod tests` の末尾に足す（このファイルに `mod tests` が無い場合は
ファイル末尾に新設する）:

```rust
#[test]
fn the_registry_files_live_in_the_journal_marker_dir() {
    // 台帳は同期に乗る場所（`.sapphire-journal/`）に置く。鍵ファイルだけが
    // キャッシュ側に残る —— この 2 つが同じ親に並んだら、トークンが同期で
    // クライアントに配られる。
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    std::fs::create_dir_all(journal_dir.join(".sapphire-journal")).unwrap();
    std::fs::write(
        journal_dir.join(".sapphire-journal").join("config.toml"),
        "",
    )
    .unwrap();

    let devices = default_devices_path(&journal_dir).unwrap();
    let users = default_users_path(&journal_dir).unwrap();

    assert_eq!(devices.file_name().unwrap(), "devices.toml");
    assert_eq!(users.file_name().unwrap(), "users.toml");
    assert_eq!(devices.parent(), users.parent());
    assert_eq!(
        devices.parent().unwrap().file_name().unwrap(),
        ".sapphire-journal",
        "台帳がマーカーディレクトリの外に出ている"
    );
    let keys = default_keys_path(&journal_dir).unwrap();
    assert_ne!(
        keys.parent(),
        devices.parent(),
        "鍵ファイルが台帳と同じ（同期される）場所にある"
    );
}
```

- [ ] **Step 3: テストが落ちることを確認する**

Run: `cargo test -p sapphire-journal-server --lib the_registry_files_live_in_the_journal_marker_dir`
Expected: コンパイルエラー（`default_devices_path` が無い）

- [ ] **Step 4: パスヘルパを実装する**

`src/serve.rs` の `default_keys_path` の直後に追記:

```rust
/// デバイス台帳の位置。鍵ファイルと違い、**journal ルートの中**（マーカー
/// ディレクトリ）に置く —— `updated_by` として content に焼かれた device_id を
/// 別のホストで名前に逆引きするには、台帳が content と一緒に同期される必要が
/// ある。秘密は入らない（トークンは `keys.toml` に留まる）。
pub fn default_devices_path(journal_dir: &Path) -> anyhow::Result<std::path::PathBuf> {
    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    Ok(journal.journal_dir().join("devices.toml"))
}

/// ユーザー台帳の位置。[`default_devices_path`] と同じディレクトリ。
pub fn default_users_path(journal_dir: &Path) -> anyhow::Result<std::path::PathBuf> {
    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    Ok(journal.journal_dir().join("users.toml"))
}
```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server --lib the_registry_files_live_in_the_journal_marker_dir`
Expected: PASS

- [ ] **Step 6: `run_user` の失敗するテストを書く**

`src/cli_device.rs` を新規作成し、まずテストだけを置く:

```rust
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
    let _ = (command, users_file);
    unimplemented!()
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
```

`src/lib.rs` に `pub mod cli_device;` を足す（`pub mod cli;` の直後）。

- [ ] **Step 7: テストが落ちることを確認する**

Run: `cargo test -p sapphire-journal-server --lib cli_device`
Expected: 3 件とも `unimplemented!()` の panic で FAIL

- [ ] **Step 8: `run_user` を実装する**

`src/cli_device.rs` の `run_user` を差し替える:

```rust
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
```

- [ ] **Step 9: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server --lib cli_device`
Expected: PASS（3 件）

- [ ] **Step 10: CLI の失敗するテストを書く**

`src/cli.rs` の `mod tests` に足す:

```rust
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
```

- [ ] **Step 11: テストが落ちることを確認する**

Run: `cargo test -p sapphire-journal-server --lib cli::`
Expected: コンパイルエラー（`Command::User` が無い）

- [ ] **Step 12: `Command::User` を足す**

`src/cli.rs` の `Command` に変種を足す（既存の `GenKey` 等はこのタスクでは残す）:

```rust
    /// Manage the users devices belong to.
    User {
        #[command(subcommand)]
        command: crate::cli_device::UserCommand,
    },
```

- [ ] **Step 13: `main.rs` を配線する**

`src/main.rs` の `match cli.command` の `Some(command)` 腕の**前**に、`User` 専用の
腕を足す:

```rust
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
```

`main.rs` の `use` に `use sapphire_journal_server::cli::Command;` を足す
（`mod tests` の中だけで使われていたので、本体にも要る）。

- [ ] **Step 14: 全体を通す**

Run: `cargo test -p sapphire-journal-server && cargo clippy -p sapphire-journal-server --all-targets`
Expected: PASS、警告なし

- [ ] **Step 15: コミット**

```bash
git add sapphire-journal-server/Cargo.toml sapphire-journal-server/src/{lib,cli,cli_device,serve,main}.rs Cargo.lock
git commit -m "feat(server): add the user subcommand and registry paths"
```

---

### Task 2: `device` サブコマンド

**Files:**
- Modify: `sapphire-journal-server/src/cli_device.rs`
- Modify: `sapphire-journal-server/src/keys.rs`（`absolute_expiry` を公開する）
- Modify: `sapphire-journal-server/src/cli.rs`
- Modify: `sapphire-journal-server/src/main.rs`

**Interfaces:**
- Consumes: `serve::default_devices_path`, `serve::default_users_path`,
  `keys::TOKEN_PREFIX`, `keys::parse_duration`
- Produces:
  - `keys::absolute_expiry(expires_in: Option<&str>) -> anyhow::Result<Option<DateTime<Utc>>>`（`pub` 化）
  - `cli_device::DeviceCommand`
    （`Add { name: String, description: Option<String>, user: Option<String>, expires_in: Option<String> }`,
    `List`, `Rotate { selector: String, expires_in: Option<String> }`,
    `Retire { selector: String, purge: bool }`）
  - `cli_device::run_device(command: DeviceCommand, devices_file: &Path, users_file: &Path, keys_file: &Path) -> anyhow::Result<()>`
  - `cli::Command::Device { command: cli_device::DeviceCommand }`

- [ ] **Step 1: `absolute_expiry` を公開する**

`src/keys.rs` の `fn absolute_expiry` を `pub fn absolute_expiry` にする。ドキュメント
コメントはそのまま。

- [ ] **Step 2: 失敗するテストを書く（1/2 — 正常系と再開）**

`src/cli_device.rs` の `mod tests` の `Files` に、Task 1 で省いた 2 つを足す:

```rust
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
```

その下に足す:

```rust
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
```

- [ ] **Step 3: 失敗するテストを書く（2/2 — rotate / retire / list）**

同じ `mod tests` に続けて足す:

```rust
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
```

- [ ] **Step 4: テストが落ちることを確認する**

Run: `cargo test -p sapphire-journal-server --lib cli_device`
Expected: コンパイルエラー（`DeviceCommand` / `run_device` が無い）

- [ ] **Step 5: `DeviceCommand` と `run_device` を実装する**

`src/cli_device.rs` の `use` を差し替え、`UserCommand` の前に `DeviceCommand` を、
`run_user` の前に `run_device` を置く:

```rust
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
```

`mod tests` の先頭の `use super::*;` に加えて、テストが使う型を足す:

```rust
    use sapphire_framework::registry::{Devices, Users};
    use sapphire_framework::remote_server::KeyStore;
```

（`use super::*;` で入るものと重複する場合は重複分を削る。）

- [ ] **Step 6: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server --lib cli_device`
Expected: PASS（全件）

- [ ] **Step 7: CLI パースの失敗するテストを書く**

`src/cli.rs` の `mod tests` に足す:

```rust
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
```

- [ ] **Step 8: テストが落ちることを確認する**

Run: `cargo test -p sapphire-journal-server --lib cli::`
Expected: コンパイルエラー（`Command::Device` が無い）

- [ ] **Step 9: `Command::Device` を足して配線する**

`src/cli.rs` の `Command` に:

```rust
    /// Manage the devices that authenticate to this server.
    Device {
        #[command(subcommand)]
        command: crate::cli_device::DeviceCommand,
    },
```

`src/main.rs` の `Some(Command::User { .. })` の腕の直後に:

```rust
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
```

- [ ] **Step 10: 全体を通す**

Run: `cargo test -p sapphire-journal-server && cargo clippy -p sapphire-journal-server --all-targets`
Expected: PASS、警告なし

- [ ] **Step 11: コミット**

```bash
git add sapphire-journal-server/src/{cli,cli_device,keys,main}.rs
git commit -m "feat(server): add the device subcommand backed by the registry"
```

---

### Task 3: 旧鍵コマンドの削除

**Files:**
- Modify: `sapphire-journal-server/src/cli.rs`
- Modify: `sapphire-journal-server/src/keys.rs`
- Modify: `sapphire-journal-server/src/main.rs`
- Modify: `sapphire-journal-server/tests/harness/mod.rs`
- Delete: `sapphire-journal-server/tests/revoke.rs`
- Create: `sapphire-journal-server/tests/retire.rs`

**Interfaces:**
- Consumes: `cli_device::run_device`, `serve::default_devices_path`
- Produces: `keys` モジュールは `TOKEN_PREFIX` / `parse_duration` /
  `absolute_expiry` だけを持つ。`keys::run` と `cli::Command::{GenKey, ListKeys,
  RotateKey, RevokeKey}` は存在しなくなる。

- [ ] **Step 1: `cli.rs` から旧変種とそのテストを消す**

`src/cli.rs` の `Command` から `GenKey` / `ListKeys` / `RotateKey` / `RevokeKey` を
削除する。`mod tests` から次のテストを削除する:

- `gen_key_takes_an_optional_label`
- `revoke_key_requires_a_selector`
- `rotate_key_requires_a_selector`
- `rotate_key_takes_a_selector_and_an_optional_expiry`

`no_subcommand_means_serve` / `addr_is_a_single_socket_addr` /
`allowed_host_*` は残す。ファイル冒頭のモジュールコメント
（「鍵の管理だけがサブコマンド」）を実態に合わせて書き換える:

```rust
//! コマンドライン引数。
//!
//! `serve` は既定動作なのでサブコマンドを持たない。サブコマンドはデバイスと
//! ユーザーの台帳管理だけ。
```

- [ ] **Step 2: `keys.rs` を縮小する**

`src/keys.rs` から次を削除する: `mask_token`, `format_key_line`, `run`、および
それらに対応するテスト（`list_keys_*`, `mask_token_*`, `gen_key_*`, `revoke_key_*`,
`rotate_key_*`, `expires_in_becomes_an_absolute_time`）。

残すのは `TOKEN_PREFIX` / `parse_duration` / `absolute_expiry` と、
`parse_duration_*` のテスト 4 件。`absolute_expiry` の範囲外テストは
`cli_device` 側（`device_add_errors_instead_of_panicking_on_an_absurd_expiry`）が
持っているので、ここには置き直さない。モジュールコメントを差し替える:

```rust
//! 鍵の有効期限まわりのヘルパ。
//!
//! 鍵ファイルの形式・生成・検証は framework の `KeyStore` が持ち、それを呼ぶ
//! 入口は [`crate::cli_device`]。ここに残るのは、相対的な有効期間を絶対時刻へ
//! 直す変換と、このアプリのトークン接頭辞だけ。
```

不要になった `use` を落とす（`chrono::Utc` は `absolute_expiry` が使うので残る）。

- [ ] **Step 3: `main.rs` の古いパス解決を消す**

`keys_path_for_key_command` と、それを使う `Some(command) => { ... }` の腕を削除する
（Task 1 と 2 で足した `User` / `Device` の腕が置き換える）。`mod tests` から
`a_key_command_with_explicit_keys_needs_no_journal_dir` と
`the_command_is_a_key_command_only_when_one_was_given` を削除し、
`a_key_command_without_either_explains_itself_in_its_own_terms` を次で置き換える
（`--keys` があっても `--journal-dir` が要る、という新しい方針を固定する）:

```rust
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
```

これを可能にするため、`main` の `match cli.command { ... }` の本体を
`fn run_command_for_test` ではなく、テストから呼べる形に切り出す。`main.rs` に
次の関数を作り、`main` はこれを呼ぶだけにする:

```rust
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
```

`main` の match は次になる:

```rust
    match cli.command.take() {
        Some(command) => run_registry_command(&cli, command),
        None => { /* 既存の serve の腕をそのまま */ }
    }
```

（`cli` を `let mut cli = Cli::parse();` にし、`journal`/`keys` のローカル束縛は
serve の腕の中で `cli` から取り直す。）

テスト内の `run_command_for_test` は `run_registry_command` の別名ではなく、
そのまま `run_registry_command(&cli, cli.command.clone().unwrap())` を呼ぶ小さな
ヘルパにする:

```rust
    fn run_command_for_test(mut cli: Cli) -> anyhow::Result<()> {
        let command = cli.command.take().expect("サブコマンドが要る");
        run_registry_command(&cli, command)
    }
```

`Cli` と `Command` に `Clone` は不要（`take()` で足りる）。

- [ ] **Step 4: テストが落ちる（＝コンパイルが通らない）ことを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: `tests/harness/mod.rs` と `tests/revoke.rs` が `keys::run` /
`Command::GenKey` を参照していてコンパイルエラー

- [ ] **Step 5: harness を `device add` 経由にする**

`tests/harness/mod.rs` の `spawn_with_allowed_hosts` の鍵発行部分を差し替える:

```rust
    let keys_path = tmp.path().join("keys.toml");
    let token = mint_device_key(&journal_dir, &keys_path, "test");
```

そしてファイル末尾（`init_journal` の隣）に足す:

```rust
/// `device add` を通してデバイス行と鍵を作り、トークンを返す。
///
/// 本物のサブコマンドを通す。トークンは stdout に出て掴まえられないので、
/// 鍵ファイルから読み直す —— 運用者が「なくしたら鍵ファイルを見ればいい」と
/// 案内されているのと同じ経路。
pub fn mint_device_key(
    journal_dir: &std::path::Path,
    keys_path: &std::path::Path,
    name: &str,
) -> String {
    let devices_path = sapphire_journal_server::serve::default_devices_path(journal_dir).unwrap();
    let users_path = sapphire_journal_server::serve::default_users_path(journal_dir).unwrap();
    sapphire_journal_server::cli_device::run_device(
        sapphire_journal_server::cli_device::DeviceCommand::Add {
            name: name.to_owned(),
            description: None,
            user: None,
            expires_in: None,
        },
        &devices_path,
        &users_path,
        keys_path,
    )
    .unwrap();
    let store = sapphire_framework::remote_server::KeyStore::load(keys_path).unwrap();
    store
        .entries()
        .iter()
        .find(|k| k.label.as_deref() == Some(name))
        .expect("いま発行した鍵が見つからない")
        .token
        .clone()
}
```

- [ ] **Step 6: `revoke.rs` を `retire.rs` に置き換える**

```bash
git mv sapphire-journal-server/tests/revoke.rs sapphire-journal-server/tests/retire.rs
```

`tests/retire.rs` の冒頭コメントと 2 箇所の `keys::run` 呼び出しを差し替える。
モジュールコメント:

```rust
//! 仕様が求めている鎖: `device add` → 両ルートが通る → `device retire` → 401。
//!
//! `KeyStore` も台帳も起動時に読んだスナップショットなので、引退が効くのは
//! サーバを組み直してからになる。だからこのテストも 2 段階で組み立てる ——
//! 「retire してもプロセスを再起動するまで通り続ける」という**現在の仕様**を
//! 変えたら、ここが 1 段階目で落ちて気づける。
//!
//! 2 つの世代は journal を別々に持つ。鍵ファイルだけが共有物。同じ journal を
//! 2 回開くと、まだ生きているほうの redb を掴んで「Database already open」に
//! なるだけで、確かめたい事柄とは無関係な失敗が混ざる。
//!
//! **台帳は 1 つ目の journal 側に置く。** `device retire` はそこを引く。
```

`use sapphire_journal_server::cli::Command;` を
`use sapphire_journal_server::cli_device::DeviceCommand;` に変える。

最初の `gen-key` ブロックを:

```rust
    let token = harness::mint_device_key(&journal_dir, &keys_path, "laptop");
```

`revoke-key` + 差し替え鍵のブロックを:

```rust
    // ── device retire ───────────────────────────────────────────────────────
    sapphire_journal_server::cli_device::run_device(
        DeviceCommand::Retire { selector: "laptop".into(), purge: false },
        &sapphire_journal_server::serve::default_devices_path(&journal_dir).unwrap(),
        &sapphire_journal_server::serve::default_users_path(&journal_dir).unwrap(),
        &keys_path,
    )
    .unwrap();

    // 引退させたら鍵は 0 本。`serve::run` はこの状態では bind すらしない
    // （tests/no_keys.rs）ので、401 を確かめるには生きたデバイスを 1 つ
    // 足しておく必要がある —— それが通ることが、401 の原因が「サーバが
    // 死んでいる」ではなく「そのデバイスが引退した」ことの対照になる。
    let replacement = harness::mint_device_key(&journal_dir, &keys_path, "replacement");
    assert_ne!(replacement, token);
```

2 段階目の router を組む部分（`journal_after` を使う側）はそのまま残す。ただし
`build_router` の呼び出しは Task 4 で引数が増えるので、このタスクでは触らない。

- [ ] **Step 7: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS（`retire.rs` を含む全テストバイナリ）

- [ ] **Step 8: clippy**

Run: `cargo clippy -p sapphire-journal-server --all-targets`
Expected: 警告なし

- [ ] **Step 9: コミット**

```bash
git add -A sapphire-journal-server
git commit -m "feat(server)!: remove gen-key/list-keys/rotate-key/revoke-key"
```

---

### Task 4: デバイス基準の認証（fail-closed）

**Files:**
- Create: `sapphire-journal-server/src/device_auth.rs`
- Modify: `sapphire-journal-server/src/lib.rs`
- Modify: `sapphire-journal-server/src/serve.rs`（`build_router`, `run_until`）
- Modify: `sapphire-journal-server/tests/harness/mod.rs`
- Modify: `sapphire-journal-server/tests/retire.rs`
- Modify: `sapphire-journal-server/tests/no_keys.rs`（起動ガードの文言だけ）
- Create: `sapphire-journal-server/tests/device_auth.rs`

**Interfaces:**
- Consumes: `serve::default_devices_path`, `cli_device::run_device`
- Produces:
  - `device_auth::DeviceAuth::load(keys_path: &Path, devices_path: &Path) -> anyhow::Result<Self>`
  - `DeviceAuth::resolve(&self, token: &str) -> Option<&sapphire_framework::registry::Device>`
  - `DeviceAuth::has_usable_device_key(&self) -> bool`
  - `DeviceAuth::orphan_key_count(&self) -> usize`
  - `device_auth::require_device(auth: Arc<DeviceAuth>, router: Router) -> Router`
  - `serve::build_router(state, journal_state, cancel, allowed_hosts, device_auth: Arc<DeviceAuth>) -> anyhow::Result<Router>`
    （**引数が 1 つ増える**。最後の位置）

- [ ] **Step 1: `DeviceAuth` の失敗するテストを書く**

`src/device_auth.rs` を新規作成し、まず型とテストだけを置く:

```rust
//! デバイス台帳を通した認証。
//!
//! framework の `protect` は `KeyStore` しか見ず、認証結果を extensions に
//! 挿すのは**内側**なので、その値を読むレイヤを外から張ることはできない。
//! よってここは Bearer を自分で読み、鍵 → `device_id` → 台帳の行、まで解決
//! できたリクエストだけを通す。
//!
//! これは起動時のスナップショット。`KeyStore` が既にそうであるように、
//! 再読み込みの経路は持たない —— `device rotate` / `device retire` が動いて
//! いるサーバに効くのは次の起動時。

use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::{Next, from_fn_with_state},
    response::Response,
};
use sapphire_framework::registry::{Device, Devices};
use sapphire_framework::remote_server::KeyStore;

pub struct DeviceAuth {
    keys: KeyStore,
    devices: Devices,
}

impl DeviceAuth {
    pub fn load(keys_path: &Path, devices_path: &Path) -> anyhow::Result<Self> {
        let _ = (keys_path, devices_path);
        unimplemented!()
    }

    pub fn resolve(&self, token: &str) -> Option<&Device> {
        let _ = token;
        unimplemented!()
    }

    pub fn has_usable_device_key(&self) -> bool {
        unimplemented!()
    }

    pub fn orphan_key_count(&self) -> usize {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sapphire_framework::registry::Devices;
    use sapphire_framework::remote_server::KeyStore;

    use super::*;
    use crate::cli_device::{DeviceCommand, run_device};

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

    fn add(f: &Files, name: &str) -> String {
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
        .unwrap();
        KeyStore::load(&f.keys)
            .unwrap()
            .entries()
            .iter()
            .find(|k| k.label.as_deref() == Some(name))
            .unwrap()
            .token
            .clone()
    }

    fn auth(f: &Files) -> DeviceAuth {
        DeviceAuth::load(&f.keys, &f.devices).unwrap()
    }

    #[test]
    fn a_live_device_resolves() {
        let f = files();
        let token = add(&f, "laptop");

        let device = auth(&f).resolve(&token).expect("生きたデバイスが弾かれた");

        assert_eq!(device.name, "laptop");
    }

    #[test]
    fn an_unknown_token_does_not_resolve() {
        let f = files();
        add(&f, "laptop");

        assert!(auth(&f).resolve("sjt_nope").is_none());
    }

    /// 移行前に `gen-key` で作られた鍵。台帳を経由しないので通さない。
    #[test]
    fn a_key_without_a_device_id_does_not_resolve() {
        let f = files();
        let mut keys = KeyStore::load(&f.keys).unwrap();
        let token = keys
            .generate(crate::keys::TOKEN_PREFIX, None, None, Some("old".into()), None)
            .unwrap()
            .token;

        assert!(auth(&f).resolve(&token).is_none());
    }

    #[test]
    fn a_key_pointing_at_a_missing_row_does_not_resolve() {
        let f = files();
        let token = add(&f, "laptop");
        // 鍵はそのまま、台帳の行だけ消す（他ホストから同期された削除の再現）。
        Devices::load(&f.devices).unwrap().purge("laptop").unwrap();

        assert!(auth(&f).resolve(&token).is_none());
    }

    /// `device retire` は鍵も失効させるので、この状態は手編集か同期でしか
    /// 起きない。それでも通してはいけない。
    #[test]
    fn a_retired_device_does_not_resolve() {
        let f = files();
        let token = add(&f, "laptop");
        Devices::load(&f.devices).unwrap().retire("laptop").unwrap();

        assert!(auth(&f).resolve(&token).is_none());
    }

    /// 期限切れの鍵は、行が生きていても通さない。
    ///
    /// `device add --expires-in` は最短でも 1 分先しか指定できない（0 と負は
    /// `parse_duration` が拒否する）ので、待たずに作るには過去の期限を直接
    /// 渡して発行する。
    #[test]
    fn an_expired_key_does_not_resolve() {
        let f = files();
        let device_id = Devices::load(&f.devices)
            .unwrap()
            .add("laptop", None, None)
            .unwrap()
            .id;
        let mut keys = KeyStore::load(&f.keys).unwrap();
        let token = keys
            .generate(
                crate::keys::TOKEN_PREFIX,
                None,
                Some(device_id),
                Some("laptop".into()),
                Some(chrono::Utc::now() - chrono::Duration::hours(1)),
            )
            .unwrap()
            .token;

        assert!(auth(&f).resolve(&token).is_none());
    }

    #[test]
    fn has_usable_device_key_is_false_without_a_live_device() {
        let f = files();
        assert!(!auth(&f).has_usable_device_key(), "鍵も台帳も無い");

        let mut keys = KeyStore::load(&f.keys).unwrap();
        keys.generate(crate::keys::TOKEN_PREFIX, None, None, Some("old".into()), None)
            .unwrap();
        assert!(
            !auth(&f).has_usable_device_key(),
            "台帳を経由しない鍵で起動できてしまう"
        );
        assert_eq!(auth(&f).orphan_key_count(), 1);

        add(&f, "laptop");
        assert!(auth(&f).has_usable_device_key());
        assert_eq!(auth(&f).orphan_key_count(), 1, "孤児の鍵は数え続ける");
    }
}
```

`src/lib.rs` に `pub mod device_auth;` を足す。

- [ ] **Step 2: テストが落ちることを確認する**

Run: `cargo test -p sapphire-journal-server --lib device_auth`
Expected: `unimplemented!()` の panic で FAIL

- [ ] **Step 3: `DeviceAuth` を実装する**

`impl DeviceAuth` を差し替える:

```rust
impl DeviceAuth {
    /// 鍵ファイルと台帳を読む。どちらも存在しなければ空として扱う
    /// （`KeyStore::load` / `Devices::load` の既定）。
    pub fn load(keys_path: &Path, devices_path: &Path) -> anyhow::Result<Self> {
        let keys = KeyStore::load(keys_path)
            .with_context(|| format!("loading API keys from {}", keys_path.display()))?;
        let devices = Devices::load(devices_path)
            .with_context(|| format!("loading device table {}", devices_path.display()))?;
        Ok(Self { keys, devices })
    }

    /// トークン → デバイス。
    ///
    /// 失敗理由（鍵が無い・期限切れ・`device_id` が無い・行が無い・引退済み）は
    /// すべて `None` に潰す。呼び出し側は全部に 401 を返し、区別はログにだけ
    /// 出す —— どの段階で落ちたかを返すと、鍵の有無を試せる口になる。
    pub fn resolve(&self, token: &str) -> Option<&Device> {
        let entry = self.keys.authenticate(token)?;
        let Some(device_id) = entry.device_id else {
            tracing::debug!(key_id = %entry.id, "key has no device_id; refusing");
            return None;
        };
        let Some(device) = self.devices.get(device_id) else {
            tracing::debug!(%device_id, "key names a device that is not in the table; refusing");
            return None;
        };
        if device.is_retired() {
            tracing::debug!(%device_id, "device is retired; refusing");
            return None;
        }
        Some(device)
    }

    /// 生きたデバイスを指す、期限切れでない鍵が 1 本以上あるか。
    ///
    /// 起動ガードが使う。0 本で待ち受けるのは、鍵が 0 本のときと同じ事故
    /// （誰も繋がらないサーバが黙って上がる）。
    pub fn has_usable_device_key(&self) -> bool {
        let now = chrono::Utc::now();
        self.keys.entries().iter().any(|k| {
            !k.is_expired(now)
                && k.device_id
                    .and_then(|id| self.devices.get(id))
                    .is_some_and(|d| !d.is_retired())
        })
    }

    /// 台帳を経由しない鍵の本数。
    ///
    /// `list-keys` を消した以上、放置された旧鍵に気づく口はここしか無い。
    /// 起動時に warn へ出す。
    pub fn orphan_key_count(&self) -> usize {
        self.keys
            .entries()
            .iter()
            .filter(|k| {
                k.device_id
                    .is_none_or(|id| self.devices.get(id).is_none())
            })
            .count()
    }
}
```

`is_none_or` が使えない場合（Rust のバージョン次第）は
`k.device_id.map_or(true, |id| self.devices.get(id).is_none())` にする。

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server --lib device_auth`
Expected: PASS

- [ ] **Step 5: レイヤを実装する**

`src/device_auth.rs` の `mod tests` の前に足す:

```rust
/// `router` にデバイス検査を被せる。
///
/// framework の `protect` は残したまま、その**外側**に置く。内側と外側で鍵を
/// 2 回引くことになるが、`protect` を外すと「レイヤの順序を間違えると素通しに
/// なる」構成を自前で抱えることになる。
///
/// 経路が増えたときの取りこぼしを避けるため、`/rpc` と `/mcp` を merge した
/// **後**の Router に 1 回だけ被せること。
pub fn require_device(auth: Arc<DeviceAuth>, router: Router) -> Router {
    router.layer(from_fn_with_state(auth, check))
}

async fn check(
    State(auth): State<Arc<DeviceAuth>>,
    request: Request,
    next: Next,
) -> std::result::Result<Response, StatusCode> {
    // `request` を `next` に渡すので、ヘッダの借用はここで閉じる。
    let allowed = {
        let presented = request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        match presented {
            Some(token) => auth.resolve(token).is_some(),
            None => {
                tracing::debug!("no bearer token; refusing");
                false
            }
        }
    };
    if !allowed {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}
```

- [ ] **Step 6: `build_router` に配線する**

`src/serve.rs`:

- `use` に `use crate::device_auth::{DeviceAuth, require_device};` を足す
- `build_router` のシグネチャに `device_auth: Arc<DeviceAuth>` を末尾の引数として
  足し、返り値を包む:

```rust
    // 認証は 3 枚重ね: framework の `protect`（鍵）が内側、その外に台帳の検査。
    // merge した後に 1 回だけ被せる —— 片方のルートだけ守られている状態を
    // 作らない。
    Ok(require_device(
        device_auth,
        router(Arc::clone(&state)).merge(protect(state, mcp)),
    ))
```

- `build_router` のドキュメントコメントに 1 段落足す:

```rust
/// `device_auth` は台帳を通した検査（fail-closed）。`state` の鍵だけでは
/// 「移行前に発行された、どのデバイスにも属さない鍵」が通ってしまう。
```

- [ ] **Step 7: 起動ガードを差し替える**

`run_until` の `match state.keys() { ... }` ブロックを次で置き換える:

```rust
    let device_auth = Arc::new(DeviceAuth::load(
        keys_path,
        &default_devices_path(journal_dir)?,
    )?);
    if !device_auth.has_usable_device_key() {
        // 鍵が 0 本のときと同じ扱い。認証を通れる資格情報が無い状態で
        // 待ち受けると、誰も繋がらないサーバが黙って上がる。
        anyhow::bail!(
            "no usable device key configured in {}; run `sapphire-journal-server \
             --journal-dir {} device add --name <NAME>` first (a key that names no device, \
             names a device that is not in the table, or has expired counts as none)",
            keys_path.display(),
            journal_dir.display()
        );
    }
    let orphans = device_auth.orphan_key_count();
    if orphans > 0 {
        // `list-keys` を消した以上、放置された旧鍵に気づく口はここだけ。
        tracing::warn!(
            count = orphans,
            path = %keys_path.display(),
            "key file holds keys that name no device in the table; they authenticate to \
             nothing and can be deleted by hand"
        );
    }
```

`build_router` の呼び出しに `Arc::clone(&device_auth)` を足す。

- [ ] **Step 8: テストの呼び出し側を直す**

`tests/harness/mod.rs` の `build_router` 呼び出しの直前に:

```rust
    let device_auth = std::sync::Arc::new(
        sapphire_journal_server::device_auth::DeviceAuth::load(
            &keys_path,
            &sapphire_journal_server::serve::default_devices_path(&journal_dir).unwrap(),
        )
        .unwrap(),
    );
```

を置き、`build_router(..., allowed_hosts, device_auth)` にする。

`tests/no_keys.rs` の既存テスト `run_refuses_to_start_with_no_usable_key` は
`message.contains("no usable API key")` を見ている。Step 7 でこの文言が消えるので、
**このタスクの中で** `message.contains("no usable device key")` に直すこと
（テストを足すのは Task 5）。ここを後回しにすると Step 10 の全体テストが落ちる。

`tests/retire.rs` は 2 箇所で `build_router` を呼ぶ。**どちらも台帳は
`journal_dir` 側のものを使う**（2 つ目の世代は journal を別に持つが、台帳と鍵は
共有物）。それぞれの呼び出しの前に同じ形で `DeviceAuth::load(&keys_path,
&default_devices_path(&journal_dir).unwrap())` を組んで渡す。

- [ ] **Step 9: 結合テストを書く**

`tests/device_auth.rs` を新規作成:

```rust
//! fail-closed の各条件が、`/rpc` と `/mcp` の両方で 401 になること。
//!
//! `DeviceAuth` の単体テストは解決だけを見る。ここが見るのは「そのレイヤが
//! 本当に両方のルートに被さっているか」—— 片方だけ守られている状態は、
//! 解決の正しさとは独立に起こりうる。

mod harness;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use sapphire_framework::remote_server::KeyStore;
use tower::ServiceExt as _;

async fn status(router: &axum::Router, uri: &str, token: &str) -> StatusCode {
    let body = match uri {
        "/rpc" => serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "workspace.snapshot", "params": {"ws": ""}
        }),
        _ => serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "device-auth-test", "version": "0.0.0"}
            }
        }),
    };
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::HOST, "localhost")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// journal と台帳と鍵を 1 式作り、Router を組む。`extra` は、組む前に鍵や
/// 台帳へ細工をするフック。その戻り値（細工で作ったトークンなど）はそのまま
/// 返す —— 呼び出し側で `&mut` を閉じ込めずに済ませるため。
async fn router_with<T>(
    tmp: &tempfile::TempDir,
    extra: impl FnOnce(&std::path::Path, &std::path::Path) -> T,
) -> (axum::Router, String, T) {
    sapphire_journal_core::init_app_context();
    let journal_dir = tmp.path().join("journal");
    harness::init_journal(&journal_dir);
    let keys_path = tmp.path().join("keys.toml");
    let live = harness::mint_device_key(&journal_dir, &keys_path, "live");
    let devices_path =
        sapphire_journal_server::serve::default_devices_path(&journal_dir).unwrap();

    let extra_out = extra(&keys_path, &devices_path);

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();
    let device_auth = Arc::new(
        sapphire_journal_server::device_auth::DeviceAuth::load(&keys_path, &devices_path).unwrap(),
    );
    let router = sapphire_journal_server::serve::build_router(
        state,
        journal_state,
        tokio_util::sync::CancellationToken::new(),
        &[],
        device_auth,
    )
    .unwrap();
    (router, live, extra_out)
}

#[tokio::test]
async fn a_device_key_passes_and_a_key_without_a_device_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let (router, live, orphan) = router_with(&tmp, |keys_path, _devices| {
        // 移行前の `gen-key` が作っていた形の鍵。
        let mut keys = KeyStore::load(keys_path).unwrap();
        keys.generate(
            sapphire_journal_server::keys::TOKEN_PREFIX,
            None,
            None,
            Some("old".into()),
            None,
        )
        .unwrap()
        .token
    })
    .await;

    for uri in ["/rpc", "/mcp"] {
        assert_ne!(
            status(&router, uri, &live).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が生きたデバイスを拒んでいる（テストの前提が崩れている）"
        );
        assert_eq!(
            status(&router, uri, &orphan).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が台帳を経由しない鍵を通している"
        );
    }
}

#[tokio::test]
async fn a_retired_device_gets_401_even_while_its_key_is_still_in_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    // `device retire` は鍵も失効させるので、この状態は手編集か同期でしか
    // 起きない。それでも通してはいけない —— 認証の判断は台帳が持つ。
    let (router, live, ()) = router_with(&tmp, |_keys, devices_path| {
        sapphire_framework::registry::Devices::load(devices_path)
            .unwrap()
            .retire("live")
            .unwrap();
    })
    .await;

    for uri in ["/rpc", "/mcp"] {
        assert_eq!(
            status(&router, uri, &live).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が引退したデバイスを通している"
        );
    }
}

#[tokio::test]
async fn a_key_naming_a_missing_row_gets_401() {
    let tmp = tempfile::tempdir().unwrap();
    let (router, live, ()) = router_with(&tmp, |_keys, devices_path| {
        sapphire_framework::registry::Devices::load(devices_path)
            .unwrap()
            .purge("live")
            .unwrap();
    })
    .await;

    for uri in ["/rpc", "/mcp"] {
        assert_eq!(
            status(&router, uri, &live).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が台帳に無いデバイスを通している"
        );
    }
}

#[tokio::test]
async fn no_bearer_header_gets_401() {
    let tmp = tempfile::tempdir().unwrap();
    let (router, _live, ()) = router_with(&tmp, |_keys, _devices| {}).await;

    for uri in ["/rpc", "/mcp"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header(header::HOST, "localhost")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}
```

`tests/device_auth.rs` は `sapphire-framework` の `registry` を直接使うので、
`sapphire-journal-server/Cargo.toml` の `[dev-dependencies]` には何も足さない
（`sapphire-framework` は通常の依存として既にある）。

- [ ] **Step 10: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS（`device_auth` の 4 テストを含む）

- [ ] **Step 11: clippy**

Run: `cargo clippy -p sapphire-journal-server --all-targets`
Expected: 警告なし

- [ ] **Step 12: コミット**

```bash
git add -A sapphire-journal-server
git commit -m "feat(server)!: authenticate through the device table, fail-closed"
```

---

### Task 5: 起動ガードのテストと README

**Files:**
- Modify: `sapphire-journal-server/tests/no_keys.rs`
- Modify: `sapphire-journal-server/README.md`

**Interfaces:**
- Consumes: Task 4 の `run_until` の新しいガード、`harness::mint_device_key`

- [ ] **Step 1: 起動ガードのテストを足す**

`tests/no_keys.rs` のモジュールコメントを差し替え、既存テストの期待文字列を直し、
新しいテストを 1 件足す:

```rust
//! `serve::run` は、認証を通れる資格情報が 1 つも無いと bind すらせずに拒否する。
//!
//! 「認証を通れる」の意味は台帳の導入で変わった —— 鍵があるだけでは足りず、
//! その鍵が生きたデバイス行を指していなければならない。`harness::spawn()` は
//! `device add` を通すため、この不変条件はそこを経由せずに直接 `run` を呼んで
//! 確かめる。
```

既存の `run_refuses_to_start_with_no_usable_key` の assert は Task 4 で
`message.contains("no usable device key")` に直っているはず。直っていなければ
ここで直す。そして足す:

```rust
/// 鍵はあるが、どのデバイスも指していない状態 —— 移行前の `gen-key` で作られた
/// 鍵ファイルをそのまま持ってきた場合がこれ。通る資格情報は 0 なので、鍵が
/// 0 本のときと同じく起動しない。
#[tokio::test]
async fn run_refuses_to_start_when_no_key_names_a_live_device() {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    harness::init_journal(&journal_dir);

    let keys_path = tmp.path().join("keys.toml");
    let mut keys = sapphire_framework::remote_server::KeyStore::load(&keys_path).unwrap();
    keys.generate(
        sapphire_journal_server::keys::TOKEN_PREFIX,
        None,
        None,
        Some("old".into()),
        None,
    )
    .unwrap();

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let result = sapphire_journal_server::serve::run(
        addr,
        &journal_dir,
        &keys_path,
        state,
        journal_state,
        &[],
    )
    .await;

    let message = result.unwrap_err().to_string();
    assert!(message.contains("no usable device key"), "{message}");
    assert!(message.contains("device add"), "逃げ道を示していない: {message}");
}

/// 対照。`device add` を通した鍵なら起動条件を満たす（bind の直前まで進む）。
#[tokio::test]
async fn run_gets_past_the_guard_with_a_device_key() {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    harness::init_journal(&journal_dir);
    let keys_path = tmp.path().join("keys.toml");
    harness::mint_device_key(&journal_dir, &keys_path, "laptop");

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();

    // すぐ止まるように、解決済みの shutdown を渡す。ガードを抜けられなければ
    // ここに到達する前にエラーで戻る。
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let result = sapphire_journal_server::serve::run_until(
        addr,
        &journal_dir,
        &keys_path,
        state,
        journal_state,
        &[],
        std::future::ready(()),
    )
    .await;

    assert!(result.is_ok(), "生きたデバイスがあるのに起動できない: {result:?}");
}
```

- [ ] **Step 2: テストを走らせる**

Run: `cargo test -p sapphire-journal-server --test no_keys`
Expected: PASS（3 件）

- [ ] **Step 3: README を書き換える**

`sapphire-journal-server/README.md` の認証の節（74 行目付近）を差し替える。
`gen-key` の説明を次の内容にする:

- 認証の主体は**デバイス**であること。鍵はデバイスに従属し、`devices.toml` に
  行が無い鍵はどのルートも通らない（401）
- 手順:

```sh
# 任意: 持ち主を登録する
sapphire-journal-server --journal-dir /path/to/your/journal user add --name fluo

# デバイスを登録し、そのトークンを発行する
sapphire-journal-server --journal-dir /path/to/your/journal device add --name laptop --user fluo
```

- トークンは stdout、id と作成日時は stderr（従来の `gen-key` と同じ規約）
- 一覧・差し替え・停止:

```sh
sapphire-journal-server --journal-dir ... device list
sapphire-journal-server --journal-dir ... device rotate laptop --expires-in 90d
sapphire-journal-server --journal-dir ... device retire laptop
```

- `rotate` の `--expires-in` は期限を**置き換える**（省略すると無期限になる）
- `rotate` / `retire` が動いているサーバに効くのは再起動時
- ファイルの置き場所: `devices.toml` / `users.toml` は
  `<root>/.sapphire-journal/`（同期される。ID・名前・説明だけで秘密は入らない）、
  `keys.toml` はキャッシュディレクトリ（同期されない）
- **移行**: `gen-key` で発行済みのトークンはすべて 401 になる。`device add` で
  発行し直してクライアントの `Authorization` ヘッダを差し替えること。古い鍵の行は
  起動時に warn として本数が出るので、`keys.toml` から手で消してよい

143 行目付近の「鍵ファイルが空のときは `gen-key` を先に実行」という案内を、
`device add` を指すように直す。「鍵が期限切れだけの場合」に加えて「どの鍵も
デバイスを指していない場合」も同じ扱いになることを書く。

- [ ] **Step 4: 全体を通す**

Run: `cargo test -p sapphire-journal-server && cargo clippy -p sapphire-journal-server --all-targets`
Expected: PASS、警告なし

- [ ] **Step 5: コミット**

```bash
git add sapphire-journal-server/tests/no_keys.rs sapphire-journal-server/README.md
git commit -m "docs(server): document the device/user commands and the migration"
```

---

### Task 6: 仕上げ

**Files:**
- Modify: 必要に応じて上記すべて

- [ ] **Step 1: ワークスペース全体のテスト**

Run: `cargo test --workspace`
Expected: PASS（CI と同じコマンド）

- [ ] **Step 2: 残骸の検索**

Run: `rg -n "gen-key|gen_key|GenKey|list-keys|ListKeys|revoke-key|RevokeKey|rotate-key|RotateKey" sapphire-journal-server README.md`
Expected: ヒット無し（`docs/superpowers/` の過去の spec / plan は歴史なので直さない）

ヒットがあれば直してコミットする。

- [ ] **Step 3: `--help` を目で見る**

Run: `cargo run -p sapphire-journal-server -- --help` と
`cargo run -p sapphire-journal-server -- device --help` と
`cargo run -p sapphire-journal-server -- device rotate --help`
Expected: `device` / `user` が並び、`rotate` のヘルプに「期限を置き換える」と
「再起動するまで効かない」が出ている

- [ ] **Step 4: 手で 1 周する**

```bash
mkdir -p /tmp/jtest && cd /tmp/jtest
cargo run -p sapphire-journal-server -- --journal-dir /tmp/jtest/j user add --name fluo
```

事前に `/tmp/jtest/j/.sapphire-journal/config.toml` を作っておくこと
（`sapphire-journal init` でも可）。`user add` → `device add` → `device list` →
`device rotate` → `device retire` の順に実行し、それぞれの出力（stdout がトークン
／id だけであること）を確認する。

- [ ] **Step 5: コミット（差分があれば）**

```bash
git add -A
git commit -m "chore(server): finish the device/user command migration"
```
