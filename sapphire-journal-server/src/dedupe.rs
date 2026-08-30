//! 同じ `GrainId` を持つエントリが 2 つ live になった状態の解消。
//!
//! エントリのファイル名は `{id}_{slug}.md`（スラグが空なら `{id}.md`）なので、
//! タイトルを変えるとパスが変わる。リネーム前のパスへ古いカーソルのクライアント
//! が push すると、パス単位 LWW では旧パスが復活し、同じ id のファイルが 2 つに
//! なる。
//!
//! push の時点で拒否する口が framework 側に無いため、ティックで検出して収束
//! させる。同期モデルがそもそも結果整合の LWW なので、事後の収束で筋が通る。
//!
//! # 重複 id の方針は 1 つ、担当が 2 つ
//!
//! 「同じ id のファイルが 2 つある」に対する方針は
//! [`sapphire_journal_core::cache`] の `increment_until_free` とここの
//! [`resolve_duplicates`] に分かれている。矛盾ではなく、**見えている対象が
//! 違う**。読む順を間違えないよう、両方にこの対応を書いてある。
//!
//! - **両方が entry として読める** → ティックは先に `journal_state.sync()` を
//!   呼ぶので `increment_until_free` が動き、後から現れたほうの id を振り直して
//!   **両方残す**。重複はここへ届かない。
//! - **片方が entry として読めない**（書きかけ、エディタの残骸、frontmatter で
//!   ない本文の `changes.push`）→ `read_entry` が失敗して `sync_cache` はその
//!   1 件を飛ばす。ファイル名の `{id}` は生きているので change log 上は重複の
//!   まま残る。**そこを拾うのがここ**。
//!
//! そして [`resolve_duplicates`] は**ファイルを消さない**。id を持つ壊れた
//! ファイルの中身が何なのかはサーバには分からない以上、消してよい根拠が無い
//! —— journal の外（cache dir 配下の `quarantine/`）へ退避する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use grain_id::GrainId;
use sapphire_framework::remote_server::WsStore;
use sapphire_journal_core::journal_state::JournalState;

/// 退避先。journal の cache dir の下に置く —— cache dir は journal ルートの
/// **外**なので、退避したファイルが同期でクライアントに配られることはない。
const QUARANTINE_DIR: &str = "quarantine";

/// ワークスペース相対パスから `{id}` を取り出す。エントリらしくなければ `None`。
///
/// 「アンダースコアの前が英数字なら id」という緩い判定だと、たまたま
/// アンダースコアなしの英数字なファイル名を持つ非エントリ同士（例:
/// `README.md` と `README_notes.md`。どちらも先頭部分は `README`）を
/// 誤って同じ id の重複と見なし、`resolve_duplicates` が一方を消してしまう
/// —— 実データに `_` 区切り以外の理由でファイルを消す事故になる。
/// `GrainId` は 7 文字の Crockford Base32 という固定形状を持つので、
/// `GrainId::from_str` を実際の判定に使う。長さもアルファベットも一致しない
/// 文字列は確実に弾かれる。
fn entry_id(path: &str) -> Option<&str> {
    let file = path.rsplit('/').next()?;
    let stem = file.strip_suffix(".md")?;
    let id = stem.split('_').next()?;
    // grain-id 0.15 の `GrainId::from_str` は `s.len()`(バイト数)で 7 を
    // 判定したあと、`s.chars()` を 7 回無条件に `.unwrap()` する。7 バイトだが
    // 7 文字ではないマルチバイト文字列(例: 日本語 3 文字 + ASCII 1 文字で
    // ちょうど 7 バイト)を渡すと、この `.unwrap()` が範囲外で panic する
    // ——上流のバグで、ここでの `.ok()?` では防げない。Crockford Base32 は
    // そもそも ASCII のみなので、非 ASCII を先に弾いておけばこの panic の
    // 引き金を引かない。「無効な id を弾く」ためではなく「上流の panic を
    // 避ける」ためのガードなので消さないこと。
    if !id.is_ascii() {
        return None;
    }
    id.parse::<GrainId>().ok()?;
    Some(id)
}

/// 同じ id を共有するパスのグループ(2 件以上のものだけ)。
pub fn find_duplicate_ids(paths: &[String]) -> Vec<Vec<String>> {
    let mut by_id: HashMap<&str, Vec<String>> = HashMap::new();
    for p in paths {
        if let Some(id) = entry_id(p) {
            by_id.entry(id).or_default().push(p.clone());
        }
    }
    by_id.into_values().filter(|g| g.len() > 1).collect()
}

/// live なドキュメントを走査し、同じ id の重複を解消する。
///
/// 残すのは **journal 自身のキャッシュがその id に対応づけているパス**。負けた
/// ほうは消さずに `quarantine/` へ退避し、空いたパスを change log に記録する。
/// 戻り値は退避した件数。
///
/// 方針全体（`increment_until_free` との分担、なぜ消さないか）はモジュールの
/// 解説を見ること。
pub fn resolve_duplicates(store: &WsStore, journal: &JournalState) -> anyhow::Result<usize> {
    let snapshot = store.snapshot()?;
    // `snapshot.docs` の並び(seq 昇順)は同値タイの決着に使うので保つ。型名を
    // 借りずに済ませているのは、framework が `SnapshotResult` を
    // `remote_server` から再エクスポートしていないため。
    let docs: Vec<(String, chrono::DateTime<chrono::Utc>)> = snapshot
        .docs
        .iter()
        .map(|c| (c.path.clone(), c.updated_at))
        .collect();
    let paths: Vec<String> = docs.iter().map(|(p, _)| p.clone()).collect();
    let origin = journal.journal.root.clone();
    let mut quarantined = 0usize;

    for group in find_duplicate_ids(&paths) {
        let (keep, chosen_by) = pick_survivor(&group, &docs, journal, &origin);
        let doomed: Vec<String> = group.iter().filter(|p| **p != keep).cloned().collect();
        if doomed.is_empty() {
            continue;
        }

        for path in &doomed {
            let abs = origin.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            // 黙って収束させない。利用者のエントリが 1 つ消えたように見える
            // 瞬間なので、何が残り何がどこへ行ったかを 1 行で言い切る。
            match quarantine(&abs, journal)? {
                Some(dest) => tracing::warn!(
                    kept = %keep,
                    quarantined = %path,
                    moved_to = %dest.display(),
                    chosen_by,
                    "two files shared one entry id; kept one and moved the other out of the journal"
                ),
                None => tracing::warn!(
                    kept = %keep,
                    vacated = %path,
                    chosen_by,
                    "two files shared one entry id; the other was already gone from disk"
                ),
            }
            quarantined += 1;
        }
        store.record_local_write(&doomed, chrono::Utc::now())?;
    }
    Ok(quarantined)
}

/// どのパスを残すか決める。戻り値は (残すパス, 根拠の名前)。
///
/// **第一の根拠は journal 自身のキャッシュ**が id → パスとして持っている値。
/// これはサーバが自分でディスクを読んで作った値で、誰かが送ってきた申告では
/// ない。
///
/// `updated_at` を第一の根拠にしてはいけないのは、あれが **push した側の
/// 名乗り**だからで、古いカーソルのクライアントが旧パスへ「今」を名乗って
/// push すれば、それだけで利用者の現在のエントリより新しく見えてしまう。実際
/// それでリネーム後の正しいエントリが負ける（そして以前はそれが削除されていた）。
///
/// キャッシュが何も言えないときだけ `updated_at` に落ちる —— どちらも entry と
/// して読めない、キャッシュが開けない、あるいはキャッシュの指すパスがこの
/// グループに無い場合。**そこでは「新しいと名乗ったほう」以上の根拠が本当に
/// 無い**ので、負けたほうも消さずに退避する（[`quarantine`]）。
fn pick_survivor(
    group: &[String],
    docs: &[(String, chrono::DateTime<chrono::Utc>)],
    journal: &JournalState,
    origin: &Path,
) -> (String, &'static str) {
    if let Some(path) = cache_choice(group, journal, origin) {
        return (path, "journal cache");
    }
    match newest_by_updated_at(group, docs) {
        Some(p) => (p, "updated_at (the cache has no entry for this id)"),
        // group は空にならない(`find_duplicate_ids` は 2 件以上に絞る)し、
        // snapshot から引けないこともないが、`unwrap` はしない。
        None => (group[0].clone(), "fallback (snapshot had no timestamps)"),
    }
}

/// `updated_at` が最も新しいパス。**client 申告の値**なので、[`pick_survivor`]
/// の第一の根拠ではなく最後の頼みの綱であることに注意。
///
/// `docs` は snapshot の並び(seq 昇順)のまま渡すこと。安定ソートなので
/// `updated_at` が同値のときはその並びが保たれ、change log に後から記録された
/// 方(seq が大きい方)が勝つ。
fn newest_by_updated_at(
    group: &[String],
    docs: &[(String, chrono::DateTime<chrono::Utc>)],
) -> Option<String> {
    let mut with_time: Vec<&(String, chrono::DateTime<chrono::Utc>)> = docs
        .iter()
        .filter(|(p, _)| group.iter().any(|g| g == p))
        .collect();
    with_time.sort_by_key(|(_, t)| *t);
    with_time.last().map(|(p, _)| p.clone())
}

/// journal のキャッシュがこの id に対応づけているパスを、グループ内の表記で返す。
fn cache_choice(group: &[String], journal: &JournalState, origin: &Path) -> Option<String> {
    let id: GrainId = entry_id(group.first()?)?.parse().ok()?;
    let conn = journal.open_conn().ok()?;
    let entry = sapphire_journal_core::cache::find_entry_by_id(&conn, id).ok()?;
    let rel = rel_posix(origin, &entry.path)?;
    group.iter().find(|p| **p == rel).cloned()
}

/// 絶対パスを origin 相対の POSIX 表記に直す。origin の外なら `None`。
fn rel_posix(origin: &Path, abs: &Path) -> Option<String> {
    Some(
        abs.strip_prefix(origin)
            .ok()?
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// 負けたファイルを journal の外（cache dir の `quarantine/`）へ退避する。
///
/// **消さない。** バグで消えたエントリは利用者には取り返せないが、退避されて
/// いれば取り返せる。ファイル名は元のまま残し、衝突しないよう時刻を後ろに足す。
///
/// 既にディスクに無ければ `Ok(None)`（change log にだけ残っていた重複）。
fn quarantine(abs: &Path, journal: &JournalState) -> anyhow::Result<Option<PathBuf>> {
    if !abs.try_exists()? {
        return Ok(None);
    }
    let dir = journal.journal.cache_dir()?.join(QUARANTINE_DIR);
    std::fs::create_dir_all(&dir)?;

    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "entry".to_owned());
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
    let mut dest = dir.join(format!("{name}.{stamp}"));
    // 同じミリ秒に同じ名前を 2 度退避することは実際上ないが、上書きだけは
    // させない —— 退避先で失うなら退避する意味がない。
    let mut n = 1;
    while dest.try_exists()? {
        dest = dir.join(format!("{name}.{stamp}.{n}"));
        n += 1;
    }

    match std::fs::rename(abs, &dest) {
        Ok(()) => Ok(Some(dest)),
        Err(_) => {
            // cache dir は journal ルートの外にあり、別ボリュームのことがある
            // (Windows なら %LOCALAPPDATA% と D:\journal など)。`rename` は
            // ボリュームをまたげないので、そのときだけコピーしてから消す。
            std::fs::copy(abs, &dest)?;
            std::fs::remove_file(abs)?;
            Ok(Some(dest))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // "0000001" / "0000002" は実際に `GrainId::from_str` が受理する 7 文字の
    // Crockford Base32(digits はどれもアルファベットに含まれる)。 id の判定が
    // 本物の GrainId 形状を見ているかを確かめたいので、フィクスチャも本物の
    // 形状にしてある。

    #[test]
    fn groups_paths_that_share_an_id() {
        let paths = vec![
            "2026/0000001_old-title.md".to_owned(),
            "2026/0000001_new-title.md".to_owned(),
            "2026/0000002_other.md".to_owned(),
        ];

        let groups = find_duplicate_ids(&paths);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
        assert!(groups[0].iter().all(|p| p.contains("0000001")));
    }

    #[test]
    fn a_bare_id_filename_still_groups() {
        // スラグが空のときファイル名は `{id}.md` になる。
        let paths = vec!["2026/0000001.md".to_owned(), "2026/0000001_titled.md".to_owned()];
        assert_eq!(find_duplicate_ids(&paths).len(), 1);
    }

    #[test]
    fn unique_ids_produce_no_groups() {
        let paths = vec!["2026/0000001_a.md".to_owned(), "2026/0000002_b.md".to_owned()];
        assert!(find_duplicate_ids(&paths).is_empty());
    }

    #[test]
    fn non_entry_files_are_ignored() {
        let paths = vec![
            ".sapphire-journal/config.toml".to_owned(),
            "README.md".to_owned(),
        ];
        assert!(find_duplicate_ids(&paths).is_empty());
    }

    #[test]
    fn similarly_named_non_entry_files_are_not_grouped() {
        // これが実際に事故を起こす組み合わせ: どちらも `_` の前が `README` で、
        // 「アンダースコア前が英数字なら id」という緩い判定だと同じ id の重複
        // として扱われ、`resolve_duplicates` が `README.md` を消してしまう。
        // `README` は 7 文字の GrainId として parse できない(6 文字)ので、
        // GrainId ベースの判定なら両方とも id なし = グループ化されない。
        let paths = vec!["README.md".to_owned(), "README_notes.md".to_owned()];
        assert!(
            find_duplicate_ids(&paths).is_empty(),
            "README.md と README_notes.md が誤って同じ id の重複としてグループ化された"
        );
    }

    #[test]
    fn the_updated_at_fallback_takes_the_newest() {
        let a = "2026/0000001_a.md".to_owned();
        let b = "2026/0000001_b.md".to_owned();
        let t0 = chrono::Utc::now();
        let docs = vec![(a.clone(), t0), (b.clone(), t0 + chrono::Duration::seconds(1))];

        assert_eq!(newest_by_updated_at(&[a, b], &docs).as_deref(), Some("2026/0000001_b.md"));
    }

    #[test]
    fn the_updated_at_fallback_breaks_a_tie_by_change_log_order() {
        // 同値タイでは snapshot の並び(seq 昇順)の後ろが勝つ。安定ソートに
        // 依存しているので、`sort_by_key` を `sort_unstable_by_key` に変えると
        // ここが落ちる。
        let a = "2026/0000001_a.md".to_owned();
        let b = "2026/0000001_b.md".to_owned();
        let t0 = chrono::Utc::now();
        let docs = vec![(b.clone(), t0), (a.clone(), t0)];

        assert_eq!(
            newest_by_updated_at(&[a, b], &docs).as_deref(),
            Some("2026/0000001_a.md")
        );
    }

    #[test]
    fn the_updated_at_fallback_ignores_paths_outside_the_group() {
        let a = "2026/0000001_a.md".to_owned();
        let b = "2026/0000001_b.md".to_owned();
        let t0 = chrono::Utc::now();
        let docs = vec![
            (a.clone(), t0),
            (b.clone(), t0 + chrono::Duration::seconds(1)),
            // 別の id の、もっと新しいドキュメント。これを拾ってはいけない。
            ("2026/0000002_c.md".to_owned(), t0 + chrono::Duration::seconds(9)),
        ];

        assert_eq!(
            newest_by_updated_at(&[a, b], &docs).as_deref(),
            Some("2026/0000001_b.md")
        );
    }

    #[test]
    fn a_seven_byte_non_ascii_stem_does_not_panic() {
        // "日本1" は 3 + 3 + 1 = 7 バイトだが 3 文字。grain-id 0.15 の
        // `GrainId::from_str` はバイト数で 7 かどうかを見てから `chars()` を
        // 7 回 `.unwrap()` するので、ガードなしで parse に渡すとここで panic
        // する。`entry_id` 側の非 ASCII ガードがこれを防いでいることを確認する。
        let paths = vec!["2026/日本1.md".to_owned()];
        assert!(find_duplicate_ids(&paths).is_empty());
    }
}
