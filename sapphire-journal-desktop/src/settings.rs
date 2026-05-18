//! Persisted UI preferences (separate from `journals.toml`).
//!
//! Currently only records the last-opened journal so the app can resume
//! straight into it on next start.  Lives at
//! `$XDG_DATA_HOME/sapphire-journal/settings.toml`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::registry::JournalRegistry;

/// On-disk user preferences for the GUI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// `id` of the journal that was open when the app last exited.
    #[serde(default)]
    pub last_opened_journal_id: Option<Uuid>,
}

impl Settings {
    /// `$XDG_DATA_HOME/sapphire-journal/settings.toml`
    pub fn path() -> PathBuf {
        JournalRegistry::data_dir().join("settings.toml")
    }

    /// Load the settings from disk, returning defaults if the file does not exist.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path)?;
        let settings: Self = toml::from_str(&contents)?;
        Ok(settings)
    }

    /// Persist the settings to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(&path, contents)?;
        Ok(())
    }
}
