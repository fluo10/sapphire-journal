use anyhow::{Context, Result};
use archelon_core::{cache, journal::Journal};
use clap::Subcommand;
use std::path::Path;

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
}

pub fn run(journal_dir: Option<&Path>, cmd: CacheCommand) -> Result<()> {
    let journal = open_journal(journal_dir)?;
    match cmd {
        CacheCommand::Info => info(&journal),
        CacheCommand::Sync => sync(&journal),
        CacheCommand::Rebuild => rebuild(&journal),
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
    let conn = cache::open_cache(journal)?;
    let info = cache::cache_info(journal, &conn)?;
    println!("path:           {}", info.db_path.display());
    println!("schema version: v{} (app: v{})", info.schema_version, cache::SCHEMA_VERSION);
    println!("files tracked:  {}", info.file_count);
    println!("entries:        {}", info.entry_count);
    println!("unique tags:    {}", info.unique_tag_count);
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
