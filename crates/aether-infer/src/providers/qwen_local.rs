use aether_config::{
    DEFAULT_QWEN_ENDPOINT, DEFAULT_QWEN_MODEL, InferenceProviderKind, OLLAMA_DEEP_SIR_NUM_PREDICT,
    OLLAMA_SIR_NUM_PREDICT,
};
use async_trait::async_trait;
use serde_json::Value;

use crate::http::{
    build_ollama_deep_generate_body, build_ollama_generate_body, build_ollama_text_generate_body,
    inference_http_client, inference_http_client_with_timeout, ollama_generate_endpoint,
};
use crate::providers::PARSE_VALIDATION_RETRIES;
use crate::sir_parsing::{
    build_retry_prompt, extract_local_text_part, normalize_candidate_json, parse_and_validate_sir,
    run_sir_parse_validation_retries, run_sir_parse_validation_retries_with_feedback,
    split_think_block,
};
use crate::sir_prompt;
use crate::types::{InferError, InferSirResult, InferenceProvider, SirContext, normalize_optional};

/// A deep-mode Ollama reply split into the SIR JSON candidate and the model's
/// `<think>` reasoning, when the model produced one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeepCandidate {
    pub candidate_json: String,
    pub reasoning_trace: Option<String>,
}

/// Separate the `<think>...</think>` block from a raw deep-mode reply before the bracket
/// parser sees it, so the reasoning is kept as a trace instead of being discarded.
pub(crate) fn parse_deep_response(raw: &str) -> DeepCandidate {
    let (reasoning_trace, remainder) = split_think_block(raw);
    DeepCandidate {
        candidate_json: normalize_candidate_json(remainder.as_str()),
        reasoning_trace: normalize_optional(reasoning_trace),
    }
}

#[derive(Debug, Clone)]
pub struct Qwen3LocalProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    max_output_tokens: u32,
}

impl Qwen3LocalProvider {
    pub fn new(endpoint: Option<String>, model: Option<String>) -> Self {
        Self {
            client: inference_http_client(),
            endpoint: normalize_optional(endpoint)
                .unwrap_or_else(|| DEFAULT_QWEN_ENDPOINT.to_owned()),
            model: normalize_optional(model).unwrap_or_else(|| DEFAULT_QWEN_MODEL.to_owned()),
            max_output_tokens: OLLAMA_SIR_NUM_PREDICT,
        }
    }

    /// Allow each request this long instead of the default 120 s, so a configured
    /// `[inference] timeout_secs` above that isn't cut short by the HTTP client.
    pub fn with_request_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.client = inference_http_client_with_timeout(timeout_secs);
        self
    }

    /// Cap each SIR reply at this many generated tokens instead of the default 2048, as set
    /// by `[inference] max_output_tokens`. Deep-mode replies keep their own cap.
    pub fn with_max_output_tokens(mut self, max_output_tokens: u32) -> Self {
        self.max_output_tokens = max_output_tokens.max(1);
        self
    }

    pub(crate) fn sir_request_body(&self, prompt: &str) -> Value {
        build_ollama_generate_body(&self.model, prompt, 4096, self.max_output_tokens)
    }

    fn deep_sir_request_body(&self, prompt: &str) -> Value {
        build_ollama_deep_generate_body(&self.model, prompt, 8192, OLLAMA_DEEP_SIR_NUM_PREDICT)
    }

    async fn request_candidate_json_with_prompt(&self, prompt: &str) -> Result<String, InferError> {
        let body = self.sir_request_body(prompt);

        let response_value: Value = self
            .client
            .post(ollama_generate_endpoint(&self.endpoint))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        self.warn_if_reply_hit_token_cap(&response_value, self.max_output_tokens);
        extract_local_text_part(&response_value)
    }

    /// Ollama reports `done_reason: "length"` when a reply stopped at `num_predict`. Its JSON
    /// is then almost certainly cut off, and this explains the parse retry that follows.
    fn warn_if_reply_hit_token_cap(&self, response_value: &Value, max_output_tokens: u32) {
        if response_value.get("done_reason").and_then(Value::as_str) == Some("length") {
            tracing::warn!(
                model = %self.model,
                max_output_tokens,
                "local SIR reply stopped at the output token cap"
            );
        }
    }

    async fn request_deep_candidate_with_prompt(
        &self,
        prompt: &str,
    ) -> Result<DeepCandidate, InferError> {
        let body = self.deep_sir_request_body(prompt);

        let response_value: Value = self
            .client
            .post(ollama_generate_endpoint(&self.endpoint))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        self.warn_if_reply_hit_token_cap(&response_value, OLLAMA_DEEP_SIR_NUM_PREDICT);
        let raw = extract_local_text_part(&response_value)?;
        Ok(parse_deep_response(raw.as_str()))
    }

    async fn request_deep_candidate_json_with_prompt(
        &self,
        prompt: String,
    ) -> Result<String, InferError> {
        Ok(self
            .request_deep_candidate_with_prompt(prompt.as_str())
            .await?
            .candidate_json)
    }

    /// Deep-mode generation that keeps the `<think>` reasoning as `reasoning_trace`,
    /// mirroring the Gemini provider's thinking capture. Parse/validation failures retry
    /// the same way the fast path does.
    async fn generate_deep_sir_result_from_prompt(
        &self,
        prompt: &str,
    ) -> Result<InferSirResult, InferError> {
        let mut last_error = String::from("unknown parse/validation failure");

        for attempt in 0..=PARSE_VALIDATION_RETRIES {
            let candidate = self.request_deep_candidate_with_prompt(prompt).await?;
            match parse_and_validate_sir(candidate.candidate_json.as_str()) {
                Ok(sir) => {
                    return Ok(InferSirResult {
                        sir,
                        provider: self.provider_name(),
                        model: self.model_name(),
                        reasoning_trace: candidate.reasoning_trace,
                    });
                }
                Err(message) => {
                    last_error = message;
                    if attempt == PARSE_VALIDATION_RETRIES {
                        break;
                    }
                }
            }
        }

        Err(InferError::ParseValidationExhausted(last_error))
    }

    async fn request_candidate_json(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<String, InferError> {
        let prompt = sir_prompt::build_sir_prompt_for_kind(symbol_text, context);
        self.request_candidate_json_with_prompt(prompt.as_str())
            .await
    }
}

#[async_trait]
impl InferenceProvider for Qwen3LocalProvider {
    fn provider_name(&self) -> String {
        InferenceProviderKind::Qwen3Local.as_str().to_owned()
    }

    fn model_name(&self) -> String {
        self.model.clone()
    }

    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<aether_sir::SirAnnotation, InferError> {
        let original_prompt = sir_prompt::build_sir_prompt_for_kind(symbol_text, context);
        run_sir_parse_validation_retries_with_feedback(
            PARSE_VALIDATION_RETRIES,
            || async { self.request_candidate_json(symbol_text, context).await },
            |previous_output, error| {
                let prompt = build_retry_prompt(&original_prompt, &error, &previous_output);
                async move {
                    self.request_candidate_json_with_prompt(prompt.as_str())
                        .await
                }
            },
        )
        .await
    }

    async fn generate_sir_from_prompt(
        &self,
        prompt: &str,
        _context: &SirContext,
        deep_mode: bool,
    ) -> Result<aether_sir::SirAnnotation, InferError> {
        if deep_mode {
            run_sir_parse_validation_retries(PARSE_VALIDATION_RETRIES, || async {
                self.request_deep_candidate_json_with_prompt(prompt.to_owned())
                    .await
            })
            .await
        } else {
            run_sir_parse_validation_retries(PARSE_VALIDATION_RETRIES, || async {
                self.request_candidate_json_with_prompt(prompt).await
            })
            .await
        }
    }

    async fn generate_sir_from_prompt_with_meta(
        &self,
        prompt: &str,
        context: &SirContext,
        deep_mode: bool,
    ) -> Result<InferSirResult, InferError> {
        if deep_mode {
            // Deep mode is where Ollama emits its <think> block; keep it as the trace.
            return self.generate_deep_sir_result_from_prompt(prompt).await;
        }
        // Fast mode is unchanged: no reasoning trace.
        let sir = self
            .generate_sir_from_prompt(prompt, context, false)
            .await?;
        Ok(InferSirResult {
            sir,
            provider: self.provider_name(),
            model: self.model_name(),
            reasoning_trace: None,
        })
    }
}

pub(crate) async fn request_qwen_summary(
    endpoint: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, InferError> {
    let prompt = format!(
        "System instruction:\n{system_prompt}\n\nUser prompt:\n{user_prompt}\n\nReturn exactly one concise sentence."
    );
    let body = build_ollama_text_generate_body(model, prompt.as_str());
    let response_value: Value = inference_http_client()
        .post(ollama_generate_endpoint(endpoint))
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    extract_local_text_part(&response_value)
}

#[cfg(test)]
mod deep_response_tests {
    use super::parse_deep_response;
    use crate::sir_parsing::parse_and_validate_sir;

    const CANNED_DEEP_RESPONSE: &str = "<think>\nThe function opens the file, so io errors are the main failure mode.\nConfidence should be moderate.\n</think>\n```json\n{\"intent\":\"Loads the configuration file\",\"inputs\":[\"path\"],\"outputs\":[\"Config\"],\"side_effects\":[],\"dependencies\":[\"std::fs\"],\"error_modes\":[\"io error\"],\"confidence\":0.7,}\n```";

    #[test]
    fn deep_response_keeps_think_block_as_reasoning_and_json_still_parses() {
        let candidate = parse_deep_response(CANNED_DEEP_RESPONSE);
        let reasoning = candidate
            .reasoning_trace
            .as_deref()
            .expect("think block must be captured");
        assert!(reasoning.starts_with("The function opens the file"));
        assert!(reasoning.contains("Confidence should be moderate."));
        assert!(!candidate.candidate_json.contains("<think>"));
        let sir = parse_and_validate_sir(candidate.candidate_json.as_str())
            .expect("SIR JSON must still parse after the think block is removed");
        assert_eq!(sir.intent, "Loads the configuration file");
        assert!((sir.confidence - 0.7).abs() < 1e-6);
    }

    #[test]
    fn deep_response_without_think_block_has_no_reasoning() {
        let candidate = parse_deep_response(
            r#"{"intent":"x","inputs":[],"outputs":[],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.5}"#,
        );
        assert_eq!(candidate.reasoning_trace, None);
        assert!(parse_and_validate_sir(candidate.candidate_json.as_str()).is_ok());
    }
}

#[cfg(test)]
mod request_body_tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    use aether_config::{OLLAMA_DEEP_SIR_NUM_PREDICT, OLLAMA_SIR_NUM_PREDICT};
    use serde_json::{Value, json};

    use super::Qwen3LocalProvider;
    use crate::sir_prompt;
    use crate::types::{InferenceProvider, SirContext};

    const SIR_REPLY: &str = r#"{"intent":"Adds two numbers","inputs":["a","b"],"outputs":["sum"],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.8}"#;
    const SYMBOL_TEXT: &str = "def add(a, b):\n    return a + b\n";

    fn context() -> SirContext {
        SirContext {
            language: "python".to_owned(),
            file_path: "calc.py".to_owned(),
            qualified_name: "calc::add".to_owned(),
            priority_score: None,
            kind: "function".to_owned(),
            is_public: true,
            line_count: 2,
        }
    }

    /// A stand-in Ollama that answers one `/api/generate` request with a valid SIR and
    /// returns the JSON body it received.
    fn serve_one_generate_request() -> (String, JoinHandle<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test ollama listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept generate request");
            let mut reader = BufReader::new(stream);
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("read request header");
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    content_length = value.trim().parse().expect("content length");
                }
            }
            let mut body = vec![0_u8; content_length];
            reader.read_exact(&mut body).expect("read request body");

            let reply =
                json!({"response": SIR_REPLY, "done": true, "done_reason": "stop"}).to_string();
            let mut stream = reader.into_inner();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .expect("write reply");
            serde_json::from_slice(&body).expect("request body is JSON")
        });
        (endpoint, server)
    }

    #[tokio::test]
    async fn sir_request_caps_generated_tokens_and_keeps_the_prompt() {
        let (endpoint, server) = serve_one_generate_request();
        let provider = Qwen3LocalProvider::new(Some(endpoint), Some("phi4-mini".to_owned()));

        let sir = provider
            .generate_sir(SYMBOL_TEXT, &context())
            .await
            .expect("generate sir");
        let body = server.join().expect("join test ollama");

        assert_eq!(sir.intent, "Adds two numbers");
        assert_eq!(
            body.pointer("/options/num_predict"),
            Some(&json!(OLLAMA_SIR_NUM_PREDICT))
        );
        assert_eq!(body.pointer("/options/num_ctx"), Some(&json!(4096)));
        assert_eq!(body.pointer("/format"), Some(&json!("json")));
        let expected_prompt = sir_prompt::build_sir_prompt_for_kind(SYMBOL_TEXT, &context());
        assert_eq!(body.pointer("/prompt"), Some(&json!(expected_prompt)));
    }

    #[tokio::test]
    async fn configured_cap_reaches_the_sir_request() {
        let (endpoint, server) = serve_one_generate_request();
        let provider = Qwen3LocalProvider::new(Some(endpoint), Some("phi4-mini".to_owned()))
            .with_max_output_tokens(1024);

        provider
            .generate_sir_from_prompt("return strict json", &context(), false)
            .await
            .expect("generate sir");
        let body = server.join().expect("join test ollama");

        assert_eq!(body.pointer("/options/num_predict"), Some(&json!(1024)));
        assert_eq!(body.pointer("/prompt"), Some(&json!("return strict json")));
    }

    #[tokio::test]
    async fn deep_request_uses_the_deep_cap() {
        let (endpoint, server) = serve_one_generate_request();
        let provider = Qwen3LocalProvider::new(Some(endpoint), Some("qwen3.5:4b".to_owned()))
            .with_max_output_tokens(1024);

        provider
            .generate_sir_from_prompt_with_meta("analyze deeply", &context(), true)
            .await
            .expect("generate deep sir");
        let body = server.join().expect("join test ollama");

        assert_eq!(
            body.pointer("/options/num_predict"),
            Some(&json!(OLLAMA_DEEP_SIR_NUM_PREDICT))
        );
        assert_eq!(body.pointer("/options/num_ctx"), Some(&json!(8192)));
        assert_eq!(body.pointer("/format"), None);
    }
}
