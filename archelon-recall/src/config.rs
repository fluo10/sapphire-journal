use std::path::PathBuf;

use anyhow::Result;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Contents of `$XDG_CONFIG_HOME/archelon-recall/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserConfig {
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,
}

impl UserConfig {
    /// Canonical path: `$XDG_CONFIG_HOME/archelon-recall/config.toml`.
    pub fn path() -> PathBuf {
        xdg_config_home().join("archelon-recall").join("config.toml")
    }

    /// Load config from disk, then apply environment variable overrides.
    ///
    /// Returns the default config if the file does not exist.
    ///
    /// | Variable                                    | Field                       |
    /// |---------------------------------------------|-----------------------------|
    /// | `ARCHELON_RECALL_EMBEDDING_ENABLED`         | `embedding.enabled`         |
    /// | `ARCHELON_RECALL_EMBEDDING_VECTOR_DB`       | `embedding.vector_db`       |
    /// | `ARCHELON_RECALL_EMBEDDING_PROVIDER`        | `embedding.provider`        |
    /// | `ARCHELON_RECALL_EMBEDDING_MODEL`           | `embedding.model`           |
    /// | `ARCHELON_RECALL_EMBEDDING_API_KEY_ENV`     | `embedding.api_key_env`     |
    /// | `ARCHELON_RECALL_EMBEDDING_BASE_URL`        | `embedding.base_url`        |
    /// | `ARCHELON_RECALL_EMBEDDING_DIMENSION`       | `embedding.dimension`       |
    pub fn load() -> Result<Self> {
        let path = Self::path();
        let mut config = if !path.exists() {
            UserConfig::default()
        } else {
            let contents = std::fs::read_to_string(&path)?;
            toml::from_str(&contents)
                .map_err(|e| anyhow::anyhow!("invalid config at {}: {e}", path.display()))?
        };
        config.apply_env_overrides();
        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        let enabled = std::env::var("ARCHELON_RECALL_EMBEDDING_ENABLED")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes"));
        let vector_db = std::env::var("ARCHELON_RECALL_EMBEDDING_VECTOR_DB")
            .ok()
            .and_then(|v| match v.as_str() {
                "none" => Some(VectorDb::None),
                "sqlite_vec" => Some(VectorDb::SqliteVec),
                "lancedb" => Some(VectorDb::LanceDb),
                _ => None,
            });
        let provider = std::env::var("ARCHELON_RECALL_EMBEDDING_PROVIDER").ok();
        let model = std::env::var("ARCHELON_RECALL_EMBEDDING_MODEL").ok();
        let api_key_env = std::env::var("ARCHELON_RECALL_EMBEDDING_API_KEY_ENV").ok();
        let base_url = std::env::var("ARCHELON_RECALL_EMBEDDING_BASE_URL").ok();
        let dimension = std::env::var("ARCHELON_RECALL_EMBEDDING_DIMENSION")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());

        let any = enabled.is_some()
            || vector_db.is_some()
            || provider.is_some()
            || model.is_some()
            || api_key_env.is_some()
            || base_url.is_some()
            || dimension.is_some();

        if any {
            let embed = self.embedding.get_or_insert_with(|| EmbeddingConfig {
                enabled: false,
                vector_db: VectorDb::default(),
                provider: String::new(),
                model: String::new(),
                api_key_env: None,
                base_url: None,
                dimension: None,
                extra: IndexMap::new(),
            });
            if let Some(v) = enabled {
                embed.enabled = v;
            }
            if let Some(v) = vector_db {
                embed.vector_db = v;
            }
            if let Some(v) = provider {
                embed.provider = v;
            }
            if let Some(v) = model {
                embed.model = v;
            }
            if let Some(v) = api_key_env {
                embed.api_key_env = Some(v);
            }
            if let Some(v) = base_url {
                embed.base_url = Some(v);
            }
            if let Some(v) = dimension {
                embed.dimension = Some(v);
            }
        }
    }
}

/// Vector database backend for approximate (semantic) search.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VectorDb {
    #[default]
    None,
    SqliteVec,
    #[serde(rename = "lancedb")]
    LanceDb,
}

impl VectorDb {
    pub fn as_str(self) -> &'static str {
        match self {
            VectorDb::None => "none",
            VectorDb::SqliteVec => "sqlite_vec",
            VectorDb::LanceDb => "lancedb",
        }
    }
}

/// Text embedding provider configuration (`[embedding]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub vector_db: VectorDb,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension: Option<u32>,
    #[serde(flatten)]
    pub extra: IndexMap<String, toml::Value>,
}

impl EmbeddingConfig {
    pub fn to_retrieve_embed_config(&self) -> archelon_retrieve::EmbeddingConfig {
        archelon_retrieve::EmbeddingConfig {
            provider: self.provider.clone(),
            model: self.model.clone(),
            api_key_env: self.api_key_env.clone(),
            base_url: self.base_url.clone(),
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
