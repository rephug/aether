use aether_config::{
    DEFAULT_GEMINI_API_KEY_ENV, EmbeddingProviderKind, InferenceProviderKind, SearchRerankerKind,
};
use aether_sir::SirAnnotation;
use async_trait::async_trait;
use thiserror::Error;

pub const GEMINI_API_KEY_ENV: &str = DEFAULT_GEMINI_API_KEY_ENV;

#[derive(Debug, Clone, PartialEq)]
pub struct SirContext {
    pub language: String,
    pub file_path: String,
    pub qualified_name: String,
    pub priority_score: Option<f64>,
    pub kind: String,
    pub is_public: bool,
    pub line_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderOverrides {
    pub provider: Option<InferenceProviderKind>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub api_key_env: Option<String>,
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmbeddingProviderOverrides {
    pub enabled: Option<bool>,
    pub provider: Option<EmbeddingProviderKind>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub api_key_env: Option<String>,
    pub task_type: Option<String>,
    pub dimensions: Option<u32>,
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
    pub provider: Box<dyn crate::RerankerProvider>,
    pub provider_name: String,
    pub model_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InferSirResult {
    pub sir: SirAnnotation,
    pub provider: String,
    pub model: String,
    pub reasoning_trace: Option<String>,
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
    #[error("provider requires endpoint to be set in config")]
    MissingEndpoint,
    #[error("provider requires model to be set in config")]
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
    #[error("no inference provider available: {0}")]
    NoProviderAvailable(String),
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
    fn provider_name(&self) -> String {
        "unknown".to_owned()
    }

    fn model_name(&self) -> String {
        "unknown".to_owned()
    }

    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError>;

    async fn generate_sir_with_meta(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<InferSirResult, InferError> {
        let sir = self.generate_sir(symbol_text, context).await?;
        Ok(InferSirResult {
            sir,
            provider: self.provider_name(),
            model: self.model_name(),
            reasoning_trace: None,
        })
    }

    async fn generate_sir_from_prompt(
        &self,
        prompt: &str,
        context: &SirContext,
        deep_mode: bool,
    ) -> Result<SirAnnotation, InferError> {
        let _ = (prompt, context, deep_mode);
        Err(InferError::InvalidConfig(
            "provider does not support custom SIR prompts".to_owned(),
        ))
    }

    async fn generate_sir_from_prompt_with_meta(
        &self,
        prompt: &str,
        context: &SirContext,
        deep_mode: bool,
    ) -> Result<InferSirResult, InferError> {
        let sir = self
            .generate_sir_from_prompt(prompt, context, deep_mode)
            .await?;
        Ok(InferSirResult {
            sir,
            provider: self.provider_name(),
            model: self.model_name(),
            reasoning_trace: None,
        })
    }
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, InferError>;

    async fn embed_text_with_purpose(
        &self,
        text: &str,
        _purpose: EmbeddingPurpose,
    ) -> Result<Vec<f32>, InferError> {
        self.embed_text(text).await
    }

    /// Embed multiple texts in a single batch call.
    ///
    /// The returned `Vec` has the same length and order as `texts`.
    /// The default implementation calls `embed_text_with_purpose` sequentially;
    /// providers with native batch APIs should override for efficiency.
    async fn embed_texts_with_purpose(
        &self,
        texts: &[&str],
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<Vec<f32>>, InferError> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed_text_with_purpose(text, purpose).await?);
        }
        Ok(results)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbeddingPurpose {
    #[default]
    Document,
    Query,
}

pub(crate) fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) fn first_non_empty(left: Option<String>, right: Option<String>) -> Option<String> {
    normalize_optional(left).or_else(|| normalize_optional(right))
}

#[cfg(test)]
mod tests {
    use aether_sir::{SirAnnotation, validate_sir};
    use async_trait::async_trait;

    use super::*;

    #[derive(Clone)]
    struct TestProvider;

    #[async_trait]
    impl InferenceProvider for TestProvider {
        async fn generate_sir(
            &self,
            _symbol_text: &str,
            context: &SirContext,
        ) -> Result<SirAnnotation, InferError> {
            Ok(SirAnnotation {
                intent: format!("Test {}", context.qualified_name),
                behavior: None,
                inputs: Vec::new(),
                outputs: Vec::new(),
                side_effects: Vec::new(),
                dependencies: Vec::new(),
                error_modes: Vec::new(),
                confidence: 0.9,
                edge_cases: None,
                complexity: None,
                method_dependencies: None,
            })
        }
    }

    #[tokio::test]
    async fn test_provider_returns_deterministic_valid_sir() {
        let provider = TestProvider;
        let context = SirContext {
            language: "rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            qualified_name: "demo::run".to_owned(),
            priority_score: None,
            kind: "function".to_owned(),
            is_public: false,
            line_count: 1,
        };

        let sir = provider
            .generate_sir("fn run() {}", &context)
            .await
            .expect("test provider should succeed");

        assert_eq!(sir.intent, "Test demo::run");
        validate_sir(&sir).expect("test sir should validate");
    }
}
