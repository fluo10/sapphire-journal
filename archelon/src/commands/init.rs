use anyhow::{bail, Context, Result};
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

    let tz = detect_timezone();
    let config = format!("[journal]\ntimezone = \"{tz}\"\n");
    std::fs::write(archelon_dir.join("config.toml"), config)
        .context("failed to write .archelon/config.toml")?;

    // Ignore the cache directory (SQLite etc.) while keeping config.toml tracked.
    std::fs::write(archelon_dir.join(".gitignore"), "cache/\n")
        .context("failed to write .archelon/.gitignore")?;

    println!("initialized archelon journal in {}", target.canonicalize()?.display());
    Ok(())
}

/// Detect the local IANA timezone name.
///
/// Resolution order:
/// 1. `TZ` environment variable
/// 2. `/etc/timezone` (Debian/Ubuntu/Arch Linux)
/// 3. Fall back to `"UTC"`
fn detect_timezone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return tz;
        }
    }
    if let Ok(contents) = std::fs::read_to_string("/etc/timezone") {
        let tz = contents.trim().to_owned();
        if !tz.is_empty() {
            return tz;
        }
    }
    "UTC".to_owned()
}
