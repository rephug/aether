use std::env;

use aether_config::InferenceProviderKind;
use aether_core::Secret;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::http::{GEMINI_API_BASE, extract_gemini_text_part, inference_http_client};
use crate::providers::PARSE_VALIDATION_RETRIES;
use crate::sir_parsing::run_sir_parse_validation_retries;
use crate::sir_prompt;
use crate::types::{InferError, InferenceProvider, SirContext, normalize_optional};

pub(crate) const GEMINI_DEFAULT_MODEL: &str = "gemini-3.1-flash-lite-preview";

#[derive(Debug, Clone)]
pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: Secret,
    model: String,
    api_base: String,
}

impl GeminiProvider {
    pub fn from_env_key(api_key_env: &str, model: Option<String>) -> Result<Self, InferError> {
        let api_key = env::var(api_key_env)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| InferError::MissingApiKey(api_key_env.to_owned()))?;

        let model = resolve_gemini_model(model);

        Ok(Self::new(Secret::new(api_key), model))
    }

    pub fn new(api_key: Secret, model: String) -> Self {
        Self {
            client: inference_http_client(),
            api_key,
            model,
            api_base: GEMINI_API_BASE.to_owned(),
        }
    }

    fn endpoint_url(&self) -> String {
        format!("{}/models/{}:generateContent", self.api_base, self.model)
    }

    async fn request_candidate_json_with_prompt(&self, prompt: &str) -> Result<String, InferError> {
        let body = json!({
            "contents": [
                {
                    "parts": [
                        {
                            "text": prompt
                        }
                    ]
                }
            ],
            "generationConfig": {
                "responseMimeType": "application/json",
                "temperature": 0.0
            }
        });

        let response_value: Value = self
            .client
            .post(self.endpoint_url())
            .header("x-goog-api-key", self.api_key.expose())
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        extract_gemini_text_part(&response_value).map(|text| text.to_owned())
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
impl InferenceProvider for GeminiProvider {
    fn provider_name(&self) -> String {
        InferenceProviderKind::Gemini.as_str().to_owned()
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

pub(crate) async fn request_gemini_summary(
    api_key: &Secret,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, InferError> {
    let endpoint = format!("{GEMINI_API_BASE}/models/{model}:generateContent");
    let body = json!({
        "systemInstruction": {
            "parts": [{"text": system_prompt}]
        },
        "contents": [
            {
                "parts": [{"text": user_prompt}]
            }
        ],
        "generationConfig": {
            "temperature": 0.1
        }
    });
    let response_value: Value = inference_http_client()
        .post(endpoint)
        .header("x-goog-api-key", api_key.expose())
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(extract_gemini_text_part(&response_value)?.to_owned())
}

pub(crate) fn resolve_gemini_model(model: Option<String>) -> String {
    let model = normalize_optional(model).unwrap_or_else(|| GEMINI_DEFAULT_MODEL.to_owned());
    if model.starts_with("qwen3-embeddings-") {
        GEMINI_DEFAULT_MODEL.to_owned()
    } else {
        model
    }
}
