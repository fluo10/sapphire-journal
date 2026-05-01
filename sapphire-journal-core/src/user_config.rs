use std::path::PathBuf;
use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub use sapphire_workspace::{EmbeddingConfig, RetrieveConfig, SyncBackendKind, SyncConfig, VectorDb};

/// Default cadence (minutes) for periodic sync when nothing is configured.
///
/// Periodic sync is enabled by default at this interval. The MCP server
/// also drives an incremental cache + retrieve refresh on the same tick, so
/// even workspaces without a git sync backend benefit from having it on.
/// Set `sync_interval_minutes = 0` to opt out.
const DEFAULT_SYNC_INTERVAL_MINUTES: u32 = 10;

fn default_sync_interval_minutes() -> Option<u32> {
    Some(DEFAULT_SYNC_INTERVAL_MINUTES)
}

/// Contents of `$XDG_CONFIG_HOME/sapphire-journal/config.toml`.
///
/// This is a user-level (host-level) config that controls machine-specific
/// settings such as caching backends. It is intentionally separate from the
/// per-journal `.sapphire-journal/config.toml` so that the same journal can be
/// shared across machines with different hardware capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default)]
    pub cache: CacheConfig,

    /// Sync backend configuration (`[sync]` section).
    #[serde(default)]
    pub sync: SyncConfig,

    /// How often to run periodic sync, in minutes.
    ///
    /// Defaults to 10 minutes. Set to `0` to disable.
    #[serde(default = "default_sync_interval_minutes", skip_serializing_if = "Option::is_none")]
    pub sync_interval_minutes: Option<u32>,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            cache: CacheConfig::default(),
            sync: SyncConfig::default(),
            sync_interval_minutes: default_sync_interval_minutes(),
        }
    }
}

impl UserConfig {
    /// Periodic sync cadence as a [`Duration`]. Returns `None` when disabled
    /// (`sync_interval_minutes = 0`).
    pub fn sync_interval(&self) -> Option<Duration> {
        self.sync_interval_minutes
            .filter(|&m| m > 0)
            .map(|m| Duration::from_secs(u64::from(m) * 60))
    }

    /// Canonical path to the user config file.
    ///
    /// Resolves to `$XDG_CONFIG_HOME/sapphire-journal/config.toml`.
    pub fn path() -> PathBuf {
        xdg_config_home().join("sapphire-journal").join("config.toml")
    }

    /// Load the user config from disk, then apply environment variable overrides.
    ///
    /// Returns the default config if the file does not exist.
    ///
    /// Environment variables:
    ///
    /// | Variable | Field |
    /// |---|---|
    /// | `SAPPHIRE_JOURNAL_CACHE_RETRIEVE_DB` | `cache.retrieve.db` |
    /// | `SAPPHIRE_JOURNAL_CACHE_EMBEDDING_ENABLED` | `cache.retrieve.embedding.enabled` |
    /// | `SAPPHIRE_JOURNAL_CACHE_EMBEDDING_PROVIDER` | `cache.retrieve.embedding.provider` |
    /// | `SAPPHIRE_JOURNAL_CACHE_EMBEDDING_MODEL` | `cache.retrieve.embedding.model` |
    /// | `SAPPHIRE_JOURNAL_CACHE_EMBEDDING_API_KEY_ENV` | `cache.retrieve.embedding.api_key_env` |
    /// | `SAPPHIRE_JOURNAL_CACHE_EMBEDDING_BASE_URL` | `cache.retrieve.embedding.base_url` |
    /// | `SAPPHIRE_JOURNAL_CACHE_EMBEDDING_DIMENSION` | `cache.retrieve.embedding.dimension` |
    /// | `SAPPHIRE_JOURNAL_SYNC_BACKEND` | `sync.backend` (`auto`/`none`/`git`) |
    /// | `SAPPHIRE_JOURNAL_SYNC_INTERVAL_MINUTES` | `sync_interval_minutes` |
    pub fn load() -> Result<Self> {
        let path = Self::path();
        let mut config = if !path.exists() {
            UserConfig::default()
        } else {
            let contents = std::fs::read_to_string(&path)?;
            toml::from_str(&contents).map_err(|e| Error::InvalidConfig(e.to_string()))?
        };
        config.apply_env_overrides();
        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        let db = std::env::var("SAPPHIRE_JOURNAL_CACHE_RETRIEVE_DB")
            .ok()
            .and_then(|v| match v.as_str() {
                "none" => Some(VectorDb::None),
                "sqlite_vec" => Some(VectorDb::SqliteVec),
                "lancedb" => Some(VectorDb::LanceDb),
                _ => None,
            });
        let enabled = std::env::var("SAPPHIRE_JOURNAL_CACHE_EMBEDDING_ENABLED").ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes"));
        let provider = std::env::var("SAPPHIRE_JOURNAL_CACHE_EMBEDDING_PROVIDER").ok();
        let model = std::env::var("SAPPHIRE_JOURNAL_CACHE_EMBEDDING_MODEL").ok();
        let api_key_env = std::env::var("SAPPHIRE_JOURNAL_CACHE_EMBEDDING_API_KEY_ENV").ok();
        let base_url = std::env::var("SAPPHIRE_JOURNAL_CACHE_EMBEDDING_BASE_URL").ok();
        let dimension = std::env::var("SAPPHIRE_JOURNAL_CACHE_EMBEDDING_DIMENSION")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());

        let any = db.is_some()
            || enabled.is_some()
            || provider.is_some()
            || model.is_some()
            || api_key_env.is_some()
            || base_url.is_some()
            || dimension.is_some();

        if any {
            if let Some(v) = db { self.cache.retrieve.db = v; }
            let embed = self.cache.retrieve.embedding.get_or_insert_with(EmbeddingConfig::default);
            if let Some(v) = enabled { embed.enabled = v; }
            if let Some(v) = provider { embed.provider = v; }
            if let Some(v) = model { embed.model = v; }
            if let Some(v) = api_key_env { embed.api_key_env = Some(v); }
            if let Some(v) = base_url { embed.base_url = Some(v); }
            if let Some(v) = dimension { embed.dimension = Some(v); }
        }

        if let Ok(v) = std::env::var("SAPPHIRE_JOURNAL_SYNC_BACKEND") {
            if let Some(backend) = parse_sync_backend(&v) {
                self.sync.backend = backend;
            }
        }
        if let Ok(v) = std::env::var("SAPPHIRE_JOURNAL_SYNC_INTERVAL_MINUTES") {
            if let Ok(n) = v.parse::<u32>() {
                self.sync_interval_minutes = Some(n);
            }
        }
    }
}

fn parse_sync_backend(s: &str) -> Option<sapphire_workspace::SyncBackendKind> {
    use sapphire_workspace::SyncBackendKind;
    match s {
        "auto" => Some(SyncBackendKind::Auto),
        "none" => Some(SyncBackendKind::None),
        "git" => Some(SyncBackendKind::Git),
        _ => None,
    }
}

/// Cache-related configuration (`[cache]` section in the user config).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Retrieve backend and embedding settings.
    #[serde(default)]
    pub retrieve: RetrieveConfig,

    /// Unknown fields preserved for round-trip TOML compatibility.
    #[serde(flatten)]
    pub extra: IndexMap<String, toml::Value>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        CacheConfig {
            retrieve: RetrieveConfig::default(),
            extra: IndexMap::new(),
        }
    }
}

fn xdg_config_home() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config");
    }
    std::env::temp_dir()
}
