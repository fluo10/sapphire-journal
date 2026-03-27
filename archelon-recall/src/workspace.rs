use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use archelon_retrieve::db::SCHEMA_VERSION;

/// A resolved workspace directory.
pub struct Workspace {
    /// Canonicalized absolute path of the workspace root.
    pub root: PathBuf,
}

impl Workspace {
    /// Resolve the workspace directory:
    /// 1. `explicit` parameter (no confirmation prompt)
    /// 2. `ARCHELON_RECALL_WORKSPACE` env var (no confirmation prompt)
    /// 3. Current working directory (TTY: ask for confirmation; non-TTY: use directly)
    pub fn resolve(explicit: Option<&Path>) -> Result<Self> {
        let root = if let Some(dir) = explicit {
            dir.canonicalize()
                .map_err(|e| anyhow!("cannot access workspace dir '{}': {e}", dir.display()))?
        } else if let Ok(val) = std::env::var("ARCHELON_RECALL_WORKSPACE") {
            if !val.is_empty() {
                Path::new(&val)
                    .canonicalize()
                    .map_err(|e| anyhow!("cannot access ARCHELON_RECALL_WORKSPACE={val}: {e}"))?
            } else {
                resolve_cwd()?
            }
        } else {
            resolve_cwd()?
        };
        Ok(Self { root })
    }

    /// `$XDG_CACHE_HOME/archelon-recall/{hash16}-{basename}/`
    pub fn cache_dir(&self) -> PathBuf {
        let hash = path_hash(&self.root);
        let basename = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "root".to_owned());
        xdg_cache_home()
            .join("archelon-recall")
            .join(format!("{:016x}-{}", hash, basename))
    }

    /// `cache_dir()/retrieve_v{SCHEMA_VERSION}.db`
    pub fn retrieve_db_path(&self) -> PathBuf {
        self.cache_dir()
            .join(format!("retrieve_v{SCHEMA_VERSION}.db"))
    }
}

fn resolve_cwd() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    if std::io::stdin().is_terminal() {
        eprint!(
            "No workspace specified. Use '{}'? [Y/n]: ",
            cwd.display()
        );
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let trimmed = line.trim();
        if !trimmed.is_empty() && !matches!(trimmed, "y" | "Y") {
            eprintln!("Aborted.");
            std::process::exit(1);
        }
    }
    Ok(cwd)
}

/// FNV-1a hash of a path (no extra crates needed).
fn path_hash(p: &Path) -> u64 {
    const OFFSET: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut h = OFFSET;
    for b in p.as_os_str().as_encoded_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

fn xdg_cache_home() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache");
    }
    std::env::temp_dir()
}
