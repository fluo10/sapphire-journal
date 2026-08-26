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

use sapphire_framework::remote_server::WsStore;

/// ワークスペース相対パスから `{id}` を取り出す。エントリらしくなければ `None`。
fn entry_id(path: &str) -> Option<&str> {
    let file = path.rsplit('/').next()?;
    let stem = file.strip_suffix(".md")?;
    let id = stem.split('_').next()?;
    // GrainId は英数字。空や明らかに違うものは弾く。
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
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
        with_time.sort_by_key(|(_, t)| *t);

        // 最後の 1 つ(最も新しい)以外を消す。
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

    #[test]
    fn groups_paths_that_share_an_id() {
        let paths = vec![
            "2026/01J1_old-title.md".to_owned(),
            "2026/01J1_new-title.md".to_owned(),
            "2026/01J2_other.md".to_owned(),
        ];

        let groups = find_duplicate_ids(&paths);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
        assert!(groups[0].iter().all(|p| p.contains("01J1")));
    }

    #[test]
    fn a_bare_id_filename_still_groups() {
        // スラグが空のときファイル名は `{id}.md` になる。
        let paths = vec!["2026/01J1.md".to_owned(), "2026/01J1_titled.md".to_owned()];
        assert_eq!(find_duplicate_ids(&paths).len(), 1);
    }

    #[test]
    fn unique_ids_produce_no_groups() {
        let paths = vec!["2026/01J1_a.md".to_owned(), "2026/01J2_b.md".to_owned()];
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
}
