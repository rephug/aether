use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use aether_config::{
    AETHER_DIR_NAME, DEFAULT_COHERE_API_KEY_ENV, DEFAULT_GEMINI_API_KEY_ENV,
    DEFAULT_OPENAI_COMPAT_API_KEY_ENV, DEFAULT_QWEN_EMBEDDING_ENDPOINT, DEFAULT_QWEN_ENDPOINT,
    DEFAULT_QWEN_MODEL, EmbeddingProviderKind, InferenceProviderKind, OLLAMA_SIR_TEMPERATURE,
    SearchRerankerKind, TieredConfig, ensure_workspace_config,
};
use aether_core::Secret;
use aether_sir::{SirAnnotation, validate_sir};
use async_trait::async_trait;
use embedding::candle::CandleEmbeddingProvider;
use reranker::candle::CandleRerankerProvider;
use reranker::cohere::CohereRerankerProvider;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

mod embedding;
mod reranker;

pub use reranker::{MockRerankerProvider, RerankCandidate, RerankResult, RerankerProvider};

pub const GEMINI_API_KEY_ENV: &str = DEFAULT_GEMINI_API_KEY_ENV;
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const GEMINI_DEFAULT_MODEL: &str = "gemini-flash-latest";
const PARSE_VALIDATION_RETRIES: usize = 2;
const OLLAMA_PULL_SUCCESS_STATUS: &str = "success";
const OPENAI_COMPAT_JSON_FALLBACK_SUFFIX: &str =
    "\n\nRespond with ONLY valid JSON. No markdown, no explanation, no code fences.";
const OPENAI_COMPAT_SIR_SYSTEM_PROMPT: &str =
    "You are generating Structured Intent Records for source code.";

/// Build a reqwest client with sensible timeouts for inference requests.
pub(crate) fn inference_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Build a reqwest client with shorter timeouts for management calls
/// (model listing, health checks).
pub(crate) fn management_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[derive(Debug, Clone, PartialEq)]
pub struct SirContext {
    pub language: String,
    pub file_path: String,
    pub qualified_name: String,
    pub priority_score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderOverrides {
    pub provider: Option<InferenceProviderKind>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmbeddingProviderOverrides {
    pub enabled: Option<bool>,
    pub provider: Option<EmbeddingProviderKind>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub candle_model_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RerankerProviderOverrides {
    pub provider: Option<SearchRerankerKind>,
    pub candle_model_dir: Option<String>,
    pub cohere_api_key_env: Option<String>,
}

pub struct LoadedProvider {
    pub provider: Box<dyn InferenceProvider>,
    pub provider_name: String,
    pub model_name: String,
}

pub struct LoadedEmbeddingProvider {
    pub provider: Box<dyn EmbeddingProvider>,
    pub provider_name: String,
    pub model_name: String,
}

pub struct LoadedRerankerProvider {
    pub provider: Box<dyn RerankerProvider>,
    pub provider_name: String,
    pub model_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaPullProgress {
    pub status: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
    pub done: bool,
}

#[derive(Debug, Error)]
pub enum InferError {
    #[error("missing inference API key: {0}")]
    MissingApiKey(String),
    #[error("missing Cohere API key in {0}")]
    MissingCohereApiKey(String),
    #[error("openai_compat provider requires endpoint to be set in config")]
    MissingEndpoint,
    #[error("openai_compat provider requires model to be set in config")]
    MissingModel,
    #[error("provider rejected response_format")]
    ProviderRejectedFormat,
    #[error("config load failed: {0}")]
    Config(#[from] aether_config::ConfigError),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("response decoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("hf-hub request failed: {0}")]
    HfHub(#[from] hf_hub::api::sync::ApiError),
    #[error("candle model operation failed: {0}")]
    Candle(#[from] candle_core::Error),
    #[error("tokenizer operation failed: {0}")]
    Tokenizer(String),
    #[error("invalid inference config: {0}")]
    InvalidConfig(String),
    #[error("invalid model response: {0}")]
    InvalidResponse(String),
    #[error("SIR validation failed: {0}")]
    Validation(#[from] aether_sir::SirError),
    #[error("failed to parse or validate SIR after retries: {0}")]
    ParseValidationExhausted(String),
    #[error("invalid embedding response: {0}")]
    InvalidEmbeddingResponse(String),
    #[error("{0}")]
    ModelUnavailable(String),
    #[error("failed to lock shared resource: {0}")]
    LockPoisoned(String),
}

#[async_trait]
pub trait InferenceProvider: Send + Sync {
    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError>;
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, InferError>;
}

pub struct TieredProvider {
    primary: Box<dyn InferenceProvider>,
    fallback: Box<dyn InferenceProvider>,
    threshold: f64,
    retry_with_fallback: bool,
    primary_name: String,
}

impl TieredProvider {
    pub fn new(
        primary: Box<dyn InferenceProvider>,
        fallback: Box<dyn InferenceProvider>,
        threshold: f64,
        retry_with_fallback: bool,
        primary_name: String,
    ) -> Self {
        Self {
            primary,
            fallback,
            threshold,
            retry_with_fallback,
            primary_name,
        }
    }
}

#[async_trait]
impl InferenceProvider for TieredProvider {
    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError> {
        let score = context.priority_score.unwrap_or(0.0);
        if score >= self.threshold {
            match self.primary.generate_sir(symbol_text, context).await {
                Ok(sir) => return Ok(sir),
                Err(err) if self.retry_with_fallback => {
                    tracing::warn!(
                        symbol = %context.qualified_name,
                        provider = %self.primary_name,
                        error = %err,
                        "Primary provider failed, falling back to local"
                    );
                }
                Err(err) => return Err(err),
            }
        }

        self.fallback.generate_sir(symbol_text, context).await
    }
}

#[derive(Debug, Clone)]
pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: Secret,
    model: String,
    api_base: String,
}

impl GeminiProvider {
    pub fn from_env_key(api_key_env: &str, model: Option<String>) -> Result<Self, InferError> {
        let api_key = read_env_non_empty(api_key_env)
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

    async fn request_candidate_json(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<String, InferError> {
        let body = json!({
            "contents": [
                {
                    "parts": [
                        {
                            "text": build_strict_json_prompt(symbol_text, context)
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
}

#[async_trait]
impl InferenceProvider for GeminiProvider {
    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError> {
        run_sir_parse_validation_retries(PARSE_VALIDATION_RETRIES, || async {
            self.request_candidate_json(symbol_text, context).await
        })
        .await
    }
}

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

    async fn request_candidate_json_with_prompt(
        &self,
        prompt: String,
    ) -> Result<String, InferError> {
        let body = build_ollama_generate_body(&self.model, &prompt);

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

    async fn request_candidate_json(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<String, InferError> {
        self.request_candidate_json_with_prompt(build_strict_json_prompt(symbol_text, context))
            .await
    }
}

#[async_trait]
impl InferenceProvider for Qwen3LocalProvider {
    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError> {
        let original_prompt = build_strict_json_prompt(symbol_text, context);
        run_sir_parse_validation_retries_with_feedback(
            PARSE_VALIDATION_RETRIES,
            || async { self.request_candidate_json(symbol_text, context).await },
            |previous_output, error| {
                let prompt = build_retry_prompt(&original_prompt, &error, &previous_output);
                async move { self.request_candidate_json_with_prompt(prompt).await }
            },
        )
        .await
    }
}

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
        let user_prompt = build_strict_json_prompt(symbol_text, context);
        let json_mode_supported = self.json_mode_supported.load(Ordering::Relaxed);

        if json_mode_supported {
            match self
                .request_chat_completion(OPENAI_COMPAT_SIR_SYSTEM_PROMPT, &user_prompt, true)
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

    async fn request_summary(
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
    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError> {
        run_sir_parse_validation_retries(PARSE_VALIDATION_RETRIES, || async {
            self.request_candidate_json(symbol_text, context).await
        })
        .await
    }
}

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

pub fn normalize_ollama_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return DEFAULT_QWEN_ENDPOINT.to_owned();
    }

    let known_suffixes = ["/api/generate", "/api/tags", "/api/pull", "/api/embeddings"];

    for suffix in known_suffixes {
        if let Some(stripped) = trimmed.strip_suffix(suffix) {
            let normalized = stripped.trim_end_matches('/');
            if normalized.is_empty() {
                return DEFAULT_QWEN_ENDPOINT.to_owned();
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

pub fn load_inference_provider_from_config(
    workspace_root: impl AsRef<Path>,
    overrides: ProviderOverrides,
) -> Result<LoadedProvider, InferError> {
    let config = ensure_workspace_config(workspace_root)?;

    let ProviderOverrides {
        provider,
        model,
        endpoint,
        api_key_env,
    } = overrides;
    let selected_provider = provider.unwrap_or(config.inference.provider);
    let selected_model = first_non_empty(model, config.inference.model);
    let selected_endpoint = first_non_empty(endpoint, config.inference.endpoint);
    let selected_api_key_env = resolve_inference_api_key_env(
        selected_provider,
        api_key_env,
        Some(config.inference.api_key_env),
    );

    match selected_provider {
        InferenceProviderKind::Auto => {
            let api_key = read_env_non_empty(&selected_api_key_env).ok_or_else(|| {
                InferError::MissingApiKey(
                    "No inference API key found. Set GEMINI_API_KEY or configure a provider."
                        .to_owned(),
                )
            })?;
            let model = resolve_gemini_model(selected_model);
            Ok(LoadedProvider {
                provider: Box::new(GeminiProvider::new(Secret::new(api_key), model.clone())),
                provider_name: InferenceProviderKind::Gemini.as_str().to_owned(),
                model_name: model,
            })
        }
        InferenceProviderKind::Tiered => {
            let tiered = config.inference.tiered.as_ref().ok_or_else(|| {
                InferError::InvalidConfig(
                    "inference.provider=tiered requires [inference.tiered]".to_owned(),
                )
            })?;
            load_tiered_provider(
                tiered,
                selected_model,
                selected_endpoint,
                selected_api_key_env,
            )
        }
        InferenceProviderKind::Gemini => {
            let provider = GeminiProvider::from_env_key(&selected_api_key_env, selected_model)?;
            Ok(LoadedProvider {
                model_name: provider.model.clone(),
                provider: Box::new(provider),
                provider_name: InferenceProviderKind::Gemini.as_str().to_owned(),
            })
        }
        InferenceProviderKind::Qwen3Local => {
            let provider = Qwen3LocalProvider::new(selected_endpoint, selected_model);
            Ok(LoadedProvider {
                model_name: provider.model.clone(),
                provider: Box::new(provider),
                provider_name: InferenceProviderKind::Qwen3Local.as_str().to_owned(),
            })
        }
        InferenceProviderKind::OpenAiCompat => {
            let api_key = read_env_non_empty(&selected_api_key_env)
                .ok_or_else(|| InferError::MissingApiKey(selected_api_key_env.clone()))?;
            let api_base = selected_endpoint.ok_or(InferError::MissingEndpoint)?;
            let model = selected_model.ok_or(InferError::MissingModel)?;
            let provider = OpenAiCompatProvider::new(Secret::new(api_key), api_base, model.clone());
            Ok(LoadedProvider {
                model_name: model,
                provider: Box::new(provider),
                provider_name: InferenceProviderKind::OpenAiCompat.as_str().to_owned(),
            })
        }
    }
}

pub fn load_provider_from_env_or_mock(
    workspace_root: impl AsRef<Path>,
    overrides: ProviderOverrides,
) -> Result<LoadedProvider, InferError> {
    load_inference_provider_from_config(workspace_root, overrides)
}

fn load_tiered_provider(
    tiered: &TieredConfig,
    selected_model: Option<String>,
    selected_endpoint: Option<String>,
    selected_api_key_env: String,
) -> Result<LoadedProvider, InferError> {
    let primary_kind = tiered.primary.trim().to_ascii_lowercase();
    let primary_model = first_non_empty(selected_model, tiered.primary_model.clone());
    let primary_endpoint = first_non_empty(selected_endpoint, tiered.primary_endpoint.clone());
    let primary_api_key_env = normalize_optional(Some(selected_api_key_env))
        .or_else(|| normalize_optional(Some(tiered.primary_api_key_env.clone())))
        .unwrap_or_else(|| DEFAULT_GEMINI_API_KEY_ENV.to_owned());

    let (primary_provider, primary_name, primary_model_name): (
        Box<dyn InferenceProvider>,
        String,
        String,
    ) = match primary_kind.as_str() {
        "gemini" => {
            let provider = GeminiProvider::from_env_key(&primary_api_key_env, primary_model)?;
            let model_name = provider.model.clone();
            (
                Box::new(provider),
                InferenceProviderKind::Gemini.as_str().to_owned(),
                model_name,
            )
        }
        "openai_compat" => {
            let api_key = read_env_non_empty(&primary_api_key_env)
                .ok_or_else(|| InferError::MissingApiKey(primary_api_key_env.clone()))?;
            let api_base = primary_endpoint.ok_or(InferError::MissingEndpoint)?;
            let model = primary_model.ok_or(InferError::MissingModel)?;
            (
                Box::new(OpenAiCompatProvider::new(
                    Secret::new(api_key),
                    api_base,
                    model.clone(),
                )),
                InferenceProviderKind::OpenAiCompat.as_str().to_owned(),
                model,
            )
        }
        other => {
            return Err(InferError::InvalidConfig(format!(
                "inference.tiered.primary must be 'gemini' or 'openai_compat' (found '{other}')"
            )));
        }
    };

    let fallback = Qwen3LocalProvider::new(
        tiered.fallback_endpoint.clone(),
        tiered.fallback_model.clone(),
    );
    let fallback_model_name = fallback.model.clone();
    let threshold = if tiered.primary_threshold.is_finite() {
        tiered.primary_threshold.clamp(0.0, 1.0)
    } else {
        0.8
    };

    let provider = TieredProvider::new(
        primary_provider,
        Box::new(fallback),
        threshold,
        tiered.retry_with_fallback,
        primary_name.clone(),
    );
    Ok(LoadedProvider {
        provider: Box::new(provider),
        provider_name: InferenceProviderKind::Tiered.as_str().to_owned(),
        model_name: format!("{primary_model_name}|{fallback_model_name}"),
    })
}

async fn summarize_text_with_tiered(
    tiered: &TieredConfig,
    score: f64,
    system_prompt: &str,
    user_prompt: &str,
    selected_model: Option<String>,
    selected_endpoint: Option<String>,
    selected_api_key_env: String,
) -> Result<Option<String>, InferError> {
    let threshold = if tiered.primary_threshold.is_finite() {
        tiered.primary_threshold.clamp(0.0, 1.0)
    } else {
        0.8
    };
    let primary_kind = tiered.primary.trim().to_ascii_lowercase();
    let primary_model = first_non_empty(selected_model, tiered.primary_model.clone());
    let primary_endpoint = first_non_empty(selected_endpoint, tiered.primary_endpoint.clone());
    let primary_api_key_env = normalize_optional(Some(selected_api_key_env))
        .or_else(|| normalize_optional(Some(tiered.primary_api_key_env.clone())))
        .unwrap_or_else(|| DEFAULT_GEMINI_API_KEY_ENV.to_owned());
    let fallback_endpoint = normalize_optional(tiered.fallback_endpoint.clone())
        .unwrap_or_else(|| DEFAULT_QWEN_ENDPOINT.to_owned());
    let fallback_model = normalize_optional(tiered.fallback_model.clone())
        .unwrap_or_else(|| DEFAULT_QWEN_MODEL.to_owned());

    if score >= threshold {
        let primary_result = match primary_kind.as_str() {
            "gemini" => {
                if let Some(api_key) = read_env_non_empty(primary_api_key_env.as_str()) {
                    let model = resolve_gemini_model(primary_model);
                    request_gemini_summary(
                        &Secret::new(api_key),
                        model.as_str(),
                        system_prompt,
                        user_prompt,
                    )
                    .await
                } else {
                    Err(InferError::MissingApiKey(primary_api_key_env))
                }
            }
            "openai_compat" => {
                let api_key = read_env_non_empty(primary_api_key_env.as_str())
                    .ok_or_else(|| InferError::MissingApiKey(primary_api_key_env.clone()))?;
                let api_base = primary_endpoint.ok_or(InferError::MissingEndpoint)?;
                let model = primary_model.ok_or(InferError::MissingModel)?;
                OpenAiCompatProvider::new(Secret::new(api_key), api_base, model)
                    .request_summary(system_prompt, user_prompt)
                    .await
            }
            other => {
                return Err(InferError::InvalidConfig(format!(
                    "inference.tiered.primary must be 'gemini' or 'openai_compat' (found '{other}')"
                )));
            }
        };

        match primary_result {
            Ok(summary) => return Ok(clean_summary(summary)),
            Err(err) if !tiered.retry_with_fallback => return Err(err),
            Err(err) => {
                tracing::warn!(error = %err, "tiered summary primary failed; using fallback");
            }
        }
    }

    let summary = request_qwen_summary(
        fallback_endpoint.as_str(),
        fallback_model.as_str(),
        system_prompt,
        user_prompt,
    )
    .await?;
    Ok(clean_summary(summary))
}

pub async fn summarize_text_with_config(
    workspace_root: impl AsRef<Path>,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<Option<String>, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let selected_provider = config.inference.provider;
    let selected_model = config.inference.model;
    let selected_endpoint = config.inference.endpoint;
    let selected_api_key_env =
        resolve_inference_api_key_env(selected_provider, None, Some(config.inference.api_key_env));
    let system_prompt = system_prompt.trim();
    let user_prompt = user_prompt.trim();
    if system_prompt.is_empty() || user_prompt.is_empty() {
        return Ok(None);
    }

    match selected_provider {
        InferenceProviderKind::Auto => {
            let api_key = read_env_non_empty(selected_api_key_env.as_str()).ok_or_else(|| {
                InferError::MissingApiKey(
                    "No inference API key found. Set GEMINI_API_KEY or configure a provider."
                        .to_owned(),
                )
            })?;
            let model = resolve_gemini_model(selected_model);
            let api_key = Secret::new(api_key);
            let summary =
                request_gemini_summary(&api_key, model.as_str(), system_prompt, user_prompt)
                    .await?;
            Ok(clean_summary(summary))
        }
        InferenceProviderKind::Tiered => {
            let tiered = config.inference.tiered.as_ref().ok_or_else(|| {
                InferError::InvalidConfig(
                    "inference.provider=tiered requires [inference.tiered]".to_owned(),
                )
            })?;
            let score = 1.0;
            summarize_text_with_tiered(
                tiered,
                score,
                system_prompt,
                user_prompt,
                selected_model,
                selected_endpoint,
                selected_api_key_env,
            )
            .await
        }
        InferenceProviderKind::Gemini => {
            let Some(api_key) = read_env_non_empty(selected_api_key_env.as_str()) else {
                return Ok(None);
            };
            let model = resolve_gemini_model(selected_model);
            let api_key = Secret::new(api_key);
            let summary =
                request_gemini_summary(&api_key, model.as_str(), system_prompt, user_prompt)
                    .await?;
            Ok(clean_summary(summary))
        }
        InferenceProviderKind::Qwen3Local => {
            let endpoint = normalize_optional(selected_endpoint)
                .unwrap_or_else(|| DEFAULT_QWEN_ENDPOINT.to_owned());
            let model =
                normalize_optional(selected_model).unwrap_or_else(|| DEFAULT_QWEN_MODEL.to_owned());
            let summary = request_qwen_summary(
                endpoint.as_str(),
                model.as_str(),
                system_prompt,
                user_prompt,
            )
            .await?;
            Ok(clean_summary(summary))
        }
        InferenceProviderKind::OpenAiCompat => {
            let Some(api_key) = read_env_non_empty(selected_api_key_env.as_str()) else {
                return Ok(None);
            };
            let api_base = selected_endpoint.ok_or(InferError::MissingEndpoint)?;
            let model = selected_model.ok_or(InferError::MissingModel)?;
            let provider = OpenAiCompatProvider::new(Secret::new(api_key), api_base, model);
            let summary = provider.request_summary(system_prompt, user_prompt).await?;
            Ok(clean_summary(summary))
        }
    }
}

pub fn load_embedding_provider_from_config(
    workspace_root: impl AsRef<Path>,
    overrides: EmbeddingProviderOverrides,
) -> Result<Option<LoadedEmbeddingProvider>, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let selected_enabled = overrides.enabled.unwrap_or(config.embeddings.enabled);
    if !selected_enabled {
        return Ok(None);
    }

    let selected_provider = overrides.provider.unwrap_or(config.embeddings.provider);
    let selected_model = first_non_empty(overrides.model, config.embeddings.model.clone());
    let selected_endpoint = first_non_empty(overrides.endpoint, config.embeddings.endpoint.clone());
    let selected_candle_model_dir = first_non_empty(
        overrides.candle_model_dir,
        config.embeddings.candle.model_dir.clone(),
    )
    .map(PathBuf::from);

    let loaded = match selected_provider {
        EmbeddingProviderKind::Qwen3Local => {
            let provider = Qwen3LocalEmbeddingProvider::new(selected_endpoint, selected_model);
            LoadedEmbeddingProvider {
                model_name: provider.model.clone(),
                provider: Box::new(provider),
                provider_name: EmbeddingProviderKind::Qwen3Local.as_str().to_owned(),
            }
        }
        EmbeddingProviderKind::Candle => {
            let model_dir = resolve_candle_model_dir(workspace_root, selected_candle_model_dir);
            let provider = CandleEmbeddingProvider::new(model_dir);
            let model_name = provider.model_name().to_owned();
            let provider_name = provider.provider_name().to_owned();
            LoadedEmbeddingProvider {
                model_name,
                provider: Box::new(provider),
                provider_name,
            }
        }
    };

    Ok(Some(loaded))
}

pub fn load_reranker_provider_from_config(
    workspace_root: impl AsRef<Path>,
    overrides: RerankerProviderOverrides,
) -> Result<Option<LoadedRerankerProvider>, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let selected_provider = overrides.provider.unwrap_or(config.search.reranker);
    let selected_candle_model_dir = first_non_empty(
        overrides.candle_model_dir,
        first_non_empty(
            config.search.candle.model_dir.clone(),
            config.embeddings.candle.model_dir.clone(),
        ),
    )
    .map(PathBuf::from);
    let selected_cohere_api_key_env = first_non_empty(
        overrides.cohere_api_key_env,
        Some(config.providers.cohere.api_key_env.clone()),
    )
    .unwrap_or_else(|| DEFAULT_COHERE_API_KEY_ENV.to_owned());

    let loaded = match selected_provider {
        SearchRerankerKind::None => return Ok(None),
        SearchRerankerKind::Candle => {
            let model_dir = resolve_candle_model_dir(workspace_root, selected_candle_model_dir);
            let provider = CandleRerankerProvider::new(model_dir);
            LoadedRerankerProvider {
                model_name: provider.model_name().to_owned(),
                provider_name: provider.provider_name().to_owned(),
                provider: Box::new(provider),
            }
        }
        SearchRerankerKind::Cohere => {
            let provider = CohereRerankerProvider::from_env(&selected_cohere_api_key_env)?;
            LoadedRerankerProvider {
                model_name: provider.model_name().to_owned(),
                provider_name: provider.provider_name().to_owned(),
                provider: Box::new(provider),
            }
        }
    };

    Ok(Some(loaded))
}

pub fn download_candle_embedding_model(
    workspace_root: impl AsRef<Path>,
    model_dir_override: Option<PathBuf>,
) -> Result<PathBuf, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let configured_model_dir =
        model_dir_override.or_else(|| config.embeddings.candle.model_dir.map(PathBuf::from));
    let model_dir = resolve_candle_model_dir(workspace_root, configured_model_dir);

    let provider = CandleEmbeddingProvider::new(model_dir);
    provider.ensure_model_downloaded()
}

pub fn download_candle_reranker_model(
    workspace_root: impl AsRef<Path>,
    model_dir_override: Option<PathBuf>,
) -> Result<PathBuf, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let configured_model_dir = model_dir_override
        .or_else(|| config.search.candle.model_dir.map(PathBuf::from))
        .or_else(|| config.embeddings.candle.model_dir.map(PathBuf::from));
    let model_dir = resolve_candle_model_dir(workspace_root, configured_model_dir);

    let provider = CandleRerankerProvider::new(model_dir);
    provider.ensure_model_downloaded()
}

fn resolve_candle_model_dir(workspace_root: &Path, model_dir: Option<PathBuf>) -> PathBuf {
    let configured = model_dir.unwrap_or_else(|| PathBuf::from(AETHER_DIR_NAME).join("models"));
    if configured.is_absolute() {
        configured
    } else {
        workspace_root.join(configured)
    }
}

fn build_strict_json_prompt(symbol_text: &str, context: &SirContext) -> String {
    format!(
        "You are generating a Leaf SIR annotation. \
Respond with STRICT JSON only (no markdown, no prose) and exactly these fields: \
intent (string), inputs (array of string), outputs (array of string), side_effects (array of string), dependencies (array of string), error_modes (array of string), confidence (number in [0.0,1.0]). \
Do not add any extra keys.\n\nContext:\n- language: {}\n- file_path: {}\n- qualified_name: {}\n\nSymbol text:\n{}",
        context.language, context.file_path, context.qualified_name, symbol_text
    )
}

fn build_retry_prompt(original_prompt: &str, error: &str, previous_output: &str) -> String {
    format!(
        "{original_prompt}\n\nYour previous response was invalid. Error: {error}. Previous output: {previous_output}. Please respond again with STRICT JSON only, fixing the error above."
    )
}

fn build_ollama_generate_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "format": "json",
        "think": false,
        "options": {
            "temperature": OLLAMA_SIR_TEMPERATURE,
            "num_ctx": 4096
        }
    })
}

fn build_ollama_text_generate_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": OLLAMA_SIR_TEMPERATURE
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

fn build_openai_chat_completion_body(
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

fn response_indicates_unsupported_json_mode(response_body: &str) -> bool {
    let lower = response_body.to_ascii_lowercase();
    lower.contains("response_format") || lower.contains("json_object")
}

fn extract_openai_error_message(response_body: &str) -> Option<String> {
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

fn extract_openai_chat_content_from_body(response_body: &str) -> Result<String, InferError> {
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

async fn run_sir_parse_validation_retries<F, Fut>(
    retries: usize,
    mut candidate_json_loader: F,
) -> Result<SirAnnotation, InferError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, InferError>>,
{
    let mut last_error = String::from("unknown parse/validation failure");

    for attempt in 0..=retries {
        let candidate_json = candidate_json_loader().await?;

        match parse_and_validate_sir(&candidate_json) {
            Ok(sir) => return Ok(sir),
            Err(message) => {
                last_error = message;
                if attempt == retries {
                    break;
                }
            }
        }
    }

    Err(InferError::ParseValidationExhausted(last_error))
}

async fn run_sir_parse_validation_retries_with_feedback<F, Fut, G, Gfut>(
    retries: usize,
    mut candidate_json_loader: F,
    mut feedback_json_loader: G,
) -> Result<SirAnnotation, InferError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, InferError>>,
    G: FnMut(String, String) -> Gfut,
    Gfut: std::future::Future<Output = Result<String, InferError>>,
{
    let mut last_error = String::from("unknown parse/validation failure");
    let mut last_output = String::new();
    let mut retry_with_feedback = false;

    for attempt in 0..=retries {
        let mut attempted_feedback = false;
        let candidate_json = if retry_with_feedback && !last_output.is_empty() {
            attempted_feedback = true;
            match feedback_json_loader(last_output.clone(), last_error.clone()).await {
                Ok(candidate) => candidate,
                Err(_) => candidate_json_loader().await?,
            }
        } else {
            candidate_json_loader().await?
        };

        match parse_and_validate_sir(&candidate_json) {
            Ok(sir) => return Ok(sir),
            Err(message) => {
                last_error = message;
                last_output = candidate_json;
                if attempt == retries {
                    break;
                }
                retry_with_feedback = !attempted_feedback;
            }
        }
    }

    Err(InferError::ParseValidationExhausted(last_error))
}

fn extract_gemini_text_part(response: &Value) -> Result<&str, InferError> {
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

fn extract_local_text_part(response: &Value) -> Result<String, InferError> {
    if let Some(text) = value_to_candidate_json(response) {
        return Ok(text);
    }

    let candidate_paths = [
        "/response",
        "/text",
        "/output",
        "/message/content",
        "/choices/0/text",
        "/choices/0/message/content",
        "/data/output",
    ];

    for path in candidate_paths {
        if let Some(value) = response.pointer(path)
            && let Some(text) = value_to_candidate_json(value)
        {
            return Ok(text);
        }
    }

    Err(InferError::InvalidResponse(
        "missing local model text/JSON response body".to_owned(),
    ))
}

async fn request_gemini_summary(
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

async fn request_qwen_summary(
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

fn clean_summary(text: String) -> Option<String> {
    let normalized = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned();
    if normalized.is_empty() {
        return None;
    }
    Some(normalized)
}

fn extract_embedding_vector(response: &Value) -> Result<Vec<f32>, InferError> {
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

fn value_to_candidate_json(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }

    if looks_like_sir_shape(value) {
        return Some(value.to_string());
    }

    None
}

fn looks_like_sir_shape(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };

    [
        "intent",
        "inputs",
        "outputs",
        "side_effects",
        "dependencies",
        "error_modes",
        "confidence",
    ]
    .iter()
    .all(|key| obj.contains_key(*key))
}

fn parse_and_validate_sir(candidate_json: &str) -> Result<SirAnnotation, String> {
    let normalized = normalize_candidate_json(candidate_json);

    let sir: SirAnnotation =
        serde_json::from_str(&normalized).map_err(|err| format!("json parse error: {err}"))?;

    validate_sir(&sir).map_err(|err| format!("sir validation error: {err}"))?;
    Ok(sir)
}

fn normalize_candidate_json(candidate_json: &str) -> String {
    let trimmed = candidate_json.trim();
    let lower = trimmed.to_ascii_lowercase();

    let extract_fenced_body = |input: &str, opening_idx: usize| -> Option<String> {
        let fence_payload = input.get((opening_idx + 3)..)?;
        let newline_idx = fence_payload.find('\n')?;
        let body_start = opening_idx + 3 + newline_idx + 1;
        let after_newline = input.get(body_start..)?;
        let closing_idx = after_newline.find("```")?;
        Some(after_newline[..closing_idx].trim().to_owned())
    };

    if let Some(idx) = lower.find("```json")
        && let Some(extracted) = extract_fenced_body(trimmed, idx)
    {
        return extracted;
    }

    if let Some(idx) = trimmed.find("```")
        && let Some(extracted) = extract_fenced_body(trimmed, idx)
    {
        return extracted;
    }

    trimmed.to_owned()
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_embedding(mut embedding: Vec<f32>) -> Option<Vec<f32>> {
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

fn resolve_gemini_model(model: Option<String>) -> String {
    let model = normalize_optional(model).unwrap_or_else(|| GEMINI_DEFAULT_MODEL.to_owned());
    if model.starts_with("qwen3-embeddings-") {
        GEMINI_DEFAULT_MODEL.to_owned()
    } else {
        model
    }
}

fn first_non_empty(left: Option<String>, right: Option<String>) -> Option<String> {
    normalize_optional(left).or_else(|| normalize_optional(right))
}

fn normalize_openai_api_base(api_base: &str) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');
    let trimmed = trimmed
        .strip_suffix("/chat/completions")
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    trimmed.to_owned()
}

fn default_api_key_env_for_provider(provider: InferenceProviderKind) -> &'static str {
    match provider {
        InferenceProviderKind::OpenAiCompat => DEFAULT_OPENAI_COMPAT_API_KEY_ENV,
        _ => DEFAULT_GEMINI_API_KEY_ENV,
    }
}

fn resolve_inference_api_key_env(
    provider: InferenceProviderKind,
    override_api_key_env: Option<String>,
    config_api_key_env: Option<String>,
) -> String {
    let selected = first_non_empty(override_api_key_env, config_api_key_env);
    match selected {
        Some(value)
            if provider == InferenceProviderKind::OpenAiCompat
                && value == DEFAULT_GEMINI_API_KEY_ENV =>
        {
            DEFAULT_OPENAI_COMPAT_API_KEY_ENV.to_owned()
        }
        Some(value) => value,
        None => default_api_key_env_for_provider(provider).to_owned(),
    }
}

fn read_env_non_empty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use aether_config::{
        EmbeddingProviderKind, InferenceProviderKind, SearchRerankerKind, ensure_workspace_config,
    };
    use tempfile::tempdir;

    use super::*;

    #[derive(Clone)]
    struct TestProvider {
        intent_prefix: String,
        fail: bool,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl InferenceProvider for TestProvider {
        async fn generate_sir(
            &self,
            _symbol_text: &str,
            context: &SirContext,
        ) -> Result<SirAnnotation, InferError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(InferError::InvalidResponse(
                    "forced provider failure".to_owned(),
                ));
            }
            Ok(SirAnnotation {
                intent: format!("{} {}", self.intent_prefix, context.qualified_name),
                inputs: Vec::new(),
                outputs: Vec::new(),
                side_effects: Vec::new(),
                dependencies: Vec::new(),
                error_modes: Vec::new(),
                confidence: 0.9,
            })
        }
    }

    #[tokio::test]
    async fn test_provider_returns_deterministic_valid_sir() {
        let provider = TestProvider {
            intent_prefix: "Test".to_owned(),
            fail: false,
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: None,
        };

        let sir = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect("test provider should succeed");

        assert_eq!(sir.intent, "Test demo::run");
        validate_sir(&sir).expect("test sir should validate");
    }

    #[tokio::test]
    async fn tiered_provider_routes_high_score_to_primary() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let provider = TieredProvider::new(
            Box::new(TestProvider {
                intent_prefix: "primary".to_owned(),
                fail: false,
                calls: primary_calls.clone(),
            }),
            Box::new(TestProvider {
                intent_prefix: "fallback".to_owned(),
                fail: false,
                calls: fallback_calls.clone(),
            }),
            0.8,
            true,
            "primary".to_owned(),
        );
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.95),
        };

        let sir = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect("tiered should succeed");
        assert!(sir.intent.starts_with("primary "));
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn tiered_provider_routes_low_score_to_fallback() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let provider = TieredProvider::new(
            Box::new(TestProvider {
                intent_prefix: "primary".to_owned(),
                fail: false,
                calls: primary_calls.clone(),
            }),
            Box::new(TestProvider {
                intent_prefix: "fallback".to_owned(),
                fail: false,
                calls: fallback_calls.clone(),
            }),
            0.8,
            true,
            "primary".to_owned(),
        );
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.3),
        };

        let sir = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect("tiered should succeed");
        assert!(sir.intent.starts_with("fallback "));
        assert_eq!(primary_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tiered_provider_falls_back_on_primary_error_when_enabled() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let provider = TieredProvider::new(
            Box::new(TestProvider {
                intent_prefix: "primary".to_owned(),
                fail: true,
                calls: primary_calls.clone(),
            }),
            Box::new(TestProvider {
                intent_prefix: "fallback".to_owned(),
                fail: false,
                calls: fallback_calls.clone(),
            }),
            0.8,
            true,
            "primary".to_owned(),
        );
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
        };

        let sir = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect("tiered fallback should succeed");
        assert!(sir.intent.starts_with("fallback "));
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tiered_provider_propagates_primary_error_when_disabled() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let provider = TieredProvider::new(
            Box::new(TestProvider {
                intent_prefix: "primary".to_owned(),
                fail: true,
                calls: primary_calls.clone(),
            }),
            Box::new(TestProvider {
                intent_prefix: "fallback".to_owned(),
                fail: false,
                calls: fallback_calls.clone(),
            }),
            0.8,
            false,
            "primary".to_owned(),
        );
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: Some(0.9),
        };

        let err = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect_err("tiered should propagate primary error");
        match err {
            InferError::InvalidResponse(message) => assert!(message.contains("forced")),
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn qwen3_local_generate_body_sets_temperature_in_options() {
        let body = build_ollama_generate_body("qwen2.5-coder:7b", "return strict json");
        assert_eq!(
            body.pointer("/options/temperature"),
            Some(&json!(OLLAMA_SIR_TEMPERATURE))
        );
        assert_eq!(body.pointer("/temperature"), None);
    }

    #[test]
    fn normalize_candidate_json_extracts_json_after_preamble() {
        let input = "Here is the data:\n```json\n{\"purpose\":\"test\"}\n```";
        assert_eq!(normalize_candidate_json(input), "{\"purpose\":\"test\"}");
    }

    #[test]
    fn normalize_candidate_json_extracts_json_with_language_tag() {
        let input = "```json\n{\"purpose\":\"test\"}\n```";
        assert_eq!(normalize_candidate_json(input), "{\"purpose\":\"test\"}");
    }

    #[test]
    fn normalize_candidate_json_returns_raw_json_when_unfenced() {
        let input = "{\"purpose\":\"test\"}";
        assert_eq!(normalize_candidate_json(input), input);
    }

    #[test]
    fn normalize_candidate_json_returns_trimmed_input_when_fence_is_unclosed() {
        let input = "```json\n{\"purpose\":\"test\"}";
        assert_eq!(normalize_candidate_json(input), input);
    }

    #[test]
    fn normalize_candidate_json_uses_first_fenced_block_when_multiple_present() {
        let input =
            "```json\n{\"purpose\":\"first\"}\n```\n\n```json\n{\"purpose\":\"second\"}\n```";
        assert_eq!(normalize_candidate_json(input), "{\"purpose\":\"first\"}");
    }

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

    #[tokio::test]
    async fn retry_with_feedback_uses_error_context_on_second_attempt() {
        let valid_json = Arc::new(
            r#"{"intent":"valid","inputs":[],"outputs":[],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.9}"#
                .to_owned(),
        );
        let scratch_calls = Arc::new(AtomicUsize::new(0));
        let feedback_inputs = Arc::new(Mutex::new(Vec::<(String, String)>::new()));

        let result = run_sir_parse_validation_retries_with_feedback(
            2,
            {
                let valid_json = valid_json.clone();
                let scratch_calls = scratch_calls.clone();
                move || {
                    let valid_json = valid_json.clone();
                    let scratch_calls = scratch_calls.clone();
                    async move {
                        if scratch_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            Ok("not json".to_owned())
                        } else {
                            Ok(valid_json.as_ref().clone())
                        }
                    }
                }
            },
            {
                let feedback_inputs = feedback_inputs.clone();
                let valid_json = valid_json.clone();
                move |previous_output: String, error: String| {
                    let feedback_inputs = feedback_inputs.clone();
                    let valid_json = valid_json.clone();
                    async move {
                        let mut guard = feedback_inputs.lock().expect("feedback lock");
                        guard.push((previous_output, error));
                        Ok(valid_json.as_ref().clone())
                    }
                }
            },
        )
        .await
        .expect("retry should succeed");

        assert_eq!(result.intent, "valid");
        assert_eq!(scratch_calls.load(Ordering::SeqCst), 1);

        let captured = feedback_inputs.lock().expect("feedback lock");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, "not json");
        assert!(captured[0].1.contains("json parse error"));
    }

    #[test]
    fn load_embedding_provider_defaults_to_disabled() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let loaded =
            load_embedding_provider_from_config(temp.path(), EmbeddingProviderOverrides::default())
                .expect("load embedding provider");
        assert!(loaded.is_none());
    }

    #[test]
    fn load_embedding_provider_reads_enabled_qwen_settings() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");

        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "auto"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true

[embeddings]
enabled = true
provider = "qwen3_local"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434/api/embeddings"
"#,
        )
        .expect("write config");

        let loaded =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
                .expect("load embedding provider")
                .expect("embedding provider should be enabled");

        assert_eq!(
            loaded.provider_name,
            EmbeddingProviderKind::Qwen3Local.as_str()
        );
        assert_eq!(loaded.model_name, "qwen3-embeddings-4B");
    }

    #[test]
    fn load_embedding_provider_reads_enabled_candle_settings() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");

        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[embeddings]
enabled = true
provider = "candle"

[embeddings.candle]
model_dir = ".aether/models"
"#,
        )
        .expect("write config");

        let loaded =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
                .expect("load embedding provider")
                .expect("embedding provider should be enabled");

        assert_eq!(loaded.provider_name, EmbeddingProviderKind::Candle.as_str());
        assert_eq!(loaded.model_name, "qwen3-embedding-0.6b");
    }

    #[test]
    fn load_provider_auto_errors_when_key_missing() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let result = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::Auto),
                api_key_env: Some("AETHER_TEST_NONEXISTENT_KEY_ZZZZZ".to_owned()),
                ..ProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::MissingApiKey(message)) => {
                assert!(message.contains("No inference API key found"))
            }
            Ok(_) => panic!("expected missing api key error, got Ok result"),
            Err(err) => panic!("expected missing api key error, got {err}"),
        }
    }

    #[test]
    fn load_provider_auto_chooses_gemini_when_key_present() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let env_name = format!(
            "AETHER_TEST_GEMINI_KEY_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );

        // SAFETY: test-scoped environment variable with unique name.
        unsafe {
            env::set_var(&env_name, "test-key");
        }

        let loaded = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::Auto),
                api_key_env: Some(env_name.clone()),
                ..ProviderOverrides::default()
            },
        )
        .expect("load provider");

        assert_eq!(loaded.provider_name, InferenceProviderKind::Gemini.as_str());
        assert_eq!(loaded.model_name, GEMINI_DEFAULT_MODEL);

        // SAFETY: cleanup of test-scoped environment variable.
        unsafe {
            env::remove_var(env_name);
        }
    }

    #[test]
    fn load_provider_openai_compat_requires_api_key_with_fabricated_env_var() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");
        let env_name = "AETHER_TEST_OPENAI_COMPAT_KEY_ZZZZZ".to_owned();

        let result = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::OpenAiCompat),
                endpoint: Some("https://api.example.com/v1".to_owned()),
                model: Some("glm-4.7".to_owned()),
                api_key_env: Some(env_name.clone()),
                ..ProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::MissingApiKey(name)) => assert_eq!(name, env_name),
            _ => panic!("expected missing api key"),
        }
    }

    #[test]
    fn load_provider_openai_compat_requires_endpoint() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");
        let env_name = format!(
            "AETHER_TEST_OPENAI_COMPAT_KEY_ENDPOINT_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        // SAFETY: test-scoped environment variable with unique name.
        unsafe {
            env::set_var(&env_name, "test-key");
        }

        let result = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::OpenAiCompat),
                model: Some("glm-4.7".to_owned()),
                api_key_env: Some(env_name),
                ..ProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::MissingEndpoint) => {}
            _ => panic!("expected missing endpoint"),
        }
    }

    #[test]
    fn load_provider_openai_compat_requires_model() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");
        let env_name = format!(
            "AETHER_TEST_OPENAI_COMPAT_KEY_MODEL_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        // SAFETY: test-scoped environment variable with unique name.
        unsafe {
            env::set_var(&env_name, "test-key");
        }

        let result = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::OpenAiCompat),
                endpoint: Some("https://api.example.com/v1".to_owned()),
                api_key_env: Some(env_name),
                ..ProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::MissingModel) => {}
            _ => panic!("expected missing model"),
        }
    }

    #[test]
    fn load_reranker_provider_defaults_to_none() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let loaded =
            load_reranker_provider_from_config(temp.path(), RerankerProviderOverrides::default())
                .expect("load reranker provider");
        assert!(loaded.is_none());
    }

    #[test]
    fn load_reranker_provider_reads_enabled_candle_settings() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");

        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[search]
reranker = "candle"

[search.candle]
model_dir = ".aether/models"
"#,
        )
        .expect("write config");

        let loaded =
            load_reranker_provider_from_config(workspace, RerankerProviderOverrides::default())
                .expect("load reranker provider")
                .expect("reranker provider should be enabled");

        assert_eq!(loaded.provider_name, SearchRerankerKind::Candle.as_str());
        assert_eq!(loaded.model_name, "qwen3-reranker-0.6b");
    }

    #[test]
    fn load_reranker_provider_requires_cohere_api_key() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");

        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[search]
reranker = "cohere"
"#,
        )
        .expect("write config");

        let env_name = format!(
            "AETHER_TEST_COHERE_KEY_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        // SAFETY: test-scoped environment variable with unique name.
        unsafe {
            env::remove_var(&env_name);
        }

        let result = load_reranker_provider_from_config(
            workspace,
            RerankerProviderOverrides {
                cohere_api_key_env: Some(env_name.clone()),
                ..RerankerProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::MissingCohereApiKey(var)) => assert_eq!(var, env_name),
            _ => panic!("expected missing cohere key error"),
        }
    }
}
