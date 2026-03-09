//! Machine-local SQLite cache for fast entry lookups.
//!
//! The cache lives at `$XDG_CACHE_HOME/archelon/{journal_id}/cache.db` — outside
//! the journal directory so it is never synced by git, Syncthing, or Nextcloud.
//!
//! # Sync strategy
//!
//! On each invocation, all `.md` files are stat()-ed (O(n), syscalls only).
//! Per-file mtime comparison is used rather than a global `last_synced_at`
//! timestamp: syncing tools such as Syncthing preserve the original mtime, so a
//! global watermark would miss files changed or deleted on another machine.
//!
//! The sync:
//! - **New / modified files** (mtime changed or path not in DB): re-parsed and upserted.
//! - **Deleted files** (path in DB but gone from disk): removed from cache.
//!   Handles Syncthing/Nextcloud propagated deletions transparently.
//!
//! Explicit deletion after `archelon entry remove` is handled by
//! [`remove_from_cache`], which avoids a full sync round-trip in that hot path.
//!
//! # Schema
//!
//! - `entries`: core metadata.  `id INTEGER PRIMARY KEY` uses CarettaId as i64
//!   via the `caretta-id` crate's `rusqlite` feature.
//! - `tags`: many-to-many tag index for efficient tag filtering.
//! - `entries_fts`: FTS5 virtual table (trigram tokenizer) over `title` + `body`
//!   for full-text search. Trigram enables substring search and CJK text with no spaces.
//!
//! # Schema versioning
//!
//! [`SCHEMA_VERSION`] is stored in SQLite's `PRAGMA user_version`.
//! - **DB version = 0** (fresh file): schema is applied and version is set.
//! - **DB version < app version**: schema changed; cache is wiped and rebuilt automatically.
//! - **DB version > app version**: the cache was created by a newer archelon; an error is
//!   returned instructing the user to update archelon or run `archelon cache rebuild`.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use caretta_id::CarettaId;
use rusqlite::{params, Connection};

use crate::{
    error::{Error, Result},
    journal::Journal,
    parser::read_entry,
};

// ── schema version ────────────────────────────────────────────────────────────

/// Stored in `PRAGMA user_version`.  Increment whenever the schema changes.
pub const SCHEMA_VERSION: i32 = 1;

// ── schema ────────────────────────────────────────────────────────────────────

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS entries (
    id              INTEGER PRIMARY KEY,
    parent_id       INTEGER REFERENCES entries(id),
    path            TEXT    NOT NULL UNIQUE,
    file_mtime      INTEGER NOT NULL,
    title           TEXT    NOT NULL DEFAULT '',
    created_at      TEXT,
    updated_at      TEXT,
    is_task         INTEGER NOT NULL DEFAULT 0,
    task_status     TEXT,
    task_due        TEXT,
    task_started_at TEXT,
    task_closed_at  TEXT,
    is_event        INTEGER NOT NULL DEFAULT 0,
    event_start     TEXT,
    event_end       TEXT,
    body            TEXT    NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_entries_parent ON entries(parent_id);

CREATE TABLE IF NOT EXISTS tags (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    tag      TEXT    NOT NULL,
    PRIMARY KEY (entry_id, tag)
);
CREATE INDEX IF NOT EXISTS idx_tags_tag ON tags(tag);

CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
    title,
    body,
    content    = 'entries',
    content_rowid = 'id',
    tokenize   = 'trigram'
);
";

// ── public API ────────────────────────────────────────────────────────────────

/// Open (or create) the SQLite cache for `journal`.
///
/// - **Fresh DB** (user_version = 0): schema is applied and version is set.
/// - **DB version < [`SCHEMA_VERSION`]**: cache is wiped and recreated automatically
///   (a notice is printed to stderr).
/// - **DB version > [`SCHEMA_VERSION`]**: returns [`Error::CacheSchemaTooNew`];
///   the user must update archelon or run `archelon cache rebuild`.
pub fn open_cache(journal: &Journal) -> Result<Connection> {
    let db_path = journal.cache_db_path()?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    open_or_init(&db_path)
}

/// Delete the existing cache and create a fresh one.
///
/// Equivalent to removing the DB files and calling [`open_cache`].
/// After this call the returned connection has an empty, schema-correct DB;
/// call [`sync_cache`] to populate it.
pub fn rebuild_cache(journal: &Journal) -> Result<Connection> {
    let db_path = journal.cache_db_path()?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    wipe_db_files(&db_path);
    open_or_init(&db_path)
}

/// Summary information about the current cache state.
pub struct CacheInfo {
    pub db_path: PathBuf,
    pub schema_version: i32,
    pub entry_count: u64,
    pub unique_tag_count: u64,
}

/// Collect cache statistics for display.
pub fn cache_info(journal: &Journal, conn: &Connection) -> Result<CacheInfo> {
    let db_path = journal.cache_db_path()?;
    let schema_version =
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))?;
    let entry_count =
        conn.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get::<_, u64>(0))?;
    let unique_tag_count =
        conn.query_row("SELECT COUNT(DISTINCT tag) FROM tags", [], |row| row.get::<_, u64>(0))?;
    Ok(CacheInfo { db_path, schema_version, entry_count, unique_tag_count })
}

// ── open helpers ──────────────────────────────────────────────────────────────

fn open_or_init(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    // WAL for better concurrency; foreign keys required for ON DELETE CASCADE.
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    let db_version: i32 =
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if db_version == 0 {
        // Fresh DB: apply schema and stamp the version.
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        return Ok(conn);
    }

    if db_version > SCHEMA_VERSION {
        return Err(Error::CacheSchemaTooNew {
            db_version,
            app_version: SCHEMA_VERSION,
        });
    }

    if db_version < SCHEMA_VERSION {
        // Schema changed: wipe the old DB and start fresh.
        eprintln!(
            "info: cache schema upgraded v{db_version} → v{SCHEMA_VERSION}, rebuilding..."
        );
        drop(conn);
        wipe_db_files(db_path);
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        return Ok(conn);
    }

    Ok(conn)
}

/// Remove the main DB file plus any WAL/SHM sidecar files.  Errors are ignored
/// (files may not exist or may already be gone).
fn wipe_db_files(db_path: &Path) {
    let base = db_path.to_string_lossy();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{base}{suffix}"));
    }
}

/// Incrementally sync the cache against the journal's `.md` files.
///
/// Files whose mtime changed or whose path is new are re-parsed and upserted.
/// Files present in the DB but gone from disk are removed (handles Syncthing/
/// Nextcloud deletions propagated with the original mtime).
///
/// FTS5 index is rebuilt in full only when at least one entry changed, avoiding
/// unnecessary work on invocations where nothing has changed.
pub fn sync_cache(journal: &Journal, conn: &Connection) -> Result<()> {
    let disk_files = collect_with_mtime(journal)?;
    let disk_paths: HashSet<String> = disk_files
        .iter()
        .map(|(p, _)| p.to_string_lossy().into_owned())
        .collect();
    let db_entries = query_all_mtimes(conn)?;

    let mut changed = false;

    conn.execute_batch("BEGIN")?;

    // ── upsert new / modified ────────────────────────────────────────────────
    for (path, mtime) in &disk_files {
        let path_str = path.to_string_lossy();
        let needs_update = db_entries
            .get(path_str.as_ref())
            .map_or(true, |&stored| stored != *mtime);

        if needs_update {
            match read_entry(path) {
                Ok(entry) => {
                    upsert_entry(conn, &entry, *mtime)?;
                    changed = true;
                }
                Err(e) => eprintln!("warn: {}: {e}", path.display()),
            }
        }
    }

    // ── delete removed files ─────────────────────────────────────────────────
    for db_path in db_entries.keys() {
        if !disk_paths.contains(db_path.as_str()) {
            conn.execute("DELETE FROM entries WHERE path = ?1", [db_path])?;
            changed = true;
        }
    }

    conn.execute_batch("COMMIT")?;

    // Rebuild FTS5 index from the entries content table.
    // Only runs when something actually changed, so clean invocations are fast.
    if changed {
        conn.execute_batch("INSERT INTO entries_fts(entries_fts) VALUES('rebuild')")?;
    }

    Ok(())
}

/// Look up an entry by ID string.
///
/// - **Full 7-char ID**: parsed as `CarettaId` and looked up by INTEGER primary key.
/// - **Prefix (< 7 chars)**: all IDs are fetched and filtered client-side.
///   This is a transitional fallback; the preferred UX is autocomplete over the
///   full ID list rather than server-side prefix queries.
///
/// If a stored path no longer exists on disk, the stale row is removed and
/// [`Error::EntryNotFound`] is returned.
pub fn find_entry_by_id(conn: &Connection, id_input: &str) -> Result<PathBuf> {
    // Fast path: full CarettaId → exact INTEGER lookup.
    if let Ok(id) = id_input.parse::<CarettaId>() {
        return match conn.query_row(
            "SELECT path FROM entries WHERE id = ?1",
            [id],
            |row| row.get::<_, String>(0),
        ) {
            Ok(path_str) => {
                let path = PathBuf::from(&path_str);
                if !path.exists() {
                    conn.execute("DELETE FROM entries WHERE id = ?1", [id])?;
                    return Err(Error::EntryNotFound(id_input.to_owned()));
                }
                Ok(path)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(Error::EntryNotFound(id_input.to_owned()))
            }
            Err(e) => Err(Error::Cache(e)),
        };
    }

    // Prefix fallback: client-side filtering over all IDs.
    let mut stmt = conn.prepare("SELECT id, path FROM entries")?;
    let matches: Vec<(CarettaId, String)> = stmt
        .query_map([], |row| Ok((row.get::<_, CarettaId>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(id, _)| id.to_string().starts_with(id_input))
        .collect();

    match matches.len() {
        0 => Err(Error::EntryNotFound(id_input.to_owned())),
        1 => {
            let path = PathBuf::from(&matches[0].1);
            if !path.exists() {
                conn.execute("DELETE FROM entries WHERE id = ?1", [matches[0].0])?;
                return Err(Error::EntryNotFound(id_input.to_owned()));
            }
            Ok(path)
        }
        n => Err(Error::AmbiguousId(id_input.to_owned(), n)),
    }
}

/// Remove an entry row from the cache by file path.
///
/// Tags are removed automatically via `ON DELETE CASCADE`.
/// The FTS5 index is updated incrementally (no full rebuild needed).
/// Call this after `archelon entry remove` to keep the cache consistent.
pub fn remove_from_cache(conn: &Connection, path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy();

    // Fetch content before deletion so we can update the FTS5 index.
    let fts_data = conn
        .query_row(
            "SELECT id, title, body FROM entries WHERE path = ?1",
            [path_str.as_ref()],
            |row| {
                Ok((
                    row.get::<_, CarettaId>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .ok();

    conn.execute("DELETE FROM entries WHERE path = ?1", [path_str.as_ref()])?;

    if let Some((id, title, body)) = fts_data {
        // Remove the entry's tokens from the FTS5 index.
        let _ = conn.execute(
            "INSERT INTO entries_fts(entries_fts, rowid, title, body) \
             VALUES('delete', ?1, ?2, ?3)",
            params![id, title, body],
        );
    }

    Ok(())
}

/// Upsert a single entry into the cache by re-reading its file.
///
/// Use this after `create_entry` or `update_entry` to keep the cache warm
/// without a full sync round-trip.
pub fn upsert_entry_from_path(conn: &Connection, path: &Path) -> Result<()> {
    let mtime = file_mtime(path)?;
    let entry = read_entry(path)?;
    upsert_entry(conn, &entry, mtime)?;
    // Rebuild FTS5 for just this one entry.
    conn.execute_batch("INSERT INTO entries_fts(entries_fts) VALUES('rebuild')")?;
    Ok(())
}

// ── internals ─────────────────────────────────────────────────────────────────

fn collect_with_mtime(journal: &Journal) -> Result<Vec<(PathBuf, i64)>> {
    let paths = journal.collect_entries()?;
    let mut result = Vec::with_capacity(paths.len());
    for path in paths {
        let mtime = file_mtime(&path)?;
        result.push((path, mtime));
    }
    Ok(result)
}

fn file_mtime(path: &Path) -> Result<i64> {
    Ok(std::fs::metadata(path)?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0))
}

fn query_all_mtimes(conn: &Connection) -> Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT path, file_mtime FROM entries")?;
    let result = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    Ok(result)
}

fn upsert_entry(conn: &Connection, entry: &crate::entry::Entry, mtime: i64) -> Result<()> {
    let fm = &entry.frontmatter;
    let path_str = entry.path.to_string_lossy();

    conn.execute(
        "INSERT OR REPLACE INTO entries (
            id, parent_id, path, file_mtime,
            title, created_at, updated_at,
            is_task, task_status, task_due, task_started_at, task_closed_at,
            is_event, event_start, event_end,
            body
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            fm.id,
            fm.parent_id,
            path_str.as_ref(),
            mtime,
            fm.title,
            fm.created_at.format("%Y-%m-%dT%H:%M").to_string(),
            fm.updated_at.format("%Y-%m-%dT%H:%M").to_string(),
            fm.task.is_some() as i32,
            fm.task.as_ref().map(|t| t.status.clone()),
            fm.task.as_ref().and_then(|t| t.due)
                .map(|d| d.format("%Y-%m-%dT%H:%M").to_string()),
            fm.task.as_ref().and_then(|t| t.started_at)
                .map(|d| d.format("%Y-%m-%dT%H:%M").to_string()),
            fm.task.as_ref().and_then(|t| t.closed_at)
                .map(|d| d.format("%Y-%m-%dT%H:%M").to_string()),
            fm.event.is_some() as i32,
            fm.event.as_ref().map(|e| e.start.format("%Y-%m-%dT%H:%M").to_string()),
            fm.event.as_ref().map(|e| e.end.format("%Y-%m-%dT%H:%M").to_string()),
            entry.body,
        ],
    )?;

    // Sync tags: delete all existing then re-insert.
    conn.execute("DELETE FROM tags WHERE entry_id = ?1", [fm.id])?;
    for tag in &fm.tags {
        conn.execute(
            "INSERT OR IGNORE INTO tags (entry_id, tag) VALUES (?1, ?2)",
            params![fm.id, tag],
        )?;
    }

    Ok(())
}
