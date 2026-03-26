//! In-memory session state: an open journal with its lazily-initialised
//! embedding infrastructure.
//!
//! [`JournalState`] is the single object that frontends (CLI, MCP, GUI) hold
//! while a workspace is active.  SQLite connections are intentionally **not**
//! stored here: rusqlite recommends one connection per thread, so callers open
//! a connection with [`JournalState::open_conn`] for each operation and drop
//! it when done.
//!
//! The vector store and embedder are expensive to initialise (ONNX model
//! load, LanceDB directory scan) so they are cached in [`tokio::sync::OnceCell`]
//! fields.  The vector store is wrapped in [`std::sync::Mutex`] because some
//! backends (sqlite-vec) hold a rusqlite connection internally, making them
//! `!Sync`.  The embedder trait requires `Send + Sync` so it can be stored
//! directly in a `Box`.
//!
//! Both fields being `Send + Sync` — together with `Journal` which is
//! `Send + Sync` — makes the whole `JournalState: Send + Sync`, which in turn
//! allows `Arc<Mutex<JournalState>>` to be `Sync` and therefore enables async
//! MCP tool handlers that capture `&ArchelonServer`.

use std::sync::Mutex;

use tokio::sync::OnceCell;

use crate::{
    cache,
    embed::Embedder,
    error::Result,
    journal::Journal,
    user_config::UserConfig,
    vector_store::VectorStore,
};

/// An open journal paired with its lazily-initialised embedding infrastructure.
///
/// Create with [`JournalState::open`] or [`JournalState::rebuild`], then:
/// - Call [`open_conn`](Self::open_conn) to get a fresh SQLite connection for
///   cache operations.
/// - Use [`vector_store`](Self::vector_store) / [`embedder`](Self::embedder)
///   after loading them with the `load_*` methods.
pub struct JournalState {
    pub journal: Journal,
    vector_store: OnceCell<Option<Mutex<Box<dyn VectorStore + Send>>>>,
    embedder: OnceCell<Option<Box<dyn Embedder + Send + Sync>>>,
}

impl JournalState {
    /// Open the cache for `journal`, creating it if it does not yet exist.
    ///
    /// Does **not** open a SQLite connection — call [`open_conn`](Self::open_conn)
    /// when one is needed.
    pub fn open(journal: Journal) -> Result<Self> {
        // Ensure the cache DB file exists (creates schema if missing).
        let conn = cache::open_cache(&journal)?;
        drop(conn);
        Ok(Self {
            journal,
            vector_store: OnceCell::new(),
            embedder: OnceCell::new(),
        })
    }

    /// Drop and recreate the cache from scratch, then return the new state.
    pub fn rebuild(journal: Journal) -> Result<Self> {
        let conn = cache::rebuild_cache(&journal)?;
        drop(conn);
        Ok(Self {
            journal,
            vector_store: OnceCell::new(),
            embedder: OnceCell::new(),
        })
    }

    /// Open a fresh SQLite connection to the cache database.
    ///
    /// Callers should open a connection, perform their operation, and drop it.
    /// Do not hold the connection across yield points in async code.
    pub fn open_conn(&self) -> Result<rusqlite::Connection> {
        cache::open_cache(&self.journal)
    }

    /// Incrementally sync the cache with the current on-disk journal state.
    pub fn sync(&self) -> Result<()> {
        let conn = self.open_conn()?;
        cache::sync_cache(&self.journal, &conn)
    }

    /// Sync the cache and, when `cache.embedding.enabled = true`, embed any
    /// pending chunks afterwards.
    ///
    /// Returns the number of newly embedded chunks (0 when embedding is
    /// disabled or nothing was pending).
    pub async fn sync_and_embed(&self, config: &UserConfig) -> Result<usize> {
        let conn = self.open_conn()?;
        cache::sync_cache(&self.journal, &conn)?;

        let Some(embed_cfg) = &config.cache.embedding else {
            return Ok(0);
        };
        if !embed_cfg.enabled {
            return Ok(0);
        }

        self.load_vector_store_async(config).await?;
        self.load_embedder_async(config).await?;

        let Some(store_mutex) = self.vector_store() else {
            return Ok(0);
        };
        let store = store_mutex.lock().unwrap();
        let Some(embedder) = self.embedder() else {
            return Ok(0);
        };

        crate::vector_store::embed_pending_chunks(&conn, &**store, embedder, |_, _| {})
    }

    /// Return cache statistics (path, schema version, entry count, etc.).
    pub fn cache_info(&self) -> Result<cache::CacheInfo> {
        let conn = self.open_conn()?;
        cache::cache_info(&self.journal, &conn)
    }

    // ── vector store ──────────────────────────────────────────────────────────

    /// Initialize the vector store from the user config if not already done (sync).
    ///
    /// Idempotent — if the vector store is already loaded this is a no-op.
    ///
    /// See also [`load_vector_store_async`](Self::load_vector_store_async) for
    /// async callers where contention-free init is preferred.
    pub fn load_vector_store(&self, config: &UserConfig) -> Result<()> {
        if self.vector_store.initialized() {
            return Ok(());
        }
        let store = build_vector_store(&self.journal, config)?.map(Mutex::new);
        let _ = self.vector_store.set(store);
        Ok(())
    }

    /// Initialize the vector store from the user config if not already done (async).
    ///
    /// Preferred over [`load_vector_store`](Self::load_vector_store) in async
    /// contexts; at most one initialization runs even under concurrent callers.
    pub async fn load_vector_store_async(&self, config: &UserConfig) -> Result<()> {
        self.vector_store
            .get_or_try_init(|| async {
                build_vector_store_async(&self.journal, config)
                    .await
                    .map(|opt| opt.map(Mutex::new))
            })
            .await?;
        Ok(())
    }

    /// Borrow the vector store mutex if it has been loaded and is configured.
    ///
    /// Returns `None` if [`load_vector_store`] has not been called yet, or if
    /// `cache.embedding.vector_db = "none"`.
    ///
    /// Lock the returned mutex to access the store:
    /// ```ignore
    /// if let Some(m) = state.vector_store() {
    ///     let store = m.lock().unwrap();
    ///     let results = store.search_similar(&query_vec, 10)?;
    /// }
    /// ```
    pub fn vector_store(&self) -> Option<&Mutex<Box<dyn VectorStore + Send>>> {
        self.vector_store.get()?.as_ref()
    }

    // ── embedder ──────────────────────────────────────────────────────────────

    /// Initialize the embedder from the user config if not already done (sync).
    ///
    /// Idempotent. For `"fastembed"` this loads the ONNX model from disk
    /// (slow on first call, instant on subsequent calls).
    ///
    /// See also [`load_embedder_async`](Self::load_embedder_async).
    pub fn load_embedder(&self, config: &UserConfig) -> Result<()> {
        if self.embedder.initialized() {
            return Ok(());
        }
        let embedder = config
            .cache
            .embedding
            .as_ref()
            .filter(|c| c.enabled)
            .map(|c| crate::embed::build_embedder(c))
            .transpose()?;
        let _ = self.embedder.set(embedder);
        Ok(())
    }

    /// Initialize the embedder from the user config if not already done (async).
    pub async fn load_embedder_async(&self, config: &UserConfig) -> Result<()> {
        self.embedder
            .get_or_try_init(|| async {
                config
                    .cache
                    .embedding
                    .as_ref()
                    .filter(|c| c.enabled)
                    .map(|c| crate::embed::build_embedder(c))
                    .transpose()
            })
            .await?;
        Ok(())
    }

    /// Borrow the embedder if it has been loaded and an embedding provider is
    /// configured.
    ///
    /// Returns `None` if [`load_embedder`](Self::load_embedder) has not been
    /// called yet, or if no `[cache.embedding]` section exists.
    pub fn embedder(&self) -> Option<&dyn Embedder> {
        Some(self.embedder.get()?.as_ref()?.as_ref())
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
    if !embed_cfg.enabled {
        return Ok(None);
    }
    let Some(dim) = embed_cfg.dimension else {
        return Ok(None);
    };

    match embed_cfg.vector_db {
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

/// Async version of [`build_vector_store`] that is safe to call from within a
/// tokio runtime.
///
/// For LanceDB, which creates its own internal runtime, the construction is
/// moved to a blocking thread via [`tokio::task::spawn_blocking`] to avoid
/// "cannot start a runtime within a runtime" panics.
async fn build_vector_store_async(
    journal: &Journal,
    config: &UserConfig,
) -> Result<Option<Box<dyn VectorStore + Send>>> {
    use crate::{user_config::VectorDb, vector_store::SqliteVecStore};

    let Some(embed_cfg) = &config.cache.embedding else {
        return Ok(None);
    };
    if !embed_cfg.enabled {
        return Ok(None);
    }
    let Some(dim) = embed_cfg.dimension else {
        return Ok(None);
    };

    match embed_cfg.vector_db {
        VectorDb::None => Ok(None),
        VectorDb::SqliteVec => {
            let store = SqliteVecStore::open(journal, dim)?;
            Ok(Some(Box::new(store) as _))
        }
        #[cfg(feature = "lancedb-store")]
        VectorDb::LanceDb => {
            use crate::lancedb_store::{self, LanceDbVectorStore};
            let root = journal.cache_dir()?;
            let dir = lancedb_store::versioned_dir(&root);
            // LanceDbVectorStore::new() creates its own tokio runtime internally.
            // Use spawn_blocking so the construction runs on a clean thread
            // without an ambient runtime context.
            let store = tokio::task::spawn_blocking(move || LanceDbVectorStore::new(&dir, dim))
                .await
                .map_err(|e| crate::error::Error::Io(std::io::Error::other(e.to_string())))??;
            Ok(Some(Box::new(store) as _))
        }
        #[cfg(not(feature = "lancedb-store"))]
        VectorDb::LanceDb => Err(crate::error::Error::InvalidConfig(
            "lancedb support is not compiled in (enable the `lancedb-store` feature)".into(),
        )),
    }
}
