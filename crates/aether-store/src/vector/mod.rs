use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use aether_config::{EmbeddingVectorBackend, load_workspace_config};
use async_trait::async_trait;

use crate::{StoreError, SymbolEmbeddingMetaRecord, SymbolEmbeddingRecord};

mod lancedb;
mod migration;
mod schema;
mod sqlite;

pub use lancedb::LanceVectorStore;
pub(crate) use lancedb::sanitize_for_table_name;
pub use sqlite::SqliteVectorStore;

pub(super) const VECTOR_TABLE_PREFIX: &str = "sir_embeddings_";
pub(super) const PROJECT_NOTES_VECTOR_TABLE_PREFIX: &str = "project_notes_vectors_";
pub(super) const MIGRATION_MARKER_FILE: &str = ".sqlite_migrated_v1";
pub(super) const NOTES_MIGRATION_MARKER_FILE: &str = ".sqlite_project_notes_migrated_v1";
/// Maximum IDs per IN clause to stay within DataFusion parser limits.
pub(super) const PUSHDOWN_CHUNK_SIZE: usize = 500;

pub type VectorRecord = SymbolEmbeddingRecord;
pub type VectorEmbeddingMetaRecord = SymbolEmbeddingMetaRecord;

#[derive(Debug, Clone, PartialEq)]
pub struct VectorSearchResult {
    pub symbol_id: String,
    pub semantic_score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectNoteVectorRecord {
    pub note_id: String,
    pub provider: String,
    pub model: String,
    pub embedding: Vec<f32>,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectNoteVectorSearchResult {
    pub note_id: String,
    pub semantic_score: f32,
}

#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert_embedding(&self, record: VectorRecord) -> Result<(), StoreError>;
    async fn upsert_embedding_batch(&self, records: Vec<VectorRecord>) -> Result<(), StoreError>;
    async fn get_embedding_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<VectorEmbeddingMetaRecord>, StoreError>;
    async fn get_embedding_metas_batch(
        &self,
        symbol_ids: &[String],
    ) -> Result<HashMap<String, VectorEmbeddingMetaRecord>, StoreError>;
    async fn delete_embedding(&self, symbol_id: &str) -> Result<(), StoreError>;
    async fn delete_embeddings(&self, symbol_ids: &[String]) -> Result<(), StoreError>;
    async fn search_nearest(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<VectorSearchResult>, StoreError>;
    async fn list_embeddings_for_symbols(
        &self,
        provider: &str,
        model: &str,
        symbol_ids: &[String],
    ) -> Result<Vec<VectorRecord>, StoreError>;
    async fn upsert_project_note_embedding(
        &self,
        record: ProjectNoteVectorRecord,
    ) -> Result<(), StoreError>;
    async fn delete_project_note_embedding(&self, note_id: &str) -> Result<(), StoreError>;
    async fn search_project_notes_nearest(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteVectorSearchResult>, StoreError>;
}

pub async fn open_vector_store(
    workspace_root: impl AsRef<Path>,
) -> Result<Arc<dyn VectorStore>, StoreError> {
    let workspace_root = workspace_root.as_ref();
    let config = load_workspace_config(workspace_root)?;

    match config.embeddings.vector_backend {
        EmbeddingVectorBackend::Sqlite => Ok(Arc::new(SqliteVectorStore::new(workspace_root)?)),
        EmbeddingVectorBackend::Lancedb => {
            Ok(Arc::new(LanceVectorStore::open(workspace_root).await?))
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn vector_record(symbol_id: &str, hash: &str) -> VectorRecord {
        VectorRecord {
            symbol_id: symbol_id.to_owned(),
            sir_hash: hash.to_owned(),
            provider: "mock".to_owned(),
            model: "mock-2d".to_owned(),
            embedding: vec![1.0, 0.0],
            updated_at: 1_700_000_000,
        }
    }

    fn vector_record_with_meta(
        symbol_id: &str,
        hash: &str,
        provider: &str,
        model: &str,
        embedding: Vec<f32>,
        updated_at: i64,
    ) -> VectorRecord {
        VectorRecord {
            symbol_id: symbol_id.to_owned(),
            sir_hash: hash.to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            embedding,
            updated_at,
        }
    }

    #[tokio::test]
    async fn sqlite_vector_store_deletes_embeddings_in_batch() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteVectorStore::new(temp.path()).expect("open sqlite vector store");

        store
            .upsert_embedding(vector_record("sym-a", "hash-a"))
            .await
            .expect("upsert sym-a");
        store
            .upsert_embedding(vector_record("sym-b", "hash-b"))
            .await
            .expect("upsert sym-b");
        store
            .upsert_embedding(vector_record("sym-c", "hash-c"))
            .await
            .expect("upsert sym-c");

        store
            .delete_embeddings(&["sym-a".to_owned(), "sym-b".to_owned()])
            .await
            .expect("delete embeddings batch");

        assert!(
            store
                .get_embedding_meta("sym-a")
                .await
                .expect("lookup sym-a")
                .is_none()
        );
        assert!(
            store
                .get_embedding_meta("sym-b")
                .await
                .expect("lookup sym-b")
                .is_none()
        );
        assert!(
            store
                .get_embedding_meta("sym-c")
                .await
                .expect("lookup sym-c")
                .is_some()
        );
    }

    #[tokio::test]
    async fn sqlite_vector_store_gets_embedding_metas_in_batch() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteVectorStore::new(temp.path()).expect("open sqlite vector store");

        store
            .upsert_embedding(vector_record("sym-a", "hash-a"))
            .await
            .expect("upsert sym-a");
        store
            .upsert_embedding(vector_record("sym-b", "hash-b"))
            .await
            .expect("upsert sym-b");

        let metas = store
            .get_embedding_metas_batch(&[
                "sym-a".to_owned(),
                "sym-missing".to_owned(),
                "sym-b".to_owned(),
            ])
            .await
            .expect("batch lookup");

        assert_eq!(metas.len(), 2);
        assert_eq!(
            metas.get("sym-a").map(|meta| meta.sir_hash.as_str()),
            Some("hash-a")
        );
        assert_eq!(
            metas.get("sym-b").map(|meta| meta.sir_hash.as_str()),
            Some("hash-b")
        );
    }

    #[tokio::test]
    async fn lance_vector_store_deletes_embeddings_in_batch() {
        let temp = tempdir().expect("tempdir");
        let store = LanceVectorStore::open(temp.path())
            .await
            .expect("open LanceDB vector store");

        store
            .upsert_embedding(vector_record("sym-a", "hash-a"))
            .await
            .expect("upsert sym-a");
        store
            .upsert_embedding(vector_record("sym-b", "hash-b"))
            .await
            .expect("upsert sym-b");
        store
            .upsert_embedding(vector_record("sym-c", "hash-c"))
            .await
            .expect("upsert sym-c");

        store
            .delete_embeddings(&["sym-a".to_owned(), "sym-b".to_owned()])
            .await
            .expect("delete embeddings batch");

        assert!(
            store
                .get_embedding_meta("sym-a")
                .await
                .expect("lookup sym-a")
                .is_none()
        );
        assert!(
            store
                .get_embedding_meta("sym-b")
                .await
                .expect("lookup sym-b")
                .is_none()
        );
        assert!(
            store
                .get_embedding_meta("sym-c")
                .await
                .expect("lookup sym-c")
                .is_some()
        );
    }

    #[tokio::test]
    async fn lance_vector_store_gets_latest_embedding_metas_in_batch() {
        let temp = tempdir().expect("tempdir");
        let store = LanceVectorStore::open(temp.path())
            .await
            .expect("open LanceDB vector store");

        store
            .upsert_embedding(vector_record_with_meta(
                "sym-a",
                "hash-old",
                "mock",
                "model-2d",
                vec![1.0, 0.0],
                100,
            ))
            .await
            .expect("upsert old sym-a");
        store
            .upsert_embedding(vector_record_with_meta(
                "sym-a",
                "hash-new",
                "mock-alt",
                "model-3d",
                vec![1.0, 0.0, 0.0],
                200,
            ))
            .await
            .expect("upsert new sym-a");
        store
            .upsert_embedding(vector_record("sym-b", "hash-b"))
            .await
            .expect("upsert sym-b");

        let metas = store
            .get_embedding_metas_batch(&[
                "sym-a".to_owned(),
                "sym-missing".to_owned(),
                "sym-b".to_owned(),
            ])
            .await
            .expect("batch lookup");

        assert_eq!(metas.len(), 2);
        assert_eq!(
            metas.get("sym-a"),
            Some(&VectorEmbeddingMetaRecord {
                symbol_id: "sym-a".to_owned(),
                sir_hash: "hash-new".to_owned(),
                provider: "mock-alt".to_owned(),
                model: "model-3d".to_owned(),
                embedding_dim: 3,
                updated_at: 200,
            })
        );
        assert_eq!(
            metas.get("sym-b").map(|meta| meta.sir_hash.as_str()),
            Some("hash-b")
        );
    }
}
