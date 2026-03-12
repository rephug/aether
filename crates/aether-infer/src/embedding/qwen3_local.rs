use aether_config::{DEFAULT_QWEN_EMBEDDING_ENDPOINT, DEFAULT_QWEN_MODEL};
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::http::{extract_embedding_vector, inference_http_client};
use crate::types::{EmbeddingProvider, InferError, normalize_optional};

#[derive(Debug, Clone)]
pub struct Qwen3LocalEmbeddingProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
}

impl Qwen3LocalEmbeddingProvider {
    pub fn new(endpoint: Option<String>, model: Option<String>) -> Self {
        Self {
            client: inference_http_client(),
            endpoint: normalize_optional(endpoint)
                .unwrap_or_else(|| DEFAULT_QWEN_EMBEDDING_ENDPOINT.to_owned()),
            model: normalize_optional(model).unwrap_or_else(|| DEFAULT_QWEN_MODEL.to_owned()),
        }
    }

    pub(crate) fn model_name(&self) -> &str {
        self.model.as_str()
    }

    async fn request_embedding(&self, text: &str) -> Result<Vec<f32>, InferError> {
        let body = json!({
            "model": self.model,
            "prompt": text
        });

        let response_value: Value = self
            .client
            .post(&self.endpoint)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        extract_embedding_vector(&response_value)
    }
}

#[async_trait]
impl EmbeddingProvider for Qwen3LocalEmbeddingProvider {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, InferError> {
        self.request_embedding(text).await
    }
}
