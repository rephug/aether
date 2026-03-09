use aether_config::EmbeddingProviderKind;
use aether_core::Secret;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
    EmbeddingProvider, InferError, extract_embedding_vector, inference_http_client,
    normalize_openai_api_base,
};

#[derive(Debug, Clone)]
pub struct OpenAiCompatEmbeddingProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: Secret,
}

impl OpenAiCompatEmbeddingProvider {
    pub fn new(endpoint: String, model: String, api_key: Secret) -> Self {
        Self {
            client: inference_http_client(),
            endpoint: normalize_embedding_endpoint(&endpoint),
            model: model.trim().to_owned(),
            api_key,
        }
    }

    pub(crate) fn provider_name(&self) -> &'static str {
        EmbeddingProviderKind::OpenAiCompat.as_str()
    }

    pub(crate) fn model_name(&self) -> &str {
        self.model.as_str()
    }

    async fn request_embedding(&self, text: &str) -> Result<Vec<f32>, InferError> {
        let url = format!("{}/embeddings", self.endpoint);
        let body = json!({
            "model": self.model,
            "input": text,
        });

        let response: Value = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key.expose()))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        extract_embedding_vector(&response)
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
}
