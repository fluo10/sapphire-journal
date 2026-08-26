//! 同じ `GrainId` を持つエントリが 2 つ live になった状態の解消。
//!
//! エントリのファイル名は `{id}_{slug}.md`（スラグが空なら `{id}.md`）なので、
//! タイトルを変えるとパスが変わる。リネーム前のパスへ古いカーソルのクライアント
//! が push すると、パス単位 LWW では旧パスが復活し、同じ id のファイルが 2 つに
//! なる。
//!
//! push の時点で拒否する口が framework 側に無いため、ティックで検出して新しい
//! ほうを残す。同期モデルがそもそも結果整合の LWW なので、事後の収束で筋が通る。

use std::collections::HashMap;
use std::path::Path;

use grain_id::GrainId;
use sapphire_framework::remote_server::WsStore;

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

/// live なドキュメントを走査し、同じ id の重複を解消する。残すのは
/// `updated_at` が新しいほう。戻り値は削除した件数。
pub fn resolve_duplicates(store: &WsStore, origin: &Path) -> anyhow::Result<usize> {
    let snapshot = store.snapshot()?;
    let paths: Vec<String> = snapshot.docs.iter().map(|c| c.path.clone()).collect();
    let mut removed = 0usize;

    for group in find_duplicate_ids(&paths) {
        // 各パスの updated_at を snapshot から引く。
        let mut with_time: Vec<(&String, chrono::DateTime<chrono::Utc>)> = group
            .iter()
            .filter_map(|p| {
                snapshot
                    .docs
                    .iter()
                    .find(|c| &c.path == p)
                    .map(|c| (p, c.updated_at))
            })
            .collect();
        // 安定ソートなので、`updated_at` が同値のときは group 内の元の並び順
        // (snapshot.docs の seq 昇順)がそのまま保たれる —— 同値タイでは
        // change log により後から記録された方(seq が大きい方)が勝つ。
        with_time.sort_by_key(|(_, t)| *t);

        // 最後の 1 つ(最も新しい。同値タイなら上のコメントの通り seq が大きい方)以外を消す。
        let doomed: Vec<String> = with_time
            .iter()
            .rev()
            .skip(1)
            .map(|(p, _)| (*p).clone())
            .collect();

        for path in &doomed {
            let abs = origin.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            match std::fs::remove_file(&abs) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            tracing::info!(path, "removed a duplicate of an entry id");
            removed += 1;
        }
        if !doomed.is_empty() {
            store.record_local_write(&doomed, chrono::Utc::now())?;
        }
    }
    Ok(removed)
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
    fn a_seven_byte_non_ascii_stem_does_not_panic() {
        // "日本1" は 3 + 3 + 1 = 7 バイトだが 3 文字。grain-id 0.15 の
        // `GrainId::from_str` はバイト数で 7 かどうかを見てから `chars()` を
        // 7 回 `.unwrap()` するので、ガードなしで parse に渡すとここで panic
        // する。`entry_id` 側の非 ASCII ガードがこれを防いでいることを確認する。
        let paths = vec!["2026/日本1.md".to_owned()];
        assert!(find_duplicate_ids(&paths).is_empty());
    }
}
