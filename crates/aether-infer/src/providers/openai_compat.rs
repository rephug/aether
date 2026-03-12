use std::sync::atomic::{AtomicBool, Ordering};

use aether_config::InferenceProviderKind;
use aether_core::Secret;
use async_trait::async_trait;

use crate::http::{
    build_openai_chat_completion_body, extract_openai_chat_content_from_body,
    extract_openai_error_message, inference_http_client, normalize_openai_api_base,
    response_indicates_unsupported_json_mode,
};
use crate::providers::PARSE_VALIDATION_RETRIES;
use crate::sir_parsing::run_sir_parse_validation_retries;
use crate::sir_prompt;
use crate::types::{InferError, InferenceProvider, SirContext};

const OPENAI_COMPAT_JSON_FALLBACK_SUFFIX: &str =
    "\n\nRespond with ONLY valid JSON. No markdown, no explanation, no code fences.";
const OPENAI_COMPAT_SIR_SYSTEM_PROMPT: &str =
    "You are generating Structured Intent Records for source code.";

pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    api_base: String,
    api_key: Secret,
    model: String,
    json_mode_supported: AtomicBool,
}

impl std::fmt::Debug for OpenAiCompatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatProvider")
            .field("client", &self.client)
            .field("api_base", &self.api_base)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field(
                "json_mode_supported",
                &self.json_mode_supported.load(Ordering::Relaxed),
            )
            .finish()
    }
}

impl Clone for OpenAiCompatProvider {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            api_base: self.api_base.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            json_mode_supported: AtomicBool::new(self.json_mode_supported.load(Ordering::Relaxed)),
        }
    }
}

impl OpenAiCompatProvider {
    pub fn new(api_key: Secret, api_base: String, model: String) -> Self {
        Self {
            client: inference_http_client(),
            api_base: normalize_openai_api_base(&api_base),
            api_key,
            model,
            json_mode_supported: AtomicBool::new(true),
        }
    }

    fn endpoint_url(&self) -> String {
        format!("{}/chat/completions", self.api_base)
    }

    async fn request_chat_completion(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        include_response_format: bool,
    ) -> Result<String, InferError> {
        let body = build_openai_chat_completion_body(
            &self.model,
            system_prompt,
            user_prompt,
            include_response_format,
        );
        let response = self
            .client
            .post(self.endpoint_url())
            .header("Authorization", format!("Bearer {}", self.api_key.expose()))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let response_body = response.text().await?;
        if !status.is_success() {
            if include_response_format
                && (status == reqwest::StatusCode::BAD_REQUEST
                    || status == reqwest::StatusCode::UNPROCESSABLE_ENTITY)
                && response_indicates_unsupported_json_mode(&response_body)
            {
                return Err(InferError::ProviderRejectedFormat);
            }

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

        extract_openai_chat_content_from_body(&response_body)
    }

    async fn request_candidate_json(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<String, InferError> {
        let user_prompt = sir_prompt::build_sir_prompt_for_kind(symbol_text, context);
        self.request_candidate_json_with_prompt(user_prompt.as_str())
            .await
    }

    async fn request_candidate_json_with_prompt(
        &self,
        user_prompt: &str,
    ) -> Result<String, InferError> {
        let json_mode_supported = self.json_mode_supported.load(Ordering::Relaxed);

        if json_mode_supported {
            match self
                .request_chat_completion(OPENAI_COMPAT_SIR_SYSTEM_PROMPT, user_prompt, true)
                .await
            {
                Ok(content) => Ok(content),
                Err(InferError::ProviderRejectedFormat) => {
                    self.json_mode_supported.store(false, Ordering::Relaxed);
                    let fallback_prompt =
                        format!("{user_prompt}{OPENAI_COMPAT_JSON_FALLBACK_SUFFIX}");
                    self.request_chat_completion(
                        OPENAI_COMPAT_SIR_SYSTEM_PROMPT,
                        &fallback_prompt,
                        false,
                    )
                    .await
                }
                Err(error) => Err(error),
            }
        } else {
            let fallback_prompt = format!("{user_prompt}{OPENAI_COMPAT_JSON_FALLBACK_SUFFIX}");
            self.request_chat_completion(OPENAI_COMPAT_SIR_SYSTEM_PROMPT, &fallback_prompt, false)
                .await
        }
    }

    pub(crate) async fn request_summary(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, InferError> {
        self.request_chat_completion(system_prompt, user_prompt, false)
            .await
    }
}

#[async_trait]
impl InferenceProvider for OpenAiCompatProvider {
    fn provider_name(&self) -> String {
        InferenceProviderKind::OpenAiCompat.as_str().to_owned()
    }

    fn model_name(&self) -> String {
        self.model.clone()
    }

    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<aether_sir::SirAnnotation, InferError> {
        run_sir_parse_validation_retries(PARSE_VALIDATION_RETRIES, || async {
            self.request_candidate_json(symbol_text, context).await
        })
        .await
    }

    async fn generate_sir_from_prompt(
        &self,
        prompt: &str,
        _context: &SirContext,
        _deep_mode: bool,
    ) -> Result<aether_sir::SirAnnotation, InferError> {
        run_sir_parse_validation_retries(PARSE_VALIDATION_RETRIES, || async {
            self.request_candidate_json_with_prompt(prompt).await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use aether_core::Secret;

    use super::*;

    #[test]
    fn openai_compat_provider_debug_redacts_api_key() {
        let provider = OpenAiCompatProvider::new(
            Secret::new("super-secret-value".to_owned()),
            "https://api.example.com/v1".to_owned(),
            "test-model".to_owned(),
        );
        let debug = format!("{provider:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret-value"));
    }
}
