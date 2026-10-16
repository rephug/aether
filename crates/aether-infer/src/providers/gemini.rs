use aether_config::{InferenceProviderKind, parse_gemini_thinking_level};
use aether_core::Secret;
use aether_sir::SirAnnotation;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::http::{
    GEMINI_API_BASE, extract_gemini_parts, extract_gemini_text_part, inference_http_client,
};
use crate::model_check::{ModelCatalog, ModelListing};
use crate::providers::PARSE_VALIDATION_RETRIES;
use crate::sir_parsing::parse_and_validate_sir;
use crate::sir_prompt;
use crate::types::{InferError, InferSirResult, InferenceProvider, SirContext, normalize_optional};

pub(crate) const GEMINI_DEFAULT_MODEL: &str = "gemini-3.1-flash-lite";

/// Whether a Gemini request for `model` may carry `temperature`. Google's Gemini 3.6 notes
/// (last updated 2026-10-13): "Starting with Gemini 3.6 Flash and Gemini 3.5 Flash-Lite",
/// for "these models and all future Gemini model releases", `temperature`, `top_p` and
/// `top_k` "are deprecated and ignored", and "in future model generations, supplying these
/// parameters returns an HTTP 400 error". So only an id older than Gemini 3.5 gets one. A
/// later id, an alias such as `gemini-flash-latest` and an id this can't read go without:
/// for them sampling would be ignored at best and a failed request at worst.
pub fn gemini_accepts_temperature(model: &str) -> bool {
    let model = model.trim();
    let bare = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    let Some(rest) = bare.strip_prefix("gemini-") else {
        return false;
    };
    let version = rest.split('-').next().unwrap_or_default();
    let mut numbers = version.split('.').map(str::parse::<u32>);
    let (Some(Ok(major)), minor) = (numbers.next(), numbers.next().unwrap_or(Ok(0))) else {
        return false;
    };
    let Ok(minor) = minor else {
        return false;
    };
    numbers.next().is_none() && (major, minor) < (3, 5)
}

#[derive(Debug, Clone)]
pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: Secret,
    model: String,
    api_base: String,
    thinking: Option<String>,
}

impl GeminiProvider {
    pub fn from_env_key(
        api_key_env: &str,
        model: Option<String>,
        thinking: Option<String>,
    ) -> Result<Self, InferError> {
        let api_key = crate::env_vars::var(api_key_env)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| InferError::MissingApiKey(api_key_env.to_owned()))?;

        let model = resolve_gemini_model(model);

        Ok(Self::new(Secret::new(api_key), model, thinking))
    }

    pub fn new(api_key: Secret, model: String, thinking: Option<String>) -> Self {
        Self {
            client: inference_http_client(),
            api_key,
            model,
            api_base: GEMINI_API_BASE.to_owned(),
            thinking: normalize_optional(thinking),
        }
    }

    fn endpoint_url(&self) -> String {
        format!("{}/models/{}:generateContent", self.api_base, self.model)
    }

    fn generation_config(&self) -> Value {
        let mut gen_config = json!({
            "responseMimeType": "application/json",
        });
        if gemini_accepts_temperature(&self.model) {
            gen_config["temperature"] = json!(0.0);
        }

        if let Some(level) = parse_gemini_thinking_level(self.thinking.as_deref()) {
            gen_config["thinkingConfig"] = json!({
                "thinkingLevel": level.api_value(),
                "includeThoughts": true
            });
        }

        gen_config
    }

    fn build_generate_content_body(&self, prompt: &str) -> Value {
        json!({
            "contents": [
                {
                    "parts": [
                        {
                            "text": prompt
                        }
                    ]
                }
            ],
            "generationConfig": self.generation_config()
        })
    }

    async fn request_candidate_parts_with_prompt(
        &self,
        prompt: &str,
    ) -> Result<(String, Option<String>), InferError> {
        let body = self.build_generate_content_body(prompt);

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

        let parts = extract_gemini_parts(&response_value)?;
        Ok((
            parts.text.to_owned(),
            normalize_optional(parts.thinking.map(str::to_owned)),
        ))
    }

    async fn generate_sir_result_from_prompt(
        &self,
        prompt: &str,
    ) -> Result<InferSirResult, InferError> {
        let mut last_error = String::from("unknown parse/validation failure");

        for attempt in 0..=PARSE_VALIDATION_RETRIES {
            let (candidate_json, reasoning_trace) =
                self.request_candidate_parts_with_prompt(prompt).await?;

            match parse_and_validate_sir(&candidate_json) {
                Ok(sir) => {
                    return Ok(InferSirResult {
                        sir,
                        provider: self.provider_name(),
                        model: self.model_name(),
                        reasoning_trace,
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

    fn build_context_prompt(&self, symbol_text: &str, context: &SirContext) -> String {
        sir_prompt::build_sir_prompt_for_kind(symbol_text, context)
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

    fn model_listings(&self) -> Vec<ModelListing> {
        vec![ModelListing::new(
            self.model.clone(),
            ModelCatalog::Gemini {
                api_base: self.api_base.clone(),
                api_key: self.api_key.clone(),
            },
        )]
    }

    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError> {
        let prompt = self.build_context_prompt(symbol_text, context);
        self.generate_sir_result_from_prompt(prompt.as_str())
            .await
            .map(|result| result.sir)
    }

    async fn generate_sir_with_meta(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<InferSirResult, InferError> {
        let prompt = self.build_context_prompt(symbol_text, context);
        self.generate_sir_result_from_prompt(prompt.as_str()).await
    }

    async fn generate_sir_from_prompt(
        &self,
        prompt: &str,
        _context: &SirContext,
        _deep_mode: bool,
    ) -> Result<SirAnnotation, InferError> {
        self.generate_sir_result_from_prompt(prompt)
            .await
            .map(|result| result.sir)
    }

    async fn generate_sir_from_prompt_with_meta(
        &self,
        prompt: &str,
        _context: &SirContext,
        _deep_mode: bool,
    ) -> Result<InferSirResult, InferError> {
        self.generate_sir_result_from_prompt(prompt).await
    }
}

pub(crate) async fn request_gemini_summary(
    api_key: &Secret,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, InferError> {
    let endpoint = format!("{GEMINI_API_BASE}/models/{model}:generateContent");
    let body = build_summary_body(model, system_prompt, user_prompt);
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

fn build_summary_body(model: &str, system_prompt: &str, user_prompt: &str) -> Value {
    let mut body = json!({
        "systemInstruction": {
            "parts": [{"text": system_prompt}]
        },
        "contents": [
            {
                "parts": [{"text": user_prompt}]
            }
        ]
    });
    if gemini_accepts_temperature(model) {
        body["generationConfig"] = json!({ "temperature": 0.1 });
    }
    body
}

pub(crate) fn resolve_gemini_model(model: Option<String>) -> String {
    let model = normalize_optional(model).unwrap_or_else(|| GEMINI_DEFAULT_MODEL.to_owned());
    if model.starts_with("qwen3-embeddings-") {
        GEMINI_DEFAULT_MODEL.to_owned()
    } else {
        model
    }
}

#[cfg(test)]
mod tests {
    use aether_config::{GeminiThinkingLevel, model_id_notice, parse_gemini_thinking_level};
    use aether_core::Secret;

    use super::{
        GEMINI_DEFAULT_MODEL, GeminiProvider, build_summary_body, gemini_accepts_temperature,
        resolve_gemini_model,
    };

    /// The default must be a GA id Google still serves: the preview it replaced was shut
    /// down on 2026-05-25, and every request naming it failed.
    #[test]
    fn default_model_is_the_ga_flash_lite_id() {
        assert_eq!(GEMINI_DEFAULT_MODEL, "gemini-3.1-flash-lite");
        assert_eq!(resolve_gemini_model(None), "gemini-3.1-flash-lite");
        assert_eq!(
            resolve_gemini_model(Some("  ".to_owned())),
            "gemini-3.1-flash-lite"
        );
        assert!(model_id_notice(GEMINI_DEFAULT_MODEL).is_none());
    }

    #[test]
    fn gemini_thinking_level_maps_supported_values() {
        assert_eq!(
            parse_gemini_thinking_level(Some("minimal")),
            Some(GeminiThinkingLevel::Minimal)
        );
        assert_eq!(
            parse_gemini_thinking_level(Some(" low ")),
            Some(GeminiThinkingLevel::Low)
        );
        assert_eq!(
            parse_gemini_thinking_level(Some("MeDiuM")),
            Some(GeminiThinkingLevel::Medium)
        );
        assert_eq!(
            parse_gemini_thinking_level(Some("high")),
            Some(GeminiThinkingLevel::High)
        );
    }

    #[test]
    fn gemini_thinking_level_omits_unsupported_values() {
        assert_eq!(parse_gemini_thinking_level(Some("")), None);
        assert_eq!(parse_gemini_thinking_level(Some("off")), None);
        assert_eq!(parse_gemini_thinking_level(Some("none")), None);
        assert_eq!(parse_gemini_thinking_level(Some("dynamic")), None);
    }

    #[test]
    fn build_generate_content_body_includes_thinking_config() {
        let provider = GeminiProvider::new(
            Secret::new("test-key".to_owned()),
            "gemini-3-flash-preview".to_owned(),
            Some("medium".to_owned()),
        );

        let body = provider.build_generate_content_body("analyze this");

        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MEDIUM"
        );
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
    }

    #[test]
    fn build_generate_content_body_omits_thinking_config_for_default_dynamic_behavior() {
        let provider = GeminiProvider::new(
            Secret::new("test-key".to_owned()),
            "gemini-3-flash-preview".to_owned(),
            Some("dynamic".to_owned()),
        );

        let body = provider.build_generate_content_body("analyze this");

        assert!(body["generationConfig"].get("thinkingConfig").is_none());
    }

    fn provider(model: &str) -> GeminiProvider {
        GeminiProvider::new(Secret::new("test-key".to_owned()), model.to_owned(), None)
    }

    /// Before Gemini 3.5 a request keeps `temperature`; from Gemini 3.6 Flash and 3.5
    /// Flash-Lite on, Google ignores it now and will reject it later, so it is left out,
    /// as it is for aliases and ids this can't place.
    #[test]
    fn requests_carry_temperature_only_for_models_that_accept_it() {
        for model in [
            "gemini-1.5-pro-002",
            "gemini-2.0-flash",
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
            "gemini-2.5-pro",
            "gemini-3-flash-preview",
            "gemini-3.1-pro-preview",
            GEMINI_DEFAULT_MODEL,
            "models/gemini-3.1-flash-lite",
        ] {
            assert!(gemini_accepts_temperature(model), "{model}");
            let body = provider(model).build_generate_content_body("analyze this");
            assert_eq!(body["generationConfig"]["temperature"], 0.0, "{model}");
            assert_eq!(
                body["generationConfig"]["responseMimeType"], "application/json",
                "{model}"
            );
            let summary = build_summary_body(model, "system", "user");
            assert_eq!(summary["generationConfig"]["temperature"], 0.1, "{model}");
        }

        for model in [
            "gemini-3.5-flash-lite",
            "gemini-3.5-flash",
            "gemini-3.6-flash",
            "gemini-3.8-flash",
            "gemini-4-argon",
            "gemini-flash-latest",
            "gemini-exp-1206",
            "gemini-3.6.1-flash",
            "",
        ] {
            assert!(!gemini_accepts_temperature(model), "{model}");
            let body = provider(model).build_generate_content_body("analyze this");
            assert!(
                body["generationConfig"].get("temperature").is_none(),
                "{model}: {body}"
            );
            assert_eq!(
                body["generationConfig"]["responseMimeType"], "application/json",
                "{model}"
            );
            let summary = build_summary_body(model, "system", "user");
            assert!(
                summary.get("generationConfig").is_none(),
                "{model}: {summary}"
            );
            assert_eq!(
                summary["contents"][0]["parts"][0]["text"], "user",
                "{model}"
            );
        }
    }

    #[test]
    fn thinking_config_survives_without_temperature() {
        let provider = GeminiProvider::new(
            Secret::new("test-key".to_owned()),
            "gemini-3.6-flash".to_owned(),
            Some("high".to_owned()),
        );
        let body = provider.build_generate_content_body("analyze this");
        assert!(body["generationConfig"].get("temperature").is_none());
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "HIGH"
        );
    }
}
