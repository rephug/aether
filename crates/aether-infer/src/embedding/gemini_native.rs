use std::time::Duration;

use aether_config::EmbeddingProviderKind;
use aether_core::Secret;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::time::sleep;

use crate::http::{GEMINI_API_BASE, inference_http_client, normalize_embedding};
use crate::types::{EmbeddingProvider, EmbeddingPurpose, InferError};

#[derive(Debug, Clone)]
pub struct GeminiNativeEmbeddingProvider {
    client: reqwest::Client,
    api_base: String,
    model: String,
    api_key: Secret,
    dimensions: Option<u32>,
}

impl GeminiNativeEmbeddingProvider {
    pub fn new(model: String, api_key: Secret, dimensions: Option<u32>) -> Self {
        Self::new_with_api_base(GEMINI_API_BASE.to_owned(), model, api_key, dimensions)
    }

    pub(crate) fn new_with_api_base(
        api_base: String,
        model: String,
        api_key: Secret,
        dimensions: Option<u32>,
    ) -> Self {
        Self {
            client: inference_http_client(),
            api_base: api_base.trim_end_matches('/').to_owned(),
            model: model.trim().to_owned(),
            api_key,
            dimensions,
        }
    }

    pub(crate) fn provider_name(&self) -> &'static str {
        EmbeddingProviderKind::GeminiNative.as_str()
    }

    pub(crate) fn model_name(&self) -> &str {
        self.model.as_str()
    }

    fn endpoint_url(&self) -> String {
        format!("{}/models/{}:embedContent", self.api_base, self.model)
    }

    async fn request_embedding_once(
        &self,
        text: &str,
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<f32>, InferError> {
        let task_type = match purpose {
            EmbeddingPurpose::Document => "RETRIEVAL_DOCUMENT",
            EmbeddingPurpose::Query => "CODE_RETRIEVAL_QUERY",
        };
        let mut body = json!({
            "content": {
                "parts": [{"text": text}]
            },
            "taskType": task_type,
        });
        if let Some(dimensions) = self.dimensions {
            body["outputDimensionality"] = json!(dimensions);
        }

        let response = self
            .client
            .post(self.endpoint_url())
            .header("x-goog-api-key", self.api_key.expose())
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let response_body = response.text().await?;
        if !status.is_success() {
            let provider_message = extract_gemini_error_message(&response_body)
                .unwrap_or_else(|| response_body.trim().to_owned());
            let provider_message = if provider_message.is_empty() {
                "unknown provider error".to_owned()
            } else {
                provider_message
            };
            return Err(InferError::InvalidResponse(format!(
                "gemini_native request failed with status {status}: {provider_message}"
            )));
        }

        let response: Value = serde_json::from_str(&response_body)?;
        extract_native_embedding_vector(&response)
    }

    async fn request_embedding(
        &self,
        text: &str,
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<f32>, InferError> {
        let mut backoff = Duration::from_secs(1);
        for attempt in 0..3 {
            match self.request_embedding_once(text, purpose).await {
                Ok(embedding) => return Ok(embedding),
                Err(err) if attempt == 2 => return Err(err),
                Err(err) => {
                    tracing::warn!(
                        attempt = attempt + 1,
                        error = %err,
                        "gemini_native embedding request failed; retrying"
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
impl EmbeddingProvider for GeminiNativeEmbeddingProvider {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, InferError> {
        self.request_embedding(text, EmbeddingPurpose::Document)
            .await
    }

    async fn embed_text_with_purpose(
        &self,
        text: &str,
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<f32>, InferError> {
        self.request_embedding(text, purpose).await
    }
}

fn extract_gemini_error_message(response_body: &str) -> Option<String> {
    let response: Value = serde_json::from_str(response_body).ok()?;
    response
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_native_embedding_vector(response: &Value) -> Result<Vec<f32>, InferError> {
    let values = response
        .pointer("/embedding/values")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            InferError::InvalidEmbeddingResponse(
                "missing embedding.values vector in gemini_native response body".to_owned(),
            )
        })?;

    let mut embedding = Vec::with_capacity(values.len());
    for item in values {
        let value = item.as_f64().ok_or_else(|| {
            InferError::InvalidEmbeddingResponse(
                "gemini_native embedding values must be finite numbers".to_owned(),
            )
        })?;
        if !value.is_finite() {
            return Err(InferError::InvalidEmbeddingResponse(
                "gemini_native embedding values must be finite numbers".to_owned(),
            ));
        }
        embedding.push(value as f32);
    }

    normalize_embedding(embedding).ok_or_else(|| {
        InferError::InvalidEmbeddingResponse(
            "gemini_native embedding vector must contain non-zero values".to_owned(),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    fn test_provider(endpoint: String, dimensions: Option<u32>) -> GeminiNativeEmbeddingProvider {
        GeminiNativeEmbeddingProvider::new_with_api_base(
            endpoint,
            "gemini-embedding-2-preview".to_owned(),
            Secret::new("test-key".to_owned()),
            dimensions,
        )
    }

    #[test]
    fn gemini_native_sends_correct_request_format() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let endpoint = format!(
            "http://{}/v1beta",
            listener.local_addr().expect("local addr")
        );

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            let request_lower = request.to_ascii_lowercase();
            assert!(
                request.starts_with("POST /v1beta/models/gemini-embedding-2-preview:embedContent ")
            );
            assert!(request_lower.contains("x-goog-api-key: test-key"));
            assert!(!request_lower.contains("authorization: bearer"));
            assert!(request.contains("\"text\":\"hello embeddings\""));
            assert!(request.contains("\"taskType\":\"RETRIEVAL_DOCUMENT\""));
            assert!(request.contains("\"outputDimensionality\":3072"));

            let response_body = "{\"embedding\":{\"values\":[3,4]}}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );

            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let provider = test_provider(endpoint, Some(3072));
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
    fn gemini_native_document_purpose_sends_retrieval_document() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let endpoint = format!(
            "http://{}/v1beta",
            listener.local_addr().expect("local addr")
        );

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            assert!(request.contains("\"taskType\":\"RETRIEVAL_DOCUMENT\""));

            let response_body = "{\"embedding\":{\"values\":[1,0]}}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let provider = test_provider(endpoint, None);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime
            .block_on(provider.embed_text_with_purpose("doc", EmbeddingPurpose::Document))
            .expect("request embedding");
        server.join().expect("join server");
    }

    #[test]
    fn gemini_native_query_purpose_sends_code_retrieval_query() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let endpoint = format!(
            "http://{}/v1beta",
            listener.local_addr().expect("local addr")
        );

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            assert!(request.contains("\"taskType\":\"CODE_RETRIEVAL_QUERY\""));

            let response_body = "{\"embedding\":{\"values\":[1,0]}}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let provider = test_provider(endpoint, None);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime
            .block_on(provider.embed_text_with_purpose("query", EmbeddingPurpose::Query))
            .expect("request embedding");
        server.join().expect("join server");
    }

    #[test]
    fn gemini_native_default_embed_uses_document_purpose() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let endpoint = format!(
            "http://{}/v1beta",
            listener.local_addr().expect("local addr")
        );

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            assert!(request.contains("\"taskType\":\"RETRIEVAL_DOCUMENT\""));

            let response_body = "{\"embedding\":{\"values\":[1,0]}}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let provider = test_provider(endpoint, None);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime
            .block_on(provider.embed_text("default"))
            .expect("request embedding");
        server.join().expect("join server");
    }

    #[test]
    fn gemini_native_includes_output_dimensionality() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let endpoint = format!(
            "http://{}/v1beta",
            listener.local_addr().expect("local addr")
        );

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            assert!(request.contains("\"outputDimensionality\":3072"));

            let response_body = "{\"embedding\":{\"values\":[1,0]}}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let provider = test_provider(endpoint, Some(3072));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime
            .block_on(provider.embed_text("default"))
            .expect("request embedding");
        server.join().expect("join server");
    }

    #[test]
    fn gemini_native_parses_native_response() {
        let parsed = extract_native_embedding_vector(&json!({
            "embedding": {
                "values": [3.0, 4.0]
            }
        }))
        .expect("parse embedding");

        assert_eq!(parsed.len(), 2);
        assert!((parsed[0] - 0.6).abs() < 1e-6);
        assert!((parsed[1] - 0.8).abs() < 1e-6);
    }
}
