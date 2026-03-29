use std::path::PathBuf;

use anyhow::Result;
use sapphire_retrieve::{Embedder, RetrieveDb, db::SCHEMA_VERSION};
use tokio::sync::OnceCell;

use crate::{
    config::{UserConfig, VectorDb},
    indexer::sync_workspace,
    workspace::Workspace,
};

/// An open workspace paired with its lazily-initialised search infrastructure.
pub struct WorkspaceState {
    pub workspace: Workspace,
    retrieve_db: RetrieveDb,
    embedder: OnceCell<Option<Box<dyn Embedder + Send + Sync>>>,
}

/// Database statistics returned by [`WorkspaceState::db_info`].
pub struct DbInfo {
    pub db_path: PathBuf,
    pub schema_version: i32,
    pub document_count: u64,
    pub embedding_dim: u32,
    pub vector_count: u64,
    pub pending_count: u64,
}

impl WorkspaceState {
    /// Open (or create) the retrieve DB for `workspace`.
    pub fn open(workspace: Workspace) -> Result<Self> {
        let retrieve_db = RetrieveDb::open(&workspace.retrieve_db_path())?;
        Ok(Self {
            workspace,
            retrieve_db,
            embedder: OnceCell::new(),
        })
    }

    /// Delete and recreate the retrieve DB from scratch.
    pub fn rebuild(workspace: Workspace) -> Result<Self> {
        let retrieve_db = RetrieveDb::rebuild(&workspace.retrieve_db_path())?;
        Ok(Self {
            workspace,
            retrieve_db,
            embedder: OnceCell::new(),
        })
    }

    pub fn retrieve_db(&self) -> &RetrieveDb {
        &self.retrieve_db
    }

    pub fn embedder(&self) -> Option<&dyn Embedder> {
        Some(self.embedder.get()?.as_ref()?.as_ref())
    }

    // ── vector backend ────────────────────────────────────────────────────────

    /// Initialise the vector backend (sync). Idempotent.
    pub fn load_retrieve_backend(&self, config: &UserConfig) -> Result<()> {
        let Some(embed_cfg) = &config.embedding else {
            return Ok(());
        };
        if !embed_cfg.enabled {
            return Ok(());
        }
        let Some(dim) = embed_cfg.dimension else {
            return Ok(());
        };
        self.init_vector_backend(embed_cfg.vector_db, dim)
    }

    /// Async version of [`load_retrieve_backend`](Self::load_retrieve_backend).
    pub async fn load_retrieve_backend_async(&self, config: &UserConfig) -> Result<()> {
        let Some(embed_cfg) = &config.embedding else {
            return Ok(());
        };
        if !embed_cfg.enabled {
            return Ok(());
        }
        let Some(dim) = embed_cfg.dimension else {
            return Ok(());
        };
        let vector_db = embed_cfg.vector_db;

        // LanceDB has its own internal Tokio runtime; init it directly to avoid
        // "cannot start a runtime within a runtime" panics.
        #[cfg(feature = "lancedb-store")]
        if vector_db == VectorDb::LanceDb {
            use sapphire_retrieve::lancedb_store;
            let lancedb_dir = lancedb_store::versioned_dir(&self.workspace.cache_dir());
            self.retrieve_db.init_lancedb(&lancedb_dir, dim)?;
            return Ok(());
        }

        self.init_vector_backend(vector_db, dim)
    }

    fn init_vector_backend(&self, vector_db: VectorDb, dim: u32) -> Result<()> {
        match vector_db {
            VectorDb::None => {}
            VectorDb::SqliteVec => {
                self.retrieve_db.init_sqlite_vec(dim)?;
            }
            #[cfg(feature = "lancedb-store")]
            VectorDb::LanceDb => {
                use sapphire_retrieve::lancedb_store;
                let lancedb_dir = lancedb_store::versioned_dir(&self.workspace.cache_dir());
                self.retrieve_db.init_lancedb(&lancedb_dir, dim)?;
            }
            #[cfg(not(feature = "lancedb-store"))]
            VectorDb::LanceDb => {
                anyhow::bail!(
                    "lancedb support is not compiled in (enable the `lancedb-store` feature)"
                );
            }
        }
        Ok(())
    }

    // ── embedder ──────────────────────────────────────────────────────────────

    /// Initialise the embedder (sync). Idempotent.
    pub fn load_embedder(&self, config: &UserConfig) -> Result<()> {
        if self.embedder.initialized() {
            return Ok(());
        }
        let embedder = config
            .embedding
            .as_ref()
            .filter(|c| c.enabled)
            .map(|c| {
                sapphire_retrieve::build_embedder(&c.to_retrieve_embed_config())
                    .map_err(anyhow::Error::msg)
            })
            .transpose()?;
        let _ = self.embedder.set(embedder);
        Ok(())
    }

    /// Async version of [`load_embedder`](Self::load_embedder).
    pub async fn load_embedder_async(&self, config: &UserConfig) -> Result<()> {
        self.embedder
            .get_or_try_init(|| async {
                config
                    .embedding
                    .as_ref()
                    .filter(|c| c.enabled)
                    .map(|c| {
                        sapphire_retrieve::build_embedder(&c.to_retrieve_embed_config())
                            .map_err(anyhow::Error::msg)
                    })
                    .transpose()
            })
            .await?;
        Ok(())
    }

    // ── sync ──────────────────────────────────────────────────────────────────

    /// Sync the workspace into the retrieve DB (FTS only).
    pub fn sync(&self) -> Result<(usize, usize)> {
        sync_workspace(&self.workspace, &self.retrieve_db)
    }

    /// Sync and, when embedding is configured, embed pending chunks.
    ///
    /// Returns `(upserted, removed, embedded)`.
    pub async fn sync_and_embed(&self, config: &UserConfig) -> Result<(usize, usize, usize)> {
        let (upserted, removed) = sync_workspace(&self.workspace, &self.retrieve_db)?;

        let Some(embed_cfg) = &config.embedding else {
            return Ok((upserted, removed, 0));
        };
        if !embed_cfg.enabled {
            return Ok((upserted, removed, 0));
        }

        self.load_retrieve_backend_async(config).await?;
        self.load_embedder_async(config).await?;

        let Some(embedder) = self.embedder() else {
            return Ok((upserted, removed, 0));
        };

        let embedded = self.retrieve_db.embed_pending(embedder, |_, _| {})?;
        Ok((upserted, removed, embedded))
    }

    /// Embed all pending chunks (sync). Loads backend and embedder if needed.
    pub fn embed_pending(
        &self,
        config: &UserConfig,
        on_progress: impl Fn(usize, usize),
    ) -> Result<usize> {
        let Some(embed_cfg) = &config.embedding else {
            return Ok(0);
        };
        if !embed_cfg.enabled {
            return Ok(0);
        }
        self.load_retrieve_backend(config)?;
        self.load_embedder(config)?;
        let Some(embedder) = self.embedder() else {
            return Ok(0);
        };
        Ok(self.retrieve_db.embed_pending(embedder, on_progress)?)
    }

    // ── info ──────────────────────────────────────────────────────────────────

    pub fn db_info(&self) -> Result<DbInfo> {
        let db_path = self.workspace.retrieve_db_path();
        let document_count = self.retrieve_db.document_count().unwrap_or(0);
        let vec_info = self.retrieve_db.vec_info().unwrap_or(sapphire_retrieve::VecInfo {
            embedding_dim: 0,
            vector_count: 0,
            pending_count: 0,
        });
        Ok(DbInfo {
            db_path,
            schema_version: SCHEMA_VERSION,
            document_count,
            embedding_dim: vec_info.embedding_dim,
            vector_count: vec_info.vector_count,
            pending_count: vec_info.pending_count,
        })
    }
}
