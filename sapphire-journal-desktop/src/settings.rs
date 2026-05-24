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

    /// HTTP MCP server settings (for AI agent integrations).
    #[serde(default)]
    pub mcp_http: McpHttpSettings,

    /// Right metadata sidebar (Obsidian-style) preferences.
    #[serde(default)]
    pub right_sidebar: RightSidebarSettings,
}

/// Persisted state of the right metadata sidebar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RightSidebarSettings {
    #[serde(default)]
    pub visible: bool,
    #[serde(default = "default_right_sidebar_width")]
    pub width: f32,
    #[serde(default)]
    pub active_tab: RightTab,
}

impl Default for RightSidebarSettings {
    fn default() -> Self {
        Self {
            visible: false,
            width: default_right_sidebar_width(),
            active_tab: RightTab::default(),
        }
    }
}

fn default_right_sidebar_width() -> f32 {
    280.0
}

/// Which tab is currently selected in the right metadata sidebar.
///
/// Only one tab exists today; the enum is laid out so future tabs (outline,
/// backlinks, etc.) can be added without breaking the on-disk format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RightTab {
    #[default]
    Metadata,
}

/// Configuration for the optional in-process MCP server exposed over HTTP.
///
/// Always binds to `127.0.0.1` — exposing the journal to other hosts on the
/// network is not supported yet and would require an authentication layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpHttpSettings {
    /// Run an MCP HTTP server alongside the GUI whenever a journal is open.
    #[serde(default)]
    pub enabled: bool,

    /// TCP port the server listens on (loopback only).
    #[serde(default = "default_mcp_http_port")]
    pub port: u16,
}

fn default_mcp_http_port() -> u16 {
    3737
}

impl Default for McpHttpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_mcp_http_port(),
        }
    }
}

impl McpHttpSettings {
    pub const BIND: &'static str = "127.0.0.1";
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
