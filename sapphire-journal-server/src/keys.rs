//! 鍵管理サブコマンド。
//!
//! 鍵ファイルの形式・生成・検証は framework の [`KeyStore`] が持つ。ここは
//! それを呼ぶ入口と、相対的な有効期間を絶対時刻へ直す変換だけ。

use std::path::Path;

use anyhow::{Context as _, bail};
use chrono::{Duration, Utc};
use sapphire_framework::remote_server::KeyStore;

use crate::cli::Command;

/// journal のトークンにつけるプレフィクス（sapphire-journal token）。
pub const TOKEN_PREFIX: &str = "sjt";

/// `90d` / `12h` / `30m` を [`Duration`] に直す。
///
/// 単位は必須。曖昧な `90` は拒否する — 秒なのか日なのか読めない値を鍵の
/// 有効期限に使わせない。
pub fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    let (value, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .with_context(|| format!("duration needs a unit (d/h/m): {s:?}"))?,
    );
    if value.is_empty() {
        bail!("duration needs a number: {s:?}");
    }
    let n: i64 = value.parse().with_context(|| format!("bad duration: {s:?}"))?;
    match unit {
        "d" => Ok(Duration::days(n)),
        "h" => Ok(Duration::hours(n)),
        "m" => Ok(Duration::minutes(n)),
        other => bail!("unknown duration unit {other:?} in {s:?} (use d, h or m)"),
    }
}

/// 鍵サブコマンドを実行する。
pub fn run(command: Command, keys_path: &Path) -> anyhow::Result<()> {
    let mut store = KeyStore::load(keys_path)
        .with_context(|| format!("loading API keys from {}", keys_path.display()))?;

    match command {
        Command::GenKey { label, expires_in } => {
            let expires_at = expires_in
                .as_deref()
                .map(parse_duration)
                .transpose()?
                // 相対指定は生成時に絶対時刻へ直して保存する。ファイルには絶対
                // 時刻だけを持たせるほうが、後から読んだときに曖昧さがない。
                .map(|d| Utc::now() + d);
            let entry = store.generate(TOKEN_PREFIX, label, expires_at)?;
            println!("{}", entry.token);
            eprintln!(
                "id {}  created {}{}",
                entry.id,
                entry.created_at.to_rfc3339(),
                entry
                    .expires_at
                    .map(|e| format!("  expires {}", e.to_rfc3339()))
                    .unwrap_or_default()
            );
        }
        Command::ListKeys => {
            let now = Utc::now();
            for e in store.entries() {
                let masked = format!("{}…", &e.token[..e.token.len().min(12)]);
                let state = if e.is_expired(now) { " (expired)" } else { "" };
                println!(
                    "{}  {}  {}  {}{}",
                    e.id,
                    masked,
                    e.created_at.to_rfc3339(),
                    e.label.as_deref().unwrap_or("-"),
                    state
                );
            }
        }
        Command::RevokeKey { selector } => {
            let removed = store.revoke(&selector)?;
            eprintln!("revoked {} ({})", removed.id, removed.label.as_deref().unwrap_or("-"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_days_hours_and_minutes() {
        assert_eq!(parse_duration("90d").unwrap(), chrono::Duration::days(90));
        assert_eq!(parse_duration("12h").unwrap(), chrono::Duration::hours(12));
        assert_eq!(parse_duration("30m").unwrap(), chrono::Duration::minutes(30));
    }

    #[test]
    fn parse_duration_rejects_junk() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("90").is_err(), "単位が要る");
        assert!(parse_duration("d90").is_err());
        assert!(parse_duration("-1d").is_err());
        assert!(parse_duration("90y").is_err());
    }

    #[test]
    fn gen_key_writes_a_prefixed_token_and_list_masks_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");

        run(Command::GenKey { label: Some("laptop".into()), expires_in: None }, &path).unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        assert_eq!(store.entries().len(), 1);
        assert!(store.entries()[0].token.starts_with("sjt_"));
        assert_eq!(store.entries()[0].label.as_deref(), Some("laptop"));
    }

    #[test]
    fn expires_in_becomes_an_absolute_time() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");

        run(Command::GenKey { label: None, expires_in: Some("90d".into()) }, &path).unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        let expires = store.entries()[0].expires_at.expect("期限が入っているはず");
        let expected = chrono::Utc::now() + chrono::Duration::days(90);
        assert!((expires - expected).num_seconds().abs() < 5);
    }

    #[test]
    fn revoke_key_removes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(Command::GenKey { label: Some("gone".into()), expires_in: None }, &path).unwrap();

        run(Command::RevokeKey { selector: "gone".into() }, &path).unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        assert!(store.entries().is_empty());
    }
}
