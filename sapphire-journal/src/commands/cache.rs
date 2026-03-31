use anyhow::{Context, Result};
use sapphire_journal_core::{
    cache,
    journal::Journal,
    user_config::{UserConfig, VectorDb},
    JournalState,
};
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
    /// Requires `[cache.embedding]` with `enabled = true`, `vector_db`, and
    /// `dimension` set in ~/.config/sapphire-journal/config.toml.
    /// When `enabled = true`, this also runs automatically after `cache sync`.
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
            .context("not a sapphire-journal — run `sapphire-journal init` to initialize one"),
        None => Journal::find()
            .context("not in a sapphire-journal — run `sapphire-journal init` to initialize one"),
    }
}

fn info(journal: &Journal) -> Result<()> {
    let user_cfg = UserConfig::load()?;
    let state = JournalState::open(journal.clone())?;
    let info = state.cache_info()?;
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
                "stale cache:    {} ({}) — run `sapphire-journal cache clean` to remove",
                names.join(", "),
                human_size(total)
            );
        }
    }

    if let Some(embed_cfg) = &user_cfg.cache.embedding {
        let enabled_str = if embed_cfg.enabled { "enabled" } else { "disabled" };
        println!("embedding:      {} (provider={}, model={})", enabled_str, embed_cfg.provider, embed_cfg.model);

        match embed_cfg.vector_db {
            VectorDb::None => {}
            VectorDb::SqliteVec => {
                if embed_cfg.dimension.is_some() {
                    state.load_retrieve_backend(&user_cfg).map_err(anyhow::Error::msg)?;
                    match state.retrieve_db().vec_info() {
                        Ok(vi) => {
                            println!("vector backend: sqlite_vec (dim={})", vi.embedding_dim);
                            println!("vectors:        {} indexed, {} pending", vi.vector_count, vi.pending_count);
                        }
                        Err(e) => eprintln!("warn: could not read vector stats: {e}"),
                    }
                } else {
                    println!("vector backend: sqlite_vec (dimension not configured)");
                }
            }
            #[cfg(feature = "lancedb-store")]
            VectorDb::LanceDb => {
                if embed_cfg.dimension.is_some() {
                    if let Ok(root) = journal.cache_dir() {
                        use sapphire_journal_core::lancedb_store;
                        let dir = lancedb_store::data_dir(&root);
                        println!("vector backend: lancedb");
                        println!("lancedb path:   {}", dir.display());
                        state.load_retrieve_backend(&user_cfg).map_err(anyhow::Error::msg)?;
                        match state.retrieve_db().vec_info() {
                            Ok(vi) => {
                                println!("vectors:        {} indexed, {} pending", vi.vector_count, vi.pending_count);
                            }
                            Err(e) => eprintln!("warn: could not read vector stats: {e}"),
                        }
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
                                "stale lancedb:  {} ({}) — run `sapphire-journal cache clean` to remove",
                                names.join(", "),
                                human_size(total)
                            );
                        }
                    }
                } else {
                    println!("vector backend: lancedb (dimension not configured)");
                }
            }
            #[cfg(not(feature = "lancedb-store"))]
            VectorDb::LanceDb => {
                println!("vector backend: lancedb (not compiled in)");
            }
        }
    }

    Ok(())
}

fn sync(journal: &Journal) -> Result<()> {
    let user_cfg = UserConfig::load()?;
    let state = JournalState::open(journal.clone())?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let embedded = rt.block_on(state.sync_and_embed(&user_cfg))?;

    let info = state.cache_info()?;
    println!("synced: {} entries", info.entry_count);
    if embedded > 0 {
        println!("embedded: {embedded} new chunks");
    }
    Ok(())
}

fn rebuild(journal: &Journal) -> Result<()> {
    let state = JournalState::rebuild(journal.clone())?;
    state.sync()?;
    let info = state.cache_info()?;
    println!("rebuilt: {} entries indexed", info.entry_count);
    Ok(())
}

fn embed(journal: &Journal) -> Result<()> {
    let user_cfg = UserConfig::load()?;
    let embed_cfg = user_cfg.cache.embedding.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "[cache.embedding] section is required in ~/.config/sapphire-journal/config.toml"
        )
    })?;
    if !embed_cfg.enabled {
        anyhow::bail!(
            "cache.embedding.enabled is false in ~/.config/sapphire-journal/config.toml"
        );
    }
    if embed_cfg.vector_db == VectorDb::None {
        anyhow::bail!(
            "cache.embedding.vector_db is \"none\" in ~/.config/sapphire-journal/config.toml — \
             set it to \"sqlite_vec\" or \"lancedb\" to use vector search"
        );
    }
    if embed_cfg.dimension.is_none() {
        anyhow::bail!(
            "`dimension` is required in [cache.embedding] \
             (e.g. dimension = 1536 for text-embedding-3-small)"
        );
    }

    let state = JournalState::open(journal.clone())?;
    state.sync()?;

    let progress = |done: usize, total: usize| {
        eprint!("\rembedding chunks: {done}/{total}");
        let _ = std::io::stderr().flush();
    };

    let total_embedded = state.embed_pending(&user_cfg, progress).map_err(anyhow::Error::msg)?;

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

/// Return `(path, size)` for every `lancedb_full_vN` directory in `root` where N ≠ SCHEMA_VERSION.
#[cfg(feature = "lancedb-store")]
fn find_stale_lancedb(root: &Path) -> Vec<(std::path::PathBuf, u64)> {
    use sapphire_journal_core::lancedb_store;
    let current = format!("lancedb_full_v{}", lancedb_store::SCHEMA_VERSION);
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let n = name.to_string_lossy();
            let suffix = n.strip_prefix("lancedb_full_v").unwrap_or("");
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
