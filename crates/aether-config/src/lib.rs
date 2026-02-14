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
pub const DEFAULT_VERIFY_CONTAINER_RUNTIME: &str = "docker";
pub const DEFAULT_VERIFY_CONTAINER_IMAGE: &str = "rust:1-bookworm";
pub const DEFAULT_VERIFY_CONTAINER_WORKDIR: &str = "/workspace";
pub const DEFAULT_VERIFY_MICROVM_RUNTIME: &str = "firecracker";
pub const DEFAULT_VERIFY_MICROVM_WORKDIR: &str = "/workspace";
pub const DEFAULT_VERIFY_MICROVM_VCPU_COUNT: u8 = 1;
pub const DEFAULT_VERIFY_MICROVM_MEMORY_MIB: u32 = 1024;
pub const DEFAULT_LOG_LEVEL: &str = "info";

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
}

impl EmbeddingProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Qwen3Local => "qwen3_local",
        }
    }
}

impl std::str::FromStr for EmbeddingProviderKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "mock" => Ok(Self::Mock),
            "qwen3_local" => Ok(Self::Qwen3Local),
            other => Err(format!(
                "invalid embedding provider '{other}', expected one of: mock, qwen3_local"
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
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
    pub verify: VerifyConfig,
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
}

impl Default for EmbeddingsConfig {
    fn default() -> Self {
        Self {
            enabled: default_embeddings_enabled(),
            provider: EmbeddingProviderKind::Mock,
            vector_backend: EmbeddingVectorBackend::Lancedb,
            model: None,
            endpoint: None,
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
    let content = toml::to_string_pretty(&config)?;
    fs::write(path, content)?;

    Ok(config)
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

fn default_verify_commands() -> Vec<String> {
    vec![
        "cargo test".to_owned(),
        "cargo clippy --workspace -- -D warnings".to_owned(),
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

fn normalize_config(mut config: AetherConfig) -> AetherConfig {
    config.general.log_level = normalize_with_default(
        std::mem::take(&mut config.general.log_level),
        default_log_level(),
    );
    config.inference.model = normalize_optional(config.inference.model.take());
    config.inference.endpoint = normalize_optional(config.inference.endpoint.take());
    config.embeddings.model = normalize_optional(config.embeddings.model.take());
    config.embeddings.endpoint = normalize_optional(config.embeddings.endpoint.take());
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

    let api_key_env = config.inference.api_key_env.trim();
    if api_key_env.is_empty() {
        config.inference.api_key_env = default_api_key_env();
    } else {
        config.inference.api_key_env = api_key_env.to_owned();
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
        assert_eq!(
            config.verify.commands,
            vec![
                "cargo test".to_owned(),
                "cargo clippy --workspace -- -D warnings".to_owned()
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
        assert!(content.contains("\"cargo test\""));
        assert!(content.contains("\"cargo clippy --workspace -- -D warnings\""));
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
        assert_eq!(
            config.verify.commands,
            vec![
                "cargo test".to_owned(),
                "cargo clippy --workspace -- -D warnings".to_owned()
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
        assert_eq!(config.verify.commands, vec!["cargo --version".to_owned()]);
        assert_eq!(config.verify.mode, VerifyMode::Container);
        assert!(config.verify.container.fallback_to_host_on_unavailable);
        assert!(config.verify.microvm.fallback_to_container_on_unavailable);
        assert!(config.verify.microvm.fallback_to_host_on_unavailable);
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
            },
            verify: VerifyConfig {
                commands: vec!["cargo test".to_owned()],
                ..VerifyConfig::default()
            },
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
}
