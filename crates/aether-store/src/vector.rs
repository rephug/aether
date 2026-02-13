use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_config::{EmbeddingVectorBackend, load_workspace_config};
use arrow_array::types::Float32Type;
use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Float64Array, Int64Array, RecordBatch,
    RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::{Connection as LanceConnection, DistanceType, Error as LanceError, connect};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{SqliteStore, Store, StoreError, SymbolEmbeddingMetaRecord, SymbolEmbeddingRecord};

const VECTOR_TABLE_PREFIX: &str = "sir_embeddings_";
const MIGRATION_MARKER_FILE: &str = ".sqlite_migrated_v1";

pub type VectorRecord = SymbolEmbeddingRecord;
pub type VectorEmbeddingMetaRecord = SymbolEmbeddingMetaRecord;

#[derive(Debug, Clone, PartialEq)]
pub struct VectorSearchResult {
    pub symbol_id: String,
    pub semantic_score: f32,
}

#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert_embedding(&self, record: VectorRecord) -> Result<(), StoreError>;
    async fn get_embedding_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<VectorEmbeddingMetaRecord>, StoreError>;
    async fn delete_embedding(&self, symbol_id: &str) -> Result<(), StoreError>;
    async fn search_nearest(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<VectorSearchResult>, StoreError>;
}

pub async fn open_vector_store(
    workspace_root: impl AsRef<Path>,
) -> Result<Arc<dyn VectorStore>, StoreError> {
    let workspace_root = workspace_root.as_ref();
    let config = load_workspace_config(workspace_root)?;

    match config.embeddings.vector_backend {
        EmbeddingVectorBackend::Sqlite => Ok(Arc::new(SqliteVectorStore::new(workspace_root))),
        EmbeddingVectorBackend::Lancedb => {
            Ok(Arc::new(LanceVectorStore::open(workspace_root).await?))
        }
    }
}

pub struct SqliteVectorStore {
    workspace_root: PathBuf,
}

impl SqliteVectorStore {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    fn store(&self) -> Result<SqliteStore, StoreError> {
        SqliteStore::open(&self.workspace_root)
    }
}

#[async_trait]
impl VectorStore for SqliteVectorStore {
    async fn upsert_embedding(&self, record: VectorRecord) -> Result<(), StoreError> {
        self.store()?.upsert_symbol_embedding(record)
    }

    async fn get_embedding_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<VectorEmbeddingMetaRecord>, StoreError> {
        self.store()?.get_symbol_embedding_meta(symbol_id)
    }

    async fn delete_embedding(&self, symbol_id: &str) -> Result<(), StoreError> {
        self.store()?.delete_symbol_embedding(symbol_id)
    }

    async fn search_nearest(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<VectorSearchResult>, StoreError> {
        let matches =
            self.store()?
                .search_symbols_semantic(query_embedding, provider, model, limit)?;

        Ok(matches
            .into_iter()
            .map(|row| VectorSearchResult {
                symbol_id: row.symbol_id,
                semantic_score: row.semantic_score,
            })
            .collect())
    }
}

pub struct LanceVectorStore {
    workspace_root: PathBuf,
    vectors_dir: PathBuf,
}

impl LanceVectorStore {
    pub async fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref().to_path_buf();
        let aether_dir = workspace_root.join(".aether");
        let vectors_dir = aether_dir.join("vectors");

        fs::create_dir_all(&vectors_dir)?;
        let _ = SqliteStore::open(&workspace_root)?;

        let store = Self {
            workspace_root,
            vectors_dir,
        };
        store.migrate_from_sqlite_if_needed().await?;
        Ok(store)
    }

    fn marker_path(&self) -> PathBuf {
        self.vectors_dir.join(MIGRATION_MARKER_FILE)
    }

    fn sqlite_path(&self) -> PathBuf {
        self.workspace_root.join(".aether").join("meta.sqlite")
    }

    async fn connect(&self) -> Result<LanceConnection, StoreError> {
        connect(self.vectors_dir.to_string_lossy().as_ref())
            .execute()
            .await
            .map_err(map_lancedb_err)
    }

    async fn migrate_from_sqlite_if_needed(&self) -> Result<(), StoreError> {
        if self.marker_path().exists() {
            return Ok(());
        }

        let records = load_sqlite_embedding_rows(&self.sqlite_path())?;
        if records.is_empty() {
            fs::write(self.marker_path(), b"empty")?;
            return Ok(());
        }

        tracing::info!(
            count = records.len(),
            "migrating SQLite embeddings into LanceDB"
        );
        let connection = self.connect().await?;
        for record in records {
            self.upsert_embedding_with_connection(&connection, &record)
                .await?;
        }

        fs::write(self.marker_path(), b"done")?;
        tracing::info!("completed LanceDB vector migration");
        Ok(())
    }

    async fn upsert_embedding_with_connection(
        &self,
        connection: &LanceConnection,
        record: &VectorRecord,
    ) -> Result<(), StoreError> {
        let embedding_dim = record.embedding.len() as i32;
        if embedding_dim <= 0 {
            return Ok(());
        }
        let table_name = table_name_for(
            record.provider.as_str(),
            record.model.as_str(),
            embedding_dim,
        );

        let table = match connection.open_table(&table_name).execute().await {
            Ok(table) => table,
            Err(LanceError::TableNotFound { .. }) => {
                let (schema, batch) = single_record_batch(record)?;
                let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
                connection
                    .create_table(&table_name, Box::new(reader))
                    .execute()
                    .await
                    .map_err(map_lancedb_err)?;
                return Ok(());
            }
            Err(err) => return Err(map_lancedb_err(err)),
        };

        let (schema, batch) = single_record_batch(record)?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["symbol_id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge
            .execute(Box::new(reader))
            .await
            .map_err(map_lancedb_err)?;
        Ok(())
    }
}

#[async_trait]
impl VectorStore for LanceVectorStore {
    async fn upsert_embedding(&self, record: VectorRecord) -> Result<(), StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        let connection = self.connect().await?;
        self.upsert_embedding_with_connection(&connection, &record)
            .await
    }

    async fn get_embedding_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<VectorEmbeddingMetaRecord>, StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        let connection = self.connect().await?;
        let predicate = format!("symbol_id = '{}'", escape_sql_string(symbol_id));

        let mut latest = None::<VectorEmbeddingMetaRecord>;
        for name in connection
            .table_names()
            .execute()
            .await
            .map_err(map_lancedb_err)?
            .into_iter()
            .filter(|name| name.starts_with(VECTOR_TABLE_PREFIX))
        {
            let Ok(table) = connection.open_table(&name).execute().await else {
                continue;
            };
            let schema = table.schema().await.map_err(map_lancedb_err)?;
            let Some(embedding_dim) = embedding_dim_from_schema(schema.as_ref()) else {
                continue;
            };

            let batches = table
                .query()
                .select(Select::columns(&[
                    "symbol_id",
                    "sir_hash",
                    "provider",
                    "model",
                    "updated_at",
                ]))
                .only_if(predicate.as_str())
                .limit(1)
                .execute()
                .await
                .map_err(map_lancedb_err)?
                .try_collect::<Vec<_>>()
                .await
                .map_err(map_lancedb_err)?;

            for batch in batches {
                if batch.num_rows() == 0 {
                    continue;
                }
                let record = VectorEmbeddingMetaRecord {
                    symbol_id: string_at(&batch, "symbol_id", 0)?,
                    sir_hash: string_at(&batch, "sir_hash", 0)?,
                    provider: string_at(&batch, "provider", 0)?,
                    model: string_at(&batch, "model", 0)?,
                    embedding_dim: i64::from(embedding_dim),
                    updated_at: int64_at(&batch, "updated_at", 0)?,
                };

                match latest.as_ref() {
                    Some(existing) if existing.updated_at >= record.updated_at => {}
                    _ => latest = Some(record),
                }
            }
        }

        Ok(latest)
    }

    async fn delete_embedding(&self, symbol_id: &str) -> Result<(), StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        let connection = self.connect().await?;
        let predicate = format!("symbol_id = '{}'", escape_sql_string(symbol_id));

        for name in connection
            .table_names()
            .execute()
            .await
            .map_err(map_lancedb_err)?
            .into_iter()
            .filter(|name| name.starts_with(VECTOR_TABLE_PREFIX))
        {
            let Ok(table) = connection.open_table(&name).execute().await else {
                continue;
            };
            table
                .delete(predicate.as_str())
                .await
                .map_err(map_lancedb_err)?;
        }

        Ok(())
    }

    async fn search_nearest(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<VectorSearchResult>, StoreError> {
        self.migrate_from_sqlite_if_needed().await?;

        let provider = provider.trim();
        let model = model.trim();
        if query_embedding.is_empty() || provider.is_empty() || model.is_empty() {
            return Ok(Vec::new());
        }

        let limit = limit.clamp(1, 100) as usize;
        let table_name = table_name_for(provider, model, query_embedding.len() as i32);
        let connection = self.connect().await?;
        let table = match connection.open_table(&table_name).execute().await {
            Ok(table) => table,
            Err(LanceError::TableNotFound { .. }) => return Ok(Vec::new()),
            Err(err) => return Err(map_lancedb_err(err)),
        };

        let query = table
            .query()
            .select(Select::columns(&["symbol_id", "_distance"]))
            .nearest_to(query_embedding)
            .map_err(map_lancedb_err)?
            .distance_type(DistanceType::Cosine)
            .limit(limit);

        let batches = query
            .execute()
            .await
            .map_err(map_lancedb_err)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(map_lancedb_err)?;

        let mut rows = Vec::new();
        for batch in batches {
            let symbol_ids = batch
                .column_by_name("symbol_id")
                .ok_or_else(|| StoreError::LanceDb("missing symbol_id column".to_owned()))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| StoreError::LanceDb("symbol_id column is not Utf8".to_owned()))?;
            let distances = batch
                .column_by_name("_distance")
                .ok_or_else(|| StoreError::LanceDb("missing _distance column".to_owned()))?;

            for idx in 0..batch.num_rows() {
                if symbol_ids.is_null(idx) {
                    continue;
                }
                let symbol_id = symbol_ids.value(idx).to_owned();
                let distance = distance_at(distances, idx)?;
                rows.push(VectorSearchResult {
                    symbol_id,
                    semantic_score: 1.0 - distance,
                });
            }
        }

        rows.sort_by(|left, right| {
            right
                .semantic_score
                .partial_cmp(&left.semantic_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });
        rows.truncate(limit);
        Ok(rows)
    }
}

fn map_lancedb_err(err: LanceError) -> StoreError {
    StoreError::LanceDb(err.to_string())
}

fn vector_schema(embedding_dim: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("symbol_id", DataType::Utf8, false),
        Field::new("sir_hash", DataType::Utf8, false),
        Field::new("provider", DataType::Utf8, false),
        Field::new("model", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embedding_dim,
            ),
            true,
        ),
        Field::new("updated_at", DataType::Int64, false),
    ]))
}

fn single_record_batch(record: &VectorRecord) -> Result<(SchemaRef, RecordBatch), StoreError> {
    let embedding_dim = record.embedding.len() as i32;
    if embedding_dim <= 0 {
        return Err(StoreError::LanceDb(
            "embedding cannot be empty for LanceDB upsert".to_owned(),
        ));
    }

    let schema = vector_schema(embedding_dim);
    let embedding = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        std::iter::once(Some(
            record
                .embedding
                .iter()
                .copied()
                .map(Some)
                .collect::<Vec<Option<f32>>>(),
        )),
        embedding_dim,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![record.symbol_id.clone()])),
        Arc::new(StringArray::from(vec![record.sir_hash.clone()])),
        Arc::new(StringArray::from(vec![record.provider.clone()])),
        Arc::new(StringArray::from(vec![record.model.clone()])),
        Arc::new(embedding),
        Arc::new(Int64Array::from(vec![record.updated_at])),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns)
        .map_err(|err| StoreError::LanceDb(err.to_string()))?;
    Ok((schema, batch))
}

fn embedding_dim_from_schema(schema: &Schema) -> Option<i32> {
    let field = schema.field_with_name("embedding").ok()?;
    match field.data_type() {
        DataType::FixedSizeList(_, dim) => Some(*dim),
        _ => None,
    }
}

fn table_name_for(provider: &str, model: &str, embedding_dim: i32) -> String {
    let provider = sanitize_for_table_name(provider);
    let model = sanitize_for_table_name(model);
    format!("{VECTOR_TABLE_PREFIX}{embedding_dim}_{provider}_{model}")
}

fn sanitize_for_table_name(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(64));
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push('_');
        }
        if output.len() >= 48 {
            break;
        }
    }
    while output.contains("__") {
        output = output.replace("__", "_");
    }
    output
        .trim_matches('_')
        .to_owned()
        .chars()
        .take(48)
        .collect::<String>()
}

fn load_sqlite_embedding_rows(sqlite_path: &Path) -> Result<Vec<VectorRecord>, StoreError> {
    if !sqlite_path.exists() {
        return Ok(Vec::new());
    }

    let conn = Connection::open(sqlite_path)?;
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sir_embeddings' LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT symbol_id, sir_hash, provider, model, embedding_json, updated_at
        FROM sir_embeddings
        "#,
    )?;
    let rows = stmt.query_map(params![], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;

    let mut records = Vec::new();
    for row in rows {
        let (symbol_id, sir_hash, provider, model, embedding_json, updated_at) = row?;
        let embedding = serde_json::from_str::<Vec<f32>>(&embedding_json)?;
        if embedding.is_empty() {
            continue;
        }
        records.push(VectorRecord {
            symbol_id,
            sir_hash,
            provider,
            model,
            embedding,
            updated_at,
        });
    }

    Ok(records)
}

fn distance_at(column: &ArrayRef, index: usize) -> Result<f32, StoreError> {
    if let Some(values) = column.as_any().downcast_ref::<Float32Array>() {
        if values.is_null(index) {
            return Err(StoreError::LanceDb("null distance value".to_owned()));
        }
        return Ok(values.value(index));
    }
    if let Some(values) = column.as_any().downcast_ref::<Float64Array>() {
        if values.is_null(index) {
            return Err(StoreError::LanceDb("null distance value".to_owned()));
        }
        return Ok(values.value(index) as f32);
    }

    Err(StoreError::LanceDb(format!(
        "unsupported _distance type: {:?}",
        column.data_type()
    )))
}

fn string_at(batch: &RecordBatch, column_name: &str, row: usize) -> Result<String, StoreError> {
    let array = batch
        .column_by_name(column_name)
        .ok_or_else(|| StoreError::LanceDb(format!("missing column {column_name}")))?
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| StoreError::LanceDb(format!("column {column_name} is not Utf8")))?;
    if array.is_null(row) {
        return Err(StoreError::LanceDb(format!(
            "column {column_name} has null at row {row}"
        )));
    }
    Ok(array.value(row).to_owned())
}

fn int64_at(batch: &RecordBatch, column_name: &str, row: usize) -> Result<i64, StoreError> {
    let array = batch
        .column_by_name(column_name)
        .ok_or_else(|| StoreError::LanceDb(format!("missing column {column_name}")))?
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| StoreError::LanceDb(format!("column {column_name} is not Int64")))?;
    if array.is_null(row) {
        return Err(StoreError::LanceDb(format!(
            "column {column_name} has null at row {row}"
        )));
    }
    Ok(array.value(row))
}

fn escape_sql_string(value: &str) -> String {
    value.replace('\'', "''")
}
