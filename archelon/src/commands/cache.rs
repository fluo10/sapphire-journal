use anyhow::{Context, Result};
use archelon_core::{cache, journal::Journal, user_config::{UserConfig, VectorDb}};
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

    /// Delete the cache database and rebuild it from scratch
    ///
    /// Use this after updating archelon when the schema has changed,
    /// or when the cache has become inconsistent.
    Rebuild,

    /// Generate and store embeddings for entries that do not yet have a vector
    ///
    /// Requires `vector_db = "sqlite_vec"` and a `[cache.embedding]` section
    /// in ~/.config/archelon/config.toml.
    Embed,
}

pub fn run(journal_dir: Option<&Path>, cmd: CacheCommand) -> Result<()> {
    let journal = open_journal(journal_dir)?;
    match cmd {
        CacheCommand::Info => info(&journal),
        CacheCommand::Sync => sync(&journal),
        CacheCommand::Rebuild => rebuild(&journal),
        CacheCommand::Embed => embed(&journal),
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

    // Show vector index stats when sqlite_vec is configured.
    if user_cfg.cache.vector_db == VectorDb::SqliteVec {
        if let Some(embed_cfg) = &user_cfg.cache.embedding {
            if let Some(dim) = embed_cfg.dimension {
                match cache::open_cache_vec(journal, dim) {
                    Ok(vec_conn) => match cache::vec_info(&vec_conn) {
                        Ok(vi) => {
                            println!("vector backend: sqlite_vec (dim={})", vi.embedding_dim);
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

    if user_cfg.cache.vector_db != VectorDb::SqliteVec {
        anyhow::bail!(
            "vector_db is not set to \"sqlite_vec\" in ~/.config/archelon/config.toml"
        );
    }

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

    // Open cache with vec extension and sync metadata first.
    let conn = cache::open_cache_vec(journal, dim)?;
    cache::sync_cache(journal, &conn)?;

    let total_embedded = cache::embed_pending_entries(&conn, embed_cfg, |done, total| {
        eprint!("\rembedding entries: {done}/{total}");
        let _ = std::io::stderr().flush();
    })?;

    if total_embedded > 0 {
        eprintln!(); // newline after progress
        println!("embedded: {total_embedded} entries");
    } else {
        println!("all entries already have embeddings");
    }
    Ok(())
}
