use aether_config::{DEFAULT_QWEN_ENDPOINT, DEFAULT_QWEN_MODEL, InferenceProviderKind};
use async_trait::async_trait;
use serde_json::Value;

use crate::http::{
    build_ollama_deep_generate_body, build_ollama_generate_body, build_ollama_text_generate_body,
    inference_http_client, ollama_generate_endpoint,
};
use crate::providers::PARSE_VALIDATION_RETRIES;
use crate::sir_parsing::{
    build_retry_prompt, extract_local_text_part, normalize_candidate_json,
    run_sir_parse_validation_retries, run_sir_parse_validation_retries_with_feedback,
};
use crate::sir_prompt;
use crate::types::{InferError, InferenceProvider, SirContext, normalize_optional};

#[derive(Debug, Clone)]
pub struct Qwen3LocalProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
}

impl Qwen3LocalProvider {
    pub fn new(endpoint: Option<String>, model: Option<String>) -> Self {
        Self {
            client: inference_http_client(),
            endpoint: normalize_optional(endpoint)
                .unwrap_or_else(|| DEFAULT_QWEN_ENDPOINT.to_owned()),
            model: normalize_optional(model).unwrap_or_else(|| DEFAULT_QWEN_MODEL.to_owned()),
        }
    }

    async fn request_candidate_json_with_prompt(&self, prompt: &str) -> Result<String, InferError> {
        let body = build_ollama_generate_body(&self.model, prompt, 4096);

        let response_value: Value = self
            .client
            .post(ollama_generate_endpoint(&self.endpoint))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        extract_local_text_part(&response_value)
    }

    async fn request_deep_candidate_json_with_prompt(
        &self,
        prompt: String,
    ) -> Result<String, InferError> {
        let body = build_ollama_deep_generate_body(&self.model, &prompt, 8192);

        let response_value: Value = self
            .client
            .post(ollama_generate_endpoint(&self.endpoint))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let raw = extract_local_text_part(&response_value)?;
        Ok(normalize_candidate_json(raw.as_str()))
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
