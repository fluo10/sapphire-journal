//! 鍵の有効期限まわりのヘルパ。
//!
//! 鍵ファイルの形式・生成・検証は framework の `KeyStore` が持ち、それを呼ぶ
//! 入口は [`crate::cli_device`]。ここに残るのは、相対的な有効期間を絶対時刻へ
//! 直す変換と、このアプリのトークン接頭辞だけ。

use anyhow::{Context as _, bail};
use chrono::{Duration, Utc};

/// journal のトークンにつけるプレフィクス（sapphire-journal token）。
pub const TOKEN_PREFIX: &str = "sjt";

/// `90d` / `12h` / `30m` を [`Duration`] に直す。
///
/// 単位は必須。曖昧な `90` は拒否する — 秒なのか日なのか読めない値を鍵の
/// 有効期限に使わせない。
///
/// `Duration::days` ではなく `try_days` を使うこと。前者は範囲外の入力で
/// panic するので、`device add --expires-in 99999999999d` が「エラー」では
/// なく「異常終了」になる —— 打ち間違いに対してプロセスが落ちるのは、この
/// 入口の応答として重すぎる。
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
    if n <= 0 {
        // `is_expired` は `expires_at <= now`。`0d` を素通しすると、
        // 絶対時刻に直した瞬間に「もう期限切れ」の鍵ができ、一度も
        // 認証できないまま死ぬ。
        bail!("duration must be positive: {s:?}");
    }
    let d = match unit {
        "d" => Duration::try_days(n),
        "h" => Duration::try_hours(n),
        "m" => Duration::try_minutes(n),
        other => bail!("unknown duration unit {other:?} in {s:?} (use d, h or m)"),
    };
    d.ok_or_else(|| anyhow::anyhow!("duration is out of range: {s:?}"))
}

/// `--expires-in` を絶対時刻に直す。
///
/// `checked_add_signed`。`Utc::now() + d` は表現できない時刻になると panic
/// するので、`parse_duration` を通った値でもまだ落ちうる（chrono の
/// `Duration` の上限は `DateTime` の上限よりずっと緩い）。打ち間違いで
/// プロセスが異常終了しないこと。
pub fn absolute_expiry(
    expires_in: Option<&str>,
) -> anyhow::Result<Option<chrono::DateTime<Utc>>> {
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
    fn parse_duration_rejects_zero_and_negative_values() {
        // `0d` を通すと `is_expired`（`expires_at <= now`）により、絶対時刻に
        // 直した瞬間もう期限切れの鍵ができる。一度も認証できないトークンを
        // 「成功」として発行してはいけない。
        for s in ["0d", "0h", "0m", "-1d", "-1h", "-1m"] {
            assert!(parse_duration(s).is_err(), "{s} が通ってしまった");
        }
    }

    #[test]
    fn parse_duration_errors_instead_of_panicking_on_an_out_of_range_value() {
        // `chrono::Duration::days` はこれらで panic する（上限は
        // i64::MAX ミリ秒 ≒ 1.07e11 日）。打ち間違い 1 つで `device add` が
        // エラーではなく異常終了になるのを防ぐ。
        for s in ["1000000000000d", "10000000000000h", "1000000000000000m"] {
            assert!(parse_duration(s).is_err(), "{s} が通ってしまった");
        }
    }
}
