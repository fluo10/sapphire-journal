use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Contents of `$XDG_CONFIG_HOME/archelon/config.toml`.
///
/// This is a user-level (host-level) config that controls machine-specific
/// settings such as caching backends. It is intentionally separate from the
/// per-journal `.archelon/config.toml` so that the same journal can be shared
/// across machines with different hardware capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserConfig {
    #[serde(default)]
    pub cache: CacheConfig,
}

impl UserConfig {
    /// Canonical path to the user config file.
    ///
    /// Resolves to `$XDG_CONFIG_HOME/archelon/config.toml`
    /// (or `~/.config/archelon/config.toml` when `XDG_CONFIG_HOME` is not set).
    pub fn path() -> PathBuf {
        xdg_config_home().join("archelon").join("config.toml")
    }

    /// Load the user config from disk.
    ///
    /// Returns the default config (all fields at their defaults) if the file
    /// does not exist.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(UserConfig::default());
        }
        let contents = std::fs::read_to_string(&path)?;
        toml::from_str(&contents).map_err(|e| Error::InvalidConfig(e.to_string()))
    }
}

/// Cache-related configuration (`[cache]` section in the user config).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Vector DB backend used for approximate (semantic) text search.
    ///
    /// Defaults to [`VectorDb::None`], which disables vector search entirely.
    /// Changing this requires a text embedding provider to also be configured
    /// in the `[cache.embedding]` section.
    #[serde(default)]
    pub vector_db: VectorDb,

    /// Text embedding provider settings.
    ///
    /// Required when `vector_db` is not [`VectorDb::None`].
    /// When `vector_db = "none"` this section is ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<EmbeddingConfig>,

    /// Unknown fields preserved for round-trip TOML compatibility.
    #[serde(flatten)]
    pub extra: IndexMap<String, toml::Value>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        CacheConfig {
            vector_db: VectorDb::default(),
            embedding: None,
            extra: IndexMap::new(),
        }
    }
}

/// Vector database backend for approximate (semantic) text search.
///
/// Select based on what your host machine supports:
///
/// | Variant      | Description                                              |
/// |--------------|----------------------------------------------------------|
/// | `none`       | Vector search disabled (default, no extra dependencies)  |
/// | `sqlite_vec` | sqlite-vec extension, integrated with the SQLite cache   |
/// | `lancedb`    | LanceDB, suitable for multimodal / larger-scale use      |
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VectorDb {
    /// Vector search is disabled. No embedding model is required.
    #[default]
    None,
    /// Use the sqlite-vec extension, stored inside the existing SQLite cache
    /// database. Lightweight and requires no additional infrastructure.
    SqliteVec,
    /// Use LanceDB for vector storage. More capable and suitable for future
    /// multimodal embeddings, but requires a separate data directory.
    LanceDb,
}

impl VectorDb {
    /// Human-readable name shown in `config show` output.
    pub fn as_str(self) -> &'static str {
        match self {
            VectorDb::None => "none",
            VectorDb::SqliteVec => "sqlite_vec",
            VectorDb::LanceDb => "lancedb",
        }
    }
}

/// Text embedding provider configuration (`[cache.embedding]` subsection).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Embedding provider identifier.
    ///
    /// Examples: `"openai"`, `"ollama"`.
    pub provider: String,

    /// Model name understood by the provider.
    ///
    /// Examples: `"text-embedding-3-small"` (OpenAI), `"nomic-embed-text"` (Ollama).
    pub model: String,

    /// Name of the environment variable that holds the API key.
    ///
    /// Used by OpenAI-compatible providers. Defaults to `OPENAI_API_KEY` when
    /// omitted. Not required for local providers such as Ollama.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,

    /// Base URL of the embedding API endpoint.
    ///
    /// Required for local providers (e.g. `"http://localhost:11434"` for Ollama).
    /// OpenAI-compatible providers default to the official API endpoint when this
    /// is omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Unknown fields preserved for round-trip TOML compatibility.
    #[serde(flatten)]
    pub extra: IndexMap<String, toml::Value>,
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
