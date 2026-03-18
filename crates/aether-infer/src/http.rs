use serde::Deserialize;
use serde_json::{Value, json};

use crate::types::{InferError, OllamaPullProgress};

pub(crate) const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const OLLAMA_PULL_SUCCESS_STATUS: &str = "success";

pub(crate) fn inference_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn management_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub(crate) async fn is_ollama_reachable(endpoint: &str) -> bool {
    let url = format!("{}/api/ps", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(1))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    client
        .get(&url)
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

pub(crate) fn is_ollama_reachable_blocking(endpoint: &str) -> bool {
    let endpoint = endpoint.to_owned();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|runtime| runtime.block_on(is_ollama_reachable(&endpoint)))
            .unwrap_or(false)
    })
    .join()
    .unwrap_or(false)
}

pub fn normalize_ollama_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return aether_config::DEFAULT_QWEN_ENDPOINT.to_owned();
    }

    let known_suffixes = ["/api/generate", "/api/tags", "/api/pull", "/api/embeddings"];

    for suffix in known_suffixes {
        if let Some(stripped) = trimmed.strip_suffix(suffix) {
            let normalized = stripped.trim_end_matches('/');
            if normalized.is_empty() {
                return aether_config::DEFAULT_QWEN_ENDPOINT.to_owned();
            }
            return normalized.to_owned();
        }
    }

    trimmed.to_owned()
}

pub fn ollama_generate_endpoint(endpoint: &str) -> String {
    format!("{}/api/generate", normalize_ollama_endpoint(endpoint))
}

pub fn ollama_tags_endpoint(endpoint: &str) -> String {
    format!("{}/api/tags", normalize_ollama_endpoint(endpoint))
}

pub fn ollama_pull_endpoint(endpoint: &str) -> String {
    format!("{}/api/pull", normalize_ollama_endpoint(endpoint))
}

pub async fn fetch_ollama_tags(endpoint: &str) -> Result<Value, InferError> {
    let response_value = management_http_client()
        .get(ollama_tags_endpoint(endpoint))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(response_value)
}

pub async fn pull_ollama_model_with_progress<F>(
    endpoint: &str,
    model: &str,
    mut on_progress: F,
) -> Result<(), InferError>
where
    F: FnMut(OllamaPullProgress),
{
    let mut response = management_http_client()
        .post(ollama_pull_endpoint(endpoint))
        .json(&json!({
            "model": model,
            "stream": true
        }))
        .send()
        .await?
        .error_for_status()?;

    let mut buffered = String::new();
    while let Some(chunk) = response.chunk().await? {
        buffered.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buffered.find('\n') {
            let line = buffered[..idx].trim().to_owned();
            buffered = buffered[idx + 1..].to_owned();
            if line.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line)?;
            on_progress(parse_pull_progress(&value));
        }
    }

    let trailing = buffered.trim();
    if !trailing.is_empty() {
        let value: Value = serde_json::from_str(trailing)?;
        on_progress(parse_pull_progress(&value));
    }

    Ok(())
}

pub(crate) fn build_ollama_generate_body(model: &str, prompt: &str, num_ctx: u32) -> Value {
    json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "format": "json",
        "think": false,
        "options": {
            "temperature": aether_config::OLLAMA_SIR_TEMPERATURE,
            "num_ctx": num_ctx
        }
    })
}

pub(crate) fn build_ollama_text_generate_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": aether_config::OLLAMA_SIR_TEMPERATURE
        }
    })
}

pub(crate) fn build_ollama_deep_generate_body(model: &str, prompt: &str, num_ctx: u32) -> Value {
    json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": 0.3,
            "num_ctx": num_ctx
        }
    })
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatChatCompletionResponse {
    choices: Vec<OpenAiCompatChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatChoice {
    message: OpenAiCompatMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatErrorEnvelope {
    error: Option<OpenAiCompatErrorBody>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatErrorBody {
    message: Option<String>,
}

pub(crate) fn build_openai_chat_completion_body(
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    include_response_format: bool,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ],
        "temperature": 0.0
    });

    if include_response_format && let Some(body_obj) = body.as_object_mut() {
        body_obj.insert("response_format".to_owned(), json!({"type": "json_object"}));
    }

    body
}

pub(crate) fn response_indicates_unsupported_json_mode(response_body: &str) -> bool {
    let lower = response_body.to_ascii_lowercase();
    lower.contains("response_format") || lower.contains("json_object")
}

pub(crate) fn extract_openai_error_message(response_body: &str) -> Option<String> {
    let envelope: OpenAiCompatErrorEnvelope = serde_json::from_str(response_body).ok()?;
    envelope.error?.message.and_then(|message| {
        let trimmed = message.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

pub(crate) fn extract_openai_chat_content_from_body(
    response_body: &str,
) -> Result<String, InferError> {
    let response: OpenAiCompatChatCompletionResponse = serde_json::from_str(response_body)?;
    let first = response
        .choices
        .first()
        .ok_or_else(|| InferError::InvalidResponse("missing choices[0]".to_owned()))?;
    let content = first.message.content.as_deref().ok_or_else(|| {
        InferError::InvalidResponse("missing choices[0].message.content".to_owned())
    })?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(InferError::InvalidResponse(
            "empty choices[0].message.content".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn parse_pull_progress(value: &Value) -> OllamaPullProgress {
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let completed = value.get("completed").and_then(Value::as_u64);
    let total = value.get("total").and_then(Value::as_u64);
    let done = value.get("done").and_then(Value::as_bool).unwrap_or(false)
        || status.eq_ignore_ascii_case(OLLAMA_PULL_SUCCESS_STATUS);

    OllamaPullProgress {
        status,
        completed,
        total,
        done,
    }
}

pub(crate) fn extract_gemini_text_part(response: &Value) -> Result<&str, InferError> {
    response
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .and_then(|parts| parts.first())
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            InferError::InvalidResponse("missing candidates[0].content.parts[0].text".to_owned())
        })
}

pub(crate) fn extract_embedding_vector(response: &Value) -> Result<Vec<f32>, InferError> {
    if let Some(vector) = value_to_embedding_vector(response) {
        return Ok(vector);
    }

    let candidate_paths = [
        "/embedding",
        "/data/0/embedding",
        "/embeddings/0/embedding",
        "/vector",
    ];

    for path in candidate_paths {
        if let Some(value) = response.pointer(path)
            && let Some(vector) = value_to_embedding_vector(value)
        {
            return Ok(vector);
        }
    }

    Err(InferError::InvalidEmbeddingResponse(
        "missing embedding vector in local model response body".to_owned(),
    ))
}

fn value_to_embedding_vector(value: &Value) -> Option<Vec<f32>> {
    let values = value.as_array()?;
    if values.is_empty() {
        return None;
    }

    let mut embedding = Vec::with_capacity(values.len());
    for item in values {
        let number = item.as_f64()?;
        if !number.is_finite() {
            return None;
        }
        embedding.push(number as f32);
    }

    normalize_embedding(embedding)
}

pub(crate) fn normalize_embedding(mut embedding: Vec<f32>) -> Option<Vec<f32>> {
    let norm_sq = embedding
        .iter()
        .map(|value| value * value)
        .fold(0.0f32, |acc, value| acc + value);
    if norm_sq <= f32::EPSILON {
        return None;
    }

    let norm = norm_sq.sqrt();
    for value in &mut embedding {
        *value /= norm;
    }

    Some(embedding)
}

pub(crate) fn normalize_openai_api_base(api_base: &str) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');
    let trimmed = trimmed
        .strip_suffix("/chat/completions")
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    trimmed.to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn qwen3_local_generate_body_sets_temperature_in_options() {
        let body = build_ollama_generate_body("qwen2.5-coder:7b", "return strict json", 4096);
        assert_eq!(
            body.pointer("/options/temperature"),
            Some(&json!(aether_config::OLLAMA_SIR_TEMPERATURE))
        );
        assert_eq!(body.pointer("/temperature"), None);
    }

    #[test]
    fn qwen3_local_deep_generate_body_enables_larger_context_without_json_format() {
        let body = build_ollama_deep_generate_body("qwen3.5:4b", "analyze deeply", 8192);
        assert_eq!(body.pointer("/options/temperature"), Some(&json!(0.3)));
        assert_eq!(body.pointer("/options/num_ctx"), Some(&json!(8192)));
        assert_eq!(body.pointer("/format"), None);
        assert_eq!(body.pointer("/think"), None);
    }

    #[test]
    fn extract_openai_chat_content_returns_content_for_valid_response() {
        let response = r#"{
            "choices": [
                {
                    "message": {
                        "content": "{\"intent\":\"ok\"}"
                    }
                }
            ]
        }"#;

        let content = extract_openai_chat_content_from_body(response).expect("valid response");
        assert_eq!(content, r#"{"intent":"ok"}"#);
    }

    #[test]
    fn extract_openai_chat_content_rejects_empty_choices() {
        let response = r#"{"choices":[]}"#;
        let error = extract_openai_chat_content_from_body(response)
            .expect_err("choices should be required");
        match error {
            InferError::InvalidResponse(message) => assert!(message.contains("choices[0]")),
            _ => panic!("expected invalid response"),
        }
    }

    #[test]
    fn extract_openai_chat_content_rejects_missing_content() {
        let response = r#"{
            "choices": [
                {
                    "message": {}
                }
            ]
        }"#;

        let error = extract_openai_chat_content_from_body(response)
            .expect_err("message content should be required");
        match error {
            InferError::InvalidResponse(message) => {
                assert!(message.contains("choices[0].message.content"))
            }
            _ => panic!("expected invalid response"),
        }
    }
}
