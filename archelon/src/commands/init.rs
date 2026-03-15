use anyhow::{bail, Context, Result};
use archelon_core::journal::JournalConfig;
use std::path::{Path, PathBuf};

pub fn run(path: Option<PathBuf>) -> Result<()> {
    let target = path.as_deref().unwrap_or(Path::new("."));

    if !target.exists() {
        std::fs::create_dir_all(target)
            .with_context(|| format!("failed to create directory {}", target.display()))?;
        println!("created: {}", target.display());
    }

    let archelon_dir = target.join(".archelon");
    if archelon_dir.exists() {
        bail!("journal already initialized at {}", target.canonicalize()?.display());
    }

    std::fs::create_dir(&archelon_dir).context("failed to create .archelon directory")?;

    let config = toml::to_string_pretty(&JournalConfig::default())
        .context("failed to serialize default config")?;
    std::fs::write(archelon_dir.join("config.toml"), config)
        .context("failed to write .archelon/config.toml")?;

    // Ignore the cache directory (SQLite etc.) while keeping config.toml tracked.
    std::fs::write(archelon_dir.join(".gitignore"), "cache/\n")
        .context("failed to write .archelon/.gitignore")?;

    println!("initialized archelon journal in {}", target.canonicalize()?.display());
    Ok(())
}
