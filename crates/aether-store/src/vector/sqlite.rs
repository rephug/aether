use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;

use crate::{
    ProjectNoteEmbeddingRecord, ProjectNoteEmbeddingStore, ProjectNoteSemanticSearchResult,
    SemanticIndexStore, SqliteStore, StoreError,
};

use super::{
    ProjectNoteVectorRecord, ProjectNoteVectorSearchResult, VectorEmbeddingMetaRecord,
    VectorRecord, VectorSearchResult, VectorStore,
};

pub struct SqliteVectorStore {
    store: SqliteStore,
}

impl SqliteVectorStore {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self {
            store: SqliteStore::open(workspace_root)?,
        })
    }
}

#[async_trait]
impl VectorStore for SqliteVectorStore {
    async fn upsert_embedding(&self, record: VectorRecord) -> Result<(), StoreError> {
        self.store.upsert_symbol_embedding(record)
    }

    async fn get_embedding_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<VectorEmbeddingMetaRecord>, StoreError> {
        self.store.get_symbol_embedding_meta(symbol_id)
    }

    async fn get_embedding_metas_batch(
        &self,
        symbol_ids: &[String],
    ) -> Result<HashMap<String, VectorEmbeddingMetaRecord>, StoreError> {
        let mut result = HashMap::new();
        for symbol_id in symbol_ids {
            if let Some(meta) = self.store.get_symbol_embedding_meta(symbol_id)? {
                result.insert(symbol_id.clone(), meta);
            }
        }
        Ok(result)
    }

    async fn delete_embedding(&self, symbol_id: &str) -> Result<(), StoreError> {
        self.store.delete_symbol_embedding(symbol_id)
    }

    async fn delete_embeddings(&self, symbol_ids: &[String]) -> Result<(), StoreError> {
        self.store.delete_symbol_embeddings(symbol_ids)
    }

    async fn search_nearest(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<VectorSearchResult>, StoreError> {
        let matches =
            self.store
                .search_symbols_semantic(query_embedding, provider, model, limit)?;

        Ok(matches
            .into_iter()
            .map(|row| VectorSearchResult {
                symbol_id: row.symbol_id,
                semantic_score: row.semantic_score,
            })
            .collect())
    }

    async fn list_embeddings_for_symbols(
        &self,
        provider: &str,
        model: &str,
        symbol_ids: &[String],
    ) -> Result<Vec<VectorRecord>, StoreError> {
        self.store
            .list_symbol_embeddings_for_ids(provider, model, symbol_ids)
    }

    async fn upsert_project_note_embedding(
        &self,
        record: ProjectNoteVectorRecord,
    ) -> Result<(), StoreError> {
        self.store
            .upsert_project_note_embedding(ProjectNoteEmbeddingRecord {
                note_id: record.note_id,
                provider: record.provider,
                model: record.model,
                embedding: record.embedding,
                content: record.content,
                created_at: record.created_at,
                updated_at: record.updated_at,
            })
    }

    async fn delete_project_note_embedding(&self, note_id: &str) -> Result<(), StoreError> {
        self.store.delete_project_note_embedding(note_id)
    }

    async fn search_project_notes_nearest(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteVectorSearchResult>, StoreError> {
        let matches =
            self.store
                .search_project_notes_semantic(query_embedding, provider, model, limit)?;

        Ok(matches
            .into_iter()
            .map(
                |row: ProjectNoteSemanticSearchResult| ProjectNoteVectorSearchResult {
                    note_id: row.note_id,
                    semantic_score: row.semantic_score,
                },
            )
            .collect())
    }
}
