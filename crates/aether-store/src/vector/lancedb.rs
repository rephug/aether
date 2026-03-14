use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use arrow_array::{Array, StringArray};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::{Connection as LanceConnection, DistanceType, Error as LanceError, connect};

use crate::{SqliteStore, StoreError};

use super::schema::{
    distance_at, embedding_at, embedding_dim_from_schema, int64_at, record_batch,
    single_project_note_record_batch, single_record_batch, string_at,
};
use super::{
    MIGRATION_MARKER_FILE, NOTES_MIGRATION_MARKER_FILE, PROJECT_NOTES_VECTOR_TABLE_PREFIX,
    PUSHDOWN_CHUNK_SIZE, ProjectNoteVectorRecord, ProjectNoteVectorSearchResult,
    VECTOR_TABLE_PREFIX, VectorEmbeddingMetaRecord, VectorRecord, VectorSearchResult, VectorStore,
};

/// Build a SQL predicate: symbol_id IN ('id1', 'id2', ...)
fn build_in_predicate(ids: &[&str]) -> String {
    let values = ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    format!("symbol_id IN ({values})")
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
        store.migrate_project_notes_from_sqlite_if_needed().await?;
        Ok(store)
    }

    pub(super) fn marker_path(&self) -> PathBuf {
        self.vectors_dir.join(MIGRATION_MARKER_FILE)
    }

    pub(super) fn project_notes_marker_path(&self) -> PathBuf {
        self.vectors_dir.join(NOTES_MIGRATION_MARKER_FILE)
    }

    pub(super) fn sqlite_path(&self) -> PathBuf {
        self.workspace_root.join(".aether").join("meta.sqlite")
    }

    pub(super) async fn connect(&self) -> Result<LanceConnection, StoreError> {
        connect(self.vectors_dir.to_string_lossy().as_ref())
            .execute()
            .await
            .map_err(map_lancedb_err)
    }

    pub async fn has_embedding_tables(&self) -> Result<bool, StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        let connection = self.connect().await?;
        Ok(connection
            .table_names()
            .execute()
            .await
            .map_err(map_lancedb_err)?
            .into_iter()
            .any(|name| name.starts_with(VECTOR_TABLE_PREFIX)))
    }

    pub async fn list_all_embedding_symbol_ids(&self) -> Result<Vec<String>, StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        let connection = self.connect().await?;
        let mut ids = HashSet::new();

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
            let batches = table
                .query()
                .select(Select::columns(&["symbol_id"]))
                .execute()
                .await
                .map_err(map_lancedb_err)?
                .try_collect::<Vec<_>>()
                .await
                .map_err(map_lancedb_err)?;

            for batch in batches {
                for row in 0..batch.num_rows() {
                    let symbol_id = string_at(&batch, "symbol_id", row)?;
                    if !symbol_id.is_empty() {
                        ids.insert(symbol_id);
                    }
                }
            }
        }

        let mut values = ids.into_iter().collect::<Vec<_>>();
        values.sort();
        Ok(values)
    }

    pub async fn list_existing_embedding_symbol_ids(
        &self,
        symbol_ids: &[String],
    ) -> Result<Vec<String>, StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        if symbol_ids.is_empty() {
            return Ok(Vec::new());
        }

        let requested = symbol_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if requested.is_empty() {
            return Ok(Vec::new());
        }

        let connection = self.connect().await?;
        let mut existing = HashSet::new();
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
            for chunk in requested.chunks(PUSHDOWN_CHUNK_SIZE) {
                let predicate = build_in_predicate(chunk);
                let batches = table
                    .query()
                    .select(Select::columns(&["symbol_id"]))
                    .only_if(predicate.as_str())
                    .execute()
                    .await
                    .map_err(map_lancedb_err)?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(map_lancedb_err)?;

                for batch in batches {
                    for row in 0..batch.num_rows() {
                        let symbol_id = string_at(&batch, "symbol_id", row)?;
                        if !symbol_id.is_empty() {
                            existing.insert(symbol_id);
                        }
                    }
                }
            }
        }

        let mut values = existing.into_iter().collect::<Vec<_>>();
        values.sort();
        Ok(values)
    }

    pub(super) async fn upsert_embedding_with_connection(
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
                let reader =
                    arrow_array::RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
                match connection
                    .create_table(&table_name, Box::new(reader))
                    .execute()
                    .await
                {
                    Ok(_) => return Ok(()),
                    Err(err) if is_table_already_exists_error(&err) => {
                        tracing::debug!(
                            table = %table_name,
                            "table created by concurrent task, opening instead"
                        );
                        connection
                            .open_table(&table_name)
                            .execute()
                            .await
                            .map_err(map_lancedb_err)?
                    }
                    Err(err) => return Err(map_lancedb_err(err)),
                }
            }
            Err(err) => return Err(map_lancedb_err(err)),
        };

        let (schema, batch) = single_record_batch(record)?;
        let reader = arrow_array::RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
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

    pub(super) async fn upsert_embedding_batch_with_connection(
        &self,
        connection: &LanceConnection,
        table_name: &str,
        records: &[VectorRecord],
    ) -> Result<(), StoreError> {
        if records.is_empty() {
            return Ok(());
        }

        let expected_dim = records[0].embedding.len();
        if expected_dim == 0 {
            return Ok(());
        }
        if records
            .iter()
            .any(|record| record.embedding.len() != expected_dim)
        {
            return Err(StoreError::LanceDb(format!(
                "inconsistent embedding dimensions in migration batch for table {table_name}"
            )));
        }

        let table = match connection.open_table(table_name).execute().await {
            Ok(table) => table,
            Err(LanceError::TableNotFound { .. }) => {
                let (schema, batch) = record_batch(records)?;
                let reader =
                    arrow_array::RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
                match connection
                    .create_table(table_name, Box::new(reader))
                    .execute()
                    .await
                {
                    Ok(_) => return Ok(()),
                    Err(err) if is_table_already_exists_error(&err) => {
                        tracing::debug!(
                            table = table_name,
                            "table created by concurrent task, opening instead"
                        );
                        connection
                            .open_table(table_name)
                            .execute()
                            .await
                            .map_err(map_lancedb_err)?
                    }
                    Err(err) => return Err(map_lancedb_err(err)),
                }
            }
            Err(err) => return Err(map_lancedb_err(err)),
        };

        let (schema, batch) = record_batch(records)?;
        let reader = arrow_array::RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["symbol_id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        if let Err(err) = merge.execute(Box::new(reader)).await {
            tracing::error!(
                error = %err,
                table = table_name,
                "LanceDB merge_insert failed during migration"
            );
            return Err(StoreError::LanceDb(format!(
                "LanceDB merge_insert failed: {err}"
            )));
        }

        Ok(())
    }

    pub(super) async fn upsert_project_note_embedding_with_connection(
        &self,
        connection: &LanceConnection,
        record: &ProjectNoteVectorRecord,
    ) -> Result<(), StoreError> {
        let embedding_dim = record.embedding.len() as i32;
        if embedding_dim <= 0 {
            return Ok(());
        }
        let table_name = project_notes_table_name_for(
            record.provider.as_str(),
            record.model.as_str(),
            embedding_dim,
        );

        let table = match connection.open_table(&table_name).execute().await {
            Ok(table) => table,
            Err(LanceError::TableNotFound { .. }) => {
                let (schema, batch) = single_project_note_record_batch(record)?;
                let reader =
                    arrow_array::RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
                match connection
                    .create_table(&table_name, Box::new(reader))
                    .execute()
                    .await
                {
                    Ok(_) => return Ok(()),
                    Err(err) if is_table_already_exists_error(&err) => {
                        tracing::debug!(
                            table = %table_name,
                            "table created by concurrent task, opening instead"
                        );
                        connection
                            .open_table(&table_name)
                            .execute()
                            .await
                            .map_err(map_lancedb_err)?
                    }
                    Err(err) => return Err(map_lancedb_err(err)),
                }
            }
            Err(err) => return Err(map_lancedb_err(err)),
        };

        let (schema, batch) = single_project_note_record_batch(record)?;
        let reader = arrow_array::RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["note_id"]);
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

    async fn get_embedding_metas_batch(
        &self,
        symbol_ids: &[String],
    ) -> Result<HashMap<String, VectorEmbeddingMetaRecord>, StoreError> {
        self.migrate_from_sqlite_if_needed().await?;

        let requested = symbol_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if requested.is_empty() {
            return Ok(HashMap::new());
        }

        let connection = self.connect().await?;
        let mut latest = HashMap::<String, VectorEmbeddingMetaRecord>::new();
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

            for chunk in requested.chunks(PUSHDOWN_CHUNK_SIZE) {
                let predicate = build_in_predicate(chunk);
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
                    .execute()
                    .await
                    .map_err(map_lancedb_err)?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(map_lancedb_err)?;

                for batch in batches {
                    for row in 0..batch.num_rows() {
                        let record = VectorEmbeddingMetaRecord {
                            symbol_id: string_at(&batch, "symbol_id", row)?,
                            sir_hash: string_at(&batch, "sir_hash", row)?,
                            provider: string_at(&batch, "provider", row)?,
                            model: string_at(&batch, "model", row)?,
                            embedding_dim: i64::from(embedding_dim),
                            updated_at: int64_at(&batch, "updated_at", row)?,
                        };

                        match latest.get(record.symbol_id.as_str()) {
                            Some(existing) if existing.updated_at >= record.updated_at => {}
                            _ => {
                                latest.insert(record.symbol_id.clone(), record);
                            }
                        }
                    }
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

    async fn delete_embeddings(&self, symbol_ids: &[String]) -> Result<(), StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        let requested = symbol_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if requested.is_empty() {
            return Ok(());
        }

        let connection = self.connect().await?;
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
            for chunk in requested.chunks(PUSHDOWN_CHUNK_SIZE) {
                let predicate = build_in_predicate(chunk);
                table
                    .delete(predicate.as_str())
                    .await
                    .map_err(map_lancedb_err)?;
            }
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

    async fn list_embeddings_for_symbols(
        &self,
        provider: &str,
        model: &str,
        symbol_ids: &[String],
    ) -> Result<Vec<VectorRecord>, StoreError> {
        self.migrate_from_sqlite_if_needed().await?;

        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() || symbol_ids.is_empty() {
            return Ok(Vec::new());
        }

        let provider = sanitize_for_table_name(provider);
        let model = sanitize_for_table_name(model);
        let suffix = format!("_{provider}_{model}");
        let symbol_set = symbol_ids
            .iter()
            .map(|item| item.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect::<HashSet<_>>();
        if symbol_set.is_empty() {
            return Ok(Vec::new());
        }

        let connection = self.connect().await?;
        let mut records = Vec::new();
        let symbol_ids = symbol_set
            .iter()
            .map(|symbol| symbol.as_str())
            .collect::<Vec<_>>();
        for table_name in connection
            .table_names()
            .execute()
            .await
            .map_err(map_lancedb_err)?
            .into_iter()
            .filter(|name| name.starts_with(VECTOR_TABLE_PREFIX) && name.ends_with(&suffix))
        {
            let Ok(table) = connection.open_table(&table_name).execute().await else {
                continue;
            };

            let mut batches = Vec::new();
            for chunk in symbol_ids.chunks(PUSHDOWN_CHUNK_SIZE) {
                let predicate = build_in_predicate(chunk);
                let chunk_batches = table
                    .query()
                    .select(Select::columns(&[
                        "symbol_id",
                        "sir_hash",
                        "provider",
                        "model",
                        "embedding",
                        "updated_at",
                    ]))
                    .only_if(predicate.as_str())
                    .execute()
                    .await
                    .map_err(map_lancedb_err)?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(map_lancedb_err)?;
                batches.extend(chunk_batches);
            }

            for batch in batches {
                for row in 0..batch.num_rows() {
                    let symbol_id = string_at(&batch, "symbol_id", row)?;
                    if !symbol_set.contains(symbol_id.as_str()) {
                        continue;
                    }

                    let embedding = embedding_at(&batch, "embedding", row)?;
                    if embedding.is_empty() {
                        continue;
                    }

                    records.push(VectorRecord {
                        symbol_id,
                        sir_hash: string_at(&batch, "sir_hash", row)?,
                        provider: string_at(&batch, "provider", row)?,
                        model: string_at(&batch, "model", row)?,
                        embedding,
                        updated_at: int64_at(&batch, "updated_at", row)?,
                    });
                }
            }
        }

        records.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
        Ok(records)
    }

    async fn upsert_project_note_embedding(
        &self,
        record: ProjectNoteVectorRecord,
    ) -> Result<(), StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        self.migrate_project_notes_from_sqlite_if_needed().await?;
        let connection = self.connect().await?;
        self.upsert_project_note_embedding_with_connection(&connection, &record)
            .await
    }

    async fn delete_project_note_embedding(&self, note_id: &str) -> Result<(), StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        self.migrate_project_notes_from_sqlite_if_needed().await?;
        let connection = self.connect().await?;
        let predicate = format!("note_id = '{}'", escape_sql_string(note_id));

        for name in connection
            .table_names()
            .execute()
            .await
            .map_err(map_lancedb_err)?
            .into_iter()
            .filter(|name| name.starts_with(PROJECT_NOTES_VECTOR_TABLE_PREFIX))
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

    async fn search_project_notes_nearest(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteVectorSearchResult>, StoreError> {
        self.migrate_from_sqlite_if_needed().await?;
        self.migrate_project_notes_from_sqlite_if_needed().await?;

        let provider = provider.trim();
        let model = model.trim();
        if query_embedding.is_empty() || provider.is_empty() || model.is_empty() {
            return Ok(Vec::new());
        }

        let limit = limit.clamp(1, 100) as usize;
        let table_name =
            project_notes_table_name_for(provider, model, query_embedding.len() as i32);
        let connection = self.connect().await?;
        let table = match connection.open_table(&table_name).execute().await {
            Ok(table) => table,
            Err(LanceError::TableNotFound { .. }) => return Ok(Vec::new()),
            Err(err) => return Err(map_lancedb_err(err)),
        };

        let query = table
            .query()
            .select(Select::columns(&["note_id", "_distance"]))
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
            let note_ids = batch
                .column_by_name("note_id")
                .ok_or_else(|| StoreError::LanceDb("missing note_id column".to_owned()))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| StoreError::LanceDb("note_id column is not Utf8".to_owned()))?;
            let distances = batch
                .column_by_name("_distance")
                .ok_or_else(|| StoreError::LanceDb("missing _distance column".to_owned()))?;

            for idx in 0..batch.num_rows() {
                if note_ids.is_null(idx) {
                    continue;
                }
                let note_id = note_ids.value(idx).to_owned();
                let distance = distance_at(distances, idx)?;
                rows.push(ProjectNoteVectorSearchResult {
                    note_id,
                    semantic_score: 1.0 - distance,
                });
            }
        }

        rows.sort_by(|left, right| {
            right
                .semantic_score
                .partial_cmp(&left.semantic_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.note_id.cmp(&right.note_id))
        });
        rows.truncate(limit);
        Ok(rows)
    }
}

pub(super) fn map_lancedb_err(err: LanceError) -> StoreError {
    StoreError::LanceDb(err.to_string())
}

fn is_table_already_exists_error(err: &LanceError) -> bool {
    err.to_string()
        .to_ascii_lowercase()
        .contains("already exists")
}

pub(super) fn table_name_for(provider: &str, model: &str, embedding_dim: i32) -> String {
    table_name_for_prefix(VECTOR_TABLE_PREFIX, provider, model, embedding_dim)
}

fn project_notes_table_name_for(provider: &str, model: &str, embedding_dim: i32) -> String {
    table_name_for_prefix(
        PROJECT_NOTES_VECTOR_TABLE_PREFIX,
        provider,
        model,
        embedding_dim,
    )
}

fn table_name_for_prefix(prefix: &str, provider: &str, model: &str, embedding_dim: i32) -> String {
    let provider = sanitize_for_table_name(provider);
    let model = sanitize_for_table_name(model);
    format!("{prefix}{embedding_dim}_{provider}_{model}")
}

pub(crate) fn sanitize_for_table_name(value: &str) -> String {
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

fn escape_sql_string(value: &str) -> String {
    value.replace('\'', "''")
}
