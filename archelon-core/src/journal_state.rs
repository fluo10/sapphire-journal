//! In-memory session state: an open journal paired with its SQLite cache connection.
//!
//! [`JournalState`] is the single object that frontends (CLI, MCP, GUI) hold while
//! a workspace is active. Passing it to `ops` functions avoids reopening the
//! journal directory and database on every call.

use tokio::sync::OnceCell;

use crate::{
    cache,
    embed::Embedder,
    error::Result,
    journal::Journal,
    user_config::UserConfig,
    vector_store::VectorStore,
};

/// An open journal paired with its SQLite cache connection.
///
/// Create with [`JournalState::open`] or [`JournalState::rebuild`], then pass
/// references to [`crate::ops`] functions.
///
/// The vector store (if configured) is loaded lazily on the first call to
/// [`JournalState::load_vector_store`] and cached for the lifetime of the state.
pub struct JournalState {
    pub journal: Journal,
    pub conn: rusqlite::Connection,
    vector_store: OnceCell<Option<Box<dyn VectorStore + Send>>>,
    embedder: OnceCell<Option<Box<dyn Embedder + Send>>>,
}

impl JournalState {
    /// Open the cache for `journal`, creating it if it does not yet exist.
    pub fn open(journal: Journal) -> Result<Self> {
        let conn = cache::open_cache(&journal)?;
        Ok(Self { journal, conn, vector_store: OnceCell::new(), embedder: OnceCell::new() })
    }

    /// Drop and recreate the cache from scratch, then return the new state.
    pub fn rebuild(journal: Journal) -> Result<Self> {
        let conn = cache::rebuild_cache(&journal)?;
        Ok(Self { journal, conn, vector_store: OnceCell::new(), embedder: OnceCell::new() })
    }

    /// Incrementally sync the cache with the current on-disk journal state.
    pub fn sync(&self) -> Result<()> {
        cache::sync_cache(&self.journal, &self.conn)
    }

    /// Return cache statistics (path, schema version, entry count, etc.).
    pub fn cache_info(&self) -> Result<cache::CacheInfo> {
        cache::cache_info(&self.journal, &self.conn)
    }

    /// Initialize the vector store from the user config if not already done (sync).
    ///
    /// Idempotent — if the vector store is already loaded this is a no-op.
    /// Returns an error if the configured backend fails to open (e.g. LanceDB
    /// directory is inaccessible).
    ///
    /// See also [`load_vector_store_async`](Self::load_vector_store_async) for
    /// async callers (e.g. Dioxus GUI) where contention-free init is preferred.
    pub fn load_vector_store(&self, config: &UserConfig) -> Result<()> {
        if self.vector_store.initialized() {
            return Ok(());
        }
        let store = build_vector_store(&self.journal, config)?;
        // `set` returns Err if another caller raced and set first; that's fine.
        let _ = self.vector_store.set(store);
        Ok(())
    }

    /// Initialize the vector store from the user config if not already done (async).
    ///
    /// Preferred over [`load_vector_store`](Self::load_vector_store) in async
    /// contexts; at most one initialization runs even under concurrent callers.
    pub async fn load_vector_store_async(&self, config: &UserConfig) -> Result<()> {
        self.vector_store
            .get_or_try_init(|| async { build_vector_store(&self.journal, config) })
            .await?;
        Ok(())
    }

    /// Borrow the vector store if it has been loaded and is configured.
    ///
    /// Returns `None` if neither [`load_vector_store`] nor
    /// [`load_vector_store_async`] has been called yet, or if
    /// `vector_db = "none"` in the user config.
    pub fn vector_store(&self) -> Option<&dyn VectorStore> {
        let boxed: &Box<dyn VectorStore + Send> = self.vector_store.get()?.as_ref()?;
        let r: &dyn VectorStore = boxed.as_ref();
        Some(r)
    }

    /// Initialize the embedder from the user config if not already done (sync).
    ///
    /// For `"fastembed"` this loads the ONNX model from disk (slow on first
    /// call, instant on subsequent calls).  REST providers are lightweight.
    /// Idempotent — if the embedder is already loaded this is a no-op.
    ///
    /// See also [`load_embedder_async`](Self::load_embedder_async) for async callers.
    pub fn load_embedder(&self, config: &UserConfig) -> Result<()> {
        if self.embedder.initialized() {
            return Ok(());
        }
        let embedder = config.cache.embedding.as_ref()
            .map(|c| crate::embed::build_embedder(c))
            .transpose()?;
        let _ = self.embedder.set(embedder);
        Ok(())
    }

    /// Initialize the embedder from the user config if not already done (async).
    ///
    /// Preferred over [`load_embedder`](Self::load_embedder) in async contexts;
    /// at most one initialization runs even under concurrent callers.
    pub async fn load_embedder_async(&self, config: &UserConfig) -> Result<()> {
        self.embedder
            .get_or_try_init(|| async {
                config.cache.embedding.as_ref()
                    .map(|c| crate::embed::build_embedder(c))
                    .transpose()
            })
            .await?;
        Ok(())
    }

    /// Borrow the embedder if it has been loaded and an embedding provider is configured.
    ///
    /// Returns `None` if [`load_embedder`](Self::load_embedder) has not been
    /// called yet, or if no `[cache.embedding]` section exists in the user config.
    pub fn embedder(&self) -> Option<&dyn Embedder> {
        let boxed: &Box<dyn Embedder + Send> = self.embedder.get()?.as_ref()?;
        let r: &dyn Embedder = boxed.as_ref();
        Some(r)
    }
}

fn build_vector_store(
    journal: &Journal,
    config: &UserConfig,
) -> Result<Option<Box<dyn VectorStore + Send>>> {
    use crate::{user_config::VectorDb, vector_store::SqliteVecStore};

    let Some(embed_cfg) = &config.cache.embedding else {
        return Ok(None);
    };
    let Some(dim) = embed_cfg.dimension else {
        return Ok(None);
    };

    match config.cache.vector_db {
        VectorDb::None => Ok(None),
        VectorDb::SqliteVec => {
            let store = SqliteVecStore::open(journal, dim)?;
            Ok(Some(Box::new(store)))
        }
        #[cfg(feature = "lancedb-store")]
        VectorDb::LanceDb => {
            use crate::lancedb_store::{self, LanceDbVectorStore};
            let root = journal.cache_dir()?;
            let store = LanceDbVectorStore::new(&lancedb_store::versioned_dir(&root), dim)?;
            Ok(Some(Box::new(store)))
        }
        #[cfg(not(feature = "lancedb-store"))]
        VectorDb::LanceDb => Err(crate::error::Error::InvalidConfig(
            "lancedb support is not compiled in (enable the `lancedb-store` feature)".into(),
        )),
    }
}
