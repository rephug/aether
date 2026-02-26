use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentVectorMatch {
    pub record_id: String,
    pub score: f32,
}

#[async_trait]
pub trait DocumentVectorBackend: Send + Sync {
    async fn upsert_embeddings(&self, domain: &str, records: &[(String, Vec<f32>)]) -> Result<usize>;
    async fn search(
        &self,
        domain: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<DocumentVectorMatch>>;
    async fn delete_by_domain(&self, domain: &str) -> Result<usize>;
}
