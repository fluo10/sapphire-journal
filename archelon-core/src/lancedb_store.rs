//! LanceDB vector store backend.
//!
//! Stores entry embeddings in a LanceDB database at
//! `$XDG_CACHE_HOME/archelon/{journal_id}/lancedb/`.
//!
//! # Design
//!
//! All public functions that touch LanceDB are `async` and must be driven by a
//! Tokio runtime.  The CLI bridges the sync/async boundary with
//! `tokio::runtime::Runtime::new()?.block_on(...)`.
//!
//! To avoid holding a `rusqlite::Connection` (which is `!Send`) across `.await`
//! points, SQLite reads are completed synchronously before any async work begins.
//! [`pending_entries`] is provided for this purpose and must be called from
//! synchronous code before entering the async context.
//!
//! # Table schema
//!
//! The `entries` table contains:
//!
//! | column      | type                        | notes              |
//! |-------------|-----------------------------|--------------------|
//! | `id`        | `Int64`                     | CarettaId as i64   |
//! | `title`     | `Utf8`                      |                    |
//! | `path`      | `Utf8`                      | absolute file path |
//! | `embedding` | `FixedSizeList<Float32, N>` | N = embedding_dim  |

use std::{
    collections::HashSet,
    path::Path,
    sync::Arc,
};

use arrow_array::{
    FixedSizeListArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt as _;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::{
    cache::SearchResult,
    error::{Error, Result},
    user_config::EmbeddingConfig,
};

const TABLE_NAME: &str = "entries";

// ── store handle ──────────────────────────────────────────────────────────────

/// An open handle to the LanceDB vector store for one journal.
pub struct LanceStore {
    table: lancedb::Table,
    dim: i32,
}

impl LanceStore {
    /// Open (or create) the LanceDB store at `data_dir` with `embedding_dim`
    /// dimensions.
    ///
    /// If the table already exists it is opened as-is; no dimension check is
    /// performed here (mismatches surface as errors on the first insert or search).
    pub async fn open(data_dir: &Path, embedding_dim: u32) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let db = lancedb::connect(data_dir.to_str().unwrap_or_default())
            .execute()
            .await
            .map_err(|e| Error::Embed(e.to_string()))?;

        let dim = embedding_dim as i32;
        let names = db
            .table_names()
            .execute()
            .await
            .map_err(|e| Error::Embed(e.to_string()))?;

        let table = if names.contains(&TABLE_NAME.to_string()) {
            db.open_table(TABLE_NAME)
                .execute()
                .await
                .map_err(|e| Error::Embed(e.to_string()))?
        } else {
            let schema = make_schema(dim);
            let empty = RecordBatch::new_empty(schema.clone());
            db.create_table(
                TABLE_NAME,
                RecordBatchIterator::new(vec![Ok(empty)], schema),
            )
            .execute()
            .await
            .map_err(|e| Error::Embed(e.to_string()))?
        };

        Ok(LanceStore { table, dim })
    }

    /// Return the set of entry IDs that already have a vector stored.
    ///
    /// Used by [`pending_entries`] to determine which entries still need
    /// embeddings.
    pub async fn embedded_ids(&self) -> Result<HashSet<i64>> {
        let batches: Vec<RecordBatch> = self
            .table
            .query()
            .select(lancedb::query::Select::Columns(vec!["id".to_string()]))
            .execute()
            .await
            .map_err(|e| Error::Embed(e.to_string()))?
            .try_collect()
            .await
            .map_err(|e| Error::Embed(e.to_string()))?;

        let mut ids = HashSet::new();
        for batch in batches {
            if let Some(col) = batch.column_by_name("id") {
                let arr = col
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| Error::Embed("unexpected type for id column".into()))?;
                for i in 0..arr.len() {
                    ids.insert(arr.value(i));
                }
            }
        }
        Ok(ids)
    }

    /// Insert a batch of entries into the store.
    ///
    /// Duplicate IDs are not checked; call [`pending_entries`] first to filter
    /// them out.
    pub async fn insert(
        &self,
        ids: &[i64],
        titles: &[String],
        paths: &[String],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let schema = make_schema(self.dim);
        let id_arr: Arc<dyn arrow_array::Array> = Arc::new(Int64Array::from(ids.to_vec()));
        let title_arr: Arc<dyn arrow_array::Array> =
            Arc::new(StringArray::from(titles.to_vec()));
        let path_arr: Arc<dyn arrow_array::Array> = Arc::new(StringArray::from(paths.to_vec()));
        let emb_arr: Arc<dyn arrow_array::Array> =
            Arc::new(make_embedding_array(embeddings, self.dim)?);

        let batch = RecordBatch::try_new(schema.clone(), vec![id_arr, title_arr, path_arr, emb_arr])
            .map_err(|e| Error::Embed(e.to_string()))?;

        self.table
            .add(RecordBatchIterator::new(vec![Ok(batch)], schema))
            .execute()
            .await
            .map_err(|e| Error::Embed(e.to_string()))?;
        Ok(())
    }

    /// KNN search: return the `limit` most similar entries to `query_vec`.
    pub async fn search_similar(
        &self,
        query_vec: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let batches: Vec<RecordBatch> = self
            .table
            .vector_search(query_vec)
            .map_err(|e| Error::Embed(e.to_string()))?
            .column("embedding")
            .limit(limit)
            .execute()
            .await
            .map_err(|e| Error::Embed(e.to_string()))?
            .try_collect()
            .await
            .map_err(|e| Error::Embed(e.to_string()))?;

        let mut results = Vec::new();
        for batch in &batches {
            let id_col = batch
                .column_by_name("id")
                .ok_or_else(|| Error::Embed("missing `id` column in search result".into()))?
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| Error::Embed("unexpected type for `id` column".into()))?;
            let title_col = batch
                .column_by_name("title")
                .ok_or_else(|| Error::Embed("missing `title` column in search result".into()))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| Error::Embed("unexpected type for `title` column".into()))?;
            let path_col = batch
                .column_by_name("path")
                .ok_or_else(|| Error::Embed("missing `path` column in search result".into()))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| Error::Embed("unexpected type for `path` column".into()))?;
            let dist_col = batch
                .column_by_name("_distance")
                .ok_or_else(|| {
                    Error::Embed("missing `_distance` column in search result".into())
                })?
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| Error::Embed("unexpected type for `_distance` column".into()))?;

            for i in 0..batch.num_rows() {
                results.push(SearchResult {
                    id: id_col.value(i),
                    title: title_col.value(i).to_owned(),
                    path: path_col.value(i).to_owned(),
                    score: dist_col.value(i) as f64,
                });
            }
        }
        Ok(results)
    }
}

// ── sync helpers (called before entering async) ───────────────────────────────

/// Return entries from the SQLite cache that are not yet in `embedded_ids`.
///
/// This is a **synchronous** function designed to be called before entering
/// any async context, avoiding the need to hold a `rusqlite::Connection`
/// (which is `!Send`) across an `.await` point.
pub fn pending_entries(
    conn: &rusqlite::Connection,
    embedded_ids: &HashSet<i64>,
) -> Result<Vec<(i64, String, String, String)>> {
    let mut stmt = conn
        .prepare("SELECT id, title, path, body FROM entries")
        .map_err(Error::Cache)?;
    let rows: Vec<(i64, String, String, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(Error::Cache)?
        .filter_map(|r| r.ok())
        .filter(|(id, _, _, _)| !embedded_ids.contains(id))
        .collect();
    Ok(rows)
}

// ── async operations ──────────────────────────────────────────────────────────

/// Generate and store embeddings for `entries` (pre-filtered by the caller).
///
/// `entries` should be the output of [`pending_entries`].
/// Calls `on_progress(done, total)` after each batch.
/// Returns the number of entries embedded.
pub async fn embed_entries(
    store: &LanceStore,
    entries: Vec<(i64, String, String, String)>,
    config: &EmbeddingConfig,
    on_progress: impl Fn(usize, usize),
) -> Result<usize> {
    let total = entries.len();
    let mut done = 0;

    for chunk in entries.chunks(100) {
        let texts: Vec<String> = chunk
            .iter()
            .map(|(_, title, _, body)| format!("{title}\n\n{body}"))
            .collect();
        let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let embeddings = crate::embed::embed_texts(config, &text_refs)?;

        let ids: Vec<i64> = chunk.iter().map(|(id, _, _, _)| *id).collect();
        let titles: Vec<String> = chunk.iter().map(|(_, t, _, _)| t.clone()).collect();
        let paths: Vec<String> = chunk.iter().map(|(_, _, p, _)| p.clone()).collect();

        store.insert(&ids, &titles, &paths, &embeddings).await?;
        done += chunk.len();
        on_progress(done, total);
    }

    Ok(total)
}

// ── Arrow helpers ─────────────────────────────────────────────────────────────

fn make_schema(dim: i32) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dim,
            ),
            false,
        ),
    ]))
}

fn make_embedding_array(embeddings: &[Vec<f32>], dim: i32) -> Result<FixedSizeListArray> {
    let flat: Vec<f32> = embeddings.iter().flat_map(|v| v.iter().copied()).collect();
    let values = Arc::new(Float32Array::from(flat));
    FixedSizeListArray::try_new(
        Arc::new(Field::new("item", DataType::Float32, true)),
        dim,
        values,
        None,
    )
    .map_err(|e| Error::Embed(e.to_string()))
}
