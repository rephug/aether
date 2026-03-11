use std::time::Duration;

use aether_config::EmbeddingProviderKind;
use aether_core::Secret;
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::time::sleep;

use crate::{
    EmbeddingProvider, InferError, extract_embedding_vector, extract_openai_error_message,
    inference_http_client, normalize_openai_api_base,
};

#[derive(Debug, Clone)]
pub struct OpenAiCompatEmbeddingProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: Secret,
    task_type: Option<String>,
    dimensions: Option<u32>,
}

impl OpenAiCompatEmbeddingProvider {
    pub fn new(
        endpoint: String,
        model: String,
        api_key: Secret,
        task_type: Option<String>,
        dimensions: Option<u32>,
    ) -> Self {
        Self {
            client: inference_http_client(),
            endpoint: normalize_embedding_endpoint(&endpoint),
            model: model.trim().to_owned(),
            api_key,
            task_type: task_type
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            dimensions,
        }
    }

    pub(crate) fn provider_name(&self) -> &'static str {
        EmbeddingProviderKind::OpenAiCompat.as_str()
    }

    pub(crate) fn model_name(&self) -> &str {
        self.model.as_str()
    }

    async fn request_embedding_once(&self, text: &str) -> Result<Vec<f32>, InferError> {
        let url = format!("{}/embeddings", self.endpoint);
        let mut body = Map::new();
        body.insert("model".to_owned(), Value::String(self.model.clone()));
        body.insert("input".to_owned(), Value::String(text.to_owned()));
        if let Some(task_type) = &self.task_type {
            body.insert("task_type".to_owned(), Value::String(task_type.clone()));
        }
        if let Some(dimensions) = self.dimensions {
            body.insert("dimensions".to_owned(), json!(dimensions));
        }

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key.expose()))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let response_body = response.text().await?;
        if !status.is_success() {
            let provider_message = extract_openai_error_message(&response_body)
                .unwrap_or_else(|| response_body.trim().to_owned());
            let provider_message = if provider_message.is_empty() {
                "unknown provider error".to_owned()
            } else {
                provider_message
            };
            return Err(InferError::InvalidResponse(format!(
                "openai_compat request failed with status {status}: {provider_message}"
            )));
        }
        let response: Value = serde_json::from_str(&response_body)?;

        extract_embedding_vector(&response)
    }

    async fn request_embedding(&self, text: &str) -> Result<Vec<f32>, InferError> {
        let mut backoff = Duration::from_secs(1);
        for attempt in 0..3 {
            match self.request_embedding_once(text).await {
                Ok(embedding) => return Ok(embedding),
                Err(err) if attempt == 2 => return Err(err),
                Err(err) => {
                    tracing::warn!(
                        attempt = attempt + 1,
                        error = %err,
                        "openai_compat embedding request failed; retrying"
                    );
                    sleep(backoff).await;
                    backoff = backoff.saturating_mul(2);
                }
            }
        }
        unreachable!("embedding retry loop must return on success or final failure");
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatEmbeddingProvider {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, InferError> {
        self.request_embedding(text).await
    }
}

fn normalize_embedding_endpoint(endpoint: &str) -> String {
    let normalized = normalize_openai_api_base(endpoint);
    let trimmed = normalized.trim_end_matches('/');
    let trimmed = trimmed
        .strip_suffix("/embeddings")
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    trimmed.to_owned()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn openai_compat_embedding_provider_posts_to_embeddings_endpoint_and_parses_response() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let endpoint = format!(
            "http://{}/v1/embeddings/",
            listener.local_addr().expect("local addr")
        );

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            let request_lower = request.to_ascii_lowercase();
            assert!(request.starts_with("POST /v1/embeddings "));
            assert!(request_lower.contains("authorization: bearer test-key"));
            assert!(request.contains("\"model\":\"text-embedding-3-large\""));
            assert!(request.contains("\"input\":\"hello embeddings\""));
            assert!(request.contains("\"task_type\":\"CODE_RETRIEVAL\""));
            assert!(request.contains("\"dimensions\":3072"));

            let response_body = "{\"data\":[{\"embedding\":[3,4]}]}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );

            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let provider = OpenAiCompatEmbeddingProvider::new(
            endpoint,
            "text-embedding-3-large".to_owned(),
            Secret::new("test-key".to_owned()),
            Some("CODE_RETRIEVAL".to_owned()),
            Some(3072),
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let embedding = runtime
            .block_on(provider.embed_text("hello embeddings"))
            .expect("request embedding");

        assert_eq!(embedding.len(), 2);
        assert!((embedding[0] - 0.6).abs() < 1e-6);
        assert!((embedding[1] - 0.8).abs() < 1e-6);
        server.join().expect("join server");
    }

    #[test]
    fn openai_compat_embedding_provider_ignores_purpose_parameter() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let endpoint = format!(
            "http://{}/v1/embeddings/",
            listener.local_addr().expect("local addr")
        );

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            assert!(request.contains("\"task_type\":\"CODE_RETRIEVAL\""));

            let response_body = "{\"data\":[{\"embedding\":[1,0]}]}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );

            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let provider = OpenAiCompatEmbeddingProvider::new(
            endpoint,
            "text-embedding-3-large".to_owned(),
            Secret::new("test-key".to_owned()),
            Some("CODE_RETRIEVAL".to_owned()),
            None,
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let embedding = runtime
            .block_on(
                provider
                    .embed_text_with_purpose("hello embeddings", crate::EmbeddingPurpose::Query),
            )
            .expect("request embedding");

        assert_eq!(embedding, vec![1.0, 0.0]);
        server.join().expect("join server");
    }
}
