use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const AETHER_DIR_NAME: &str = ".aether";
pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const DEFAULT_GEMINI_API_KEY_ENV: &str = "GEMINI_API_KEY";
pub const DEFAULT_OPENAI_COMPAT_API_KEY_ENV: &str = "OPENAI_COMPAT_API_KEY";
pub const DEFAULT_QWEN_ENDPOINT: &str = "http://127.0.0.1:11434";
pub const DEFAULT_QWEN_MODEL: &str = "qwen3.5:4b";
pub const DEFAULT_QWEN_EMBEDDING_ENDPOINT: &str = "http://127.0.0.1:11434/api/embeddings";
pub const RECOMMENDED_OLLAMA_MODEL: &str = "qwen3.5:4b";
pub const OLLAMA_DEFAULT_ENDPOINT: &str = "http://127.0.0.1:11434";
pub const SIR_QUALITY_FLOOR_CONFIDENCE: f32 = 0.3;
pub const SIR_QUALITY_FLOOR_WINDOW: usize = 10;
pub const OLLAMA_SIR_TEMPERATURE: f32 = 0.1;
pub const DEFAULT_SIR_CONCURRENCY: usize = 2;
pub const GEMINI_DEFAULT_CONCURRENCY: usize = 16;
pub const DEFAULT_COHERE_API_KEY_ENV: &str = "COHERE_API_KEY";
pub const DEFAULT_VERIFY_CONTAINER_RUNTIME: &str = "docker";
pub const DEFAULT_VERIFY_CONTAINER_IMAGE: &str = "rust:1-bookworm";
pub const DEFAULT_VERIFY_CONTAINER_WORKDIR: &str = "/workspace";
pub const DEFAULT_VERIFY_MICROVM_RUNTIME: &str = "firecracker";
pub const DEFAULT_VERIFY_MICROVM_WORKDIR: &str = "/workspace";
pub const DEFAULT_VERIFY_MICROVM_VCPU_COUNT: u8 = 1;
pub const DEFAULT_VERIFY_MICROVM_MEMORY_MIB: u32 = 1024;
pub const DEFAULT_LOG_LEVEL: &str = "info";
pub const DEFAULT_SEARCH_THRESHOLD_DEFAULT: f32 = 0.65;
pub const DEFAULT_SEARCH_THRESHOLD_RUST: f32 = 0.70;
pub const DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT: f32 = 0.65;
pub const DEFAULT_SEARCH_THRESHOLD_PYTHON: f32 = 0.60;
pub const MIN_SEARCH_THRESHOLD: f32 = 0.3;
pub const MAX_SEARCH_THRESHOLD: f32 = 0.95;
pub const DEFAULT_DRIFT_THRESHOLD: f32 = 0.85;
pub const DEFAULT_DRIFT_ANALYSIS_WINDOW: &str = "100 commits";
pub const DEFAULT_DRIFT_HUB_PERCENTILE: u32 = 95;
pub const DEFAULT_DASHBOARD_PORT: u16 = 9720;
pub const DEFAULT_HEALTH_PAGERANK_WEIGHT: f64 = 0.3;
pub const DEFAULT_HEALTH_TEST_GAP_WEIGHT: f64 = 0.25;
pub const DEFAULT_HEALTH_DRIFT_WEIGHT: f64 = 0.2;
pub const DEFAULT_HEALTH_NO_SIR_WEIGHT: f64 = 0.15;
pub const DEFAULT_HEALTH_RECENCY_WEIGHT: f64 = 0.1;
pub const DEFAULT_HEALTH_SCORE_FILE_LOC_WARN: usize = 800;
pub const DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL: usize = 1500;
pub const DEFAULT_HEALTH_SCORE_TRAIT_METHOD_WARN: usize = 20;
pub const DEFAULT_HEALTH_SCORE_TRAIT_METHOD_FAIL: usize = 35;
pub const DEFAULT_HEALTH_SCORE_INTERNAL_DEP_WARN: usize = 6;
pub const DEFAULT_HEALTH_SCORE_INTERNAL_DEP_FAIL: usize = 10;
pub const DEFAULT_HEALTH_SCORE_TODO_DENSITY_WARN: f32 = 5.0;
pub const DEFAULT_HEALTH_SCORE_TODO_DENSITY_FAIL: f32 = 15.0;
pub const DEFAULT_HEALTH_SCORE_DEAD_FEATURE_WARN: usize = 1;
pub const DEFAULT_HEALTH_SCORE_DEAD_FEATURE_FAIL: usize = 5;
pub const DEFAULT_HEALTH_SCORE_STALE_REF_WARN: usize = 1;
pub const DEFAULT_HEALTH_SCORE_STALE_REF_FAIL: usize = 3;
pub const DEFAULT_HEALTH_SCORE_CHURN_30D_HIGH: usize = 15;
pub const DEFAULT_HEALTH_SCORE_CHURN_90D_HIGH: usize = 30;
pub const DEFAULT_HEALTH_SCORE_AUTHOR_COUNT_HIGH: usize = 6;
pub const DEFAULT_HEALTH_SCORE_BLAME_AGE_SPREAD_HIGH_SECS: u64 = 15_552_000;
pub const DEFAULT_HEALTH_SCORE_DRIFT_DENSITY_HIGH: f32 = 0.30;
pub const DEFAULT_HEALTH_SCORE_STALE_SIR_HIGH: f32 = 0.40;
pub const DEFAULT_HEALTH_SCORE_TEST_GAP_HIGH: f32 = 0.50;
pub const DEFAULT_HEALTH_SCORE_BOUNDARY_LEAKAGE_HIGH: f32 = 0.50;
pub const DEFAULT_HEALTH_SCORE_STRUCTURAL_WEIGHT: f64 = 0.40;
pub const DEFAULT_HEALTH_SCORE_GIT_WEIGHT: f64 = 0.25;
pub const DEFAULT_HEALTH_SCORE_SEMANTIC_WEIGHT: f64 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InferenceProviderKind {
    #[default]
    Auto,
    Tiered,
    Gemini,
    Qwen3Local,
    #[serde(rename = "openai_compat")]
    OpenAiCompat,
}

impl InferenceProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Tiered => "tiered",
            Self::Gemini => "gemini",
            Self::Qwen3Local => "qwen3_local",
            Self::OpenAiCompat => "openai_compat",
        }
    }
}

impl std::str::FromStr for InferenceProviderKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "auto" => Ok(Self::Auto),
            "tiered" => Ok(Self::Tiered),
            "gemini" => Ok(Self::Gemini),
            "qwen3_local" => Ok(Self::Qwen3Local),
            "openai_compat" => Ok(Self::OpenAiCompat),
            other => Err(format!(
                "invalid provider '{other}', expected one of: auto, tiered, gemini, qwen3_local, openai_compat"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProviderKind {
    #[default]
    Qwen3Local,
    Candle,
    GeminiNative,
    #[serde(rename = "openai_compat")]
    OpenAiCompat,
}

impl EmbeddingProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Qwen3Local => "qwen3_local",
            Self::Candle => "candle",
            Self::GeminiNative => "gemini_native",
            Self::OpenAiCompat => "openai_compat",
        }
    }
}

impl std::str::FromStr for EmbeddingProviderKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "qwen3_local" => Ok(Self::Qwen3Local),
            "candle" => Ok(Self::Candle),
            "gemini_native" => Ok(Self::GeminiNative),
            "openai_compat" => Ok(Self::OpenAiCompat),
            other => Err(format!(
                "invalid embedding provider '{other}', expected one of: qwen3_local, candle, gemini_native, openai_compat"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchRerankerKind {
    #[default]
    None,
    Candle,
    Cohere,
}

impl SearchRerankerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Candle => "candle",
            Self::Cohere => "cohere",
        }
    }
}

impl std::str::FromStr for SearchRerankerKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "none" => Ok(Self::None),
            "candle" => Ok(Self::Candle),
            "cohere" => Ok(Self::Cohere),
            other => Err(format!(
                "invalid search reranker '{other}', expected one of: none, candle, cohere"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingVectorBackend {
    #[default]
    Lancedb,
    Sqlite,
}

impl EmbeddingVectorBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lancedb => "lancedb",
            Self::Sqlite => "sqlite",
        }
    }
}

impl std::str::FromStr for EmbeddingVectorBackend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "lancedb" => Ok(Self::Lancedb),
            "sqlite" => Ok(Self::Sqlite),
            other => Err(format!(
                "invalid vector backend '{other}', expected one of: lancedb, sqlite"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GraphBackend {
    #[default]
    Surreal,
    Cozo,
    Sqlite,
}

impl GraphBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Surreal => "surreal",
            Self::Cozo => "cozo",
            Self::Sqlite => "sqlite",
        }
    }
}

impl std::str::FromStr for GraphBackend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "surreal" => Ok(Self::Surreal),
            "cozo" => Ok(Self::Cozo),
            "sqlite" => Ok(Self::Sqlite),
            other => Err(format!(
                "invalid graph backend '{other}', expected one of: surreal, cozo, sqlite"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AetherConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub inference: InferenceConfig,
    #[serde(default)]
    pub sir_quality: SirQualityConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub embeddings: EmbeddingsConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub verify: VerifyConfig,
    #[serde(default)]
    pub coupling: CouplingConfig,
    #[serde(default)]
    pub drift: DriftConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub planner: PlannerConfig,
    #[serde(default)]
    pub health_score: HealthScoreConfig,
    #[serde(default)]
    pub dashboard: DashboardConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InferenceConfig {
    #[serde(default)]
    pub provider: InferenceProviderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_sir_concurrency")]
    pub concurrency: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiered: Option<TieredConfig>,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            provider: InferenceProviderKind::Auto,
            model: None,
            endpoint: None,
            api_key_env: default_api_key_env(),
            concurrency: default_sir_concurrency(),
            tiered: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SirQualityConfig {
    #[serde(default)]
    pub triage_pass: bool,

    #[serde(default = "default_triage_priority_threshold")]
    pub triage_priority_threshold: f64,

    #[serde(default = "default_triage_confidence_threshold")]
    pub triage_confidence_threshold: f64,

    #[serde(default)]
    pub triage_provider: Option<String>,

    #[serde(default)]
    pub triage_model: Option<String>,

    #[serde(default)]
    pub triage_endpoint: Option<String>,

    #[serde(default)]
    pub triage_api_key_env: Option<String>,

    #[serde(default = "default_triage_max_symbols")]
    pub triage_max_symbols: usize,

    #[serde(default = "default_triage_concurrency")]
    pub triage_concurrency: usize,

    #[serde(default = "default_triage_timeout_secs")]
    pub triage_timeout_secs: u64,

    #[serde(default)]
    pub deep_pass: bool,

    #[serde(default = "default_deep_priority_threshold")]
    pub deep_priority_threshold: f64,

    #[serde(default = "default_deep_confidence_threshold")]
    pub deep_confidence_threshold: f64,

    #[serde(default)]
    pub deep_provider: Option<String>,

    #[serde(default)]
    pub deep_model: Option<String>,

    #[serde(default)]
    pub deep_endpoint: Option<String>,

    #[serde(default)]
    pub deep_api_key_env: Option<String>,

    #[serde(default = "default_deep_max_symbols")]
    pub deep_max_symbols: usize,

    #[serde(default = "default_deep_max_neighbors")]
    pub deep_max_neighbors: usize,

    #[serde(default = "default_deep_concurrency")]
    pub deep_concurrency: usize,

    #[serde(default = "default_deep_timeout_secs")]
    pub deep_timeout_secs: u64,
}

impl Default for SirQualityConfig {
    fn default() -> Self {
        Self {
            triage_pass: false,
            triage_priority_threshold: default_triage_priority_threshold(),
            triage_confidence_threshold: default_triage_confidence_threshold(),
            triage_provider: None,
            triage_model: None,
            triage_endpoint: None,
            triage_api_key_env: None,
            triage_max_symbols: default_triage_max_symbols(),
            triage_concurrency: default_triage_concurrency(),
            triage_timeout_secs: default_triage_timeout_secs(),
            deep_pass: false,
            deep_priority_threshold: default_deep_priority_threshold(),
            deep_confidence_threshold: default_deep_confidence_threshold(),
            deep_provider: None,
            deep_model: None,
            deep_endpoint: None,
            deep_api_key_env: None,
            deep_max_symbols: default_deep_max_symbols(),
            deep_max_neighbors: default_deep_max_neighbors(),
            deep_concurrency: default_deep_concurrency(),
            deep_timeout_secs: default_deep_timeout_secs(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TieredConfig {
    pub primary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_endpoint: Option<String>,
    #[serde(default = "default_api_key_env")]
    pub primary_api_key_env: String,
    #[serde(default = "default_tiered_primary_threshold")]
    pub primary_threshold: f64,
    #[serde(
        default = "default_tiered_fallback_model",
        skip_serializing_if = "Option::is_none"
    )]
    pub fallback_model: Option<String>,
    #[serde(
        default = "default_tiered_fallback_endpoint",
        skip_serializing_if = "Option::is_none"
    )]
    pub fallback_endpoint: Option<String>,
    #[serde(default = "default_tiered_retry_with_fallback")]
    pub retry_with_fallback: bool,
}

impl Default for TieredConfig {
    fn default() -> Self {
        Self {
            primary: "gemini".to_owned(),
            primary_model: None,
            primary_endpoint: None,
            primary_api_key_env: default_api_key_env(),
            primary_threshold: default_tiered_primary_threshold(),
            fallback_model: default_tiered_fallback_model(),
            fallback_endpoint: default_tiered_fallback_endpoint(),
            retry_with_fallback: default_tiered_retry_with_fallback(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_mirror_sir_files")]
    pub mirror_sir_files: bool,
    #[serde(default)]
    pub graph_backend: GraphBackend,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            mirror_sir_files: default_mirror_sir_files(),
            graph_backend: default_graph_backend(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    #[serde(default = "default_embeddings_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub provider: EmbeddingProviderKind,
    #[serde(default)]
    pub vector_backend: EmbeddingVectorBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
    #[serde(default, skip_serializing_if = "CandleEmbeddingsConfig::is_empty")]
    pub candle: CandleEmbeddingsConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CandleEmbeddingsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<String>,
}

impl CandleEmbeddingsConfig {
    fn is_empty(&self) -> bool {
        self.model_dir.is_none()
    }
}

impl Default for EmbeddingsConfig {
    fn default() -> Self {
        Self {
            enabled: default_embeddings_enabled(),
            provider: EmbeddingProviderKind::Qwen3Local,
            vector_backend: EmbeddingVectorBackend::Lancedb,
            model: None,
            endpoint: None,
            api_key_env: None,
            task_type: None,
            dimensions: None,
            candle: CandleEmbeddingsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(default)]
    pub reranker: SearchRerankerKind,
    #[serde(default = "default_rerank_window")]
    pub rerank_window: u32,
    #[serde(default)]
    pub thresholds: SearchThresholdsConfig,
    #[serde(
        default,
        skip_serializing_if = "SearchCalibratedThresholdsConfig::is_empty"
    )]
    pub calibrated_thresholds: SearchCalibratedThresholdsConfig,
    #[serde(default, skip_serializing_if = "SearchCandleConfig::is_empty")]
    pub candle: SearchCandleConfig,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            reranker: SearchRerankerKind::None,
            rerank_window: default_rerank_window(),
            thresholds: SearchThresholdsConfig::default(),
            calibrated_thresholds: SearchCalibratedThresholdsConfig::default(),
            candle: SearchCandleConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchThresholdsConfig {
    #[serde(default = "default_search_threshold_default")]
    pub default: f32,
    #[serde(default = "default_search_threshold_rust")]
    pub rust: f32,
    #[serde(default = "default_search_threshold_typescript")]
    pub typescript: f32,
    #[serde(default = "default_search_threshold_python")]
    pub python: f32,
}

impl SearchThresholdsConfig {
    pub fn value_for_language(&self, language: &str) -> f32 {
        match normalize_threshold_language(language) {
            "rust" => self.rust,
            "typescript" => self.typescript,
            "python" => self.python,
            _ => self.default,
        }
    }

    pub fn baseline_for_language(language: &str) -> f32 {
        match normalize_threshold_language(language) {
            "rust" => DEFAULT_SEARCH_THRESHOLD_RUST,
            "typescript" => DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT,
            "python" => DEFAULT_SEARCH_THRESHOLD_PYTHON,
            _ => DEFAULT_SEARCH_THRESHOLD_DEFAULT,
        }
    }

    pub fn is_manual_override_for_language(&self, language: &str) -> bool {
        (self.value_for_language(language) - Self::baseline_for_language(language)).abs() > 1e-6
    }
}

impl Default for SearchThresholdsConfig {
    fn default() -> Self {
        Self {
            default: default_search_threshold_default(),
            rust: default_search_threshold_rust(),
            typescript: default_search_threshold_typescript(),
            python: default_search_threshold_python(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SearchCalibratedThresholdsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typescript: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<f32>,
}

impl SearchCalibratedThresholdsConfig {
    pub fn is_empty(&self) -> bool {
        self.default.is_none()
            && self.rust.is_none()
            && self.typescript.is_none()
            && self.python.is_none()
    }

    pub fn value_for_language(&self, language: &str) -> Option<f32> {
        match normalize_threshold_language(language) {
            "rust" => self.rust,
            "typescript" => self.typescript,
            "python" => self.python,
            _ => self.default,
        }
    }

    pub fn set_for_language(&mut self, language: &str, value: Option<f32>) {
        match normalize_threshold_language(language) {
            "rust" => self.rust = value,
            "typescript" => self.typescript = value,
            "python" => self.python = value,
            _ => self.default = value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SearchCandleConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<String>,
}

impl SearchCandleConfig {
    fn is_empty(&self) -> bool {
        self.model_dir.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub cohere: CohereProviderConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CohereProviderConfig {
    #[serde(default = "default_cohere_api_key_env")]
    pub api_key_env: String,
}

impl Default for CohereProviderConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_cohere_api_key_env(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyConfig {
    #[serde(default = "default_verify_commands")]
    pub commands: Vec<String>,
    #[serde(default)]
    pub mode: VerifyMode,
    #[serde(default)]
    pub container: VerifyContainerConfig,
    #[serde(default)]
    pub microvm: VerifyMicrovmConfig,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            commands: default_verify_commands(),
            mode: VerifyMode::Host,
            container: VerifyContainerConfig::default(),
            microvm: VerifyMicrovmConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    #[default]
    Host,
    Container,
    Microvm,
}

impl VerifyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Container => "container",
            Self::Microvm => "microvm",
        }
    }
}

impl std::str::FromStr for VerifyMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "host" => Ok(Self::Host),
            "container" => Ok(Self::Container),
            "microvm" => Ok(Self::Microvm),
            other => Err(format!(
                "invalid verify mode '{other}', expected one of: host, container, microvm"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyContainerConfig {
    #[serde(default = "default_verify_container_runtime")]
    pub runtime: String,
    #[serde(default = "default_verify_container_image")]
    pub image: String,
    #[serde(default = "default_verify_container_workdir")]
    pub workdir: String,
    #[serde(default)]
    pub fallback_to_host_on_unavailable: bool,
}

impl Default for VerifyContainerConfig {
    fn default() -> Self {
        Self {
            runtime: default_verify_container_runtime(),
            image: default_verify_container_image(),
            workdir: default_verify_container_workdir(),
            fallback_to_host_on_unavailable: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyMicrovmConfig {
    #[serde(default = "default_verify_microvm_runtime")]
    pub runtime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rootfs_image: Option<String>,
    #[serde(default = "default_verify_microvm_workdir")]
    pub workdir: String,
    #[serde(default = "default_verify_microvm_vcpu_count")]
    pub vcpu_count: u8,
    #[serde(default = "default_verify_microvm_memory_mib")]
    pub memory_mib: u32,
    #[serde(default)]
    pub fallback_to_container_on_unavailable: bool,
    #[serde(default)]
    pub fallback_to_host_on_unavailable: bool,
}

impl Default for VerifyMicrovmConfig {
    fn default() -> Self {
        Self {
            runtime: default_verify_microvm_runtime(),
            kernel_image: None,
            rootfs_image: None,
            workdir: default_verify_microvm_workdir(),
            vcpu_count: default_verify_microvm_vcpu_count(),
            memory_mib: default_verify_microvm_memory_mib(),
            fallback_to_container_on_unavailable: false,
            fallback_to_host_on_unavailable: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CouplingConfig {
    #[serde(default = "default_coupling_enabled")]
    pub enabled: bool,
    #[serde(default = "default_coupling_commit_window")]
    pub commit_window: u32,
    #[serde(default = "default_coupling_min_co_change_count")]
    pub min_co_change_count: u32,
    #[serde(default = "default_coupling_exclude_patterns")]
    pub exclude_patterns: Vec<String>,
    #[serde(default = "default_coupling_bulk_commit_threshold")]
    pub bulk_commit_threshold: u32,
    #[serde(default = "default_coupling_temporal_weight")]
    pub temporal_weight: f32,
    #[serde(default = "default_coupling_static_weight")]
    pub static_weight: f32,
    #[serde(default = "default_coupling_semantic_weight")]
    pub semantic_weight: f32,
}

impl Default for CouplingConfig {
    fn default() -> Self {
        Self {
            enabled: default_coupling_enabled(),
            commit_window: default_coupling_commit_window(),
            min_co_change_count: default_coupling_min_co_change_count(),
            exclude_patterns: default_coupling_exclude_patterns(),
            bulk_commit_threshold: default_coupling_bulk_commit_threshold(),
            temporal_weight: default_coupling_temporal_weight(),
            static_weight: default_coupling_static_weight(),
            semantic_weight: default_coupling_semantic_weight(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftConfig {
    #[serde(default = "default_drift_enabled")]
    pub enabled: bool,
    #[serde(default = "default_drift_threshold")]
    pub drift_threshold: f32,
    #[serde(default = "default_drift_analysis_window")]
    pub analysis_window: String,
    #[serde(default = "default_drift_auto_analyze")]
    pub auto_analyze: bool,
    #[serde(default = "default_drift_hub_percentile")]
    pub hub_percentile: u32,
}

impl Default for DriftConfig {
    fn default() -> Self {
        Self {
            enabled: default_drift_enabled(),
            drift_threshold: default_drift_threshold(),
            analysis_window: default_drift_analysis_window(),
            auto_analyze: default_drift_auto_analyze(),
            hub_percentile: default_drift_hub_percentile(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub risk_weights: RiskWeights,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: default_health_enabled(),
            risk_weights: RiskWeights::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskWeights {
    #[serde(default = "default_health_pagerank_weight")]
    pub pagerank: f64,
    #[serde(default = "default_health_test_gap_weight")]
    pub test_gap: f64,
    #[serde(default = "default_health_drift_weight")]
    pub drift: f64,
    #[serde(default = "default_health_no_sir_weight")]
    pub no_sir: f64,
    #[serde(default = "default_health_recency_weight")]
    pub recency: f64,
}

impl Default for RiskWeights {
    fn default() -> Self {
        Self {
            pagerank: default_health_pagerank_weight(),
            test_gap: default_health_test_gap_weight(),
            drift: default_health_drift_weight(),
            no_sir: default_health_no_sir_weight(),
            recency: default_health_recency_weight(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerConfig {
    #[serde(default = "default_planner_semantic_rescue_threshold")]
    pub semantic_rescue_threshold: f32,
    #[serde(default = "default_planner_semantic_rescue_max_k")]
    pub semantic_rescue_max_k: usize,
    #[serde(default = "default_planner_community_resolution")]
    pub community_resolution: f64,
    #[serde(default = "default_planner_min_community_size")]
    pub min_community_size: usize,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            semantic_rescue_threshold: default_planner_semantic_rescue_threshold(),
            semantic_rescue_max_k: default_planner_semantic_rescue_max_k(),
            community_resolution: default_planner_community_resolution(),
            min_community_size: default_planner_min_community_size(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthScoreConfig {
    #[serde(default = "default_health_score_file_loc_warn")]
    pub file_loc_warn: usize,
    #[serde(default = "default_health_score_file_loc_fail")]
    pub file_loc_fail: usize,
    #[serde(default = "default_health_score_trait_method_warn")]
    pub trait_method_warn: usize,
    #[serde(default = "default_health_score_trait_method_fail")]
    pub trait_method_fail: usize,
    #[serde(default = "default_health_score_internal_dep_warn")]
    pub internal_dep_warn: usize,
    #[serde(default = "default_health_score_internal_dep_fail")]
    pub internal_dep_fail: usize,
    #[serde(default = "default_health_score_todo_density_warn")]
    pub todo_density_warn: f32,
    #[serde(default = "default_health_score_todo_density_fail")]
    pub todo_density_fail: f32,
    #[serde(default = "default_health_score_dead_feature_warn")]
    pub dead_feature_warn: usize,
    #[serde(default = "default_health_score_dead_feature_fail")]
    pub dead_feature_fail: usize,
    #[serde(default = "default_health_score_stale_ref_warn")]
    pub stale_ref_warn: usize,
    #[serde(default = "default_health_score_stale_ref_fail")]
    pub stale_ref_fail: usize,
    #[serde(default = "default_health_score_stale_ref_patterns")]
    pub stale_ref_patterns: Vec<String>,
    #[serde(default = "default_health_score_churn_30d_high")]
    pub churn_30d_high: usize,
    #[serde(default = "default_health_score_churn_90d_high")]
    pub churn_90d_high: usize,
    #[serde(default = "default_health_score_author_count_high")]
    pub author_count_high: usize,
    #[serde(default = "default_health_score_blame_age_spread_high_secs")]
    pub blame_age_spread_high_secs: u64,
    #[serde(default = "default_health_score_drift_density_high")]
    pub drift_density_high: f32,
    #[serde(default = "default_health_score_stale_sir_high")]
    pub stale_sir_high: f32,
    #[serde(default = "default_health_score_test_gap_high")]
    pub test_gap_high: f32,
    #[serde(default = "default_health_score_boundary_leakage_high")]
    pub boundary_leakage_high: f32,
    #[serde(default)]
    pub structural_weight: Option<f64>,
    #[serde(default)]
    pub git_weight: Option<f64>,
    #[serde(default)]
    pub semantic_weight: Option<f64>,
}

impl Default for HealthScoreConfig {
    fn default() -> Self {
        Self {
            file_loc_warn: default_health_score_file_loc_warn(),
            file_loc_fail: default_health_score_file_loc_fail(),
            trait_method_warn: default_health_score_trait_method_warn(),
            trait_method_fail: default_health_score_trait_method_fail(),
            internal_dep_warn: default_health_score_internal_dep_warn(),
            internal_dep_fail: default_health_score_internal_dep_fail(),
            todo_density_warn: default_health_score_todo_density_warn(),
            todo_density_fail: default_health_score_todo_density_fail(),
            dead_feature_warn: default_health_score_dead_feature_warn(),
            dead_feature_fail: default_health_score_dead_feature_fail(),
            stale_ref_warn: default_health_score_stale_ref_warn(),
            stale_ref_fail: default_health_score_stale_ref_fail(),
            stale_ref_patterns: default_health_score_stale_ref_patterns(),
            churn_30d_high: default_health_score_churn_30d_high(),
            churn_90d_high: default_health_score_churn_90d_high(),
            author_count_high: default_health_score_author_count_high(),
            blame_age_spread_high_secs: default_health_score_blame_age_spread_high_secs(),
            drift_density_high: default_health_score_drift_density_high(),
            stale_sir_high: default_health_score_stale_sir_high(),
            test_gap_high: default_health_score_test_gap_high(),
            boundary_leakage_high: default_health_score_boundary_leakage_high(),
            structural_weight: None,
            git_weight: None,
            semantic_weight: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardConfig {
    #[serde(default = "default_dashboard_port")]
    pub port: u16,
    #[serde(default = "default_dashboard_enabled")]
    pub enabled: bool,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            port: default_dashboard_port(),
            enabled: default_dashboard_enabled(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config TOML: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("failed to serialize config TOML: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

pub fn aether_dir(workspace_root: impl AsRef<Path>) -> PathBuf {
    workspace_root.as_ref().join(AETHER_DIR_NAME)
}

pub fn config_path(workspace_root: impl AsRef<Path>) -> PathBuf {
    aether_dir(workspace_root).join(CONFIG_FILE_NAME)
}

pub fn load_workspace_config(
    workspace_root: impl AsRef<Path>,
) -> Result<AetherConfig, ConfigError> {
    let path = config_path(workspace_root);
    if !path.exists() {
        return Ok(AetherConfig::default());
    }

    let raw = fs::read_to_string(path)?;
    let parsed = parse_workspace_config_str(&raw)?;
    Ok(normalize_config(parsed))
}

pub fn ensure_workspace_config(
    workspace_root: impl AsRef<Path>,
) -> Result<AetherConfig, ConfigError> {
    let workspace_root = workspace_root.as_ref();
    fs::create_dir_all(aether_dir(workspace_root))?;

    let path = config_path(workspace_root);
    if path.exists() {
        return load_workspace_config(workspace_root);
    }

    let config = AetherConfig::default();
    save_workspace_config(workspace_root, &config)?;

    Ok(config)
}

pub fn save_workspace_config(
    workspace_root: impl AsRef<Path>,
    config: &AetherConfig,
) -> Result<(), ConfigError> {
    let workspace_root = workspace_root.as_ref();
    fs::create_dir_all(aether_dir(workspace_root))?;
    let normalized = normalize_config(config.clone());
    let content = toml::to_string_pretty(&normalized)?;
    fs::write(config_path(workspace_root), content)?;
    Ok(())
}

fn parse_workspace_config_str(raw: &str) -> Result<AetherConfig, ConfigError> {
    let mut parsed: toml::Value = toml::from_str(raw)?;
    rewrite_legacy_sir_quality_keys(&mut parsed);
    parsed.try_into().map_err(Into::into)
}

fn rewrite_legacy_sir_quality_keys(parsed: &mut toml::Value) {
    let Some(root) = parsed.as_table_mut() else {
        return;
    };
    let Some(sir_quality) = root
        .get_mut("sir_quality")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };

    let has_new_triage_schema = sir_quality.keys().any(|key| key.starts_with("triage_"));
    if has_new_triage_schema {
        return;
    }

    for (legacy_key, triage_key) in [
        ("deep_pass", "triage_pass"),
        ("deep_provider", "triage_provider"),
        ("deep_model", "triage_model"),
        ("deep_endpoint", "triage_endpoint"),
        ("deep_api_key_env", "triage_api_key_env"),
        ("deep_priority_threshold", "triage_priority_threshold"),
        ("deep_confidence_threshold", "triage_confidence_threshold"),
        ("deep_max_symbols", "triage_max_symbols"),
        ("deep_concurrency", "triage_concurrency"),
    ] {
        if let Some(value) = sir_quality.remove(legacy_key) {
            sir_quality.insert(triage_key.to_owned(), value);
        }
    }
}

pub fn validate_config(config: &AetherConfig) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();

    if config.storage.graph_backend == GraphBackend::Cozo {
        warnings.push(ConfigWarning {
            code: "graph_backend_cozo_deprecated",
            message:
                "storage.graph_backend=cozo is deprecated; run `aether graph-migrate` and switch to surreal"
                    .to_owned(),
        });
    }

    if !config.embeddings.enabled {
        if config.embeddings.model.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_model_ignored",
                message:
                    "embeddings.model is set but embeddings.enabled=false; model will be ignored"
                        .to_owned(),
            });
        }
        if config.embeddings.endpoint.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_endpoint_ignored",
                message: "embeddings.endpoint is set but embeddings.enabled=false; endpoint will be ignored".to_owned(),
            });
        }
        if config.embeddings.candle.model_dir.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_candle_model_dir_ignored",
                message: "embeddings.candle.model_dir is set but embeddings.enabled=false; model_dir will be ignored".to_owned(),
            });
        }
    } else if matches!(
        config.embeddings.provider,
        EmbeddingProviderKind::Qwen3Local
    ) && config.embeddings.candle.model_dir.is_some()
    {
        warnings.push(ConfigWarning {
            code: "embeddings_candle_model_dir_unused_for_qwen3_local",
            message: "embeddings.provider=qwen3_local ignores embeddings.candle.model_dir"
                .to_owned(),
        });
    } else if matches!(
        config.embeddings.provider,
        EmbeddingProviderKind::OpenAiCompat
    ) {
        if config.embeddings.endpoint.is_none() {
            warnings.push(ConfigWarning {
                code: "embeddings_endpoint_missing_for_openai_compat",
                message: "embeddings.provider=openai_compat requires embeddings.endpoint"
                    .to_owned(),
            });
        }
    } else if matches!(
        config.embeddings.provider,
        EmbeddingProviderKind::GeminiNative
    ) {
        if config.embeddings.endpoint.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_endpoint_unused_for_gemini_native",
                message: "embeddings.provider=gemini_native ignores embeddings.endpoint".to_owned(),
            });
        }
        if config.embeddings.task_type.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_task_type_unused_for_gemini_native",
                message: "embeddings.provider=gemini_native ignores embeddings.task_type"
                    .to_owned(),
            });
        }
    }

    if matches!(config.search.reranker, SearchRerankerKind::None)
        && config.search.candle.model_dir.is_some()
    {
        warnings.push(ConfigWarning {
            code: "search_candle_model_dir_unused_for_none",
            message: "search.reranker=none ignores search.candle.model_dir".to_owned(),
        });
    } else if matches!(config.search.reranker, SearchRerankerKind::Cohere)
        && config.search.candle.model_dir.is_some()
    {
        warnings.push(ConfigWarning {
            code: "search_candle_model_dir_unused_for_cohere",
            message: "search.reranker=cohere ignores search.candle.model_dir".to_owned(),
        });
    }

    if !matches!(config.search.reranker, SearchRerankerKind::Cohere)
        && config.providers.cohere.api_key_env != DEFAULT_COHERE_API_KEY_ENV
    {
        warnings.push(ConfigWarning {
            code: "providers_cohere_api_key_env_ignored",
            message: "providers.cohere.api_key_env is ignored unless search.reranker=cohere"
                .to_owned(),
        });
    }

    match config.inference.provider {
        InferenceProviderKind::Auto => {}
        InferenceProviderKind::Tiered => {
            if config.inference.tiered.is_none() {
                warnings.push(ConfigWarning {
                    code: "inference_tiered_config_missing",
                    message: "inference.provider=tiered requires [inference.tiered] config"
                        .to_owned(),
                });
            }
            if config.inference.model.is_some() {
                warnings.push(ConfigWarning {
                    code: "inference_model_ignored_for_tiered",
                    message: "inference.provider=tiered ignores inference.model".to_owned(),
                });
            }
            if config.inference.endpoint.is_some() {
                warnings.push(ConfigWarning {
                    code: "inference_endpoint_ignored_for_tiered",
                    message: "inference.provider=tiered ignores inference.endpoint".to_owned(),
                });
            }
            if config.inference.api_key_env != DEFAULT_GEMINI_API_KEY_ENV {
                warnings.push(ConfigWarning {
                    code: "inference_api_key_env_ignored_for_tiered",
                    message: "inference.api_key_env is ignored when inference.provider=tiered"
                        .to_owned(),
                });
            }
        }
        InferenceProviderKind::Gemini => {
            if config.inference.endpoint.is_some() {
                warnings.push(ConfigWarning {
                    code: "inference_endpoint_ignored_for_gemini",
                    message: "inference.provider=gemini ignores inference.endpoint".to_owned(),
                });
            }
        }
        InferenceProviderKind::Qwen3Local => {
            if config.inference.api_key_env != DEFAULT_GEMINI_API_KEY_ENV {
                warnings.push(ConfigWarning {
                    code: "inference_api_key_env_ignored_for_qwen3_local",
                    message: "inference.api_key_env is ignored when inference.provider=qwen3_local"
                        .to_owned(),
                });
            }
        }
        InferenceProviderKind::OpenAiCompat => {}
    }

    if config.verify.commands.is_empty() {
        warnings.push(ConfigWarning {
            code: "verify_commands_empty",
            message: "verify.commands is empty; aetherd --verify and aether_verify will have no commands to run".to_owned(),
        });
    }

    let container_defaults = VerifyContainerConfig::default();
    let container_settings_ignored =
        config.verify.mode == VerifyMode::Host && config.verify.container != container_defaults;
    if container_settings_ignored {
        warnings.push(ConfigWarning {
            code: "verify_container_settings_ignored_for_host",
            message: "verify.mode=host ignores verify.container settings".to_owned(),
        });
    }

    let microvm_defaults = VerifyMicrovmConfig::default();
    let microvm_settings_ignored =
        config.verify.mode != VerifyMode::Microvm && config.verify.microvm != microvm_defaults;
    if microvm_settings_ignored {
        warnings.push(ConfigWarning {
            code: "verify_microvm_settings_ignored_for_non_microvm",
            message: "verify.microvm settings are ignored unless verify.mode=microvm".to_owned(),
        });
    }

    if config.verify.mode == VerifyMode::Microvm
        && (config.verify.microvm.kernel_image.is_none()
            || config.verify.microvm.rootfs_image.is_none())
    {
        warnings.push(ConfigWarning {
            code: "verify_microvm_assets_missing",
            message: "verify.mode=microvm requires verify.microvm.kernel_image and verify.microvm.rootfs_image".to_owned(),
        });
    }

    let coupling_sum = config.coupling.temporal_weight
        + config.coupling.static_weight
        + config.coupling.semantic_weight;
    if (coupling_sum - 1.0).abs() > 0.01 {
        warnings.push(ConfigWarning {
            code: "coupling_weights_normalized",
            message: format!(
                "coupling weights should sum to 1.0 (found {coupling_sum:.3}); values will be normalized"
            ),
        });
    }

    warnings
}

fn default_api_key_env() -> String {
    DEFAULT_GEMINI_API_KEY_ENV.to_owned()
}

fn default_sir_concurrency() -> usize {
    DEFAULT_SIR_CONCURRENCY
}

fn default_triage_priority_threshold() -> f64 {
    0.7
}

fn default_triage_confidence_threshold() -> f64 {
    0.85
}

fn default_triage_max_symbols() -> usize {
    0
}

fn default_triage_concurrency() -> usize {
    4
}

fn default_triage_timeout_secs() -> u64 {
    180
}

fn default_deep_priority_threshold() -> f64 {
    0.9
}

fn default_deep_confidence_threshold() -> f64 {
    0.85
}

fn default_deep_max_symbols() -> usize {
    20
}

fn default_deep_max_neighbors() -> usize {
    10
}

fn default_deep_concurrency() -> usize {
    4
}

fn default_deep_timeout_secs() -> u64 {
    180
}

fn default_tiered_primary_threshold() -> f64 {
    0.8
}

fn default_tiered_fallback_model() -> Option<String> {
    Some(DEFAULT_QWEN_MODEL.to_owned())
}

fn default_tiered_fallback_endpoint() -> Option<String> {
    Some(DEFAULT_QWEN_ENDPOINT.to_owned())
}

fn default_tiered_retry_with_fallback() -> bool {
    true
}

fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_owned()
}

fn default_mirror_sir_files() -> bool {
    true
}

fn default_graph_backend() -> GraphBackend {
    GraphBackend::Surreal
}

fn default_embeddings_enabled() -> bool {
    false
}

fn default_rerank_window() -> u32 {
    50
}

fn default_search_threshold_default() -> f32 {
    DEFAULT_SEARCH_THRESHOLD_DEFAULT
}

fn default_search_threshold_rust() -> f32 {
    DEFAULT_SEARCH_THRESHOLD_RUST
}

fn default_search_threshold_typescript() -> f32 {
    DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT
}

fn default_search_threshold_python() -> f32 {
    DEFAULT_SEARCH_THRESHOLD_PYTHON
}

fn default_cohere_api_key_env() -> String {
    DEFAULT_COHERE_API_KEY_ENV.to_owned()
}

fn default_verify_commands() -> Vec<String> {
    vec![
        "cargo fmt --all --check".to_owned(),
        "cargo clippy --workspace -- -D warnings".to_owned(),
        "cargo test --workspace".to_owned(),
    ]
}

fn default_verify_container_runtime() -> String {
    DEFAULT_VERIFY_CONTAINER_RUNTIME.to_owned()
}

fn default_verify_container_image() -> String {
    DEFAULT_VERIFY_CONTAINER_IMAGE.to_owned()
}

fn default_verify_container_workdir() -> String {
    DEFAULT_VERIFY_CONTAINER_WORKDIR.to_owned()
}

fn default_verify_microvm_runtime() -> String {
    DEFAULT_VERIFY_MICROVM_RUNTIME.to_owned()
}

fn default_verify_microvm_workdir() -> String {
    DEFAULT_VERIFY_MICROVM_WORKDIR.to_owned()
}

fn default_verify_microvm_vcpu_count() -> u8 {
    DEFAULT_VERIFY_MICROVM_VCPU_COUNT
}

fn default_verify_microvm_memory_mib() -> u32 {
    DEFAULT_VERIFY_MICROVM_MEMORY_MIB
}

fn default_coupling_enabled() -> bool {
    true
}

fn default_coupling_commit_window() -> u32 {
    500
}

fn default_coupling_min_co_change_count() -> u32 {
    3
}

fn default_coupling_exclude_patterns() -> Vec<String> {
    vec![
        "*.lock".to_owned(),
        "*.generated.*".to_owned(),
        ".gitignore".to_owned(),
    ]
}

fn default_coupling_bulk_commit_threshold() -> u32 {
    30
}

fn default_coupling_temporal_weight() -> f32 {
    0.5
}

fn default_coupling_static_weight() -> f32 {
    0.3
}

fn default_coupling_semantic_weight() -> f32 {
    0.2
}

fn default_drift_enabled() -> bool {
    true
}

fn default_drift_threshold() -> f32 {
    DEFAULT_DRIFT_THRESHOLD
}

fn default_drift_analysis_window() -> String {
    DEFAULT_DRIFT_ANALYSIS_WINDOW.to_owned()
}

fn default_drift_auto_analyze() -> bool {
    false
}

fn default_drift_hub_percentile() -> u32 {
    DEFAULT_DRIFT_HUB_PERCENTILE
}

fn default_health_enabled() -> bool {
    true
}

fn default_health_pagerank_weight() -> f64 {
    DEFAULT_HEALTH_PAGERANK_WEIGHT
}

fn default_health_test_gap_weight() -> f64 {
    DEFAULT_HEALTH_TEST_GAP_WEIGHT
}

fn default_health_drift_weight() -> f64 {
    DEFAULT_HEALTH_DRIFT_WEIGHT
}

fn default_health_no_sir_weight() -> f64 {
    DEFAULT_HEALTH_NO_SIR_WEIGHT
}

fn default_health_recency_weight() -> f64 {
    DEFAULT_HEALTH_RECENCY_WEIGHT
}

fn default_planner_semantic_rescue_threshold() -> f32 {
    0.70
}

fn default_planner_semantic_rescue_max_k() -> usize {
    3
}

fn default_planner_community_resolution() -> f64 {
    0.5
}

fn default_planner_min_community_size() -> usize {
    3
}

fn default_health_score_file_loc_warn() -> usize {
    DEFAULT_HEALTH_SCORE_FILE_LOC_WARN
}

fn default_health_score_file_loc_fail() -> usize {
    DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL
}

fn default_health_score_trait_method_warn() -> usize {
    DEFAULT_HEALTH_SCORE_TRAIT_METHOD_WARN
}

fn default_health_score_trait_method_fail() -> usize {
    DEFAULT_HEALTH_SCORE_TRAIT_METHOD_FAIL
}

fn default_health_score_internal_dep_warn() -> usize {
    DEFAULT_HEALTH_SCORE_INTERNAL_DEP_WARN
}

fn default_health_score_internal_dep_fail() -> usize {
    DEFAULT_HEALTH_SCORE_INTERNAL_DEP_FAIL
}

fn default_health_score_todo_density_warn() -> f32 {
    DEFAULT_HEALTH_SCORE_TODO_DENSITY_WARN
}

fn default_health_score_todo_density_fail() -> f32 {
    DEFAULT_HEALTH_SCORE_TODO_DENSITY_FAIL
}

fn default_health_score_dead_feature_warn() -> usize {
    DEFAULT_HEALTH_SCORE_DEAD_FEATURE_WARN
}

fn default_health_score_dead_feature_fail() -> usize {
    DEFAULT_HEALTH_SCORE_DEAD_FEATURE_FAIL
}

fn default_health_score_stale_ref_warn() -> usize {
    DEFAULT_HEALTH_SCORE_STALE_REF_WARN
}

fn default_health_score_stale_ref_fail() -> usize {
    DEFAULT_HEALTH_SCORE_STALE_REF_FAIL
}

fn default_health_score_stale_ref_patterns() -> Vec<String> {
    vec![
        "CozoGraphStore".to_owned(),
        "cozo".to_owned(),
        "CozoDB".to_owned(),
    ]
}

fn default_health_score_churn_30d_high() -> usize {
    DEFAULT_HEALTH_SCORE_CHURN_30D_HIGH
}

fn default_health_score_churn_90d_high() -> usize {
    DEFAULT_HEALTH_SCORE_CHURN_90D_HIGH
}

fn default_health_score_author_count_high() -> usize {
    DEFAULT_HEALTH_SCORE_AUTHOR_COUNT_HIGH
}

fn default_health_score_blame_age_spread_high_secs() -> u64 {
    DEFAULT_HEALTH_SCORE_BLAME_AGE_SPREAD_HIGH_SECS
}

fn default_health_score_drift_density_high() -> f32 {
    DEFAULT_HEALTH_SCORE_DRIFT_DENSITY_HIGH
}

fn default_health_score_stale_sir_high() -> f32 {
    DEFAULT_HEALTH_SCORE_STALE_SIR_HIGH
}

fn default_health_score_test_gap_high() -> f32 {
    DEFAULT_HEALTH_SCORE_TEST_GAP_HIGH
}

fn default_health_score_boundary_leakage_high() -> f32 {
    DEFAULT_HEALTH_SCORE_BOUNDARY_LEAKAGE_HIGH
}

fn default_dashboard_port() -> u16 {
    DEFAULT_DASHBOARD_PORT
}

fn default_dashboard_enabled() -> bool {
    true
}

fn normalize_optional(input: Option<String>) -> Option<String> {
    input
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_with_default(input: String, default: String) -> String {
    let normalized = input.trim();
    if normalized.is_empty() {
        default
    } else {
        normalized.to_owned()
    }
}

fn normalize_commands(commands: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();

    for raw in commands {
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }

        if !normalized.iter().any(|existing| existing == value) {
            normalized.push(value.to_owned());
        }
    }

    normalized
}

fn normalize_threshold_language(language: &str) -> &str {
    let normalized = language.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "rust" | "rs" => "rust",
        "typescript" | "ts" | "tsx" | "javascript" | "js" => "typescript",
        "python" | "py" => "python",
        _ => "default",
    }
}

fn normalize_threshold_value(value: f32, fallback: f32) -> f32 {
    if !value.is_finite() {
        return fallback;
    }

    value.clamp(MIN_SEARCH_THRESHOLD, MAX_SEARCH_THRESHOLD)
}

fn normalize_optional_threshold(value: Option<f32>) -> Option<f32> {
    value.and_then(|inner| {
        if !inner.is_finite() {
            None
        } else {
            Some(inner.clamp(MIN_SEARCH_THRESHOLD, MAX_SEARCH_THRESHOLD))
        }
    })
}

fn normalize_probability(value: f64, fallback: f64) -> f64 {
    if !value.is_finite() {
        fallback
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn normalize_provider_concurrency(provider: InferenceProviderKind, concurrency: usize) -> usize {
    if provider == InferenceProviderKind::Gemini && concurrency == DEFAULT_SIR_CONCURRENCY {
        GEMINI_DEFAULT_CONCURRENCY
    } else {
        concurrency.max(1)
    }
}

fn normalize_sir_quality_config(config: &mut SirQualityConfig) {
    config.triage_priority_threshold = normalize_probability(
        config.triage_priority_threshold,
        default_triage_priority_threshold(),
    );
    config.triage_confidence_threshold = normalize_probability(
        config.triage_confidence_threshold,
        default_triage_confidence_threshold(),
    );
    config.triage_provider = normalize_optional(config.triage_provider.take());
    config.triage_model = normalize_optional(config.triage_model.take());
    config.triage_endpoint = normalize_optional(config.triage_endpoint.take());
    config.triage_api_key_env = normalize_optional(config.triage_api_key_env.take());
    if config.triage_concurrency == 0 {
        config.triage_concurrency = default_triage_concurrency();
    }
    if config.triage_timeout_secs == 0 {
        config.triage_timeout_secs = default_triage_timeout_secs();
    }

    config.deep_priority_threshold = normalize_probability(
        config.deep_priority_threshold,
        default_deep_priority_threshold(),
    );
    config.deep_confidence_threshold = normalize_probability(
        config.deep_confidence_threshold,
        default_deep_confidence_threshold(),
    );
    config.deep_provider = normalize_optional(config.deep_provider.take());
    config.deep_model = normalize_optional(config.deep_model.take());
    config.deep_endpoint = normalize_optional(config.deep_endpoint.take());
    config.deep_api_key_env = normalize_optional(config.deep_api_key_env.take());
    if config.deep_max_neighbors == 0 {
        config.deep_max_neighbors = default_deep_max_neighbors();
    }
    if config.deep_concurrency == 0 {
        config.deep_concurrency = default_deep_concurrency();
    }
    if config.deep_timeout_secs == 0 {
        config.deep_timeout_secs = default_deep_timeout_secs();
    }
}

fn normalize_patterns(patterns: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for raw in patterns {
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }
        if !normalized.iter().any(|existing| existing == value) {
            normalized.push(value.to_owned());
        }
    }
    normalized
}

fn normalize_weight_value(value: f32, fallback: f32) -> f32 {
    if !value.is_finite() || value < 0.0 {
        fallback
    } else {
        value
    }
}

fn normalize_coupling_weights(config: &mut CouplingConfig) {
    config.temporal_weight =
        normalize_weight_value(config.temporal_weight, default_coupling_temporal_weight());
    config.static_weight =
        normalize_weight_value(config.static_weight, default_coupling_static_weight());
    config.semantic_weight =
        normalize_weight_value(config.semantic_weight, default_coupling_semantic_weight());

    let sum = config.temporal_weight + config.static_weight + config.semantic_weight;
    if (sum - 1.0).abs() <= 0.01 {
        return;
    }

    if sum <= f32::EPSILON {
        config.temporal_weight = default_coupling_temporal_weight();
        config.static_weight = default_coupling_static_weight();
        config.semantic_weight = default_coupling_semantic_weight();
        eprintln!("AETHER config warning: coupling weights summed to {sum:.3}; reset to defaults");
        return;
    }

    config.temporal_weight /= sum;
    config.static_weight /= sum;
    config.semantic_weight /= sum;
    eprintln!("AETHER config warning: coupling weights summed to {sum:.3}; normalized to 1.0");
}

fn normalize_health_weight_value(value: f64, fallback: f64) -> f64 {
    if !value.is_finite() || value < 0.0 {
        fallback
    } else {
        value
    }
}

fn normalize_health_weights(config: &mut HealthConfig) {
    config.risk_weights.pagerank = normalize_health_weight_value(
        config.risk_weights.pagerank,
        default_health_pagerank_weight(),
    );
    config.risk_weights.test_gap = normalize_health_weight_value(
        config.risk_weights.test_gap,
        default_health_test_gap_weight(),
    );
    config.risk_weights.drift =
        normalize_health_weight_value(config.risk_weights.drift, default_health_drift_weight());
    config.risk_weights.no_sir =
        normalize_health_weight_value(config.risk_weights.no_sir, default_health_no_sir_weight());
    config.risk_weights.recency =
        normalize_health_weight_value(config.risk_weights.recency, default_health_recency_weight());

    let sum = config.risk_weights.pagerank
        + config.risk_weights.test_gap
        + config.risk_weights.drift
        + config.risk_weights.no_sir
        + config.risk_weights.recency;
    if (sum - 1.0).abs() <= 0.000_001 {
        return;
    }

    if sum <= f64::EPSILON {
        config.risk_weights = RiskWeights::default();
        eprintln!(
            "AETHER config warning: health risk weights summed to {sum:.3}; reset to defaults"
        );
        return;
    }

    config.risk_weights.pagerank /= sum;
    config.risk_weights.test_gap /= sum;
    config.risk_weights.drift /= sum;
    config.risk_weights.no_sir /= sum;
    config.risk_weights.recency /= sum;
    eprintln!("AETHER config warning: health risk weights summed to {sum:.3}; normalized to 1.0");
}

fn normalize_health_score_usize_pair(
    warn: &mut usize,
    fail: &mut usize,
    default_warn: usize,
    default_fail: usize,
) {
    if *warn == 0 || *fail == 0 || *fail <= *warn {
        *warn = default_warn;
        *fail = default_fail;
    }
}

fn normalize_health_score_f32_pair(
    warn: &mut f32,
    fail: &mut f32,
    default_warn: f32,
    default_fail: f32,
) {
    if !warn.is_finite() || !fail.is_finite() || *warn <= 0.0 || *fail <= *warn {
        *warn = default_warn;
        *fail = default_fail;
    }
}

fn normalize_health_score_positive_f32(value: &mut f32, default_value: f32) {
    if !value.is_finite() || *value <= 0.0 {
        *value = default_value;
    }
}

fn normalize_health_score_positive_u64(value: &mut u64, default_value: u64) {
    if *value == 0 {
        *value = default_value;
    }
}

fn normalize_optional_positive_f64(value: &mut Option<f64>) {
    if value.is_some_and(|raw| !raw.is_finite() || raw <= 0.0) {
        *value = None;
    }
}

fn normalize_planner_config(config: &mut PlannerConfig) {
    if !config.semantic_rescue_threshold.is_finite() {
        config.semantic_rescue_threshold = default_planner_semantic_rescue_threshold();
    }
    config.semantic_rescue_threshold = config.semantic_rescue_threshold.clamp(0.3, 0.95);

    if config.semantic_rescue_max_k == 0 {
        config.semantic_rescue_max_k = default_planner_semantic_rescue_max_k();
    }
    config.semantic_rescue_max_k = config.semantic_rescue_max_k.clamp(1, 10);

    if !config.community_resolution.is_finite() {
        config.community_resolution = default_planner_community_resolution();
    }
    config.community_resolution = config.community_resolution.clamp(0.1, 3.0);

    if config.min_community_size == 0 {
        config.min_community_size = default_planner_min_community_size();
    }
    config.min_community_size = config.min_community_size.clamp(1, 20);
}

fn normalize_health_score_config(config: &mut HealthScoreConfig) {
    normalize_health_score_usize_pair(
        &mut config.file_loc_warn,
        &mut config.file_loc_fail,
        default_health_score_file_loc_warn(),
        default_health_score_file_loc_fail(),
    );
    normalize_health_score_usize_pair(
        &mut config.trait_method_warn,
        &mut config.trait_method_fail,
        default_health_score_trait_method_warn(),
        default_health_score_trait_method_fail(),
    );
    normalize_health_score_usize_pair(
        &mut config.internal_dep_warn,
        &mut config.internal_dep_fail,
        default_health_score_internal_dep_warn(),
        default_health_score_internal_dep_fail(),
    );
    normalize_health_score_f32_pair(
        &mut config.todo_density_warn,
        &mut config.todo_density_fail,
        default_health_score_todo_density_warn(),
        default_health_score_todo_density_fail(),
    );
    normalize_health_score_usize_pair(
        &mut config.dead_feature_warn,
        &mut config.dead_feature_fail,
        default_health_score_dead_feature_warn(),
        default_health_score_dead_feature_fail(),
    );
    normalize_health_score_usize_pair(
        &mut config.stale_ref_warn,
        &mut config.stale_ref_fail,
        default_health_score_stale_ref_warn(),
        default_health_score_stale_ref_fail(),
    );
    config.stale_ref_patterns = normalize_patterns(std::mem::take(&mut config.stale_ref_patterns));
    if config.stale_ref_patterns.is_empty() {
        config.stale_ref_patterns = default_health_score_stale_ref_patterns();
    }
    normalize_health_score_usize_pair(
        &mut config.churn_30d_high,
        &mut config.churn_90d_high,
        default_health_score_churn_30d_high(),
        default_health_score_churn_90d_high(),
    );
    if config.author_count_high <= 1 {
        config.author_count_high = default_health_score_author_count_high();
    }
    normalize_health_score_positive_u64(
        &mut config.blame_age_spread_high_secs,
        default_health_score_blame_age_spread_high_secs(),
    );
    normalize_health_score_positive_f32(
        &mut config.drift_density_high,
        default_health_score_drift_density_high(),
    );
    normalize_health_score_positive_f32(
        &mut config.stale_sir_high,
        default_health_score_stale_sir_high(),
    );
    normalize_health_score_positive_f32(
        &mut config.test_gap_high,
        default_health_score_test_gap_high(),
    );
    normalize_health_score_positive_f32(
        &mut config.boundary_leakage_high,
        default_health_score_boundary_leakage_high(),
    );
    normalize_optional_positive_f64(&mut config.structural_weight);
    normalize_optional_positive_f64(&mut config.git_weight);
    normalize_optional_positive_f64(&mut config.semantic_weight);
}

fn normalize_config(mut config: AetherConfig) -> AetherConfig {
    config.general.log_level = normalize_with_default(
        std::mem::take(&mut config.general.log_level),
        default_log_level(),
    );
    config.inference.model = normalize_optional(config.inference.model.take());
    config.inference.endpoint = normalize_optional(config.inference.endpoint.take());
    if config.inference.concurrency == 0 {
        config.inference.concurrency = default_sir_concurrency();
    }
    config.inference.concurrency =
        normalize_provider_concurrency(config.inference.provider, config.inference.concurrency);
    if let Some(tiered) = config.inference.tiered.as_mut() {
        tiered.primary =
            normalize_with_default(std::mem::take(&mut tiered.primary), "gemini".to_owned());
        tiered.primary_model = normalize_optional(tiered.primary_model.take());
        tiered.primary_endpoint = normalize_optional(tiered.primary_endpoint.take());
        tiered.primary_api_key_env = normalize_with_default(
            std::mem::take(&mut tiered.primary_api_key_env),
            default_api_key_env(),
        );
        if !tiered.primary_threshold.is_finite() {
            tiered.primary_threshold = default_tiered_primary_threshold();
        }
        tiered.primary_threshold = tiered.primary_threshold.clamp(0.0, 1.0);
        tiered.fallback_model =
            normalize_optional(tiered.fallback_model.take()).or_else(default_tiered_fallback_model);
        tiered.fallback_endpoint = normalize_optional(tiered.fallback_endpoint.take())
            .or_else(default_tiered_fallback_endpoint);
    }
    normalize_sir_quality_config(&mut config.sir_quality);
    config.embeddings.model = normalize_optional(config.embeddings.model.take());
    config.embeddings.endpoint = normalize_optional(config.embeddings.endpoint.take());
    config.embeddings.api_key_env = normalize_optional(config.embeddings.api_key_env.take());
    config.embeddings.task_type = normalize_optional(config.embeddings.task_type.take());
    config.embeddings.candle.model_dir =
        normalize_optional(config.embeddings.candle.model_dir.take());
    config.search.candle.model_dir = normalize_optional(config.search.candle.model_dir.take());
    if config.search.rerank_window == 0 {
        config.search.rerank_window = default_rerank_window();
    }
    config.search.thresholds.default = normalize_threshold_value(
        config.search.thresholds.default,
        default_search_threshold_default(),
    );
    config.search.thresholds.rust = normalize_threshold_value(
        config.search.thresholds.rust,
        default_search_threshold_rust(),
    );
    config.search.thresholds.typescript = normalize_threshold_value(
        config.search.thresholds.typescript,
        default_search_threshold_typescript(),
    );
    config.search.thresholds.python = normalize_threshold_value(
        config.search.thresholds.python,
        default_search_threshold_python(),
    );
    config.search.calibrated_thresholds.default =
        normalize_optional_threshold(config.search.calibrated_thresholds.default.take());
    config.search.calibrated_thresholds.rust =
        normalize_optional_threshold(config.search.calibrated_thresholds.rust.take());
    config.search.calibrated_thresholds.typescript =
        normalize_optional_threshold(config.search.calibrated_thresholds.typescript.take());
    config.search.calibrated_thresholds.python =
        normalize_optional_threshold(config.search.calibrated_thresholds.python.take());
    config.verify.commands = normalize_commands(std::mem::take(&mut config.verify.commands));
    config.verify.container.runtime = normalize_with_default(
        std::mem::take(&mut config.verify.container.runtime),
        default_verify_container_runtime(),
    );
    config.verify.container.image = normalize_with_default(
        std::mem::take(&mut config.verify.container.image),
        default_verify_container_image(),
    );
    config.verify.container.workdir = normalize_with_default(
        std::mem::take(&mut config.verify.container.workdir),
        default_verify_container_workdir(),
    );
    config.verify.microvm.runtime = normalize_with_default(
        std::mem::take(&mut config.verify.microvm.runtime),
        default_verify_microvm_runtime(),
    );
    config.verify.microvm.kernel_image =
        normalize_optional(config.verify.microvm.kernel_image.take());
    config.verify.microvm.rootfs_image =
        normalize_optional(config.verify.microvm.rootfs_image.take());
    config.verify.microvm.workdir = normalize_with_default(
        std::mem::take(&mut config.verify.microvm.workdir),
        default_verify_microvm_workdir(),
    );
    if config.verify.microvm.vcpu_count == 0 {
        config.verify.microvm.vcpu_count = default_verify_microvm_vcpu_count();
    }
    if config.verify.microvm.memory_mib == 0 {
        config.verify.microvm.memory_mib = default_verify_microvm_memory_mib();
    }
    if config.coupling.commit_window == 0 {
        config.coupling.commit_window = default_coupling_commit_window();
    }
    if config.coupling.min_co_change_count == 0 {
        config.coupling.min_co_change_count = default_coupling_min_co_change_count();
    }
    if config.coupling.bulk_commit_threshold == 0 {
        config.coupling.bulk_commit_threshold = default_coupling_bulk_commit_threshold();
    }
    config.coupling.exclude_patterns =
        normalize_patterns(std::mem::take(&mut config.coupling.exclude_patterns));
    if config.coupling.exclude_patterns.is_empty() {
        config.coupling.exclude_patterns = default_coupling_exclude_patterns();
    }
    normalize_coupling_weights(&mut config.coupling);
    config.drift.analysis_window = normalize_with_default(
        std::mem::take(&mut config.drift.analysis_window),
        default_drift_analysis_window(),
    );
    if !config.drift.drift_threshold.is_finite() {
        config.drift.drift_threshold = default_drift_threshold();
    } else {
        config.drift.drift_threshold = config.drift.drift_threshold.clamp(0.0, 1.0);
    }
    config.drift.hub_percentile = config.drift.hub_percentile.clamp(1, 100);
    normalize_health_weights(&mut config.health);
    normalize_planner_config(&mut config.planner);
    normalize_health_score_config(&mut config.health_score);

    let api_key_env = config.inference.api_key_env.trim();
    if api_key_env.is_empty() {
        config.inference.api_key_env = default_api_key_env();
    } else {
        config.inference.api_key_env = api_key_env.to_owned();
    }

    if config.embeddings.provider == EmbeddingProviderKind::OpenAiCompat
        && config.embeddings.api_key_env.is_none()
    {
        config.embeddings.api_key_env = Some(DEFAULT_OPENAI_COMPAT_API_KEY_ENV.to_owned());
    }

    let cohere_api_key_env = config.providers.cohere.api_key_env.trim();
    if cohere_api_key_env.is_empty() {
        config.providers.cohere.api_key_env = default_cohere_api_key_env();
    } else {
        config.providers.cohere.api_key_env = cohere_api_key_env.to_owned();
    }

    config
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn ensure_workspace_config_creates_default_file() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        let config = ensure_workspace_config(workspace).expect("ensure config");

        assert_eq!(config.general.log_level, DEFAULT_LOG_LEVEL);
        assert_eq!(config.inference.provider, InferenceProviderKind::Auto);
        assert_eq!(config.inference.api_key_env, DEFAULT_GEMINI_API_KEY_ENV);
        assert_eq!(config.inference.concurrency, DEFAULT_SIR_CONCURRENCY);
        assert!(!config.sir_quality.triage_pass);
        assert_eq!(config.sir_quality.triage_priority_threshold, 0.7);
        assert_eq!(config.sir_quality.triage_confidence_threshold, 0.85);
        assert_eq!(config.sir_quality.triage_max_symbols, 0);
        assert_eq!(config.sir_quality.triage_concurrency, 4);
        assert_eq!(config.sir_quality.triage_timeout_secs, 180);
        assert!(!config.sir_quality.deep_pass);
        assert_eq!(config.sir_quality.deep_priority_threshold, 0.9);
        assert_eq!(config.sir_quality.deep_confidence_threshold, 0.85);
        assert_eq!(config.sir_quality.deep_max_symbols, 20);
        assert_eq!(config.sir_quality.deep_max_neighbors, 10);
        assert_eq!(config.sir_quality.deep_concurrency, 4);
        assert_eq!(config.sir_quality.deep_timeout_secs, 180);
        assert!(config.storage.mirror_sir_files);
        assert_eq!(config.storage.graph_backend, GraphBackend::Surreal);
        assert!(!config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::Qwen3Local
        );
        assert_eq!(config.search.reranker, SearchRerankerKind::None);
        assert_eq!(config.search.rerank_window, 50);
        assert_eq!(
            config.search.thresholds.default,
            DEFAULT_SEARCH_THRESHOLD_DEFAULT
        );
        assert_eq!(config.search.thresholds.rust, DEFAULT_SEARCH_THRESHOLD_RUST);
        assert_eq!(
            config.search.thresholds.typescript,
            DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT
        );
        assert_eq!(
            config.search.thresholds.python,
            DEFAULT_SEARCH_THRESHOLD_PYTHON
        );
        assert!(config.search.calibrated_thresholds.is_empty());
        assert_eq!(
            config.providers.cohere.api_key_env,
            DEFAULT_COHERE_API_KEY_ENV
        );
        assert_eq!(
            config.verify.commands,
            vec![
                "cargo fmt --all --check".to_owned(),
                "cargo clippy --workspace -- -D warnings".to_owned(),
                "cargo test --workspace".to_owned()
            ]
        );
        assert_eq!(config.verify.mode, VerifyMode::Host);
        assert_eq!(
            config.verify.container.runtime,
            DEFAULT_VERIFY_CONTAINER_RUNTIME
        );
        assert_eq!(
            config.verify.container.image,
            DEFAULT_VERIFY_CONTAINER_IMAGE
        );
        assert_eq!(
            config.verify.container.workdir,
            DEFAULT_VERIFY_CONTAINER_WORKDIR
        );
        assert!(!config.verify.container.fallback_to_host_on_unavailable);
        assert_eq!(
            config.verify.microvm.runtime,
            DEFAULT_VERIFY_MICROVM_RUNTIME
        );
        assert_eq!(config.verify.microvm.kernel_image, None);
        assert_eq!(config.verify.microvm.rootfs_image, None);
        assert_eq!(
            config.verify.microvm.workdir,
            DEFAULT_VERIFY_MICROVM_WORKDIR
        );
        assert_eq!(
            config.verify.microvm.vcpu_count,
            DEFAULT_VERIFY_MICROVM_VCPU_COUNT
        );
        assert_eq!(
            config.verify.microvm.memory_mib,
            DEFAULT_VERIFY_MICROVM_MEMORY_MIB
        );
        assert!(!config.verify.microvm.fallback_to_container_on_unavailable);
        assert!(!config.verify.microvm.fallback_to_host_on_unavailable);
        assert!(config.coupling.enabled);
        assert_eq!(config.coupling.commit_window, 500);
        assert_eq!(config.coupling.min_co_change_count, 3);
        assert_eq!(config.coupling.bulk_commit_threshold, 30);
        assert!((config.coupling.temporal_weight - 0.5).abs() < 1e-6);
        assert!((config.coupling.static_weight - 0.3).abs() < 1e-6);
        assert!((config.coupling.semantic_weight - 0.2).abs() < 1e-6);
        assert_eq!(
            config.coupling.exclude_patterns,
            vec![
                "*.lock".to_owned(),
                "*.generated.*".to_owned(),
                ".gitignore".to_owned()
            ]
        );
        assert!(config.drift.enabled);
        assert_eq!(config.drift.drift_threshold, DEFAULT_DRIFT_THRESHOLD);
        assert_eq!(
            config.drift.analysis_window,
            DEFAULT_DRIFT_ANALYSIS_WINDOW.to_owned()
        );
        assert!(!config.drift.auto_analyze);
        assert_eq!(config.drift.hub_percentile, DEFAULT_DRIFT_HUB_PERCENTILE);
        assert!(config.health.enabled);
        assert_eq!(
            config.health.risk_weights.pagerank,
            DEFAULT_HEALTH_PAGERANK_WEIGHT
        );
        assert_eq!(
            config.health.risk_weights.test_gap,
            DEFAULT_HEALTH_TEST_GAP_WEIGHT
        );
        assert_eq!(
            config.health.risk_weights.drift,
            DEFAULT_HEALTH_DRIFT_WEIGHT
        );
        assert_eq!(
            config.health.risk_weights.no_sir,
            DEFAULT_HEALTH_NO_SIR_WEIGHT
        );
        assert_eq!(
            config.health.risk_weights.recency,
            DEFAULT_HEALTH_RECENCY_WEIGHT
        );
        assert_eq!(config.planner.semantic_rescue_threshold, 0.70);
        assert_eq!(config.planner.semantic_rescue_max_k, 3);
        assert_eq!(config.planner.community_resolution, 0.5);
        assert_eq!(config.planner.min_community_size, 3);
        assert_eq!(
            config.health_score.file_loc_warn,
            DEFAULT_HEALTH_SCORE_FILE_LOC_WARN
        );
        assert_eq!(
            config.health_score.file_loc_fail,
            DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL
        );
        assert_eq!(
            config.health_score.trait_method_warn,
            DEFAULT_HEALTH_SCORE_TRAIT_METHOD_WARN
        );
        assert_eq!(
            config.health_score.trait_method_fail,
            DEFAULT_HEALTH_SCORE_TRAIT_METHOD_FAIL
        );
        assert_eq!(
            config.health_score.internal_dep_warn,
            DEFAULT_HEALTH_SCORE_INTERNAL_DEP_WARN
        );
        assert_eq!(
            config.health_score.internal_dep_fail,
            DEFAULT_HEALTH_SCORE_INTERNAL_DEP_FAIL
        );
        assert_eq!(
            config.health_score.todo_density_warn,
            DEFAULT_HEALTH_SCORE_TODO_DENSITY_WARN
        );
        assert_eq!(
            config.health_score.todo_density_fail,
            DEFAULT_HEALTH_SCORE_TODO_DENSITY_FAIL
        );
        assert_eq!(
            config.health_score.dead_feature_warn,
            DEFAULT_HEALTH_SCORE_DEAD_FEATURE_WARN
        );
        assert_eq!(
            config.health_score.dead_feature_fail,
            DEFAULT_HEALTH_SCORE_DEAD_FEATURE_FAIL
        );
        assert_eq!(
            config.health_score.stale_ref_warn,
            DEFAULT_HEALTH_SCORE_STALE_REF_WARN
        );
        assert_eq!(
            config.health_score.stale_ref_fail,
            DEFAULT_HEALTH_SCORE_STALE_REF_FAIL
        );
        assert_eq!(
            config.health_score.stale_ref_patterns,
            default_health_score_stale_ref_patterns()
        );
        assert_eq!(
            config.health_score.churn_30d_high,
            DEFAULT_HEALTH_SCORE_CHURN_30D_HIGH
        );
        assert_eq!(
            config.health_score.churn_90d_high,
            DEFAULT_HEALTH_SCORE_CHURN_90D_HIGH
        );
        assert_eq!(
            config.health_score.author_count_high,
            DEFAULT_HEALTH_SCORE_AUTHOR_COUNT_HIGH
        );
        assert_eq!(
            config.health_score.blame_age_spread_high_secs,
            DEFAULT_HEALTH_SCORE_BLAME_AGE_SPREAD_HIGH_SECS
        );
        assert_eq!(
            config.health_score.drift_density_high,
            DEFAULT_HEALTH_SCORE_DRIFT_DENSITY_HIGH
        );
        assert_eq!(
            config.health_score.stale_sir_high,
            DEFAULT_HEALTH_SCORE_STALE_SIR_HIGH
        );
        assert_eq!(
            config.health_score.test_gap_high,
            DEFAULT_HEALTH_SCORE_TEST_GAP_HIGH
        );
        assert_eq!(
            config.health_score.boundary_leakage_high,
            DEFAULT_HEALTH_SCORE_BOUNDARY_LEAKAGE_HIGH
        );
        assert!(config.health_score.structural_weight.is_none());
        assert!(config.health_score.git_weight.is_none());
        assert!(config.health_score.semantic_weight.is_none());
        assert_eq!(config.dashboard.port, DEFAULT_DASHBOARD_PORT);
        assert!(config.dashboard.enabled);
        assert!(config_path(workspace).exists());

        let content = fs::read_to_string(config_path(workspace)).expect("read config file");
        assert!(content.contains("[general]"));
        assert!(content.contains("log_level = \"info\""));
        assert!(content.contains("[inference]"));
        assert!(content.contains("provider = \"auto\""));
        assert!(content.contains("concurrency = 2"));
        assert!(content.contains("[sir_quality]"));
        assert!(content.contains("triage_pass = false"));
        assert!(content.contains("deep_pass = false"));
        assert!(content.contains("[storage]"));
        assert!(content.contains("mirror_sir_files = true"));
        assert!(content.contains("graph_backend = \"surreal\""));
        assert!(content.contains("[embeddings]"));
        assert!(content.contains("enabled = false"));
        assert!(content.contains("provider = \"qwen3_local\""));
        assert!(content.contains("vector_backend = \"lancedb\""));
        assert!(content.contains("[search]"));
        assert!(content.contains("reranker = \"none\""));
        assert!(content.contains("rerank_window = 50"));
        assert!(content.contains("[search.thresholds]"));
        assert!(content.contains("default = "));
        assert!(content.contains("rust = "));
        assert!(content.contains("typescript = "));
        assert!(content.contains("python = "));
        assert!(!content.contains("[search.calibrated_thresholds]"));
        assert!(content.contains("[providers.cohere]"));
        assert!(content.contains("api_key_env = \"COHERE_API_KEY\""));
        assert!(content.contains("[verify]"));
        assert!(content.contains("commands = ["));
        assert!(content.contains("mode = \"host\""));
        assert!(content.contains("[verify.container]"));
        assert!(content.contains("runtime = \"docker\""));
        assert!(content.contains("image = \"rust:1-bookworm\""));
        assert!(content.contains("workdir = \"/workspace\""));
        assert!(content.contains("[verify.microvm]"));
        assert!(content.contains("runtime = \"firecracker\""));
        assert!(content.contains("workdir = \"/workspace\""));
        assert!(content.contains("vcpu_count = 1"));
        assert!(content.contains("memory_mib = 1024"));
        assert!(content.contains("[coupling]"));
        assert!(content.contains("enabled = true"));
        assert!(content.contains("commit_window = 500"));
        assert!(content.contains("min_co_change_count = 3"));
        assert!(content.contains("bulk_commit_threshold = 30"));
        assert!(content.contains("temporal_weight = 0.5"));
        assert!(content.contains("static_weight = 0.3"));
        assert!(content.contains("semantic_weight = 0.2"));
        assert!(content.contains("[drift]"));
        assert!(content.contains("drift_threshold = 0.85"));
        assert!(content.contains("analysis_window = \"100 commits\""));
        assert!(content.contains("auto_analyze = false"));
        assert!(content.contains("hub_percentile = 95"));
        assert!(content.contains("[health]"));
        assert!(content.contains("enabled = true"));
        assert!(content.contains("risk_weights = {") || content.contains("[health.risk_weights]"));
        assert!(content.contains("pagerank = 0.3"));
        assert!(content.contains("test_gap = 0.25"));
        assert!(content.contains("drift = 0.2"));
        assert!(content.contains("no_sir = 0.15"));
        assert!(content.contains("recency = 0.1"));
        assert!(content.contains("[planner]"));
        assert!(content.contains("semantic_rescue_threshold"));
        assert!(content.contains("semantic_rescue_max_k = 3"));
        assert!(content.contains("community_resolution"));
        assert!(content.contains("min_community_size = 3"));
        assert!(content.contains("[health_score]"));
        assert!(content.contains("file_loc_warn = 800"));
        assert!(content.contains("trait_method_fail = 35"));
        assert!(content.contains("stale_ref_patterns = ["));
        assert!(content.contains("\"CozoGraphStore\""));
        assert!(content.contains("\"CozoDB\""));
        assert!(content.contains("[dashboard]"));
        assert!(content.contains("port = 9720"));
        assert!(content.contains("enabled = true"));
        assert!(content.contains("\"cargo fmt --all --check\""));
        assert!(content.contains("\"cargo clippy --workspace -- -D warnings\""));
        assert!(content.contains("\"cargo test --workspace\""));
    }

    #[test]
    fn load_workspace_config_parses_inference_storage_and_embedding_values() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[general]
log_level = " debug "

[inference]
provider = "qwen3_local"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434"
api_key_env = "CUSTOM_GEMINI_KEY"

[storage]
mirror_sir_files = false
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434/api/embeddings"

[search]
reranker = "cohere"
rerank_window = 0

[search.candle]
model_dir = " .aether/models "

[providers.cohere]
api_key_env = " CUSTOM_COHERE_KEY "

[verify]
mode = "microvm"
commands = [
    " cargo test ",
    "",
    "cargo clippy --workspace -- -D warnings",
    "cargo test"
]

[verify.container]
runtime = " docker "
image = " rust:1-bookworm "
workdir = " /workspace "
fallback_to_host_on_unavailable = true

[verify.microvm]
runtime = " firecracker "
kernel_image = " ./assets/vmlinux "
rootfs_image = " ./assets/rootfs.ext4 "
workdir = " /workspace "
vcpu_count = 0
memory_mib = 0
fallback_to_container_on_unavailable = true
fallback_to_host_on_unavailable = true

[drift]
enabled = false
drift_threshold = 0.9
analysis_window = " 50 commits "
auto_analyze = true
hub_percentile = 0

[health]
enabled = false
risk_weights = { pagerank = 3.0, test_gap = 1.0, drift = 1.0, no_sir = 1.0, recency = 2.0 }

[health_score]
file_loc_warn = 1000
file_loc_fail = 2000
trait_method_warn = 25
trait_method_fail = 40
internal_dep_warn = 7
internal_dep_fail = 11
todo_density_warn = 6.0
todo_density_fail = 16.0
dead_feature_warn = 2
dead_feature_fail = 6
stale_ref_warn = 2
stale_ref_fail = 4
stale_ref_patterns = [" CozoGraphStore ", "", "LegacyStore"]

[dashboard]
port = 9800
enabled = false
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");

        assert_eq!(config.general.log_level, "debug");
        assert_eq!(config.inference.provider, InferenceProviderKind::Qwen3Local);
        assert_eq!(
            config.inference.model.as_deref(),
            Some("qwen3-embeddings-4B")
        );
        assert_eq!(
            config.inference.endpoint.as_deref(),
            Some("http://127.0.0.1:11434")
        );
        assert_eq!(config.inference.api_key_env, "CUSTOM_GEMINI_KEY");
        assert!(!config.storage.mirror_sir_files);
        assert_eq!(config.storage.graph_backend, GraphBackend::Sqlite);
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::Qwen3Local
        );
        assert_eq!(
            config.embeddings.vector_backend,
            EmbeddingVectorBackend::Sqlite
        );
        assert_eq!(
            config.embeddings.model.as_deref(),
            Some("qwen3-embeddings-4B")
        );
        assert_eq!(
            config.embeddings.endpoint.as_deref(),
            Some("http://127.0.0.1:11434/api/embeddings")
        );
        assert_eq!(config.search.reranker, SearchRerankerKind::Cohere);
        assert_eq!(config.search.rerank_window, 50);
        assert_eq!(
            config.search.thresholds.default,
            DEFAULT_SEARCH_THRESHOLD_DEFAULT
        );
        assert_eq!(config.search.thresholds.rust, DEFAULT_SEARCH_THRESHOLD_RUST);
        assert_eq!(
            config.search.thresholds.typescript,
            DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT
        );
        assert_eq!(
            config.search.thresholds.python,
            DEFAULT_SEARCH_THRESHOLD_PYTHON
        );
        assert_eq!(
            config.search.candle.model_dir.as_deref(),
            Some(".aether/models")
        );
        assert_eq!(config.providers.cohere.api_key_env, "CUSTOM_COHERE_KEY");
        assert_eq!(
            config.verify.commands,
            vec![
                "cargo test".to_owned(),
                "cargo clippy --workspace -- -D warnings".to_owned(),
            ]
        );
        assert_eq!(config.verify.mode, VerifyMode::Microvm);
        assert_eq!(config.verify.container.runtime, "docker");
        assert_eq!(config.verify.container.image, "rust:1-bookworm");
        assert_eq!(config.verify.container.workdir, "/workspace");
        assert!(config.verify.container.fallback_to_host_on_unavailable);
        assert_eq!(config.verify.microvm.runtime, "firecracker");
        assert_eq!(
            config.verify.microvm.kernel_image.as_deref(),
            Some("./assets/vmlinux")
        );
        assert_eq!(
            config.verify.microvm.rootfs_image.as_deref(),
            Some("./assets/rootfs.ext4")
        );
        assert_eq!(config.verify.microvm.workdir, "/workspace");
        assert_eq!(config.verify.microvm.vcpu_count, 1);
        assert_eq!(config.verify.microvm.memory_mib, 1024);
        assert!(config.verify.microvm.fallback_to_container_on_unavailable);
        assert!(config.verify.microvm.fallback_to_host_on_unavailable);
        assert!(config.coupling.enabled);
        assert_eq!(config.coupling.commit_window, 500);
        assert_eq!(config.coupling.min_co_change_count, 3);
        assert_eq!(config.coupling.bulk_commit_threshold, 30);
        assert!((config.coupling.temporal_weight - 0.5).abs() < 1e-6);
        assert!((config.coupling.static_weight - 0.3).abs() < 1e-6);
        assert!((config.coupling.semantic_weight - 0.2).abs() < 1e-6);
        assert!(!config.drift.enabled);
        assert_eq!(config.drift.drift_threshold, 0.9);
        assert_eq!(config.drift.analysis_window, "50 commits");
        assert!(config.drift.auto_analyze);
        assert_eq!(config.drift.hub_percentile, 1);
        assert!(!config.health.enabled);
        let health_sum = config.health.risk_weights.pagerank
            + config.health.risk_weights.test_gap
            + config.health.risk_weights.drift
            + config.health.risk_weights.no_sir
            + config.health.risk_weights.recency;
        assert!((health_sum - 1.0).abs() < 1e-6);
        assert_eq!(config.health_score.file_loc_warn, 1000);
        assert_eq!(config.health_score.file_loc_fail, 2000);
        assert_eq!(config.health_score.trait_method_warn, 25);
        assert_eq!(config.health_score.trait_method_fail, 40);
        assert_eq!(config.health_score.internal_dep_warn, 7);
        assert_eq!(config.health_score.internal_dep_fail, 11);
        assert_eq!(config.health_score.todo_density_warn, 6.0);
        assert_eq!(config.health_score.todo_density_fail, 16.0);
        assert_eq!(config.health_score.dead_feature_warn, 2);
        assert_eq!(config.health_score.dead_feature_fail, 6);
        assert_eq!(config.health_score.stale_ref_warn, 2);
        assert_eq!(config.health_score.stale_ref_fail, 4);
        assert_eq!(
            config.health_score.stale_ref_patterns,
            vec!["CozoGraphStore".to_owned(), "LegacyStore".to_owned()]
        );
        assert_eq!(config.dashboard.port, 9800);
        assert!(!config.dashboard.enabled);
    }

    #[test]
    fn load_workspace_config_parses_candle_embedding_provider() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[embeddings]
enabled = true
provider = "candle"
vector_backend = "lancedb"

[embeddings.candle]
model_dir = " .aether/models "
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert!(config.embeddings.enabled);
        assert_eq!(config.embeddings.provider, EmbeddingProviderKind::Candle);
        assert_eq!(
            config.embeddings.candle.model_dir.as_deref(),
            Some(".aether/models")
        );
    }

    #[test]
    fn ensure_workspace_config_does_not_overwrite_existing_values() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[general]
log_level = ""

[inference]
provider = "qwen3_local"
api_key_env = "CUSTOM_KEY"

[storage]
mirror_sir_files = false
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
model = "qwen3-embeddings-4B"

[verify]
mode = "container"
commands = ["cargo --version"]
[verify.container]
runtime = "docker"
image = "rust:1-bookworm"
workdir = "/workspace"
fallback_to_host_on_unavailable = true
[verify.microvm]
runtime = "firecracker"
kernel_image = "./kernel"
rootfs_image = "./rootfs.ext4"
workdir = "/workspace"
vcpu_count = 2
memory_mib = 2048
fallback_to_container_on_unavailable = true
fallback_to_host_on_unavailable = true
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let before = fs::read_to_string(config_path(workspace)).expect("read before");
        let config = ensure_workspace_config(workspace).expect("ensure config");
        let after = fs::read_to_string(config_path(workspace)).expect("read after");

        assert_eq!(before, after);
        assert_eq!(config.general.log_level, DEFAULT_LOG_LEVEL);
        assert_eq!(config.inference.provider, InferenceProviderKind::Qwen3Local);
        assert_eq!(config.inference.api_key_env, "CUSTOM_KEY");
        assert!(!config.storage.mirror_sir_files);
        assert_eq!(config.storage.graph_backend, GraphBackend::Sqlite);
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::Qwen3Local
        );
        assert_eq!(
            config.embeddings.vector_backend,
            EmbeddingVectorBackend::Sqlite
        );
        assert_eq!(config.search.reranker, SearchRerankerKind::None);
        assert_eq!(config.search.rerank_window, 50);
        assert_eq!(
            config.search.thresholds.default,
            DEFAULT_SEARCH_THRESHOLD_DEFAULT
        );
        assert_eq!(config.search.thresholds.rust, DEFAULT_SEARCH_THRESHOLD_RUST);
        assert_eq!(
            config.search.thresholds.typescript,
            DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT
        );
        assert_eq!(
            config.search.thresholds.python,
            DEFAULT_SEARCH_THRESHOLD_PYTHON
        );
        assert_eq!(
            config.providers.cohere.api_key_env,
            DEFAULT_COHERE_API_KEY_ENV
        );
        assert_eq!(config.verify.commands, vec!["cargo --version".to_owned()]);
        assert_eq!(config.verify.mode, VerifyMode::Container);
        assert!(config.verify.container.fallback_to_host_on_unavailable);
        assert!(config.verify.microvm.fallback_to_container_on_unavailable);
        assert!(config.verify.microvm.fallback_to_host_on_unavailable);
        assert!(config.coupling.enabled);
        assert_eq!(config.coupling.commit_window, 500);
        assert_eq!(config.coupling.min_co_change_count, 3);
        assert_eq!(config.coupling.bulk_commit_threshold, 30);
        assert!((config.coupling.temporal_weight - 0.5).abs() < 1e-6);
        assert!((config.coupling.static_weight - 0.3).abs() < 1e-6);
        assert!((config.coupling.semantic_weight - 0.2).abs() < 1e-6);
        assert!(config.drift.enabled);
        assert_eq!(config.drift.drift_threshold, DEFAULT_DRIFT_THRESHOLD);
        assert_eq!(config.drift.analysis_window, DEFAULT_DRIFT_ANALYSIS_WINDOW);
        assert!(!config.drift.auto_analyze);
        assert_eq!(config.drift.hub_percentile, DEFAULT_DRIFT_HUB_PERCENTILE);
        assert!(config.health.enabled);
        assert_eq!(
            config.health.risk_weights.pagerank,
            DEFAULT_HEALTH_PAGERANK_WEIGHT
        );
        assert_eq!(
            config.health_score.file_loc_warn,
            DEFAULT_HEALTH_SCORE_FILE_LOC_WARN
        );
        assert_eq!(
            config.health_score.file_loc_fail,
            DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL
        );
    }

    #[test]
    fn load_workspace_config_normalizes_coupling_weights_when_sum_is_invalid() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[coupling]
temporal_weight = 2.0
static_weight = 1.0
semantic_weight = 1.0
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        let sum = config.coupling.temporal_weight
            + config.coupling.static_weight
            + config.coupling.semantic_weight;
        assert!((sum - 1.0).abs() < 1e-6);
        assert!((config.coupling.temporal_weight - 0.5).abs() < 1e-6);
        assert!((config.coupling.static_weight - 0.25).abs() < 1e-6);
        assert!((config.coupling.semantic_weight - 0.25).abs() < 1e-6);
    }

    #[test]
    fn load_workspace_config_normalizes_health_weights_when_sum_is_invalid() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[health]
risk_weights = { pagerank = 2.0, test_gap = 1.0, drift = 1.0, no_sir = 1.0, recency = 1.0 }
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        let sum = config.health.risk_weights.pagerank
            + config.health.risk_weights.test_gap
            + config.health.risk_weights.drift
            + config.health.risk_weights.no_sir
            + config.health.risk_weights.recency;
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn load_workspace_config_normalizes_health_score_values() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[health_score]
file_loc_warn = 0
file_loc_fail = 0
trait_method_warn = 50
trait_method_fail = 20
internal_dep_warn = 9
internal_dep_fail = 9
todo_density_warn = 0.0
todo_density_fail = 0.0
dead_feature_warn = 0
dead_feature_fail = 0
stale_ref_warn = 0
stale_ref_fail = 0
stale_ref_patterns = ["", "  "]
churn_30d_high = 0
churn_90d_high = 0
author_count_high = 1
blame_age_spread_high_secs = 0
drift_density_high = 0.0
stale_sir_high = 0.0
test_gap_high = 0.0
boundary_leakage_high = 0.0
structural_weight = -1.0
git_weight = 0.0
semantic_weight = -2.0
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert_eq!(
            config.health_score.file_loc_warn,
            DEFAULT_HEALTH_SCORE_FILE_LOC_WARN
        );
        assert_eq!(
            config.health_score.file_loc_fail,
            DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL
        );
        assert_eq!(
            config.health_score.trait_method_warn,
            DEFAULT_HEALTH_SCORE_TRAIT_METHOD_WARN
        );
        assert_eq!(
            config.health_score.trait_method_fail,
            DEFAULT_HEALTH_SCORE_TRAIT_METHOD_FAIL
        );
        assert_eq!(
            config.health_score.internal_dep_warn,
            DEFAULT_HEALTH_SCORE_INTERNAL_DEP_WARN
        );
        assert_eq!(
            config.health_score.internal_dep_fail,
            DEFAULT_HEALTH_SCORE_INTERNAL_DEP_FAIL
        );
        assert_eq!(
            config.health_score.todo_density_warn,
            DEFAULT_HEALTH_SCORE_TODO_DENSITY_WARN
        );
        assert_eq!(
            config.health_score.todo_density_fail,
            DEFAULT_HEALTH_SCORE_TODO_DENSITY_FAIL
        );
        assert_eq!(
            config.health_score.stale_ref_patterns,
            default_health_score_stale_ref_patterns()
        );
        assert_eq!(
            config.health_score.churn_30d_high,
            DEFAULT_HEALTH_SCORE_CHURN_30D_HIGH
        );
        assert_eq!(
            config.health_score.churn_90d_high,
            DEFAULT_HEALTH_SCORE_CHURN_90D_HIGH
        );
        assert_eq!(
            config.health_score.author_count_high,
            DEFAULT_HEALTH_SCORE_AUTHOR_COUNT_HIGH
        );
        assert_eq!(
            config.health_score.blame_age_spread_high_secs,
            DEFAULT_HEALTH_SCORE_BLAME_AGE_SPREAD_HIGH_SECS
        );
        assert_eq!(
            config.health_score.drift_density_high,
            DEFAULT_HEALTH_SCORE_DRIFT_DENSITY_HIGH
        );
        assert_eq!(
            config.health_score.stale_sir_high,
            DEFAULT_HEALTH_SCORE_STALE_SIR_HIGH
        );
        assert_eq!(
            config.health_score.test_gap_high,
            DEFAULT_HEALTH_SCORE_TEST_GAP_HIGH
        );
        assert_eq!(
            config.health_score.boundary_leakage_high,
            DEFAULT_HEALTH_SCORE_BOUNDARY_LEAKAGE_HIGH
        );
        assert!(config.health_score.structural_weight.is_none());
        assert!(config.health_score.git_weight.is_none());
        assert!(config.health_score.semantic_weight.is_none());
    }

    #[test]
    fn load_workspace_config_normalizes_sir_quality_and_inference_concurrency() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[inference]
provider = "gemini"
concurrency = 0

[sir_quality]
deep_pass = true
deep_priority_threshold = 9.9
deep_confidence_threshold = -3.0
deep_max_neighbors = 0
deep_concurrency = 0
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert_eq!(config.inference.concurrency, GEMINI_DEFAULT_CONCURRENCY);
        assert!(config.sir_quality.triage_pass);
        assert_eq!(config.sir_quality.triage_priority_threshold, 1.0);
        assert_eq!(config.sir_quality.triage_confidence_threshold, 0.0);
        assert_eq!(config.sir_quality.triage_concurrency, 4);
        assert_eq!(config.sir_quality.deep_max_neighbors, 10);
        assert_eq!(config.sir_quality.deep_concurrency, 4);
        assert_eq!(config.sir_quality.deep_timeout_secs, 180);
    }

    #[test]
    fn validate_config_reports_ignored_fields() {
        let config = AetherConfig {
            general: GeneralConfig::default(),
            inference: InferenceConfig {
                provider: InferenceProviderKind::Auto,
                model: None,
                endpoint: Some("http://127.0.0.1:11434".to_owned()),
                api_key_env: DEFAULT_GEMINI_API_KEY_ENV.to_owned(),
                concurrency: default_sir_concurrency(),
                tiered: None,
            },
            sir_quality: SirQualityConfig::default(),
            storage: StorageConfig {
                mirror_sir_files: true,
                graph_backend: GraphBackend::Cozo,
            },
            embeddings: EmbeddingsConfig {
                enabled: false,
                provider: EmbeddingProviderKind::Qwen3Local,
                vector_backend: EmbeddingVectorBackend::Lancedb,
                model: Some("mock-x".to_owned()),
                endpoint: Some("http://127.0.0.1:11434/api/embeddings".to_owned()),
                api_key_env: None,
                task_type: None,
                dimensions: None,
                candle: CandleEmbeddingsConfig::default(),
            },
            search: SearchConfig::default(),
            providers: ProvidersConfig::default(),
            verify: VerifyConfig {
                commands: vec!["cargo test".to_owned()],
                ..VerifyConfig::default()
            },
            coupling: CouplingConfig::default(),
            drift: DriftConfig::default(),
            health: HealthConfig::default(),
            planner: PlannerConfig::default(),
            health_score: HealthScoreConfig::default(),
            dashboard: DashboardConfig::default(),
        };

        let warnings = validate_config(&config);
        let codes = warnings
            .iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"embeddings_model_ignored"));
        assert!(codes.contains(&"embeddings_endpoint_ignored"));
        assert!(codes.contains(&"graph_backend_cozo_deprecated"));
    }

    #[test]
    fn load_workspace_config_keeps_new_triage_and_deep_schema_distinct() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[sir_quality]
triage_pass = true
triage_priority_threshold = 0.25
triage_confidence_threshold = 0.5
triage_concurrency = 0
triage_timeout_secs = 0
deep_pass = true
deep_priority_threshold = 0.95
deep_confidence_threshold = 0.8
deep_max_symbols = 7
deep_concurrency = 0
deep_timeout_secs = 0
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert!(config.sir_quality.triage_pass);
        assert_eq!(config.sir_quality.triage_priority_threshold, 0.25);
        assert_eq!(config.sir_quality.triage_confidence_threshold, 0.5);
        assert_eq!(config.sir_quality.triage_concurrency, 4);
        assert_eq!(config.sir_quality.triage_timeout_secs, 180);
        assert!(config.sir_quality.deep_pass);
        assert_eq!(config.sir_quality.deep_priority_threshold, 0.95);
        assert_eq!(config.sir_quality.deep_confidence_threshold, 0.8);
        assert_eq!(config.sir_quality.deep_max_symbols, 7);
        assert_eq!(config.sir_quality.deep_concurrency, 4);
        assert_eq!(config.sir_quality.deep_timeout_secs, 180);
    }

    #[test]
    fn validate_config_is_quiet_for_defaults() {
        let warnings = validate_config(&AetherConfig::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_config_warns_when_verify_commands_empty() {
        let config = AetherConfig {
            verify: VerifyConfig {
                commands: Vec::new(),
                ..VerifyConfig::default()
            },
            ..AetherConfig::default()
        };

        let warnings = validate_config(&config);
        let codes = warnings
            .iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"verify_commands_empty"));
    }

    #[test]
    fn validate_config_warns_when_host_mode_ignores_container_settings() {
        let mut config = AetherConfig::default();
        config.verify.container.image = "rust:latest".to_owned();

        let warnings = validate_config(&config);
        let codes = warnings
            .iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"verify_container_settings_ignored_for_host"));
    }

    #[test]
    fn validate_config_warns_when_non_microvm_mode_ignores_microvm_settings() {
        let mut config = AetherConfig::default();
        config.verify.mode = VerifyMode::Host;
        config.verify.microvm.runtime = "custom-runtime".to_owned();

        let warnings = validate_config(&config);
        let codes = warnings
            .iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"verify_microvm_settings_ignored_for_non_microvm"));
    }

    #[test]
    fn validate_config_warns_when_microvm_assets_missing() {
        let mut config = AetherConfig::default();
        config.verify.mode = VerifyMode::Microvm;
        config.verify.microvm.kernel_image = Some("kernel".to_owned());
        config.verify.microvm.rootfs_image = None;

        let warnings = validate_config(&config);
        let codes = warnings
            .iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"verify_microvm_assets_missing"));
    }

    #[test]
    fn load_workspace_config_parses_all_search_reranker_values() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        for (raw_value, expected) in [
            ("none", SearchRerankerKind::None),
            ("candle", SearchRerankerKind::Candle),
            ("cohere", SearchRerankerKind::Cohere),
        ] {
            fs::write(
                config_path(workspace),
                format!(
                    r#"
[search]
reranker = "{raw_value}"
"#
                ),
            )
            .expect("write config");

            let config = load_workspace_config(workspace).expect("load config");
            assert_eq!(config.search.reranker, expected);
        }
    }

    #[test]
    fn load_workspace_config_clamps_manual_and_calibrated_thresholds() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[search]
reranker = "none"
rerank_window = 50

[search.thresholds]
default = 1.2
rust = 0.2
typescript = 0.66
python = -10.0

[search.calibrated_thresholds]
default = 0.1
rust = 0.97
typescript = 0.67
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert_eq!(config.search.thresholds.default, MAX_SEARCH_THRESHOLD);
        assert_eq!(config.search.thresholds.rust, MIN_SEARCH_THRESHOLD);
        assert_eq!(config.search.thresholds.typescript, 0.66);
        assert_eq!(config.search.thresholds.python, MIN_SEARCH_THRESHOLD);
        assert_eq!(
            config.search.calibrated_thresholds.default,
            Some(MIN_SEARCH_THRESHOLD)
        );
        assert_eq!(
            config.search.calibrated_thresholds.rust,
            Some(MAX_SEARCH_THRESHOLD)
        );
        assert_eq!(config.search.calibrated_thresholds.typescript, Some(0.67));
        assert_eq!(config.search.calibrated_thresholds.python, None);
    }

    #[test]
    fn save_workspace_config_writes_search_threshold_sections() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        let mut config = AetherConfig::default();
        config.search.thresholds.rust = 0.73;
        config.search.calibrated_thresholds.rust = Some(0.71);

        save_workspace_config(workspace, &config).expect("save config");
        let stored = load_workspace_config(workspace).expect("load config");
        assert_eq!(stored.search.thresholds.rust, 0.73);
        assert_eq!(stored.search.calibrated_thresholds.rust, Some(0.71));

        let rendered = fs::read_to_string(config_path(workspace)).expect("read config");
        assert!(rendered.contains("[search.thresholds]"));
        assert!(rendered.contains("[search.calibrated_thresholds]"));
    }

    #[test]
    fn save_workspace_config_writes_health_score_section() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        let mut config = AetherConfig::default();
        config.health_score.file_loc_warn = 900;
        config.health_score.stale_ref_patterns =
            vec!["CozoGraphStore".to_owned(), "LegacyStore".to_owned()];

        save_workspace_config(workspace, &config).expect("save config");
        let stored = load_workspace_config(workspace).expect("load config");
        assert_eq!(stored.health_score.file_loc_warn, 900);
        assert_eq!(
            stored.health_score.stale_ref_patterns,
            vec!["CozoGraphStore".to_owned(), "LegacyStore".to_owned()]
        );

        let rendered = fs::read_to_string(config_path(workspace)).expect("read config");
        assert!(rendered.contains("[health_score]"));
        assert!(rendered.contains("file_loc_warn = 900"));
    }

    #[test]
    fn inference_provider_kind_from_str_accepts_openai_compat() {
        let parsed: InferenceProviderKind =
            "openai_compat".parse().expect("openai_compat should parse");
        assert_eq!(parsed, InferenceProviderKind::OpenAiCompat);
    }

    #[test]
    fn inference_provider_kind_openai_compat_as_str_matches_config_value() {
        assert_eq!(
            InferenceProviderKind::OpenAiCompat.as_str(),
            "openai_compat"
        );
    }

    #[test]
    fn load_workspace_config_parses_openai_compat_provider() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");
        fs::write(
            config_path(workspace),
            r#"
[inference]
provider = "openai_compat"
model = "glm-4.7"
endpoint = "https://api.z.ai/api/paas/v4"
"#,
        )
        .expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert_eq!(
            config.inference.provider,
            InferenceProviderKind::OpenAiCompat
        );
    }

    #[test]
    fn embedding_provider_kind_from_str_accepts_openai_compat() {
        let parsed: EmbeddingProviderKind =
            "openai_compat".parse().expect("openai_compat should parse");
        assert_eq!(parsed, EmbeddingProviderKind::OpenAiCompat);
    }

    #[test]
    fn embedding_provider_kind_openai_compat_as_str_matches_config_value() {
        assert_eq!(
            EmbeddingProviderKind::OpenAiCompat.as_str(),
            "openai_compat"
        );
    }

    #[test]
    fn embedding_provider_kind_from_str_accepts_gemini_native() {
        let parsed: EmbeddingProviderKind =
            "gemini_native".parse().expect("gemini_native should parse");
        assert_eq!(parsed, EmbeddingProviderKind::GeminiNative);
    }

    #[test]
    fn embedding_provider_kind_gemini_native_as_str_matches_config_value() {
        assert_eq!(
            EmbeddingProviderKind::GeminiNative.as_str(),
            "gemini_native"
        );
    }

    #[test]
    fn load_workspace_config_parses_openai_compat_embedding_provider() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");
        fs::write(
            config_path(workspace),
            r#"
[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
endpoint = "https://openrouter.ai/api/v1"
task_type = " CODE_RETRIEVAL "
dimensions = 3072
"#,
        )
        .expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::OpenAiCompat
        );
        assert_eq!(
            config.embeddings.api_key_env.as_deref(),
            Some(DEFAULT_OPENAI_COMPAT_API_KEY_ENV)
        );
        assert_eq!(
            config.embeddings.task_type.as_deref(),
            Some("CODE_RETRIEVAL")
        );
        assert_eq!(config.embeddings.dimensions, Some(3072));
    }

    #[test]
    fn load_workspace_config_parses_gemini_native_embedding_provider() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");
        fs::write(
            config_path(workspace),
            r#"
[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = " GEMINI_API_KEY "
dimensions = 3072
"#,
        )
        .expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::GeminiNative
        );
        assert_eq!(
            config.embeddings.model.as_deref(),
            Some("gemini-embedding-2-preview")
        );
        assert_eq!(
            config.embeddings.api_key_env.as_deref(),
            Some("GEMINI_API_KEY")
        );
        assert_eq!(config.embeddings.dimensions, Some(3072));
    }

    #[test]
    fn validate_config_warns_on_openai_compat_without_endpoint() {
        let config = parse_workspace_config_str(
            r#"
[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
"#,
        )
        .expect("parse config");

        let warnings = validate_config(&config);
        let codes = warnings
            .iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"embeddings_endpoint_missing_for_openai_compat"));
    }

    #[test]
    fn validate_config_warns_on_unused_gemini_native_fields() {
        let config = parse_workspace_config_str(
            r#"
[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = "GEMINI_API_KEY"
endpoint = "https://ignored.example/v1"
task_type = "RETRIEVAL_DOCUMENT"
"#,
        )
        .expect("parse config");

        let warnings = validate_config(&config);
        let codes = warnings
            .iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"embeddings_endpoint_unused_for_gemini_native"));
        assert!(codes.contains(&"embeddings_task_type_unused_for_gemini_native"));
    }

    #[test]
    fn normalize_preserves_explicit_api_key_env() {
        let config = parse_workspace_config_str(
            r#"
[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
endpoint = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
"#,
        )
        .expect("parse config");

        assert_eq!(
            config.embeddings.api_key_env.as_deref(),
            Some("OPENROUTER_API_KEY")
        );
    }

    #[test]
    fn planner_config_normalizes_new_fields() {
        let config = parse_workspace_config_str(
            r#"
[planner]
semantic_rescue_threshold = 42.0
semantic_rescue_max_k = 0
community_resolution = -5.0
min_community_size = 99
"#,
        )
        .expect("parse config");
        let normalized = normalize_config(config);

        assert_eq!(normalized.planner.semantic_rescue_threshold, 0.95);
        assert_eq!(normalized.planner.semantic_rescue_max_k, 3);
        assert_eq!(normalized.planner.community_resolution, 0.1);
        assert_eq!(normalized.planner.min_community_size, 20);
    }

    #[test]
    fn planner_config_section_parses() {
        let config = parse_workspace_config_str(
            r#"
[planner]
semantic_rescue_threshold = 0.82
semantic_rescue_max_k = 5
community_resolution = 0.65
min_community_size = 4
"#,
        )
        .expect("parse config");
        let normalized = normalize_config(config);

        assert_eq!(normalized.planner.semantic_rescue_threshold, 0.82);
        assert_eq!(normalized.planner.semantic_rescue_max_k, 5);
        assert_eq!(normalized.planner.community_resolution, 0.65);
        assert_eq!(normalized.planner.min_community_size, 4);
    }
}
