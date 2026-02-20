use std::env;
use std::path::{Path, PathBuf};

use aether_config::{
    AETHER_DIR_NAME, DEFAULT_COHERE_API_KEY_ENV, DEFAULT_GEMINI_API_KEY_ENV,
    DEFAULT_QWEN_EMBEDDING_ENDPOINT, DEFAULT_QWEN_ENDPOINT, DEFAULT_QWEN_MODEL,
    EmbeddingProviderKind, InferenceProviderKind, OLLAMA_SIR_TEMPERATURE, SearchRerankerKind,
    ensure_workspace_config,
};
use aether_sir::{SirAnnotation, validate_sir};
use async_trait::async_trait;
use embedding::candle::CandleEmbeddingProvider;
use reranker::candle::CandleRerankerProvider;
use reranker::cohere::CohereRerankerProvider;
use serde_json::{Value, json};
use thiserror::Error;

mod embedding;
mod reranker;

pub use reranker::{MockRerankerProvider, RerankCandidate, RerankResult, RerankerProvider};

pub const GEMINI_API_KEY_ENV: &str = DEFAULT_GEMINI_API_KEY_ENV;
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const GEMINI_DEFAULT_MODEL: &str = "gemini-2.0-flash";
const PARSE_VALIDATION_RETRIES: usize = 2;
const MOCK_EMBEDDING_DIM: usize = 64;
const OLLAMA_PULL_SUCCESS_STATUS: &str = "success";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirContext {
    pub language: String,
    pub file_path: String,
    pub qualified_name: String,
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
    #[error("missing Gemini API key in {0}")]
    MissingApiKey(String),
    #[error("missing Cohere API key in {0}")]
    MissingCohereApiKey(String),
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

#[derive(Debug, Default, Clone, Copy)]
pub struct MockProvider;

#[async_trait]
impl InferenceProvider for MockProvider {
    async fn generate_sir(
        &self,
        _symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError> {
        let sir = SirAnnotation {
            intent: format!("Mock summary for {}", context.qualified_name),
            inputs: Vec::new(),
            outputs: Vec::new(),
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 1.0,
        };

        validate_sir(&sir)?;
        Ok(sir)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MockEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, InferError> {
        Ok(mock_embedding_for_text(text))
    }
}

#[derive(Debug, Clone)]
pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    api_base: String,
}

impl GeminiProvider {
    pub fn from_env_key(api_key_env: &str, model: Option<String>) -> Result<Self, InferError> {
        let api_key = read_env_non_empty(api_key_env)
            .ok_or_else(|| InferError::MissingApiKey(api_key_env.to_owned()))?;

        let model = resolve_gemini_model(model);

        Ok(Self::new(api_key, model))
    }

    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            api_base: GEMINI_API_BASE.to_owned(),
        }
    }

    fn endpoint_url(&self) -> String {
        format!(
            "{}/models/{}:generateContent?key={}",
            self.api_base, self.model, self.api_key
        )
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
            client: reqwest::Client::new(),
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

#[derive(Debug, Clone)]
pub struct Qwen3LocalEmbeddingProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
}

impl Qwen3LocalEmbeddingProvider {
    pub fn new(endpoint: Option<String>, model: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
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
    let response_value = reqwest::Client::new()
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
    let mut response = reqwest::Client::new()
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

pub fn load_provider_from_env_or_mock(
    workspace_root: impl AsRef<Path>,
    overrides: ProviderOverrides,
) -> Result<LoadedProvider, InferError> {
    let config = ensure_workspace_config(workspace_root)?;

    let selected_provider = overrides.provider.unwrap_or(config.inference.provider);
    let selected_model = first_non_empty(overrides.model, config.inference.model);
    let selected_endpoint = first_non_empty(overrides.endpoint, config.inference.endpoint);
    let selected_api_key_env =
        first_non_empty(overrides.api_key_env, Some(config.inference.api_key_env))
            .unwrap_or_else(|| DEFAULT_GEMINI_API_KEY_ENV.to_owned());

    match selected_provider {
        InferenceProviderKind::Auto => {
            if let Some(api_key) = read_env_non_empty(&selected_api_key_env) {
                let model = resolve_gemini_model(selected_model);
                Ok(LoadedProvider {
                    provider: Box::new(GeminiProvider::new(api_key, model.clone())),
                    provider_name: InferenceProviderKind::Gemini.as_str().to_owned(),
                    model_name: model,
                })
            } else {
                Ok(LoadedProvider {
                    provider: Box::new(MockProvider),
                    provider_name: InferenceProviderKind::Mock.as_str().to_owned(),
                    model_name: "mock".to_owned(),
                })
            }
        }
        InferenceProviderKind::Mock => Ok(LoadedProvider {
            provider: Box::new(MockProvider),
            provider_name: InferenceProviderKind::Mock.as_str().to_owned(),
            model_name: "mock".to_owned(),
        }),
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
    }
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
    let selected_api_key_env = config.inference.api_key_env;
    let system_prompt = system_prompt.trim();
    let user_prompt = user_prompt.trim();
    if system_prompt.is_empty() || user_prompt.is_empty() {
        return Ok(None);
    }

    match selected_provider {
        InferenceProviderKind::Mock => Ok(None),
        InferenceProviderKind::Auto => {
            let Some(api_key) = read_env_non_empty(selected_api_key_env.as_str()) else {
                return Ok(None);
            };
            let model = resolve_gemini_model(selected_model);
            let summary = request_gemini_summary(
                api_key.as_str(),
                model.as_str(),
                system_prompt,
                user_prompt,
            )
            .await?;
            Ok(clean_summary(summary))
        }
        InferenceProviderKind::Gemini => {
            let Some(api_key) = read_env_non_empty(selected_api_key_env.as_str()) else {
                return Ok(None);
            };
            let model = resolve_gemini_model(selected_model);
            let summary = request_gemini_summary(
                api_key.as_str(),
                model.as_str(),
                system_prompt,
                user_prompt,
            )
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
        EmbeddingProviderKind::Mock => LoadedEmbeddingProvider {
            provider: Box::new(MockEmbeddingProvider),
            provider_name: EmbeddingProviderKind::Mock.as_str().to_owned(),
            model_name: format!("mock-{MOCK_EMBEDDING_DIM}d"),
        },
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
        "options": {
            "temperature": OLLAMA_SIR_TEMPERATURE
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
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, InferError> {
    let endpoint = format!("{GEMINI_API_BASE}/models/{model}:generateContent?key={api_key}");
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
    let response_value: Value = reqwest::Client::new()
        .post(endpoint)
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
    let response_value: Value = reqwest::Client::new()
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

    if !trimmed.starts_with("```") {
        return trimmed.to_owned();
    }

    let mut lines = trimmed.lines();
    let _fence_line = lines.next();

    let mut body: Vec<&str> = lines.collect();
    if body.last().is_some_and(|line| line.trim() == "```") {
        body.pop();
    }

    body.join("\n").trim().to_owned()
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn mock_embedding_for_text(text: &str) -> Vec<f32> {
    let mut embedding = vec![0.0f32; MOCK_EMBEDDING_DIM];
    let mut saw_token = false;

    for token in tokenize_for_embedding(text) {
        saw_token = true;
        let normalized = token.to_ascii_lowercase();
        let hash = fnv1a_64(normalized.as_bytes());
        let index = (hash as usize) % MOCK_EMBEDDING_DIM;
        let sign = if ((hash >> 8) & 1) == 0 { 1.0 } else { -1.0 };
        embedding[index] += sign;
    }

    if !saw_token {
        return embedding;
    }

    normalize_embedding(embedding).unwrap_or_else(|| vec![0.0f32; MOCK_EMBEDDING_DIM])
}

fn tokenize_for_embedding(text: &str) -> impl Iterator<Item = &str> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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

fn read_env_non_empty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use aether_config::{
        EmbeddingProviderKind, InferenceProviderKind, SearchRerankerKind, ensure_workspace_config,
    };
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn mock_provider_returns_deterministic_valid_sir() {
        let provider = MockProvider;
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
        };

        let sir = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect("mock provider should succeed");

        assert_eq!(sir.intent, "Mock summary for demo::run");
        assert!(sir.inputs.is_empty());
        assert!(sir.outputs.is_empty());
        assert!(sir.side_effects.is_empty());
        assert!(sir.dependencies.is_empty());
        assert!(sir.error_modes.is_empty());
        assert_eq!(sir.confidence, 1.0);

        validate_sir(&sir).expect("mock sir should validate");
    }

    #[tokio::test]
    async fn mock_embedding_provider_is_deterministic_and_normalized() {
        let provider = MockEmbeddingProvider;

        let first = provider
            .embed_text("Network retry logic with backoff")
            .await
            .expect("first embedding");
        let second = provider
            .embed_text("network RETRY logic with backoff")
            .await
            .expect("second embedding");

        assert_eq!(first.len(), MOCK_EMBEDDING_DIM);
        assert_eq!(first, second);

        let norm_sq = first
            .iter()
            .map(|value| value * value)
            .fold(0.0f32, |acc, value| acc + value);
        assert!((norm_sq - 1.0).abs() < 1e-5);
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
    fn load_provider_auto_chooses_mock_when_key_missing() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let loaded = load_provider_from_env_or_mock(temp.path(), ProviderOverrides::default())
            .expect("load provider");

        assert_eq!(loaded.provider_name, InferenceProviderKind::Mock.as_str());
        assert_eq!(loaded.model_name, "mock");
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
