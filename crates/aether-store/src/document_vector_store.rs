use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_document::{
    DocumentError, DocumentVectorBackend, DocumentVectorMatch, Result as DocumentResult,
};
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

use crate::vector::sanitize_for_table_name;

pub struct LanceDocumentVectorStore {
    vectors_dir: PathBuf,
    provider: String,
    model: String,
}

impl LanceDocumentVectorStore {
    pub fn new(vectors_dir: PathBuf, provider: &str, model: &str) -> Self {
        Self {
            vectors_dir,
            provider: provider.to_owned(),
            model: model.to_owned(),
        }
    }

    async fn connect(&self) -> DocumentResult<LanceConnection> {
        fs::create_dir_all(&self.vectors_dir).map_err(|err| {
            DocumentError::Backend(format!("failed to create vectors dir: {err}"))
        })?;
        connect(self.vectors_dir.to_string_lossy().as_ref())
            .execute()
            .await
            .map_err(map_lancedb_err)
    }

    fn table_name_for_domain(&self, domain: &str) -> DocumentResult<String> {
        let domain = sanitize_for_table_name(domain);
        let provider = sanitize_for_table_name(self.provider.as_str());
        let model = sanitize_for_table_name(self.model.as_str());
        if domain.is_empty() || provider.is_empty() || model.is_empty() {
            return Err(DocumentError::InvalidInput(
                "domain/provider/model produced empty LanceDB table name segment".to_owned(),
            ));
        }
        Ok(format!("doc_{domain}_{provider}_{model}"))
    }
}

#[async_trait]
impl DocumentVectorBackend for LanceDocumentVectorStore {
    async fn upsert_embeddings(
        &self,
        domain: &str,
        records: &[(String, Vec<f32>)],
    ) -> DocumentResult<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let expected_dim = records
            .first()
            .map(|(_, embedding)| embedding.len())
            .unwrap_or_default();
        if expected_dim == 0 {
            return Err(DocumentError::Backend(
                "document embeddings cannot be empty".to_owned(),
            ));
        }
        if records.iter().any(|(_, embedding)| embedding.is_empty()) {
            return Err(DocumentError::Backend(
                "document embedding batch contains empty embedding".to_owned(),
            ));
        }
        if records
            .iter()
            .any(|(_, embedding)| embedding.len() != expected_dim)
        {
            return Err(DocumentError::Backend(
                "document embedding dimensions must match within a batch".to_owned(),
            ));
        }

        let table_name = self.table_name_for_domain(domain)?;
        let connection = self.connect().await?;
        let updated_at = now_epoch_seconds()?;
        let (schema, batch) = document_record_batch(domain, updated_at, records)?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());

        let table = match connection.open_table(&table_name).execute().await {
            Ok(table) => {
                let existing_schema = table.schema().await.map_err(map_lancedb_err)?;
                let dim = embedding_dim_from_schema(existing_schema.as_ref()).ok_or_else(|| {
                    DocumentError::Backend("missing or invalid embedding column".to_owned())
                })?;
                if dim != expected_dim as i32 {
                    return Err(DocumentError::Backend(format!(
                        "embedding dimension mismatch for table {table_name}: expected {dim}, got {}",
                        expected_dim
                    )));
                }
                table
            }
            Err(LanceError::TableNotFound { .. }) => match connection
                .create_table(&table_name, Box::new(reader))
                .execute()
                .await
            {
                Ok(_) => return Ok(records.len()),
                Err(err) if is_table_already_exists_error(&err) => connection
                    .open_table(&table_name)
                    .execute()
                    .await
                    .map_err(map_lancedb_err)?,
                Err(err) => return Err(map_lancedb_err(err)),
            },
            Err(err) => return Err(map_lancedb_err(err)),
        };

        let (schema, batch) = document_record_batch(domain, updated_at, records)?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["record_id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge
            .execute(Box::new(reader))
            .await
            .map_err(map_lancedb_err)?;
        Ok(records.len())
    }

    async fn search(
        &self,
        domain: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> DocumentResult<Vec<DocumentVectorMatch>> {
        if query_embedding.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let table_name = self.table_name_for_domain(domain)?;
        let connection = self.connect().await?;
        let table = match connection.open_table(&table_name).execute().await {
            Ok(table) => table,
            Err(LanceError::TableNotFound { .. }) => return Ok(Vec::new()),
            Err(err) => return Err(map_lancedb_err(err)),
        };
        let schema = table.schema().await.map_err(map_lancedb_err)?;
        let expected_dim = embedding_dim_from_schema(schema.as_ref()).ok_or_else(|| {
            DocumentError::Backend(format!("invalid embedding schema for table {table_name}"))
        })?;
        if expected_dim != query_embedding.len() as i32 {
            return Err(DocumentError::Backend(format!(
                "query embedding dimension {} does not match table dimension {expected_dim}",
                query_embedding.len()
            )));
        }

        let query = table
            .query()
            .select(Select::columns(&["record_id", "_distance"]))
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
            let record_ids = batch
                .column_by_name("record_id")
                .ok_or_else(|| DocumentError::Backend("missing record_id column".to_owned()))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| DocumentError::Backend("record_id column is not Utf8".to_owned()))?;
            let distances = batch
                .column_by_name("_distance")
                .ok_or_else(|| DocumentError::Backend("missing _distance column".to_owned()))?;

            for row in 0..batch.num_rows() {
                if record_ids.is_null(row) {
                    continue;
                }
                let record_id = record_ids.value(row).to_owned();
                let distance = distance_at(distances, row)?;
                rows.push(DocumentVectorMatch {
                    record_id,
                    score: 1.0 - distance,
                });
            }
        }

        rows.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.record_id.cmp(&right.record_id))
        });
        rows.truncate(limit);
        Ok(rows)
    }

    async fn delete_by_domain(&self, domain: &str) -> DocumentResult<usize> {
        let table_name = self.table_name_for_domain(domain)?;
        let connection = self.connect().await?;
        let mut deleted = 0usize;
        for name in connection
            .table_names()
            .execute()
            .await
            .map_err(map_lancedb_err)?
            .into_iter()
            .filter(|name| name == &table_name)
        {
            connection
                .drop_table(name.as_str(), &[])
                .await
                .map_err(map_lancedb_err)?;
            deleted += 1;
        }
        Ok(deleted)
    }
}

fn document_vector_schema(embedding_dim: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("record_id", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embedding_dim,
            ),
            true,
        ),
        Field::new("domain", DataType::Utf8, false),
        Field::new("updated_at", DataType::Int64, false),
    ]))
}

fn document_record_batch(
    domain: &str,
    updated_at: i64,
    records: &[(String, Vec<f32>)],
) -> DocumentResult<(SchemaRef, RecordBatch)> {
    if records.is_empty() {
        return Err(DocumentError::Backend(
            "cannot build document vector batch from empty records".to_owned(),
        ));
    }

    let embedding_dim = records[0].1.len() as i32;
    if embedding_dim <= 0 {
        return Err(DocumentError::Backend(
            "document embeddings cannot be empty".to_owned(),
        ));
    }
    if records
        .iter()
        .any(|(_, embedding)| embedding.len() as i32 != embedding_dim)
    {
        return Err(DocumentError::Backend(
            "document embedding dimensions must match within a batch".to_owned(),
        ));
    }

    let schema = document_vector_schema(embedding_dim);
    let embedding = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        records.iter().map(|(_, embedding)| {
            Some(
                embedding
                    .iter()
                    .copied()
                    .map(Some)
                    .collect::<Vec<Option<f32>>>(),
            )
        }),
        embedding_dim,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(
            records
                .iter()
                .map(|(record_id, _)| record_id.clone())
                .collect::<Vec<_>>(),
        )),
        Arc::new(embedding),
        Arc::new(StringArray::from(vec![domain.to_owned(); records.len()])),
        Arc::new(Int64Array::from(vec![updated_at; records.len()])),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns).map_err(|err| {
        DocumentError::Backend(format!("failed to build document vector batch: {err}"))
    })?;
    Ok((schema, batch))
}

fn embedding_dim_from_schema(schema: &Schema) -> Option<i32> {
    let field = schema.field_with_name("embedding").ok()?;
    match field.data_type() {
        DataType::FixedSizeList(_, dim) => Some(*dim),
        _ => None,
    }
}

fn distance_at(column: &ArrayRef, index: usize) -> DocumentResult<f32> {
    if let Some(values) = column.as_any().downcast_ref::<Float32Array>() {
        if values.is_null(index) {
            return Err(DocumentError::Backend("null distance value".to_owned()));
        }
        return Ok(values.value(index));
    }
    if let Some(values) = column.as_any().downcast_ref::<Float64Array>() {
        if values.is_null(index) {
            return Err(DocumentError::Backend("null distance value".to_owned()));
        }
        return Ok(values.value(index) as f32);
    }
    Err(DocumentError::Backend(format!(
        "unsupported _distance type: {:?}",
        column.data_type()
    )))
}

fn now_epoch_seconds() -> DocumentResult<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| DocumentError::Backend(format!("system time before UNIX_EPOCH: {err}")))?;
    i64::try_from(duration.as_secs())
        .map_err(|_| DocumentError::Backend("current timestamp exceeds i64 range".to_owned()))
}

fn map_lancedb_err(err: LanceError) -> DocumentError {
    DocumentError::Backend(format!("LanceDB error: {err}"))
}

fn is_table_already_exists_error(err: &LanceError) -> bool {
    err.to_string()
        .to_ascii_lowercase()
        .contains("already exists")
}
