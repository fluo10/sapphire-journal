use std::path::{Path, PathBuf};

use grain_id::GrainId;
use chrono::Datelike as _;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};

const ARCHELON_DIR: &str = ".sapphire-journal";

/// A located journal — a directory tree that contains a `.sapphire-journal` directory.
#[derive(Debug, Clone)]
pub struct Journal {
    /// The directory that directly contains `.sapphire-journal/`.
    pub root: PathBuf,
}

impl Journal {
    /// Create a `Journal` from an explicit root path.
    ///
    /// Returns `Err(Error::JournalNotFound)` if `root` does not contain a `.sapphire-journal` directory.
    pub fn from_root(root: PathBuf) -> Result<Self> {
        if root.join(ARCHELON_DIR).is_dir() {
            Ok(Journal { root })
        } else {
            Err(Error::JournalNotFound)
        }
    }

    /// Walk up from `start` until a directory containing `.sapphire-journal/` is found.
    ///
    /// Returns `Err(Error::JournalNotFound)` if no such directory exists.
    pub fn find_from(start: &Path) -> Result<Self> {
        let mut current = start.to_path_buf();
        loop {
            if current.join(ARCHELON_DIR).is_dir() {
                return Ok(Journal { root: current });
            }
            if !current.pop() {
                return Err(Error::JournalNotFound);
            }
        }
    }

    /// Walk up from the current working directory.
    pub fn find() -> Result<Self> {
        let cwd = std::env::current_dir()?;
        Self::find_from(&cwd)
    }

    /// Path to the `.sapphire-journal` directory itself.
    pub fn journal_dir(&self) -> PathBuf {
        self.root.join(ARCHELON_DIR)
    }

    /// Path to the directory that directly contains year subdirectories.
    ///
    /// Returns `root.join(entries_dir)` when `entries_dir` is configured in
    /// `.sapphire-journal/config.toml`, otherwise returns `root` itself.
    pub fn entries_root(&self) -> Result<PathBuf> {
        let config = self.config()?;
        Ok(match config.journal.entries_dir {
            Some(ref dir) if !dir.is_empty() => self.root.join(dir),
            _ => self.root.clone(),
        })
    }

    /// Read the journal config from `.sapphire-journal/config.toml`.
    /// Returns the default config if the file does not exist.
    pub fn config(&self) -> Result<JournalConfig> {
        let path = self.journal_dir().join("config.toml");
        if !path.exists() {
            return Ok(JournalConfig::default());
        }
        let contents = std::fs::read_to_string(&path)?;
        toml::from_str(&contents).map_err(|e| Error::InvalidConfig(e.to_string()))
    }

    /// Find a single `.md` entry file whose stem starts with `id_prefix`.
    ///
    /// Scans `self.root` and all direct year subdirectories.
    /// Returns `Err(EntryNotFound)` if nothing matches, or `Err(AmbiguousId)`
    /// if more than one file matches.
    pub fn find_entry_by_id(&self, id_prefix: &str) -> Result<PathBuf> {
        let mut matches = Vec::new();
        for dir in std::iter::once(self.entries_root()?).chain(self.year_subdirs()?) {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("md")
                    && p.file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|stem| stem.starts_with(id_prefix))
                {
                    matches.push(p);
                }
            }
        }

        match matches.len() {
            0 => Err(Error::EntryNotFound(id_prefix.to_owned())),
            1 => Ok(matches.remove(0)),
            n => Err(Error::AmbiguousId(id_prefix.to_owned(), n)),
        }
    }

    /// Collect all `.md` entry files in the journal: root + year subdirectories.
    pub fn collect_entries(&self) -> Result<Vec<PathBuf>> {
        let entries_root = self.entries_root()?;
        let mut paths = Vec::new();
        collect_md_in(&entries_root, &mut paths)?;
        for subdir in self.year_subdirs()? {
            collect_md_in(&subdir, &mut paths)?;
        }
        paths.sort();
        Ok(paths)
    }

    fn year_subdirs(&self) -> Result<Vec<PathBuf>> {
        let root = self.entries_root()?;
        let mut dirs = Vec::new();
        for entry in std::fs::read_dir(&root)?.filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.is_dir() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if !name.starts_with('.') && name.chars().all(|c| c.is_ascii_digit()) {
                        dirs.push(p);
                    }
                }
            }
        }
        Ok(dirs)
    }

    /// Return the stable journal ID, generating and persisting it if not yet set.
    pub fn journal_id(&self) -> Result<Uuid> {
        let config = self.config()?;
        if let Some(id) = config.journal.id {
            return Ok(id);
        }
        let id = Uuid::new_v4();
        self.save_journal_id(id)?;
        Ok(id)
    }

    fn save_journal_id(&self, id: Uuid) -> Result<()> {
        let mut config = self.config()?;
        config.journal.id = Some(id);
        let path = self.journal_dir().join("config.toml");
        let content = toml::to_string_pretty(&config)
            .map_err(|e| Error::InvalidConfig(e.to_string()))?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Machine-local cache directory for this journal.
    ///
    /// Resolves to `$XDG_CACHE_HOME/sapphire-journal/{journal_id}/`
    /// (or `~/.cache/sapphire-journal/...` when `XDG_CACHE_HOME` is not set).
    /// This directory is intentionally outside the journal directory so it is
    /// never synced by git, Syncthing, or Nextcloud.
    ///
    /// Individual cache files within this directory are named with their schema
    /// version (e.g. `cache_v2.db`) so that old data survives schema upgrades
    /// until explicitly removed with `sapphire-journal cache clean`.
    pub fn cache_dir(&self) -> Result<PathBuf> {
        let id = self.journal_id()?;
        Ok(xdg_cache_home().join("sapphire-journal").join(id.to_string()))
    }

    /// Path to the retrieve database (FTS + vector index) for this journal.
    ///
    /// Resolves to `{cache_dir}/retrieve_v1.db`.
    pub fn retrieve_db_path(&self) -> Result<PathBuf> {
        use sapphire_retrieve::db::SCHEMA_VERSION as RETRIEVE_SCHEMA_VERSION;
        Ok(self.cache_dir()?.join(format!("retrieve_v{RETRIEVE_SCHEMA_VERSION}.db")))
    }

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

fn collect_md_in(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(()) };
    for entry in rd.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(p);
        }
    }
    Ok(())
}

// ── config ────────────────────────────────────────────────────────────────────

/// Contents of `.sapphire-journal/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JournalConfig {
    #[serde(default)]
    pub journal: JournalSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalSection {
    /// First day of the week, used by `--this-week`. Defaults to `monday`.
    #[serde(default)]
    pub week_start: WeekStart,

    /// Stable identifier for this journal, used to locate the machine-local
    /// SQLite cache at `$XDG_CACHE_HOME/sapphire-journal/{id}/cache.db`.
    /// Generated on first cache access and stored here so the cache survives
    /// directory moves and is never synced by git/Syncthing/Nextcloud.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,

    /// What to do when two entries share the same title during cache sync.
    ///
    /// - `allow` (default): duplicates are silently permitted.
    /// - `warn`: a warning is printed to stderr but sync succeeds.
    /// - `error`: sync fails immediately with [`Error::DuplicateTitle`].
    #[serde(default)]
    pub duplicate_title: DuplicateTitlePolicy,

    /// Sub-directory (relative to the journal root) where year directories are
    /// created.  When unset, year directories are placed directly under the
    /// journal root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries_dir: Option<String>,

    /// Unknown fields preserved for round-trip compatibility.
    #[serde(flatten)]
    pub extra: IndexMap<String, toml::Value>,
}

impl Default for JournalSection {
    fn default() -> Self {
        JournalSection {
            week_start: WeekStart::Monday,
            id: None,
            duplicate_title: DuplicateTitlePolicy::default(),
            entries_dir: None,
            extra: IndexMap::new(),
        }
    }
}

/// Controls how duplicate entry titles are treated during cache sync.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateTitlePolicy {
    /// Duplicates are silently allowed.
    Allow,
    /// Print a warning to stderr for each duplicate title, but continue.
    #[default]
    Warn,
    /// Abort sync with [`Error::DuplicateTitle`] on the first duplicate found.
    Error,
}

/// First day of the week for `--this-week` calculations.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    #[default]
    Monday,
    Sunday,
}

// ── filename helpers ──────────────────────────────────────────────────────────

/// Convert a title to a filename-safe slug.
///
/// Lowercases the string, replaces whitespace with `_`, and strips any
/// character that is not ASCII alphanumeric or `_`.
///
/// ```
/// # use sapphire_journal_core::journal::slugify;
/// assert_eq!(slugify("My Example Entry!"), "my_example_entry");
/// ```
pub fn slugify(title: &str) -> String {
    title
        .chars()
        .map(|c| match c {
            c if c.is_whitespace() => '_',
            c if c.is_ascii_uppercase() => c.to_ascii_lowercase(),
            c if c.is_ascii_alphanumeric() => c,
            '-' | '_' | '.' => c,
            c if !c.is_ascii() => c,
            _ => '_',
        })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// Build the canonical entry filename: `{id}_{slug}.md`.
///
/// If the slug is empty the filename is just `{id}.md`.
pub fn entry_filename(id: GrainId, title: &str) -> String {
    let slug = slugify(title);
    if slug.is_empty() {
        format!("{id}.md")
    } else {
        format!("{id}_{slug}.md")
    }
}

/// Generate a relative path for a new entry: `{year}/{id}_{slug}.md`.
///
/// The ID is based on the current Unix time (`GrainId::now_unix()`), so
/// filenames sort chronologically within a year directory.
///
/// Returns `(relative_path, id)` so the caller can embed the ID in frontmatter.
pub fn new_entry_path(title: &str) -> (PathBuf, GrainId) {
    let id = GrainId::now_unix();
    let year = chrono::Local::now().year();
    let path = PathBuf::from(year.to_string()).join(entry_filename(id, title));
    (path, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My Example Entry"), "my_example_entry");
    }

    #[test]
    fn slugify_preserves_hyphens() {
        assert_eq!(slugify("2026-03-17"), "2026-03-17");
    }

    #[test]
    fn slugify_preserves_japanese() {
        assert_eq!(slugify("日本語タイトル"), "日本語タイトル");
    }

    #[test]
    fn slugify_japanese_with_spaces() {
        assert_eq!(slugify("今日の メモ"), "今日の_メモ");
    }

    #[test]
    fn slugify_replaces_special_chars() {
        assert_eq!(slugify("Hello, World! (2026)"), "hello_world_2026");
        assert_eq!(slugify("a/b:c*d"), "a_b_c_d");
    }

    #[test]
    fn slugify_trims_underscores() {
        assert_eq!(slugify("  leading"), "leading");
    }
}
