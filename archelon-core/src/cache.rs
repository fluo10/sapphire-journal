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
//! - `files`: tracks every `.md` file ever scanned (path + mtime).  Covers both
//!   managed entries and non-managed files (e.g. `README.md`).  A file whose mtime
//!   is unchanged is skipped entirely on subsequent syncs — preventing repeated
//!   parse-failure warnings for unmanaged files.
//! - `entries`: managed-entry metadata.  `id INTEGER PRIMARY KEY` uses CarettaId as
//!   i64.  `path` has an FK to `files(path) ON DELETE CASCADE` so removing a row
//!   from `files` automatically removes the corresponding entry.
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
use rusqlite::{params, Connection, OptionalExtension as _};

use crate::{
    error::{Error, Result},
    journal::{DuplicateTitlePolicy, Journal},
    parser::{read_entry, render_entry},
};

// ── schema version ────────────────────────────────────────────────────────────

/// Stored in `PRAGMA user_version`.  Increment whenever the schema changes.
pub const SCHEMA_VERSION: i32 = 1;

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
    event_end       TEXT,
    body            TEXT    NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_entries_parent     ON entries(parent_id);
CREATE INDEX IF NOT EXISTS idx_entries_title      ON entries(title);
CREATE INDEX IF NOT EXISTS idx_entries_created_at ON entries(created_at);
CREATE INDEX IF NOT EXISTS idx_entries_updated_at ON entries(updated_at);
CREATE INDEX IF NOT EXISTS idx_entries_task_status ON entries(task_status);
CREATE INDEX IF NOT EXISTS idx_entries_task_due   ON entries(task_due);
CREATE INDEX IF NOT EXISTS idx_entries_event_start ON entries(event_start);

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
    /// Total `.md` files tracked (managed entries + unmanaged files like README.md).
    pub file_count: u64,
    pub entry_count: u64,
    pub unique_tag_count: u64,
}

/// Collect cache statistics for display.
pub fn cache_info(journal: &Journal, conn: &Connection) -> Result<CacheInfo> {
    let db_path = journal.cache_db_path()?;
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

    // `files` table tracks ALL scanned .md files (managed + unmanaged).
    // Using it as the mtime store means non-managed files (e.g. README.md) whose
    // mtime hasn't changed are skipped entirely — no repeated parse-failure warn.
    let db_files = query_all_mtimes(conn)?;

    let mut entry_changed = false;

    // Defer FK checks to commit time so children can be inserted before their
    // parents (e.g. when syncing a journal from scratch or after a Syncthing
    // propagation that delivers files out of topological order).
    conn.execute_batch("PRAGMA defer_foreign_keys=ON; BEGIN")?;

    // ── delete files removed from disk ───────────────────────────────────────
    // Process deletions first so that renamed files (old path gone, new path
    // present) are cleaned up before the upsert loop runs.  Without this
    // ordering the duplicate-ID check below would fire on the stale cache row
    // that still holds the same ID as the renamed file's new path.
    for db_path in db_files.keys() {
        if !disk_paths.contains(db_path.as_str()) {
            let was_entry = conn
                .query_row(
                    "SELECT 1 FROM entries WHERE path = ?1",
                    [db_path.as_str()],
                    |_| Ok(()),
                )
                .is_ok();
            // Deleting from `files` cascades to `entries` and then `tags`.
            conn.execute("DELETE FROM files WHERE path = ?1", [db_path])?;
            if was_entry {
                entry_changed = true;
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
                    // ── duplicate ID check ────────────────────────────────
                    // On collision, increment the ID until a free slot is
                    // found, rename the file, and rewrite the frontmatter.
                    let entry = increment_until_free(conn, entry)?;
                    let final_mtime = file_mtime(&entry.path)?;
                    let final_str = entry.path.to_string_lossy();
                    // Record in `files`; `entries.path` has an FK to `files.path`.
                    conn.execute(
                        "INSERT OR REPLACE INTO files (path, file_mtime) VALUES (?1, ?2)",
                        params![final_str.as_ref(), final_mtime],
                    )?;
                    upsert_entry(conn, &entry)?;
                    entry_changed = true;
                }
                Err(e) => {
                    // File changed but is not a valid entry — still track it
                    // so mtime comparison skips it on future syncs.
                    conn.execute(
                        "INSERT OR REPLACE INTO files (path, file_mtime) VALUES (?1, ?2)",
                        params![path_str.as_ref(), mtime],
                    )?;
                    // Remove any stale entry row.
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
    // Runs inside the transaction so it reflects all changes made above.
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

    // Rebuild FTS5 only when the entries table actually changed.
    if entry_changed {
        conn.execute_batch("INSERT INTO entries_fts(entries_fts) VALUES('rebuild')")?;
    }

    Ok(())
}

/// Look up an entry by its [`CarettaId`].
///
/// If the stored path no longer exists on disk, the stale row is removed and
/// [`Error::EntryNotFound`] is returned.
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
///
/// Returns [`Error::AmbiguousTitle`] if more than one entry matches,
/// and [`Error::EntryNotFoundByTitle`] if none do.
///
/// If the matched path no longer exists on disk the stale row is removed and
/// [`Error::EntryNotFoundByTitle`] is returned.
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
///
/// Both a sync and a cache-open are expected to have been done by the caller.
/// `slug` and unknown frontmatter fields are not stored in the cache; they
/// default to `None`/empty in the returned structs.
pub fn list_entries_from_cache(conn: &Connection) -> Result<Vec<crate::entry::EntryHeader>> {
    use chrono::NaiveDateTime;
    use indexmap::IndexMap;
    use crate::entry::{EntryHeader, EventMeta, Frontmatter, TaskMeta};

    // Fetch all tags in one query to avoid N+1 queries.
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

        let flags = crate::labels::entry_flags(
            frontmatter.task.as_ref(),
            frontmatter.event.as_ref(),
            frontmatter.created_at,
            frontmatter.updated_at,
        );
        result.push(EntryHeader { path: PathBuf::from(path), frontmatter, flags });
    }

    Ok(result)
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

    // Deleting from `files` cascades to `entries` (and then to `tags`).
    conn.execute("DELETE FROM files WHERE path = ?1", [path_str.as_ref()])?;

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
///
/// If the entry's ID collides with an existing cache entry, the ID is
/// incremented until a free slot is found, the file is renamed on disk, and
/// the frontmatter is rewritten — all silently.
pub fn upsert_entry_from_path(conn: &Connection, path: &Path) -> Result<()> {
    let entry = read_entry(path)?;
    // Resolve ID collision by incrementing until a free slot is found.
    let entry = increment_until_free(conn, entry)?;
    let mtime = file_mtime(&entry.path)?;
    let path_str = entry.path.to_string_lossy();
    // Insert into `files` first; `entries.path` has an FK to `files.path`.
    conn.execute(
        "INSERT OR REPLACE INTO files (path, file_mtime) VALUES (?1, ?2)",
        params![path_str.as_ref(), mtime],
    )?;
    upsert_entry(conn, &entry)?;
    conn.execute_batch("INSERT INTO entries_fts(entries_fts) VALUES('rebuild')")?;
    Ok(())
}

// ── internals ─────────────────────────────────────────────────────────────────

/// Fetch a single entry (all columns + tags) from the cache by its numeric ID.
///
/// Returns `Error::Cache(QueryReturnedNoRows)` if no row exists.
fn fetch_full_entry(
    conn: &Connection,
    id: CarettaId,
) -> Result<crate::entry::Entry> {
    use chrono::NaiveDateTime;
    use indexmap::IndexMap;
    use crate::entry::{Entry, EventMeta, Frontmatter, TaskMeta};

    let (parent_id, path_str, title, slug, created_at, updated_at,
         task_status, task_due, task_started_at, task_closed_at,
         event_start, event_end, body) = conn.query_row(
        "SELECT parent_id, path, title, slug, created_at, updated_at,
                task_status, task_due, task_started_at, task_closed_at,
                event_start, event_end, body
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
                row.get::<_, String>(12)?,
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

    Ok(Entry { path: PathBuf::from(path_str), frontmatter, body })
}

/// Resolves an ID collision by calling `increment()` until a free slot is
/// found, then renames the file on disk and rewrites its frontmatter.
/// Returns the entry with its final (non-colliding) ID and path.
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
        let new_path = entry.path.parent().unwrap_or_else(|| Path::new(".")).join(&new_name);
        std::fs::rename(&entry.path, &new_path)?;
        std::fs::write(&new_path, render_entry(&entry))?;
        entry.path = new_path;
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
            event_start, event_end,
            body
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
