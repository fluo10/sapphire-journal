use anyhow::{Context, Result};
use archelon_core::{
    cache,
    embed,
    journal::Journal,
    user_config::{UserConfig, VectorDb},
    vector_store::{self, SqliteVecStore},
};
#[cfg(feature = "lancedb-store")]
use archelon_core::lancedb_store::{self, LanceDbVectorStore};
use clap::Subcommand;
use std::{io::Write as _, path::Path};

#[derive(Subcommand)]
pub enum CacheCommand {
    /// Show cache location, schema version, and entry count
    Info,

    /// Incrementally sync the cache with the current journal state
    ///
    /// This is the same sync that runs automatically before ID lookups.
    /// Useful to warm the cache explicitly or to verify it is up to date.
    Sync,

    /// Delete the current-version cache and rebuild it from scratch
    ///
    /// Only the current-version files are removed; older versions are
    /// left untouched. Use `cache clean` to remove stale versions.
    Rebuild,

    /// Generate and store embeddings for entries that do not yet have a vector
    ///
    /// Requires `vector_db = "sqlite_vec"` or `vector_db = "lancedb"` and a
    /// `[cache.embedding]` section in ~/.config/archelon/config.toml.
    Embed,

    /// Remove stale cache files from previous schema versions
    ///
    /// Deletes old `cache.vN.db` files and `lancedb/vN/` directories that
    /// were kept after a schema upgrade. The current version is never touched.
    Clean,
}

pub fn run(journal_dir: Option<&Path>, cmd: CacheCommand) -> Result<()> {
    let journal = open_journal(journal_dir)?;
    match cmd {
        CacheCommand::Info => info(&journal),
        CacheCommand::Sync => sync(&journal),
        CacheCommand::Rebuild => rebuild(&journal),
        CacheCommand::Embed => embed(&journal),
        CacheCommand::Clean => clean(&journal),
    }
}

fn open_journal(journal_dir: Option<&Path>) -> Result<Journal> {
    match journal_dir {
        Some(dir) => Journal::from_root(dir.to_path_buf())
            .context("not an archelon journal — run `archelon init` to initialize one"),
        None => Journal::find()
            .context("not in an archelon journal — run `archelon init` to initialize one"),
    }
}

fn info(journal: &Journal) -> Result<()> {
    let user_cfg = UserConfig::load()?;
    let conn = cache::open_cache(journal)?;
    let info = cache::cache_info(journal, &conn)?;
    println!("path:           {}", info.db_path.display());
    println!("schema version: v{} (app: v{})", info.schema_version, cache::SCHEMA_VERSION);
    println!("files tracked:  {}", info.file_count);
    println!("entries:        {}", info.entry_count);
    println!("unique tags:    {}", info.unique_tag_count);

    // Show stale SQLite versions if any.
    if let Ok(cache_dir) = journal.cache_dir() {
        let stale_dbs = find_stale_sqlite(&cache_dir);
        if !stale_dbs.is_empty() {
            let total = stale_dbs.iter().map(|(_, sz)| sz).sum::<u64>();
            let names: Vec<String> = stale_dbs
                .iter()
                .map(|(p, _)| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
                .collect();
            println!(
                "stale cache:    {} ({}) — run `archelon cache clean` to remove",
                names.join(", "),
                human_size(total)
            );
        }
    }

    match user_cfg.cache.vector_db {
        VectorDb::None => {}
        VectorDb::SqliteVec => {
            if let Some(embed_cfg) = &user_cfg.cache.embedding {
                if let Some(dim) = embed_cfg.dimension {
                    match SqliteVecStore::open(journal, dim) {
                        Ok(store) => match store.vec_info() {
                            Ok(vi) => {
                                println!(
                                    "vector backend: sqlite_vec (dim={})",
                                    vi.embedding_dim
                                );
                                println!(
                                    "vectors:        {} indexed, {} pending",
                                    vi.vector_count, vi.pending_count
                                );
                            }
                            Err(e) => eprintln!("warn: could not read vector stats: {e}"),
                        },
                        Err(e) => eprintln!("warn: could not open vector index: {e}"),
                    }
                } else {
                    println!("vector backend: sqlite_vec (dimension not configured)");
                }
            } else {
                println!("vector backend: sqlite_vec (no [cache.embedding] configured)");
            }
        }
        #[cfg(feature = "lancedb-store")]
        VectorDb::LanceDb => {
            if let Some(embed_cfg) = &user_cfg.cache.embedding {
                if let Some(dim) = embed_cfg.dimension {
                    match journal.cache_dir() {
                        Ok(root) => {
                            let dir = lancedb_store::versioned_dir(&root);
                            println!("vector backend: lancedb");
                            println!("lancedb path:   {}", dir.display());
                            match LanceDbVectorStore::new(&dir, dim) {
                                Ok(store) => match store.vec_info(&conn) {
                                    Ok(vi) => {
                                        println!(
                                            "vectors:        {} indexed, {} pending",
                                            vi.vector_count, vi.pending_count
                                        );
                                    }
                                    Err(e) => eprintln!("warn: could not read vector stats: {e}"),
                                },
                                Err(e) => eprintln!("warn: could not open lancedb store: {e}"),
                            }
                            // Show stale LanceDB versions if any.
                            let stale_dirs = find_stale_lancedb(&root);
                            if !stale_dirs.is_empty() {
                                let total = stale_dirs.iter().map(|(_, sz)| sz).sum::<u64>();
                                let names: Vec<String> = stale_dirs
                                    .iter()
                                    .map(|(p, _)| {
                                        p.file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned()
                                    })
                                    .collect();
                                println!(
                                    "stale lancedb:  {} ({}) — run `archelon cache clean` to remove",
                                    names.join(", "),
                                    human_size(total)
                                );
                            }
                        }
                        Err(e) => eprintln!("warn: could not determine lancedb path: {e}"),
                    }
                } else {
                    println!("vector backend: lancedb (dimension not configured)");
                }
            } else {
                println!("vector backend: lancedb (no [cache.embedding] configured)");
            }
        }
        #[cfg(not(feature = "lancedb-store"))]
        VectorDb::LanceDb => {
            println!("vector backend: lancedb (not compiled in)");
        }
    }

    Ok(())
}

fn sync(journal: &Journal) -> Result<()> {
    let conn = cache::open_cache(journal)?;
    cache::sync_cache(journal, &conn)?;

    let info = cache::cache_info(journal, &conn)?;
    println!("synced: {} entries", info.entry_count);
    Ok(())
}

fn rebuild(journal: &Journal) -> Result<()> {
    let conn = cache::rebuild_cache(journal)?;
    cache::sync_cache(journal, &conn)?;

    let info = cache::cache_info(journal, &conn)?;
    println!("rebuilt: {} entries indexed", info.entry_count);
    Ok(())
}

fn embed(journal: &Journal) -> Result<()> {
    let user_cfg = UserConfig::load()?;
    let embed_cfg = user_cfg.cache.embedding.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "[cache.embedding] section is required in ~/.config/archelon/config.toml"
        )
    })?;
    let dim = embed_cfg.dimension.ok_or_else(|| {
        anyhow::anyhow!(
            "`dimension` is required in [cache.embedding] \
             (e.g. dimension = 1536 for text-embedding-3-small)"
        )
    })?;

    let progress = |done: usize, total: usize| {
        eprint!("\rembedding chunks: {done}/{total}");
        let _ = std::io::stderr().flush();
    };

    let total_embedded = match user_cfg.cache.vector_db {
        VectorDb::None => {
            anyhow::bail!(
                "vector_db is \"none\" in ~/.config/archelon/config.toml — \
                 set it to \"sqlite_vec\" or \"lancedb\" to use vector search"
            );
        }
        VectorDb::SqliteVec => {
            let store = SqliteVecStore::open(journal, dim)?;
            cache::sync_cache(journal, store.conn())?;
            let embedder = embed::build_embedder(embed_cfg)?;
            vector_store::embed_pending_chunks(store.conn(), &store, embedder.as_ref(), progress)?
        }
        #[cfg(feature = "lancedb-store")]
        VectorDb::LanceDb => {
            let conn = cache::open_cache(journal)?;
            cache::sync_cache(journal, &conn)?;
            let root = journal.cache_dir()?;
            let store = LanceDbVectorStore::new(&lancedb_store::versioned_dir(&root), dim)?;
            let embedder = embed::build_embedder(embed_cfg)?;
            vector_store::embed_pending_chunks(&conn, &store, embedder.as_ref(), progress)?
        }
        #[cfg(not(feature = "lancedb-store"))]
        VectorDb::LanceDb => {
            anyhow::bail!("lancedb support is not compiled in (enable the `lancedb-store` feature)");
        }
    };

    if total_embedded > 0 {
        eprintln!(); // newline after progress line
        println!("embedded: {total_embedded} chunks");
    } else {
        println!("all chunks already have embeddings");
    }
    Ok(())
}

fn clean(journal: &Journal) -> Result<()> {
    let mut removed_any = false;

    // ── stale SQLite files ────────────────────────────────────────────────────
    if let Ok(cache_dir) = journal.cache_dir() {
        for (path, size) in find_stale_sqlite(&cache_dir) {
            // Remove the main file plus any WAL/SHM sidecars.
            let base = path.to_string_lossy();
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{base}{suffix}"));
            }
            println!("removed: {} ({})", path.display(), human_size(size));
            removed_any = true;
        }
    }

    // ── stale LanceDB directories ─────────────────────────────────────────────
    #[cfg(feature = "lancedb-store")]
    if let Ok(root) = journal.cache_dir() {
        for (path, size) in find_stale_lancedb(&root) {
            std::fs::remove_dir_all(&path)?;
            println!("removed: {} ({})", path.display(), human_size(size));
            removed_any = true;
        }
    }

    if !removed_any {
        println!("nothing to clean");
    }
    Ok(())
}

// ── stale version discovery ───────────────────────────────────────────────────

/// Return `(path, size)` for every `cache.vN.db` in `cache_dir` where N ≠ SCHEMA_VERSION.
fn find_stale_sqlite(cache_dir: &Path) -> Vec<(std::path::PathBuf, u64)> {
    let current = format!("cache_v{}.db", cache::SCHEMA_VERSION);
    let Ok(rd) = std::fs::read_dir(cache_dir) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let n = name.to_string_lossy();
            n.starts_with("cache_v") && n.ends_with(".db") && n != current.as_str()
        })
        .map(|e| {
            let p = e.path();
            let sz = file_size(&p);
            (p, sz)
        })
        .collect()
}

/// Return `(path, size)` for every `vN` directory in `lancedb_root` where N ≠ LANCEDB_SCHEMA_VERSION.
#[cfg(feature = "lancedb-store")]
fn find_stale_lancedb(root: &Path) -> Vec<(std::path::PathBuf, u64)> {
    let current = format!("lancedb_v{}", lancedb_store::LANCEDB_SCHEMA_VERSION);
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let n = name.to_string_lossy();
            let suffix = n.strip_prefix("lancedb_v").unwrap_or("");
            !suffix.is_empty() && suffix.parse::<i32>().is_ok() && n != current.as_str()
        })
        .map(|e| {
            let p = e.path();
            let sz = dir_size(&p);
            (p, sz)
        })
        .collect()
}

// ── size helpers ──────────────────────────────────────────────────────────────

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn dir_size(path: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else { return 0 };
    rd.filter_map(|e| e.ok())
        .map(|e| {
            let p = e.path();
            if p.is_dir() { dir_size(&p) } else { file_size(&p) }
        })
        .sum()
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{bytes} B")
    }
}
