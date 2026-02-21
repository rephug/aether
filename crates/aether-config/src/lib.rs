use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const AETHER_DIR_NAME: &str = ".aether";
pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const DEFAULT_GEMINI_API_KEY_ENV: &str = "GEMINI_API_KEY";
pub const DEFAULT_QWEN_ENDPOINT: &str = "http://127.0.0.1:11434";
pub const DEFAULT_QWEN_MODEL: &str = "qwen3-embeddings-0.6B";
pub const DEFAULT_QWEN_EMBEDDING_ENDPOINT: &str = "http://127.0.0.1:11434/api/embeddings";
pub const RECOMMENDED_OLLAMA_MODEL: &str = "qwen2.5-coder:7b-instruct-q4_K_M";
pub const OLLAMA_DEFAULT_ENDPOINT: &str = "http://127.0.0.1:11434";
pub const SIR_QUALITY_FLOOR_CONFIDENCE: f32 = 0.3;
pub const SIR_QUALITY_FLOOR_WINDOW: usize = 10;
pub const OLLAMA_SIR_TEMPERATURE: f32 = 0.1;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InferenceProviderKind {
    #[default]
    Auto,
    Mock,
    Gemini,
    Qwen3Local,
}

impl InferenceProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Mock => "mock",
            Self::Gemini => "gemini",
            Self::Qwen3Local => "qwen3_local",
        }
    }
}

impl std::str::FromStr for InferenceProviderKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "auto" => Ok(Self::Auto),
            "mock" => Ok(Self::Mock),
            "gemini" => Ok(Self::Gemini),
            "qwen3_local" => Ok(Self::Qwen3Local),
            other => Err(format!(
                "invalid provider '{other}', expected one of: auto, mock, gemini, qwen3_local"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProviderKind {
    #[default]
    Mock,
    Qwen3Local,
    Candle,
}

impl EmbeddingProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Qwen3Local => "qwen3_local",
            Self::Candle => "candle",
        }
    }
}

impl std::str::FromStr for EmbeddingProviderKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "mock" => Ok(Self::Mock),
            "qwen3_local" => Ok(Self::Qwen3Local),
            "candle" => Ok(Self::Candle),
            other => Err(format!(
                "invalid embedding provider '{other}', expected one of: mock, qwen3_local, candle"
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
    Cozo,
    Sqlite,
}

impl GraphBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cozo => "cozo",
            Self::Sqlite => "sqlite",
        }
    }
}

impl std::str::FromStr for GraphBackend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "cozo" => Ok(Self::Cozo),
            "sqlite" => Ok(Self::Sqlite),
            other => Err(format!(
                "invalid graph backend '{other}', expected one of: cozo, sqlite"
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceConfig {
    #[serde(default)]
    pub provider: InferenceProviderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            provider: InferenceProviderKind::Auto,
            model: None,
            endpoint: None,
            api_key_env: default_api_key_env(),
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
            provider: EmbeddingProviderKind::Mock,
            vector_backend: EmbeddingVectorBackend::Lancedb,
            model: None,
            endpoint: None,
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
    let parsed: AetherConfig = toml::from_str(&raw)?;
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

pub fn validate_config(config: &AetherConfig) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();

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
    } else if matches!(config.embeddings.provider, EmbeddingProviderKind::Mock) {
        if config.embeddings.model.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_model_unused_for_mock",
                message: "embeddings.provider=mock ignores embeddings.model".to_owned(),
            });
        }
        if config.embeddings.endpoint.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_endpoint_unused_for_mock",
                message: "embeddings.provider=mock ignores embeddings.endpoint".to_owned(),
            });
        }
        if config.embeddings.candle.model_dir.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_candle_model_dir_unused_for_mock",
                message: "embeddings.provider=mock ignores embeddings.candle.model_dir".to_owned(),
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
        InferenceProviderKind::Auto => {
            if config.inference.endpoint.is_some() {
                warnings.push(ConfigWarning {
                    code: "inference_endpoint_ignored_for_auto",
                    message: "inference.endpoint is ignored when inference.provider=auto"
                        .to_owned(),
                });
            }
        }
        InferenceProviderKind::Mock => {
            if config.inference.model.is_some() {
                warnings.push(ConfigWarning {
                    code: "inference_model_ignored_for_mock",
                    message: "inference.provider=mock ignores inference.model".to_owned(),
                });
            }
            if config.inference.endpoint.is_some() {
                warnings.push(ConfigWarning {
                    code: "inference_endpoint_ignored_for_mock",
                    message: "inference.provider=mock ignores inference.endpoint".to_owned(),
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

fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_owned()
}

fn default_mirror_sir_files() -> bool {
    true
}

fn default_graph_backend() -> GraphBackend {
    GraphBackend::Cozo
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

fn normalize_config(mut config: AetherConfig) -> AetherConfig {
    config.general.log_level = normalize_with_default(
        std::mem::take(&mut config.general.log_level),
        default_log_level(),
    );
    config.inference.model = normalize_optional(config.inference.model.take());
    config.inference.endpoint = normalize_optional(config.inference.endpoint.take());
    config.embeddings.model = normalize_optional(config.embeddings.model.take());
    config.embeddings.endpoint = normalize_optional(config.embeddings.endpoint.take());
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

    let api_key_env = config.inference.api_key_env.trim();
    if api_key_env.is_empty() {
        config.inference.api_key_env = default_api_key_env();
    } else {
        config.inference.api_key_env = api_key_env.to_owned();
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
        assert!(config.storage.mirror_sir_files);
        assert_eq!(config.storage.graph_backend, GraphBackend::Cozo);
        assert!(!config.embeddings.enabled);
        assert_eq!(config.embeddings.provider, EmbeddingProviderKind::Mock);
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
        assert!(config_path(workspace).exists());

        let content = fs::read_to_string(config_path(workspace)).expect("read config file");
        assert!(content.contains("[general]"));
        assert!(content.contains("log_level = \"info\""));
        assert!(content.contains("[inference]"));
        assert!(content.contains("provider = \"auto\""));
        assert!(content.contains("[storage]"));
        assert!(content.contains("mirror_sir_files = true"));
        assert!(content.contains("graph_backend = \"cozo\""));
        assert!(content.contains("[embeddings]"));
        assert!(content.contains("enabled = false"));
        assert!(content.contains("provider = \"mock\""));
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
provider = "mock"
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
        assert_eq!(config.inference.provider, InferenceProviderKind::Mock);
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
    fn validate_config_reports_ignored_fields() {
        let config = AetherConfig {
            general: GeneralConfig::default(),
            inference: InferenceConfig {
                provider: InferenceProviderKind::Auto,
                model: None,
                endpoint: Some("http://127.0.0.1:11434".to_owned()),
                api_key_env: DEFAULT_GEMINI_API_KEY_ENV.to_owned(),
            },
            storage: StorageConfig {
                mirror_sir_files: true,
                graph_backend: GraphBackend::Cozo,
            },
            embeddings: EmbeddingsConfig {
                enabled: false,
                provider: EmbeddingProviderKind::Mock,
                vector_backend: EmbeddingVectorBackend::Lancedb,
                model: Some("mock-x".to_owned()),
                endpoint: Some("http://127.0.0.1:11434/api/embeddings".to_owned()),
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
        };

        let warnings = validate_config(&config);
        let codes = warnings
            .iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"inference_endpoint_ignored_for_auto"));
        assert!(codes.contains(&"embeddings_model_ignored"));
        assert!(codes.contains(&"embeddings_endpoint_ignored"));
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
}
