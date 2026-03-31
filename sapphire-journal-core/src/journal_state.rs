//! In-memory session state: an open journal with its lazily-initialised
//! embedding infrastructure.
//!
//! [`JournalState`] is the single object that frontends (CLI, MCP, GUI) hold
//! while a workspace is active.  SQLite connections are intentionally **not**
//! stored here: rusqlite recommends one connection per thread, so callers open
//! a connection with [`JournalState::open_conn`] for each operation and drop
//! it when done.
//!
//! [`RetrieveDb`] is always present (for FTS).  The vector backend inside it
//! is lazily initialised via [`load_retrieve_backend`](JournalState::load_retrieve_backend)
//! when embedding is configured.
//!
//! The embedder is expensive to initialise (ONNX model load), so it is cached
//! in a [`tokio::sync::OnceCell`] field.

use tokio::sync::OnceCell;
use sapphire_retrieve::{Embedder, RetrieveDb};

use crate::{
    cache,
    error::Result,
    journal::Journal,
    user_config::{UserConfig, VectorDb},
};

/// An open journal paired with its lazily-initialised search infrastructure.
pub struct JournalState {
    pub journal: Journal,
    /// Always-present retrieve database (FTS + optional vector backend).
    retrieve_db: RetrieveDb,
    embedder: OnceCell<Option<Box<dyn Embedder + Send + Sync>>>,
}

impl JournalState {
    /// Open the cache for `journal`, creating it if it does not yet exist.
    pub fn open(journal: Journal) -> Result<Self> {
        let conn = cache::open_cache(&journal)?;
        drop(conn);
        let retrieve_db = RetrieveDb::open(&journal.retrieve_db_path()?)?;
        Ok(Self {
            journal,
            retrieve_db,
            embedder: OnceCell::new(),
        })
    }

    /// Drop and recreate both the cache and the retrieve database from scratch.
    pub fn rebuild(journal: Journal) -> Result<Self> {
        let conn = cache::rebuild_cache(&journal)?;
        drop(conn);
        let retrieve_db = RetrieveDb::rebuild(&journal.retrieve_db_path()?)?;
        #[cfg(feature = "lancedb-store")]
        {
            use sapphire_retrieve::lancedb_store;
            let _ = std::fs::remove_dir_all(lancedb_store::data_dir(&journal.cache_dir()?));
        }
        Ok(Self {
            journal,
            retrieve_db,
            embedder: OnceCell::new(),
        })
    }

    /// Open a fresh SQLite connection to the cache database.
    pub fn open_conn(&self) -> Result<rusqlite::Connection> {
        cache::open_cache(&self.journal)
    }

    /// Borrow the retrieve database (FTS + optional vector search).
    pub fn retrieve_db(&self) -> &RetrieveDb {
        &self.retrieve_db
    }

    /// Incrementally sync the cache with the current on-disk journal state.
    ///
    /// Also syncs documents into the retrieve database (FTS index).
    pub fn sync(&self) -> Result<()> {
        let conn = self.open_conn()?;
        cache::sync_cache(&self.journal, &conn, &self.retrieve_db)
    }

    /// Sync the cache and, when embedding is enabled, embed any pending chunks.
    ///
    /// Returns the number of newly embedded chunks (0 when embedding is
    /// disabled or nothing was pending).
    pub async fn sync_and_embed(&self, config: &UserConfig) -> Result<usize> {
        let conn = self.open_conn()?;
        cache::sync_cache(&self.journal, &conn, &self.retrieve_db)?;
        drop(conn);

        let Some(embed_cfg) = &config.cache.embedding else {
            return Ok(0);
        };
        if !embed_cfg.enabled {
            return Ok(0);
        }

        self.load_retrieve_backend_async(config).await?;
        self.load_embedder_async(config).await?;

        let Some(embedder) = self.embedder() else {
            return Ok(0);
        };

        Ok(self.retrieve_db.embed_pending(embedder, |_, _| {})?)
    }

    /// Return cache statistics (path, schema version, entry count, etc.).
    pub fn cache_info(&self) -> Result<cache::CacheInfo> {
        let conn = self.open_conn()?;
        cache::cache_info(&self.journal, &conn, &self.retrieve_db)
    }

    // ── vector backend ────────────────────────────────────────────────────────

    /// Initialise the vector backend in the retrieve database (sync).
    ///
    /// Idempotent — if the backend is already loaded this is a no-op.
    pub fn load_retrieve_backend(&self, config: &UserConfig) -> Result<()> {
        let Some(embed_cfg) = &config.cache.embedding else {
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
        let Some(embed_cfg) = &config.cache.embedding else {
            return Ok(());
        };
        if !embed_cfg.enabled {
            return Ok(());
        }
        let Some(dim) = embed_cfg.dimension else {
            return Ok(());
        };
        let vector_db = embed_cfg.vector_db;

        // LanceDB uses block_in_place internally when called from an async context,
        // so it is safe to call directly here.
        #[cfg(feature = "lancedb-store")]
        if vector_db == VectorDb::LanceDb {
            use sapphire_retrieve::lancedb_store;
            let lancedb_dir = lancedb_store::data_dir(&self.journal.cache_dir()?);
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
                let lancedb_dir = lancedb_store::data_dir(&self.journal.cache_dir()?);
                self.retrieve_db.init_lancedb(&lancedb_dir, dim)?;
            }
            #[cfg(not(feature = "lancedb-store"))]
            VectorDb::LanceDb => {
                return Err(crate::error::Error::InvalidConfig(
                    "lancedb support is not compiled in (enable the `lancedb-store` feature)".into(),
                ));
            }
        }
        Ok(())
    }

    // ── embedder ──────────────────────────────────────────────────────────────

    /// Initialise the embedder (sync).  Idempotent.
    pub fn load_embedder(&self, config: &UserConfig) -> Result<()> {
        if self.embedder.initialized() {
            return Ok(());
        }
        let embedder = config
            .cache
            .embedding
            .as_ref()
            .filter(|c| c.enabled)
            .map(|c| sapphire_retrieve::build_embedder(&c.to_embed_config()).map_err(crate::error::Error::from))
            .transpose()?;
        let _ = self.embedder.set(embedder);
        Ok(())
    }

    /// Async version of [`load_embedder`](Self::load_embedder).
    pub async fn load_embedder_async(&self, config: &UserConfig) -> Result<()> {
        self.embedder
            .get_or_try_init(|| async {
                config
                    .cache
                    .embedding
                    .as_ref()
                    .filter(|c| c.enabled)
                    .map(|c| {
                        sapphire_retrieve::build_embedder(&c.to_embed_config())
                            .map_err(crate::error::Error::from)
                    })
                    .transpose()
            })
            .await?;
        Ok(())
    }

    /// Borrow the embedder if it has been loaded and an embedding provider is configured.
    pub fn embedder(&self) -> Option<&dyn Embedder> {
        Some(self.embedder.get()?.as_ref()?.as_ref())
    }

    // ── embedding ─────────────────────────────────────────────────────────────

    /// Embed all pending chunks in the retrieve database (sync).
    ///
    /// Loads both the vector backend and the embedder if not already done.
    /// Returns the number of newly embedded chunks.
    pub fn embed_pending(
        &self,
        config: &UserConfig,
        on_progress: impl Fn(usize, usize),
    ) -> Result<usize> {
        let Some(embed_cfg) = &config.cache.embedding else { return Ok(0) };
        if !embed_cfg.enabled { return Ok(0) }

        self.load_retrieve_backend(config)?;
        self.load_embedder(config)?;

        let Some(embedder) = self.embedder() else { return Ok(0) };
        Ok(self.retrieve_db.embed_pending(embedder, on_progress)?)
    }
}
