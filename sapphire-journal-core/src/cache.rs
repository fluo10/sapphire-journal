//! Machine-local SQLite cache for fast entry lookups.
//!
//! The cache lives at `$XDG_CACHE_HOME/sapphire-journal/{journal_id}/cache.db` — outside
//! the journal directory so it is never synced by git, Syncthing, or Nextcloud.
//!
//! # Sync strategy
//!
//! On each invocation, all `.md` files are stat()-ed (O(n), syscalls only).
//! Per-file mtime comparison is used rather than a global `last_synced_at`
//! timestamp: syncing tools such as Syncthing preserve the original mtime, so a
//! global watermark would miss files changed or deleted on another machine.
//!
//! # Schema
//!
//! - `files`: tracks every `.md` file ever scanned (path + mtime).
//! - `entries`: managed-entry metadata (no body — body lives in the retrieve DB).
//! - `tags`: many-to-many tag index for efficient tag filtering.
//!
//! Full-text search and vector search are delegated to
//! [`sapphire_retrieve::RetrieveDb`].
//!
//! # Schema versioning
//!
//! [`SCHEMA_VERSION`] is stored in SQLite's `PRAGMA user_version`.
//! - **DB version = 0** (fresh file): schema is applied and version is set.
//! - **DB version = app version**: opened as-is.
//! - **DB version ≠ app version**: returns [`Error::CacheSchemaTooNew`].

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use caretta_id::CarettaId;
use rusqlite::{params, Connection, OptionalExtension as _};
use sapphire_retrieve::RetrieveDb;

use crate::{
    error::{Error, Result},
    journal::{DuplicateTitlePolicy, Journal},
    parser::{read_entry, render_entry},
};

// ── schema version ────────────────────────────────────────────────────────────

/// Stored in `PRAGMA user_version`.  Increment whenever the schema changes.
pub const SCHEMA_VERSION: i32 = 3;

// ── schema ────────────────────────────────────────────────────────────────────

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files (
    path       TEXT    PRIMARY KEY,
    file_mtime INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS entries (
    id              INTEGER PRIMARY KEY,
    parent_id       INTEGER REFERENCES entries(id),
    path            TEXT    NOT NULL UNIQUE REFERENCES files(path) ON DELETE CASCADE,
    title           TEXT    NOT NULL DEFAULT '',
    slug            TEXT    NOT NULL DEFAULT '',
    created_at      TEXT,
    updated_at      TEXT,
    task_status     TEXT,
    task_due        TEXT,
    task_started_at TEXT,
    task_closed_at  TEXT,
    event_start     TEXT,
    event_end       TEXT
);
CREATE INDEX IF NOT EXISTS idx_entries_parent      ON entries(parent_id);
CREATE INDEX IF NOT EXISTS idx_entries_title       ON entries(title);
CREATE INDEX IF NOT EXISTS idx_entries_created_at  ON entries(created_at);
CREATE INDEX IF NOT EXISTS idx_entries_updated_at  ON entries(updated_at);
CREATE INDEX IF NOT EXISTS idx_entries_task_status ON entries(task_status);
CREATE INDEX IF NOT EXISTS idx_entries_task_due    ON entries(task_due);
CREATE INDEX IF NOT EXISTS idx_entries_event_start ON entries(event_start);

CREATE TABLE IF NOT EXISTS tags (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    tag      TEXT    NOT NULL,
    PRIMARY KEY (entry_id, tag)
);
CREATE INDEX IF NOT EXISTS idx_tags_tag ON tags(tag);
";

// ── public API ────────────────────────────────────────────────────────────────

/// Compute the path to the current-version SQLite cache file within `cache_dir`.
pub fn db_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(format!("cache_v{SCHEMA_VERSION}.db"))
}

/// Open (or create) the SQLite cache for `journal`.
pub fn open_cache(journal: &Journal) -> Result<Connection> {
    let cache_dir = journal.cache_dir()?;
    std::fs::create_dir_all(&cache_dir)?;
    open_or_init(&db_path(&cache_dir))
}

/// Delete the current-version cache file and create a fresh one.
pub fn rebuild_cache(journal: &Journal) -> Result<Connection> {
    let cache_dir = journal.cache_dir()?;
    std::fs::create_dir_all(&cache_dir)?;
    let p = db_path(&cache_dir);
    wipe_db_files(&p);
    open_or_init(&p)
}

/// Summary information about the current cache state.
pub struct CacheInfo {
    pub db_path: PathBuf,
    pub schema_version: i32,
    /// Total `.md` files tracked (managed entries + unmanaged files like README.md).
    pub file_count: u64,
    pub entry_count: u64,
    pub unique_tag_count: u64,
}

/// Collect cache statistics for display.
pub fn cache_info(journal: &Journal, conn: &Connection) -> Result<CacheInfo> {
    let db_path = db_path(&journal.cache_dir()?);
    let schema_version =
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))?;
    let file_count =
        conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, u64>(0))?;
    let entry_count =
        conn.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get::<_, u64>(0))?;
    let unique_tag_count =
        conn.query_row("SELECT COUNT(DISTINCT tag) FROM tags", [], |row| row.get::<_, u64>(0))?;
    Ok(CacheInfo { db_path, schema_version, file_count, entry_count, unique_tag_count })
}

// ── open helpers ──────────────────────────────────────────────────────────────

fn open_or_init(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    let db_version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if db_version == 0 {
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        return Ok(conn);
    }

    if db_version == SCHEMA_VERSION {
        return Ok(conn);
    }

    Err(Error::CacheSchemaTooNew {
        db_version,
        app_version: SCHEMA_VERSION,
    })
}

fn wipe_db_files(db_path: &Path) {
    let base = db_path.to_string_lossy();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{base}{suffix}"));
    }
}

/// Incrementally sync the cache against the journal's `.md` files.
///
/// Files whose mtime changed or whose path is new are re-parsed and upserted.
/// Files present in the DB but gone from disk are removed.
///
/// When `retrieve` is provided, document upserts and removals are mirrored to
/// the retrieve database (FTS + vector index) in the same pass.
pub fn sync_cache(
    journal: &Journal,
    conn: &Connection,
    retrieve: &RetrieveDb,
) -> Result<()> {
    let disk_files = collect_with_mtime(journal)?;
    let disk_paths: HashSet<String> = disk_files
        .iter()
        .map(|(p, _)| p.to_string_lossy().into_owned())
        .collect();

    let db_files = query_all_mtimes(conn)?;

    let mut entry_changed = false;

    conn.execute_batch("PRAGMA defer_foreign_keys=ON; BEGIN")?;

    // ── delete files removed from disk ───────────────────────────────────────
    for db_path in db_files.keys() {
        if !disk_paths.contains(db_path.as_str()) {
            // Fetch entry ID before cascade-deleting so we can remove it from
            // the retrieve DB as well.
            let entry_id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM entries WHERE path = ?1",
                    [db_path.as_str()],
                    |row| row.get(0),
                )
                .ok();
            conn.execute("DELETE FROM files WHERE path = ?1", [db_path])?;
            if let Some(id) = entry_id {
                entry_changed = true;
                let _ = retrieve.remove_document(id);
            }
        }
    }

    // ── upsert new / modified files ──────────────────────────────────────────
    for (path, mtime) in &disk_files {
        let path_str = path.to_string_lossy();
        let needs_update = db_files
            .get(path_str.as_ref())
            .map_or(true, |&stored| stored != *mtime);

        if needs_update {
            match read_entry(path) {
                Ok(entry) => {
                    let entry = increment_until_free(conn, entry)?;
                    let final_mtime = file_mtime(&entry.path)?;
                    let final_str = entry.path.to_string_lossy();
                    conn.execute(
                        "INSERT OR REPLACE INTO files (path, file_mtime) VALUES (?1, ?2)",
                        params![final_str.as_ref(), final_mtime],
                    )?;
                    upsert_entry(conn, &entry)?;
                    let doc = entry_to_document(&entry);
                    let _ = retrieve.upsert_document(&doc);
                    entry_changed = true;
                }
                Err(e) => {
                    conn.execute(
                        "INSERT OR REPLACE INTO files (path, file_mtime) VALUES (?1, ?2)",
                        params![path_str.as_ref(), mtime],
                    )?;
                    conn.execute(
                        "DELETE FROM entries WHERE path = ?1",
                        [path_str.as_ref()],
                    )?;
                    eprintln!("warn: {}: {e}", path.display());
                }
            }
        }
    }

    // ── duplicate title check ─────────────────────────────────────────────────
    let dup_policy = journal.config().unwrap_or_default().journal.duplicate_title;
    if dup_policy != DuplicateTitlePolicy::Allow {
        let mut stmt = conn.prepare(
            "SELECT title FROM entries WHERE title != '' \
             GROUP BY title HAVING COUNT(*) > 1 LIMIT 1",
        )?;
        let dup: Option<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .next()
            .transpose()?;

        if let Some(title) = dup {
            match dup_policy {
                DuplicateTitlePolicy::Warn => {
                    eprintln!("warn: duplicate title detected: `{title}`");
                }
                DuplicateTitlePolicy::Error => {
                    conn.execute_batch("ROLLBACK")?;
                    return Err(Error::DuplicateTitle(title));
                }
                DuplicateTitlePolicy::Allow => unreachable!(),
            }
        }
    }

    conn.execute_batch("COMMIT")?;

    if entry_changed {
        let _ = retrieve.rebuild_fts();
    }

    Ok(())
}

/// Look up an entry by its [`CarettaId`].
pub fn find_entry_by_id(conn: &Connection, id: CarettaId) -> Result<crate::entry::Entry> {
    match fetch_full_entry(conn, id) {
        Ok(entry) => {
            if !entry.path.exists() {
                conn.execute("DELETE FROM entries WHERE id = ?1", [id])?;
                return Err(Error::EntryNotFound(id.to_string()));
            }
            Ok(entry)
        }
        Err(Error::Cache(rusqlite::Error::QueryReturnedNoRows)) => {
            Err(Error::EntryNotFound(id.to_string()))
        }
        Err(e) => Err(e),
    }
}

/// Look up an entry by its exact title.
pub fn find_entry_by_title(
    conn: &Connection,
    title: &str,
) -> Result<crate::entry::Entry> {
    let mut stmt = conn.prepare("SELECT id, path FROM entries WHERE title = ?1")?;
    let rows: Vec<(CarettaId, String)> = stmt
        .query_map([title], |row| Ok((row.get::<_, CarettaId>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    match rows.len() {
        0 => Err(Error::EntryNotFoundByTitle(title.to_owned())),
        1 => {
            let (id, path_str) = rows.into_iter().next().unwrap();
            if !PathBuf::from(&path_str).exists() {
                conn.execute("DELETE FROM files WHERE path = ?1", [&path_str])?;
                return Err(Error::EntryNotFoundByTitle(title.to_owned()));
            }
            fetch_full_entry(conn, id)
        }
        n => Err(Error::AmbiguousTitle(title.to_owned(), n)),
    }
}

/// Read all entries from the cache as [`EntryHeader`] structs (no body).
pub fn list_entries_from_cache(conn: &Connection) -> Result<Vec<crate::entry::EntryHeader>> {
    use chrono::NaiveDateTime;
    use crate::entry::{EntryHeader, EventMetaView, FrontmatterView, TaskMetaView};

    let mut tag_map: HashMap<CarettaId, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT entry_id, tag FROM tags ORDER BY entry_id, tag")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, CarettaId>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, tag) in rows {
            tag_map.entry(id).or_default().push(tag);
        }
    }

    let parse_dt = |s: &str| {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap_or_default()
    };
    let parse_dt_opt = |s: Option<String>| -> Option<NaiveDateTime> {
        s.as_deref()
            .and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").ok())
    };

    let mut stmt = conn.prepare(
        "SELECT id, parent_id, path, title, slug, created_at, updated_at,
                task_status, task_due, task_started_at, task_closed_at,
                event_start, event_end
         FROM entries ORDER BY id",
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, CarettaId>(0)?,
                row.get::<_, Option<CarettaId>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut result = Vec::with_capacity(rows.len());
    for (id, parent_id, path, title, slug, created_at, updated_at,
         task_status, task_due, task_started_at, task_closed_at,
         event_start, event_end) in rows
    {
        let tags = tag_map.remove(&id).unwrap_or_default();

        let task = task_status.map(|status| TaskMetaView {
            status,
            due: parse_dt_opt(task_due),
            started_at: parse_dt_opt(task_started_at),
            closed_at: parse_dt_opt(task_closed_at),
        });

        let event = match (parse_dt_opt(event_start), parse_dt_opt(event_end)) {
            (Some(start), Some(end)) => Some(EventMetaView { start, end }),
            _ => None,
        };

        let frontmatter = FrontmatterView {
            id,
            parent_id,
            title,
            slug,
            tags,
            created_at: parse_dt(&created_at),
            updated_at: parse_dt(&updated_at),
            task,
            event,
        };

        let flags = crate::labels::entry_flags(
            frontmatter.task.as_ref(),
            frontmatter.event.as_ref(),
            frontmatter.created_at,
            frontmatter.updated_at,
        );
        result.push(EntryHeader { path, frontmatter, flags });
    }

    Ok(result)
}

/// Remove an entry row from the cache by file path.
///
/// Tags are removed automatically via `ON DELETE CASCADE`.
/// Also removes the document from the retrieve database (FTS + chunks).
pub fn remove_from_cache(
    conn: &Connection,
    path: &Path,
    retrieve: &RetrieveDb,
) -> Result<()> {
    let path_str = path.to_string_lossy();

    let entry_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM entries WHERE path = ?1",
            [path_str.as_ref()],
            |row| row.get(0),
        )
        .ok();

    conn.execute("DELETE FROM files WHERE path = ?1", [path_str.as_ref()])?;

    if let Some(id) = entry_id {
        let _ = retrieve.remove_document(id);
    }

    Ok(())
}

/// Upsert a single entry into the cache by re-reading its file.
///
/// Use this after `create_entry` or `update_entry` to keep the cache warm
/// without a full sync round-trip.
pub fn upsert_entry_from_path(
    conn: &Connection,
    path: &Path,
    retrieve: &RetrieveDb,
) -> Result<()> {
    let entry = read_entry(path)?;
    let entry = increment_until_free(conn, entry)?;
    let mtime = file_mtime(&entry.path)?;
    let path_str = entry.path.to_string_lossy();
    conn.execute(
        "INSERT OR REPLACE INTO files (path, file_mtime) VALUES (?1, ?2)",
        params![path_str.as_ref(), mtime],
    )?;
    upsert_entry(conn, &entry)?;
    let doc = entry_to_document(&entry);
    let _ = retrieve.upsert_document(&doc);
    let _ = retrieve.rebuild_fts();
    Ok(())
}

// ── internals ─────────────────────────────────────────────────────────────────

fn fetch_full_entry(
    conn: &Connection,
    id: CarettaId,
) -> Result<crate::entry::Entry> {
    use chrono::NaiveDateTime;
    use indexmap::IndexMap;
    use crate::entry::{Entry, EventMeta, Frontmatter, TaskMeta};

    let (parent_id, path_str, title, slug, created_at, updated_at,
         task_status, task_due, task_started_at, task_closed_at,
         event_start, event_end) = conn.query_row(
        "SELECT parent_id, path, title, slug, created_at, updated_at,
                task_status, task_due, task_started_at, task_closed_at,
                event_start, event_end
         FROM entries WHERE id = ?1",
        [id],
        |row| {
            Ok((
                row.get::<_, Option<CarettaId>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        },
    )?;

    let mut tags_stmt = conn.prepare("SELECT tag FROM tags WHERE entry_id = ?1 ORDER BY tag")?;
    let tags: Vec<String> = tags_stmt
        .query_map([id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let parse_dt = |s: &str| {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap_or_default()
    };
    let parse_dt_opt = |s: Option<String>| -> Option<NaiveDateTime> {
        s.as_deref().and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").ok())
    };

    let task = task_status.map(|status| TaskMeta {
        status,
        due: parse_dt_opt(task_due),
        started_at: parse_dt_opt(task_started_at),
        closed_at: parse_dt_opt(task_closed_at),
        extra: IndexMap::new(),
    });

    let event = match (parse_dt_opt(event_start), parse_dt_opt(event_end)) {
        (Some(start), Some(end)) => Some(EventMeta { start, end, extra: IndexMap::new() }),
        _ => None,
    };

    let frontmatter = Frontmatter {
        id,
        parent_id,
        title,
        slug,
        tags,
        created_at: parse_dt(&created_at),
        updated_at: parse_dt(&updated_at),
        task,
        event,
        extra: IndexMap::new(),
    };

    // Body is not stored in the cache; callers that need it should read from disk.
    Ok(Entry { path: PathBuf::from(path_str), frontmatter, body: String::new() })
}

fn increment_until_free(
    conn: &Connection,
    mut entry: crate::entry::Entry,
) -> Result<crate::entry::Entry> {
    loop {
        let path_str = entry.path.to_string_lossy();
        let conflict: Option<String> = conn
            .query_row(
                "SELECT path FROM entries WHERE id = ?1 AND path != ?2",
                params![entry.frontmatter.id, path_str.as_ref()],
                |row| row.get(0),
            )
            .optional()?;
        if conflict.is_none() {
            break;
        }
        entry.frontmatter.id = entry.frontmatter.id.increment();
        let new_name = crate::ops::entry_filename_from_frontmatter(
            entry.frontmatter.id,
            &entry.frontmatter,
        );
        let new_path = entry.path.with_file_name(new_name);
        std::fs::rename(&entry.path, &new_path)?;
        render_entry(&entry);
        entry.path = new_path;
        std::fs::write(&entry.path, render_entry(&entry))?;
    }
    Ok(entry)
}

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
    let mut stmt = conn.prepare("SELECT path, file_mtime FROM files")?;
    let result = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    Ok(result)
}

fn upsert_entry(conn: &Connection, entry: &crate::entry::Entry) -> Result<()> {
    let fm = &entry.frontmatter;
    let path_str = entry.path.to_string_lossy();

    conn.execute(
        "INSERT OR REPLACE INTO entries (
            id, parent_id, path,
            title, slug, created_at, updated_at,
            task_status, task_due, task_started_at, task_closed_at,
            event_start, event_end
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            fm.id,
            fm.parent_id,
            path_str.as_ref(),
            fm.title,
            fm.slug,
            fm.created_at.format("%Y-%m-%dT%H:%M").to_string(),
            fm.updated_at.format("%Y-%m-%dT%H:%M").to_string(),
            fm.task.as_ref().map(|t| t.status.clone()),
            fm.task.as_ref().and_then(|t| t.due)
                .map(|d| d.format("%Y-%m-%dT%H:%M").to_string()),
            fm.task.as_ref().and_then(|t| t.started_at)
                .map(|d| d.format("%Y-%m-%dT%H:%M").to_string()),
            fm.task.as_ref().and_then(|t| t.closed_at)
                .map(|d| d.format("%Y-%m-%dT%H:%M").to_string()),
            fm.event.as_ref().map(|e| e.start.format("%Y-%m-%dT%H:%M").to_string()),
            fm.event.as_ref().map(|e| e.end.format("%Y-%m-%dT%H:%M").to_string()),
        ],
    )?;

    conn.execute("DELETE FROM tags WHERE entry_id = ?1", [fm.id])?;
    for tag in &fm.tags {
        conn.execute(
            "INSERT OR IGNORE INTO tags (entry_id, tag) VALUES (?1, ?2)",
            params![fm.id, tag],
        )?;
    }

    Ok(())
}

/// Convert an in-memory [`Entry`] to a [`sapphire_retrieve::Document`] for indexing.
fn entry_to_document(entry: &crate::entry::Entry) -> sapphire_retrieve::Document {
    sapphire_retrieve::Document {
        id: u64::from(entry.frontmatter.id) as i64,
        title: entry.frontmatter.title.clone(),
        body: entry.body.clone(),
        path: entry.path.to_string_lossy().into_owned(),
    }
}
