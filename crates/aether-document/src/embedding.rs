use std::sync::Arc;

use aether_infer::EmbeddingProvider;

use crate::{DocumentVectorBackend, DocumentVectorMatch, GenericRecord, Result, SemanticRecord};

pub struct DocumentEmbedder {
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    pub vector_backend: Arc<dyn DocumentVectorBackend>,
    pub provider_name: String,
    pub model_name: String,
}

impl DocumentEmbedder {
    pub fn new(
        embedding_provider: Arc<dyn EmbeddingProvider>,
        vector_backend: Arc<dyn DocumentVectorBackend>,
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
    ) -> Self {
        Self {
            embedding_provider,
            vector_backend,
            provider_name: provider_name.into(),
            model_name: model_name.into(),
        }
    }

    pub async fn embed_records(&self, domain: &str, records: &[GenericRecord]) -> Result<usize> {
        let mut prepared = Vec::with_capacity(records.len());
        for record in records {
            match self
                .embedding_provider
                .embed_text(record.embedding_text())
                .await
            {
                Ok(embedding) if !embedding.is_empty() => {
                    prepared.push((record.record_id.clone(), embedding));
                }
                Ok(_) => {
                    tracing::warn!(
                        domain = domain,
                        record_id = %record.record_id,
                        "document embedding provider returned empty embedding; skipping"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        domain = domain,
                        record_id = %record.record_id,
                        error = %err,
                        "document embedding failed; skipping record"
                    );
                }
            }
        }

        if prepared.is_empty() {
            return Ok(0);
        }

        self.vector_backend
            .upsert_embeddings(domain, prepared.as_slice())
            .await
    }

    pub async fn search(
        &self,
        domain: &str,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<DocumentVectorMatch>> {
        let embedding = self.embedding_provider.embed_text(query_text).await?;
        if embedding.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        self.vector_backend
            .search(domain, embedding.as_slice(), limit)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use aether_infer::{EmbeddingProvider, InferError};
    use async_trait::async_trait;
    use serde_json::json;

    use crate::{DocumentError, DocumentVectorBackend, DocumentVectorMatch, GenericRecord, Result};

    use super::DocumentEmbedder;

    #[derive(Default)]
    struct MockBackend {
        upserts: Mutex<Vec<(String, Vec<(String, Vec<f32>)>)>>,
        searches: Mutex<Vec<(String, Vec<f32>, usize)>>,
    }

    #[async_trait]
    impl DocumentVectorBackend for MockBackend {
        async fn upsert_embeddings(
            &self,
            domain: &str,
            records: &[(String, Vec<f32>)],
        ) -> Result<usize> {
            self.upserts
                .lock()
                .map_err(|_| DocumentError::Backend("mock backend mutex poisoned".to_owned()))?
                .push((domain.to_owned(), records.to_vec()));
            Ok(records.len())
        }

        async fn search(
            &self,
            domain: &str,
            query_embedding: &[f32],
            limit: usize,
        ) -> Result<Vec<DocumentVectorMatch>> {
            self.searches
                .lock()
                .map_err(|_| DocumentError::Backend("mock backend mutex poisoned".to_owned()))?
                .push((domain.to_owned(), query_embedding.to_vec(), limit));
            Ok(vec![DocumentVectorMatch {
                record_id: "record-1".to_owned(),
                score: 0.99,
            }])
        }

        async fn delete_by_domain(&self, _domain: &str) -> Result<usize> {
            Ok(0)
        }
    }

    #[derive(Default)]
    struct FailOnTokenProvider;

    #[async_trait]
    impl EmbeddingProvider for FailOnTokenProvider {
        async fn embed_text(&self, text: &str) -> std::result::Result<Vec<f32>, InferError> {
            if text.contains("FAIL") {
                return Err(InferError::InvalidEmbeddingResponse(
                    "intentional test failure".to_owned(),
                ));
            }
            Ok(vec![1.0, 0.0, 0.5])
        }
    }

    #[derive(Default)]
    struct TestEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for TestEmbeddingProvider {
        async fn embed_text(&self, text: &str) -> std::result::Result<Vec<f32>, InferError> {
            if text.trim().is_empty() {
                return Ok(Vec::new());
            }
            Ok(vec![1.0, 0.0, 0.5])
        }
    }

    fn make_record(id_suffix: &str, embedding_text: &str) -> GenericRecord {
        GenericRecord::new(
            format!("unit-{id_suffix}"),
            "docs",
            "entity",
            "v1",
            json!({"name": id_suffix}),
            embedding_text,
        )
        .expect("record")
    }

    #[tokio::test]
    async fn embed_records_uses_embedding_provider_and_upserts_results() {
        let backend = Arc::new(MockBackend::default());
        let embedder = DocumentEmbedder::new(
            Arc::new(TestEmbeddingProvider),
            backend.clone(),
            "mock",
            "mock-64d",
        );
        let records = vec![make_record("one", "alpha"), make_record("two", "beta")];

        let inserted = embedder
            .embed_records("docs", records.as_slice())
            .await
            .expect("embed records");
        assert_eq!(inserted, 2);

        let upserts = backend.upserts.lock().expect("upserts lock");
        assert_eq!(upserts.len(), 1);
        assert_eq!(upserts[0].0, "docs");
        assert_eq!(upserts[0].1.len(), 2);
        assert_eq!(upserts[0].1[0].0, records[0].record_id);
        assert_eq!(upserts[0].1[1].0, records[1].record_id);
        assert!(!upserts[0].1[0].1.is_empty());
        assert!(!upserts[0].1[1].1.is_empty());
    }

    #[tokio::test]
    async fn embed_records_skips_individual_failures() {
        let backend = Arc::new(MockBackend::default());
        let embedder = DocumentEmbedder::new(
            Arc::new(FailOnTokenProvider),
            backend.clone(),
            "mock",
            "test",
        );
        let records = vec![
            make_record("one", "ok"),
            make_record("two", "FAIL this one"),
            make_record("three", "ok again"),
        ];

        let inserted = embedder
            .embed_records("docs", records.as_slice())
            .await
            .expect("embed records");
        assert_eq!(inserted, 2);

        let upserts = backend.upserts.lock().expect("upserts lock");
        assert_eq!(upserts.len(), 1);
        assert_eq!(upserts[0].1.len(), 2);
    }

    #[tokio::test]
    async fn search_embeds_query_then_calls_backend_search() {
        let backend = Arc::new(MockBackend::default());
        let embedder = DocumentEmbedder::new(
            Arc::new(TestEmbeddingProvider),
            backend.clone(),
            "mock",
            "mock-64d",
        );

        let matches = embedder
            .search("docs", "query text", 5)
            .await
            .expect("search");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].record_id, "record-1");

        let searches = backend.searches.lock().expect("searches lock");
        assert_eq!(searches.len(), 1);
        assert_eq!(searches[0].0, "docs");
        assert_eq!(searches[0].2, 5);
        assert!(!searches[0].1.is_empty());
    }
}
