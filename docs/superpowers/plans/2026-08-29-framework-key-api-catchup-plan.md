# framework の鍵 API 追従と `rotate-key` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `sapphire-framework` のピンを `main` へ上げ、変わった鍵 API に追従し、抜けている `rotate-key` サブコマンドを埋める。

**Architecture:** `sapphire-journal-server` は鍵の形式も生成もすべて framework の `KeyStore` に委ねている（`src/keys.rs` の冒頭コメント）。この計画も同じ線を守る — 追加するのは CLI の入口と表示だけで、鍵の意味論には触れない。

**Tech Stack:** Rust 2024 edition, `clap` 4（derive）, `chrono`, `anyhow`, `sapphire-framework`（`remote-server` feature）

**Spec:** `docs/superpowers/specs/2026-08-29-framework-key-api-catchup-design.md`

## Global Constraints

- `sapphire-framework` は git の `branch = "main"` を参照している。ピンの更新は `cargo update -p sapphire-framework` で行い、`Cargo.lock` の変更をコミットする。
- **framework 側の `sapphire-framework-registry` は使わない。** デバイス／ユーザー台帳との連動は本計画のスコープ外で、Task 4 でイシューに上げるだけ。
- `KeyStore::generate` は `main` で **5 引数**（`prefix`, `id: Option<Uuid>`, `device_id: Option<GrainId>`, `label`, `expires_at`）。journal-server は `id` にも `device_id` にも `None` を渡す。
- 出力規約は既存の `gen-key` に合わせる — **トークンだけ stdout、メタデータは stderr**。
- ドキュメントコメントは既存ファイルに合わせて**日本語**で書く。

---

### Task 1: framework のピンを上げて `generate` に追従する

**Files:**
- Modify: `Cargo.lock`
- Modify: `sapphire-journal-server/src/keys.rs:~100`（`store.generate(...)` の呼び出し）
- Test: `sapphire-journal-server/src/keys.rs`（既存の `#[cfg(test)] mod tests`。変更不要 — 既存テストが回帰の網になる）

**Interfaces:**
- Consumes: `sapphire_framework::remote_server::KeyStore::generate`（5 引数版）
- Produces: なし（既存の `keys::run` の挙動は変わらない）

- [ ] **Step 1: 現状のテストが通ることを確認する（ベースライン）**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS

ここで落ちるなら、それは本計画と無関係の既存の失敗。先に記録してから進むこと。

- [ ] **Step 2: framework のピンを上げる**

Run: `cargo update -p sapphire-framework`

`Cargo.lock` の `sapphire-framework*` の `source` 行のリビジョンが変わることを確認する:

Run: `git diff Cargo.lock | grep "^[+-]source"`
Expected: `34ba131...` または `5918b03...` から新しいリビジョンへの変更が並ぶ

- [ ] **Step 3: ビルドが壊れることを確認する**

Run: `cargo build -p sapphire-journal-server`
Expected: `src/keys.rs` で「this function takes 5 arguments but 3 arguments were supplied」

これが起きなければピンが上がっていない。Step 2 に戻ること。

- [ ] **Step 4: `generate` の呼び出しを直す**

`sapphire-journal-server/src/keys.rs` の `Command::GenKey` アーム:

```rust
// 変更前
let entry = store.generate(TOKEN_PREFIX, label, expires_at)?;
// 変更後
// id と device_id はどちらも None。journal-server は鍵の内部 id を自分で
// 決める理由が無く、デバイス台帳との連動もまだ持たない（issue 参照）。
let entry = store.generate(TOKEN_PREFIX, None, None, label, expires_at)?;
```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS（Step 1 と同じ本数）

- [ ] **Step 6: Commit**

```bash
git add Cargo.lock sapphire-journal-server/src/keys.rs
git commit -m "chore(server): follow the framework's 5-arg KeyStore::generate"
```

---

### Task 2: `rotate-key` サブコマンド

**Files:**
- Modify: `sapphire-journal-server/src/cli.rs`（`enum Command` に `RotateKey`）
- Modify: `sapphire-journal-server/src/keys.rs`（`run` の match に アーム追加）
- Test: `sapphire-journal-server/src/cli.rs`（既存の `#[cfg(test)] mod tests`）
- Test: `sapphire-journal-server/src/keys.rs`（既存の `#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes: `KeyStore::rotate(&mut self, prefix: &str, selector: &str, expires_at: Option<DateTime<Utc>>) -> Result<KeyEntry>`、`keys::parse_duration`（既存）
- Produces:
  - `cli::Command::RotateKey { selector: String, expires_in: Option<String> }`
  - `keys::run` が `RotateKey` を処理する

- [ ] **Step 1: 失敗するテストを書く（CLI パース）**

`sapphire-journal-server/src/cli.rs` の `mod tests` に:

```rust
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
```

- [ ] **Step 2: 失敗するテストを書く（挙動）**

`sapphire-journal-server/src/keys.rs` の `mod tests` に:

```rust
    #[test]
    fn rotate_key_replaces_the_token_but_keeps_the_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(
            Command::GenKey { label: Some("laptop".into()), expires_in: None },
            &path,
        )
        .unwrap();
        let before = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        let (old_token, id, created) = {
            let e = &before.entries()[0];
            (e.token.clone(), e.id, e.created_at)
        };

        run(
            Command::RotateKey { selector: "laptop".into(), expires_in: None },
            &path,
        )
        .unwrap();

        let after = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        let e = &after.entries()[0];
        assert_ne!(e.token, old_token, "トークンが差し替わっていない");
        assert!(e.token.starts_with("sjt_"));
        assert_eq!(e.id, id, "id は保たれる");
        assert_eq!(e.created_at, created, "created_at は保たれる");
        assert_eq!(e.label.as_deref(), Some("laptop"));
        assert!(e.rotated_at.is_some(), "rotated_at が立っていない");
        assert!(after.authenticate(&old_token).is_none(), "旧トークンが生きている");
    }

    /// framework の `rotate` は `expires_at` を**保持ではなく置き換え**る。
    /// 「トークンを差し替えるだけ」のつもりで呼ぶと期限が黙って消えるので、
    /// それが意図した挙動であることをここで固定する。CLI のヘルプにも書く。
    #[test]
    fn rotate_key_without_expires_in_drops_the_existing_expiry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(
            Command::GenKey { label: Some("laptop".into()), expires_in: Some("90d".into()) },
            &path,
        )
        .unwrap();

        run(
            Command::RotateKey { selector: "laptop".into(), expires_in: None },
            &path,
        )
        .unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        assert!(
            store.entries()[0].expires_at.is_none(),
            "期限が引き継がれてしまっている"
        );
    }

    #[test]
    fn rotate_key_sets_a_new_expiry_when_asked() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(Command::GenKey { label: Some("laptop".into()), expires_in: None }, &path).unwrap();

        run(
            Command::RotateKey {
                selector: "laptop".into(),
                expires_in: Some("30d".into()),
            },
            &path,
        )
        .unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        let expires = store.entries()[0].expires_at.expect("期限が入っているはず");
        let expected = chrono::Utc::now() + chrono::Duration::days(30);
        assert!((expires - expected).num_seconds().abs() < 5);
    }

    #[test]
    fn rotate_key_errors_on_an_unknown_selector() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(Command::GenKey { label: Some("laptop".into()), expires_in: None }, &path).unwrap();

        let result = run(
            Command::RotateKey { selector: "nope".into(), expires_in: None },
            &path,
        );

        assert!(result.is_err());
    }

    /// `gen-key` と同じで、範囲外の期限は panic ではなくエラーにする。
    #[test]
    fn rotate_key_errors_instead_of_panicking_on_an_absurd_expiry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(Command::GenKey { label: Some("laptop".into()), expires_in: None }, &path).unwrap();

        let result = run(
            Command::RotateKey {
                selector: "laptop".into(),
                expires_in: Some("99999999999d".into()),
            },
            &path,
        );

        assert!(result.is_err(), "表現できない期限が受理された");
    }
```

- [ ] **Step 3: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: コンパイルエラー（`Command::RotateKey` が存在しない）

- [ ] **Step 4: `cli.rs` に `RotateKey` を足す**

`enum Command` の `RevokeKey` の前に:

```rust
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
```

- [ ] **Step 5: `keys.rs` の `run` にアームを足す**

`Command::GenKey` の `expires_at` を組み立てるロジックは `RotateKey` でも同じなので、
`run` の match の前にヘルパを切り出す。`Command::GenKey` アームの該当箇所を
この関数の呼び出しに置き換えること。

```rust
/// `--expires-in` を絶対時刻に直す。
///
/// `checked_add_signed`。`Utc::now() + d` は表現できない時刻になると panic
/// するので、`parse_duration` を通った値でもまだ落ちうる（chrono の
/// `Duration` の上限は `DateTime` の上限よりずっと緩い）。打ち間違いで
/// プロセスが異常終了しないこと。
fn absolute_expiry(expires_in: Option<&str>) -> anyhow::Result<Option<chrono::DateTime<Utc>>> {
    expires_in
        .map(parse_duration)
        .transpose()?
        .map(|d| {
            Utc::now()
                .checked_add_signed(d)
                .ok_or_else(|| anyhow::anyhow!("expiry is too far in the future: {d}"))
        })
        .transpose()
}
```

`run` の match に足す:

```rust
        Command::RotateKey {
            selector,
            expires_in,
        } => {
            let expires_at = absolute_expiry(expires_in.as_deref())?;
            let entry = store.rotate(TOKEN_PREFIX, &selector, expires_at)?;
            println!("{}", entry.token);
            eprintln!(
                "rotated {}  ({}){}",
                entry.id,
                entry.label.as_deref().unwrap_or("-"),
                entry
                    .expires_at
                    .map(|e| format!("  expires {}", e.to_rfc3339()))
                    .unwrap_or_else(|| "  no expiry".to_owned())
            );
            eprintln!(
                "a running server keeps using the old token until it reloads this file"
            );
        }
```

- [ ] **Step 6: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add sapphire-journal-server/src/cli.rs sapphire-journal-server/src/keys.rs
git commit -m "feat(server): rotate-key, the one key operation that was missing"
```

---

### Task 3: `list-keys` に `rotated_at` を出す

**Files:**
- Modify: `sapphire-journal-server/src/keys.rs`（`format_key_line`）
- Test: `sapphire-journal-server/src/keys.rs`（既存の `#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes: `KeyEntry.rotated_at: Option<DateTime<Utc>>`
- Produces: なし（表示のみ）

- [ ] **Step 1: 失敗するテストを書く**

`sapphire-journal-server/src/keys.rs` の `mod tests` に:

```rust
    #[test]
    fn list_keys_shows_when_a_key_was_last_rotated() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(Command::GenKey { label: Some("laptop".into()), expires_in: None }, &path).unwrap();
        run(
            Command::RotateKey { selector: "laptop".into(), expires_in: None },
            &path,
        )
        .unwrap();
        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        let entry = &store.entries()[0];

        let line = format_key_line(entry, chrono::Utc::now());

        assert!(
            line.contains(&entry.rotated_at.unwrap().to_rfc3339()),
            "再発行の日時が出ていない: {line}"
        );
    }

    #[test]
    fn list_keys_says_nothing_special_for_a_key_that_was_never_rotated() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(Command::GenKey { label: Some("laptop".into()), expires_in: None }, &path).unwrap();
        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();

        let line = format_key_line(&store.entries()[0], chrono::Utc::now());

        // 一度も再発行していない鍵には日時が無いので、その列は `-`。
        assert!(line.contains(" -  ") || line.ends_with(" -"), "{line}");
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-server list_keys_shows_when`
Expected: FAIL（`rotated_at` が行に出ていない）

- [ ] **Step 3: `format_key_line` を直す**

```rust
/// `list-keys` の 1 行: id・トークン（マスク済）・作成日時・**再発行日時**・
/// **期限**・ラベル。
///
/// 期限は日付ごと出す。`(expired)` とだけ書いても、いつ切れたのかも、まだ
/// 切れていない鍵がいつ切れるのかも分からず、失効を計画できない。再発行日時も
/// 同じ理由で日付ごと — 「いつ差し替えたか」が分からないと、漏洩した疑いの
/// あるトークンが既に無効化済みかどうか判断できない。
fn format_key_line(
    e: &sapphire_framework::remote_server::KeyEntry,
    now: chrono::DateTime<Utc>,
) -> String {
    let expires = match e.expires_at {
        Some(at) if e.is_expired(now) => format!("expired {}", at.to_rfc3339()),
        Some(at) => format!("expires {}", at.to_rfc3339()),
        None => "no expiry".to_owned(),
    };
    let rotated = e
        .rotated_at
        .map(|at| format!("rotated {}", at.to_rfc3339()))
        .unwrap_or_else(|| "-".to_owned());
    format!(
        "{}  {}  {}  {}  {}  {}",
        e.id,
        mask_token(&e.token),
        e.created_at.to_rfc3339(),
        rotated,
        expires,
        e.label.as_deref().unwrap_or("-"),
    )
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add sapphire-journal-server/src/keys.rs
git commit -m "feat(server): list-keys says when a key was last rotated"
```

---

### Task 4: README とイシュー

**Files:**
- Modify: `sapphire-journal-server/README.md`（鍵管理の節）

**Interfaces:**
- Consumes: Task 1–3
- Produces: なし

- [ ] **Step 1: README に `rotate-key` を書く**

Run: `grep -n "revoke-key" sapphire-journal-server/README.md`

見つかった節に、同じ書式で `rotate-key` を足す。最低限:

```
sapphire-journal-server --keys ./keys.toml rotate-key <selector> [--expires-in 90d]
```

`--expires-in` を省くと**期限が引き継がれず無期限になる**こと、および
**動いているサーバは次に鍵ファイルを読み直すまで旧トークンを受け付け続ける**
ことを本文に書く。後者は `revoke-key` の説明が既に持っているはずなので、
同じ言い回しを使うこと。

- [ ] **Step 2: 全体のテストと lint**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS

Run: `cargo clippy -p sapphire-journal-server --all-targets -- -D warnings`
Expected: 警告なし

Run: `cargo fmt --all -- --check`
Expected: 差分なし

- [ ] **Step 3: デバイス／ユーザー台帳のイシューを立てる**

```bash
gh issue create \
  --title "Device/user registry and updated_by in the frontmatter" \
  --body 'sapphire-framework が `sapphire-framework-registry` を持つようになった
（`Device` / `User` と `.{app_name}/{devices,users}.toml`）。journal-server も
これを使って、エントリの最終更新者を記録したい。

やること:

- `.sapphire-journal/devices.toml` と `.sapphire-journal/users.toml` を読む
- `gen-key` にデバイスを指定する引数を足し、`KeyEntry.device_id` を埋める
- エントリ書き込み時に、認証されたデバイスをそこまで引き回す
- フロントマターに `updated_by: <device_id>` を焼く
- 表示時に `device_id -> device.user_id -> user.name` で逆引きし、
  最終更新者が人間か AI かを出す

ID はこのアプリの台帳の中だけで意味を持ち、他のアプリと共有しない。
`sapphire-agent` は MCP クライアントとして 1 つのデバイスとして台帳に載る。

設計: `docs/superpowers/specs/2026-08-29-framework-key-api-catchup-design.md`
の「立てるイシュー」節'
```

- [ ] **Step 4: Commit**

```bash
git add sapphire-journal-server/README.md
git commit -m "docs(server): document rotate-key"
```

---

## 完了条件

- `cargo test -p sapphire-journal-server` が通る
- `cargo clippy -p sapphire-journal-server --all-targets -- -D warnings` が通る
- `Cargo.lock` の `sapphire-framework*` が `main` の最新を指している
- `rotate-key` が動き、`--expires-in` 無しで期限が落ちることがテストで固定されている
- `list-keys` が `rotated_at` を出す
- デバイス／ユーザー台帳のイシューが立っている
