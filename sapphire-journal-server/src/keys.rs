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
///
/// `Duration::days` ではなく `try_days` を使うこと。前者は範囲外の入力で
/// panic するので、`gen-key --expires-in 99999999999d` が「エラー」ではなく
/// 「異常終了」になる —— 打ち間違いに対してプロセスが落ちるのは、この入口の
/// 応答として重すぎる。
pub fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    let (value, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .with_context(|| format!("duration needs a unit (d/h/m): {s:?}"))?,
    );
    if value.is_empty() {
        bail!("duration must start with digits before the unit: {s:?}");
    }
    let n: i64 = value
        .parse()
        .with_context(|| format!("bad duration: {s:?}"))?;
    let d = match unit {
        "d" => Duration::try_days(n),
        "h" => Duration::try_hours(n),
        "m" => Duration::try_minutes(n),
        other => bail!("unknown duration unit {other:?} in {s:?} (use d, h or m)"),
    };
    d.ok_or_else(|| anyhow::anyhow!("duration is out of range: {s:?}"))
}

/// トークンを先頭 12 文字に切り詰めてマスクする。
///
/// バイト単位で切ると、マルチバイト文字の途中で切れて panic しうる —
/// 鍵ファイルはヘッダで「`token` 行だけ手で足してよい」と案内しており、手書きの
/// トークンは検証されずに読み込まれる。文字境界で切ることで、鍵を一覧する
/// この一手だけが手書きトークンで壊れる事態を避ける。
fn mask_token(token: &str) -> String {
    format!("{}…", token.chars().take(12).collect::<String>())
}

/// `list-keys` の 1 行: id・トークン（マスク済）・作成日時・**期限**・ラベル。
///
/// 期限は日付ごと出す。`(expired)` とだけ書いても、いつ切れたのかも、まだ
/// 切れていない鍵がいつ切れるのかも分からず、失効を計画できない。
fn format_key_line(e: &sapphire_framework::remote_server::KeyEntry, now: chrono::DateTime<Utc>) -> String {
    let expires = match e.expires_at {
        Some(at) if e.is_expired(now) => format!("expired {}", at.to_rfc3339()),
        Some(at) => format!("expires {}", at.to_rfc3339()),
        None => "no expiry".to_owned(),
    };
    format!(
        "{}  {}  {}  {}  {}",
        e.id,
        mask_token(&e.token),
        e.created_at.to_rfc3339(),
        expires,
        e.label.as_deref().unwrap_or("-"),
    )
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
                //
                // `checked_add_signed`。`Utc::now() + d` は表現できない時刻に
                // なると panic するので、`parse_duration` を通った値でもまだ
                // 落ちうる（chrono の `Duration` の上限は `DateTime` の上限より
                // ずっと緩い）。打ち間違いでプロセスが異常終了しないこと。
                .map(|d| {
                    Utc::now()
                        .checked_add_signed(d)
                        .ok_or_else(|| anyhow::anyhow!("expiry is too far in the future: {d}"))
                })
                .transpose()?;
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
                println!("{}", format_key_line(e, now));
            }
        }
        Command::RevokeKey { selector } => {
            let removed = store.revoke(&selector)?;
            eprintln!(
                "revoked {} ({})",
                removed.id,
                removed.label.as_deref().unwrap_or("-")
            );
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
        assert_eq!(
            parse_duration("30m").unwrap(),
            chrono::Duration::minutes(30)
        );
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
    fn parse_duration_errors_instead_of_panicking_on_an_out_of_range_value() {
        // `chrono::Duration::days` はこれらで panic する（上限は
        // i64::MAX ミリ秒 ≒ 1.07e11 日）。打ち間違い 1 つで `gen-key` が
        // エラーではなく異常終了になるのを防ぐ。
        for s in ["1000000000000d", "10000000000000h", "1000000000000000m"] {
            assert!(parse_duration(s).is_err(), "{s} が通ってしまった");
        }
    }

    #[test]
    fn gen_key_errors_instead_of_panicking_on_an_absurd_expiry() {
        // `parse_duration` を通っても、`Utc::now() + d` がまだ落ちうる:
        // chrono の `Duration` の上限（約 1.07e11 日）は `DateTime` の上限
        // （西暦 262143 年）よりずっと緩い。ここが本当の入口。
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");

        let result = run(
            Command::GenKey { label: None, expires_in: Some("99999999999d".into()) },
            &path,
        );

        assert!(result.is_err(), "表現できない期限が受理された");
    }

    #[test]
    fn list_keys_prints_the_expiry_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(
            Command::GenKey { label: Some("laptop".into()), expires_in: Some("90d".into()) },
            &path,
        )
        .unwrap();
        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        let entry = &store.entries()[0];
        let expires_at = entry.expires_at.unwrap();

        let line = format_key_line(entry, chrono::Utc::now());

        assert!(
            line.contains(&expires_at.to_rfc3339()),
            "期限の日時が出ていない: {line}"
        );
        assert!(line.contains(&entry.id.to_string()), "{line}");
        assert!(line.contains("laptop"), "{line}");
        assert!(!line.contains(&entry.token), "生のトークンが出ている: {line}");
    }

    #[test]
    fn list_keys_says_when_an_expired_key_expired() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(Command::GenKey { label: None, expires_in: Some("1m".into()) }, &path).unwrap();
        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        let entry = &store.entries()[0];

        // 期限の後から見た場合。
        let line = format_key_line(entry, chrono::Utc::now() + chrono::Duration::hours(1));

        assert!(line.contains("expired"), "{line}");
        assert!(
            line.contains(&entry.expires_at.unwrap().to_rfc3339()),
            "失効済みでも日時は出すこと: {line}"
        );
    }

    #[test]
    fn list_keys_says_so_when_there_is_no_expiry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(Command::GenKey { label: None, expires_in: None }, &path).unwrap();
        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();

        let line = format_key_line(&store.entries()[0], chrono::Utc::now());

        assert!(line.contains("no expiry"), "{line}");
    }

    #[test]
    fn gen_key_writes_a_prefixed_token_and_persists_the_label() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");

        run(
            Command::GenKey {
                label: Some("laptop".into()),
                expires_in: None,
            },
            &path,
        )
        .unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        assert_eq!(store.entries().len(), 1);
        assert!(store.entries()[0].token.starts_with("sjt_"));
        assert_eq!(store.entries()[0].label.as_deref(), Some("laptop"));
    }

    /// `sjt_aaaaaaa` は 11 バイト、続く `あ` は 3 バイトで 11..14 を占める。
    /// バイト 12 はその文字の途中に落ちるので、バイト単位の切り詰めなら panic する。
    fn boundary_breaking_token() -> String {
        format!("sjt_{}あrest", "a".repeat(7))
    }

    #[test]
    fn mask_token_returns_first_12_chars_with_ellipsis() {
        let token = "sjt_abcdefghijklmnopqrst";
        assert_eq!(mask_token(token), "sjt_abcdefgh…");
    }

    #[test]
    fn mask_token_does_not_panic_on_a_multibyte_token_near_the_boundary() {
        let token = boundary_breaking_token();

        let masked = mask_token(&token);

        assert!(String::from_utf8(masked.clone().into_bytes()).is_ok());
        assert_eq!(masked.chars().count(), 13, "先頭 12 文字 + 省略記号");
    }

    #[test]
    fn list_keys_does_not_panic_on_a_hand_written_multibyte_token() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        // ヘッダが案内する通り、`token` 行だけの手書きの鍵。id / created_at は
        // load が補うので検証されない — マルチバイトのトークンもそのまま通る。
        std::fs::write(
            &path,
            format!("[[key]]\ntoken = \"{}\"\n", boundary_breaking_token()),
        )
        .unwrap();

        let result = run(Command::ListKeys, &path);

        assert!(result.is_ok());
    }

    #[test]
    fn expires_in_becomes_an_absolute_time() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");

        run(
            Command::GenKey {
                label: None,
                expires_in: Some("90d".into()),
            },
            &path,
        )
        .unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        let expires = store.entries()[0].expires_at.expect("期限が入っているはず");
        let expected = chrono::Utc::now() + chrono::Duration::days(90);
        assert!((expires - expected).num_seconds().abs() < 5);
    }

    #[test]
    fn revoke_key_removes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(
            Command::GenKey {
                label: Some("gone".into()),
                expires_in: None,
            },
            &path,
        )
        .unwrap();

        run(
            Command::RevokeKey {
                selector: "gone".into(),
            },
            &path,
        )
        .unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        assert!(store.entries().is_empty());
    }
}
