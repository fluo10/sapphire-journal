//! Vector store abstraction and sqlite-vec backend.
//!
//! # Architecture
//!
//! [`VectorStore`] is a **synchronous** trait that abstracts over the two
//! supported vector backends:
//!
//! - [`SqliteVecStore`] — stores chunk vectors inside the existing SQLite
//!   cache using the sqlite-vec extension.
//! - `LanceDbVectorStore` (in `lancedb_store`) — stores chunk vectors in a
//!   separate LanceDB directory; async LanceDB calls are wrapped in an
//!   internal Tokio runtime so the trait remains sync.
//!
//! # Chunk identity
//!
//! Each chunk is identified by the pair `(entry_id, chunk_index)`.  This key
//! is stable across SQLite cache rebuilds because `entry_id` is the CarettaId
//! (derived from the file itself) and `chunk_index` is reproducibly derived
//! from the paragraph order of the entry body.
//!
//! # Shared helpers
//!
//! [`embed_pending_chunks`] is the one function called by both CLI commands
//! (`cache embed`, `entry search --semantic`).  It:
//!
//! 1. Queries `embedded_chunk_keys()` from the store.
//! 2. Calls [`pending_chunks`] on the SQLite connection to collect chunks not
//!    yet embedded.
//! 3. Calls the embedding API in batches of 100.
//! 4. Stores the results via `insert_embeddings()`.

use std::collections::HashSet;

use rusqlite::params;

use crate::{
    cache,
    embed::Embedder,
    error::Result,
    journal::Journal,
};

// ── public types ──────────────────────────────────────────────────────────────

/// A single paragraph-level chunk derived from an entry, ready to be embedded.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Entry ID (CarettaId as i64) — stable, used as part of the chunk key.
    pub entry_id: i64,
    /// Zero-based position of this paragraph in the entry body.
    pub chunk_index: usize,
    /// Embeddable text: title prepended to the paragraph body.
    pub text: String,
    /// Denormalised entry title (for display in search results).
    pub entry_title: String,
    /// Denormalised absolute file path (for display in search results).
    pub entry_path: String,
}

/// A result returned by [`VectorStore::search_similar`].
#[derive(Debug, Clone)]
pub struct ChunkSearchResult {
    pub entry_id: i64,
    pub entry_title: String,
    pub entry_path: String,
    /// Position of the matching chunk within the entry (0-based).
    pub chunk_index: usize,
    /// The text of the matching chunk.
    pub chunk_text: String,
    /// L2 distance (lower = more similar).
    pub score: f64,
}

/// Statistics about the vector index shown by `cache info`.
pub struct VecInfo {
    /// Embedding dimension (number of f32 values per vector).
    pub embedding_dim: u32,
    /// Number of chunks that have an embedding stored.
    pub vector_count: u64,
    /// Number of chunks that do not yet have an embedding.
    pub pending_count: u64,
}

// ── trait ─────────────────────────────────────────────────────────────────────

/// Abstraction over a vector storage backend.
///
/// All methods are **synchronous**.  Backends that are inherently async
/// (e.g. LanceDB) wrap their async operations in an internal Tokio runtime.
pub trait VectorStore {
    /// Return the `(entry_id, chunk_index)` pairs that already have embeddings
    /// stored, so callers can compute the pending set.
    fn embedded_chunk_keys(&self) -> Result<HashSet<(i64, usize)>>;

    /// Store embeddings for a batch of chunks.
    ///
    /// `chunks` and `embeddings` are parallel slices of equal length.
    fn insert_embeddings(&self, chunks: &[Chunk], embeddings: &[Vec<f32>]) -> Result<()>;

    /// Find the `limit` most similar chunks to `query_vec`, ordered by
    /// ascending distance.
    fn search_similar(&self, query_vec: &[f32], limit: usize) -> Result<Vec<ChunkSearchResult>>;
}

// ── SqliteVecStore ────────────────────────────────────────────────────────────

/// Vector store backed by the sqlite-vec extension, living inside the
/// existing SQLite cache database.
pub struct SqliteVecStore {
    conn: rusqlite::Connection,
}

impl SqliteVecStore {
    /// Open (or create) the sqlite-vec store for `journal` with
    /// `embedding_dim` dimensions.
    ///
    /// The returned store owns the underlying connection.  Call
    /// [`Self::conn`] to pass it to [`cache::sync_cache`].
    pub fn open(journal: &Journal, embedding_dim: u32) -> Result<Self> {
        let conn = cache::open_cache_vec(journal, embedding_dim)?;
        Ok(Self { conn })
    }

    /// Borrow the underlying SQLite connection.
    ///
    /// Needed to call [`cache::sync_cache`] with the same connection that
    /// already has the sqlite-vec extension loaded.
    pub fn conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    /// Read vector index statistics for display in `cache info`.
    pub fn vec_info(&self) -> Result<VecInfo> {
        let embedding_dim: u32 = self
            .conn
            .query_row(
                "SELECT value FROM vec_meta WHERE key = 'embedding_dim'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let vector_count: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunk_vectors", [], |row| row.get(0))
            .unwrap_or(0);

        let chunk_count: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap_or(0);

        Ok(VecInfo {
            embedding_dim,
            vector_count,
            pending_count: chunk_count.saturating_sub(vector_count),
        })
    }
}

impl VectorStore for SqliteVecStore {
    fn embedded_chunk_keys(&self) -> Result<HashSet<(i64, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.entry_id, c.chunk_index
             FROM chunks c
             JOIN chunk_vectors cv ON cv.chunk_id = c.id",
        )?;
        let keys = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as usize))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(keys)
    }

    fn insert_embeddings(&self, chunks: &[Chunk], embeddings: &[Vec<f32>]) -> Result<()> {
        for (chunk, emb) in chunks.iter().zip(embeddings) {
            let chunk_id: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM chunks WHERE entry_id = ?1 AND chunk_index = ?2",
                    params![chunk.entry_id, chunk.chunk_index as i64],
                    |row| row.get(0),
                )
                .ok();

            if let Some(id) = chunk_id {
                let blob = vec_serialize(emb);
                self.conn.execute(
                    "INSERT OR REPLACE INTO chunk_vectors (chunk_id, embedding) \
                     VALUES (?1, ?2)",
                    params![id, blob],
                )?;
            }
        }
        Ok(())
    }

    fn search_similar(&self, query_vec: &[f32], limit: usize) -> Result<Vec<ChunkSearchResult>> {
        let blob = vec_serialize(query_vec);
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.title, e.path, c.chunk_index, c.text, cv.distance
             FROM chunk_vectors cv
             JOIN chunks c ON c.id = cv.chunk_id
             JOIN entries e ON e.id = c.entry_id
             WHERE cv.embedding MATCH ?1 AND k = ?2
             ORDER BY cv.distance",
        )?;
        let results = stmt
            .query_map(params![blob, limit as i64], |row| {
                Ok(ChunkSearchResult {
                    entry_id: row.get::<_, i64>(0)?,
                    entry_title: row.get::<_, String>(1)?,
                    entry_path: row.get::<_, String>(2)?,
                    chunk_index: row.get::<_, i64>(3)? as usize,
                    chunk_text: row.get::<_, String>(4)?,
                    score: row.get::<_, f64>(5).unwrap_or(0.0),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(results)
    }
}

// ── shared helpers ────────────────────────────────────────────────────────────

/// Collect all chunks from the SQLite cache that are not yet embedded.
///
/// Must be called **synchronously** (before any async context) because it
/// borrows a `rusqlite::Connection` which is `!Send`.
pub fn pending_chunks(
    conn: &rusqlite::Connection,
    embedded_keys: &HashSet<(i64, usize)>,
) -> Result<Vec<Chunk>> {
    let mut stmt = conn.prepare(
        "SELECT c.entry_id, c.chunk_index, c.text, e.title, e.path
         FROM chunks c
         JOIN entries e ON e.id = c.entry_id",
    )?;
    let chunks = stmt
        .query_map([], |row| {
            Ok(Chunk {
                entry_id: row.get::<_, i64>(0)?,
                chunk_index: row.get::<_, i64>(1)? as usize,
                text: row.get::<_, String>(2)?,
                entry_title: row.get::<_, String>(3)?,
                entry_path: row.get::<_, String>(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .filter(|c| !embedded_keys.contains(&(c.entry_id, c.chunk_index)))
        .collect();
    Ok(chunks)
}

/// Generate and store embeddings for all pending chunks.
///
/// This is the single entry point used by both `cache embed` and
/// `entry search --semantic`.
///
/// 1. Queries `store.embedded_chunk_keys()` to find what is already stored.
/// 2. Calls [`pending_chunks`] on `conn` to collect unembedded chunks.
/// 3. Calls the embedder in batches of 100.
/// 4. Calls `store.insert_embeddings()` for each batch.
///
/// Calls `on_progress(done, total)` after each batch.
/// Returns the total number of newly embedded chunks.
pub fn embed_pending_chunks(
    conn: &rusqlite::Connection,
    store: &dyn VectorStore,
    embedder: &dyn Embedder,
    on_progress: impl Fn(usize, usize),
) -> Result<usize> {
    let embedded_keys = store.embedded_chunk_keys()?;
    let pending = pending_chunks(conn, &embedded_keys)?;
    let total = pending.len();
    let mut done = 0;

    for batch in pending.chunks(100) {
        let texts: Vec<&str> = batch.iter().map(|c| c.text.as_str()).collect();
        let embeddings = embedder.embed_texts(&texts)?;
        store.insert_embeddings(batch, &embeddings)?;
        done += batch.len();
        on_progress(done, total);
    }

    Ok(total)
}

// ── internal ──────────────────────────────────────────────────────────────────

/// Serialize a float slice to the little-endian bytes expected by sqlite-vec.
pub(crate) fn vec_serialize(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}
