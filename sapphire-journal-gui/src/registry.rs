use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, Result};

/// One registered journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Stable journal ID from `.sapphire-journal/config.toml [journal].id`.
    pub id: Uuid,
    /// User-visible display name.
    pub name: String,
    /// Absolute path to the journal's git repository root
    /// (`~/.local/share/sapphire-journal/journals/<uuid>/`).
    pub storage_path: PathBuf,
}

/// The list of all journals known to the GUI, persisted as TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JournalRegistry {
    #[serde(default)]
    pub journals: Vec<RegistryEntry>,
}

impl JournalRegistry {
    /// `$XDG_DATA_HOME/sapphire-journal/`
    pub fn data_dir() -> PathBuf {
        xdg_data_home().join("sapphire-journal")
    }

    /// `$XDG_DATA_HOME/sapphire-journal/journals/`
    pub fn journals_dir() -> PathBuf {
        Self::data_dir().join("journals")
    }

    /// `$XDG_DATA_HOME/sapphire-journal/journals.toml`
    pub fn registry_path() -> PathBuf {
        Self::data_dir().join("journals.toml")
    }

    /// Load the registry from disk, returning an empty registry if the file does not exist.
    pub fn load() -> Result<Self> {
        let path = Self::registry_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path)?;
        let registry: Self = toml::from_str(&contents)?;
        Ok(registry)
    }

    /// Persist the registry to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::registry_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(&path, contents)?;
        Ok(())
    }

    /// Add an entry, ignoring duplicates (by `id`).
    pub fn add(&mut self, entry: RegistryEntry) {
        if !self.journals.iter().any(|e| e.id == entry.id) {
            self.journals.push(entry);
        }
    }

    /// Remove the entry with the given journal config ID.
    pub fn remove_by_id(&mut self, id: Uuid) {
        self.journals.retain(|e| e.id != id);
    }
}

fn xdg_data_home() -> PathBuf {
    if let Ok(val) = std::env::var("XDG_DATA_HOME") {
        if !val.is_empty() {
            return PathBuf::from(val);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local").join("share");
    }
    std::env::temp_dir()
}

/// Initialize a new journal at `storage_path`:
/// - runs `git init`
/// - creates `.sapphire-journal/config.toml` and `.sapphire-journal/.gitignore`
/// - returns the stable journal ID
pub fn init_journal(storage_path: &PathBuf) -> Result<Uuid> {
    use sapphire_journal_core::journal::{Journal, JournalConfig};

    std::fs::create_dir_all(storage_path)?;
    git2::Repository::init(storage_path)?;

    let journal_dir = storage_path.join(".sapphire-journal");
    if journal_dir.exists() {
        return Err(AppError::Domain(
            "journal already initialized at this path".to_string(),
        ));
    }
    std::fs::create_dir(&journal_dir)?;

    let config = toml::to_string_pretty(&JournalConfig::default())?;
    std::fs::write(journal_dir.join("config.toml"), config)?;
    std::fs::write(journal_dir.join(".gitignore"), "cache/\n")?;

    let id = Journal::from_root(storage_path.clone())
        .map_err(|e| AppError::Domain(e.to_string()))?
        .journal_id()
        .map_err(|e| AppError::Domain(e.to_string()))?;

    Ok(id)
}
