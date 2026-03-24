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

    /// Load the user config from disk, then apply environment variable overrides.
    ///
    /// Returns the default config (all fields at their defaults) if the file
    /// does not exist.
    ///
    /// The following environment variables override the corresponding `config.toml`
    /// fields when set to a non-empty value:
    ///
    /// | Variable                              | Field                          |
    /// |---------------------------------------|--------------------------------|
    /// | `ARCHELON_CACHE_VECTOR_DB`            | `cache.vector_db`              |
    /// | `ARCHELON_CACHE_EMBEDDING_PROVIDER`   | `cache.embedding.provider`     |
    /// | `ARCHELON_CACHE_EMBEDDING_MODEL`      | `cache.embedding.model`        |
    /// | `ARCHELON_CACHE_EMBEDDING_API_KEY_ENV`| `cache.embedding.api_key_env`  |
    /// | `ARCHELON_CACHE_EMBEDDING_BASE_URL`   | `cache.embedding.base_url`     |
    /// | `ARCHELON_CACHE_EMBEDDING_DIMENSION`  | `cache.embedding.dimension`    |
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

    /// Apply environment variable overrides on top of the already-loaded config.
    fn apply_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("ARCHELON_CACHE_VECTOR_DB") {
            match val.as_str() {
                "none" => self.cache.vector_db = VectorDb::None,
                "sqlite_vec" => self.cache.vector_db = VectorDb::SqliteVec,
                "lancedb" => self.cache.vector_db = VectorDb::LanceDb,
                _ => {}
            }
        }

        let provider = std::env::var("ARCHELON_CACHE_EMBEDDING_PROVIDER").ok();
        let model = std::env::var("ARCHELON_CACHE_EMBEDDING_MODEL").ok();
        let api_key_env = std::env::var("ARCHELON_CACHE_EMBEDDING_API_KEY_ENV").ok();
        let base_url = std::env::var("ARCHELON_CACHE_EMBEDDING_BASE_URL").ok();
        let dimension = std::env::var("ARCHELON_CACHE_EMBEDDING_DIMENSION")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());

        let any_embedding = provider.is_some()
            || model.is_some()
            || api_key_env.is_some()
            || base_url.is_some()
            || dimension.is_some();

        if any_embedding {
            let embed = self.cache.embedding.get_or_insert_with(|| EmbeddingConfig {
                provider: String::new(),
                model: String::new(),
                api_key_env: None,
                base_url: None,
                dimension: None,
                extra: IndexMap::new(),
            });
            if let Some(v) = provider { embed.provider = v; }
            if let Some(v) = model { embed.model = v; }
            if let Some(v) = api_key_env { embed.api_key_env = Some(v); }
            if let Some(v) = base_url { embed.base_url = Some(v); }
            if let Some(v) = dimension { embed.dimension = Some(v); }
        }
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
    #[serde(rename = "lancedb")]
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
    /// - `"openai"` — OpenAI-compatible REST API
    /// - `"ollama"` — local Ollama server
    /// - `"fastembed"` — local ONNX inference, no server required
    pub provider: String,

    /// Model name understood by the provider.
    ///
    /// - OpenAI: `"text-embedding-3-small"`, `"text-embedding-3-large"`, …
    /// - Ollama: `"nomic-embed-text"`, `"mxbai-embed-large"`, …
    /// - fastembed: `"AllMiniLML6V2"` (384), `"BGESmallENV15"` (384),
    ///   `"BGEBaseENV15"` (768), `"NomicEmbedTextV1"` (768), …
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

    /// Output vector dimension of the model.
    ///
    /// Required when `vector_db = "sqlite_vec"`.  Must exactly match the model's
    /// actual output size; a mismatch will cause the vector table to be recreated.
    ///
    /// Common values:
    /// - `1536` — `text-embedding-3-small` (OpenAI)
    /// - `3072` — `text-embedding-3-large` (OpenAI)
    /// - `768`  — `nomic-embed-text` (Ollama)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension: Option<u32>,

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
