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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AetherConfig {
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
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            mirror_sir_files: default_mirror_sir_files(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    #[serde(default = "default_embeddings_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub provider: EmbeddingProviderKind,
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
            model: None,
            endpoint: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyConfig {
    #[serde(default = "default_verify_commands")]
    pub commands: Vec<String>,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            commands: default_verify_commands(),
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

    warnings
}

fn default_api_key_env() -> String {
    DEFAULT_GEMINI_API_KEY_ENV.to_owned()
}

fn default_mirror_sir_files() -> bool {
    true
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

fn normalize_optional(input: Option<String>) -> Option<String> {
    input
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
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
    config.inference.model = normalize_optional(config.inference.model.take());
    config.inference.endpoint = normalize_optional(config.inference.endpoint.take());
    config.embeddings.model = normalize_optional(config.embeddings.model.take());
    config.embeddings.endpoint = normalize_optional(config.embeddings.endpoint.take());
    config.verify.commands = normalize_commands(std::mem::take(&mut config.verify.commands));

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

        assert_eq!(config.inference.provider, InferenceProviderKind::Auto);
        assert_eq!(config.inference.api_key_env, DEFAULT_GEMINI_API_KEY_ENV);
        assert!(config.storage.mirror_sir_files);
        assert!(!config.embeddings.enabled);
        assert_eq!(config.embeddings.provider, EmbeddingProviderKind::Mock);
        assert_eq!(
            config.verify.commands,
            vec![
                "cargo test".to_owned(),
                "cargo clippy --workspace -- -D warnings".to_owned()
            ]
        );
        assert!(config_path(workspace).exists());

        let content = fs::read_to_string(config_path(workspace)).expect("read config file");
        assert!(content.contains("[inference]"));
        assert!(content.contains("provider = \"auto\""));
        assert!(content.contains("[storage]"));
        assert!(content.contains("mirror_sir_files = true"));
        assert!(content.contains("[embeddings]"));
        assert!(content.contains("enabled = false"));
        assert!(content.contains("provider = \"mock\""));
        assert!(content.contains("[verify]"));
        assert!(content.contains("commands = ["));
        assert!(content.contains("\"cargo test\""));
        assert!(content.contains("\"cargo clippy --workspace -- -D warnings\""));
    }

    #[test]
    fn load_workspace_config_parses_inference_storage_and_embedding_values() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[inference]
provider = "qwen3_local"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434"
api_key_env = "CUSTOM_GEMINI_KEY"

[storage]
mirror_sir_files = false

[embeddings]
enabled = true
provider = "qwen3_local"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434/api/embeddings"

[verify]
commands = [
    " cargo test ",
    "",
    "cargo clippy --workspace -- -D warnings",
    "cargo test"
]
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");

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
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::Qwen3Local
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
    }

    #[test]
    fn ensure_workspace_config_does_not_overwrite_existing_values() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[inference]
provider = "mock"
api_key_env = "CUSTOM_KEY"

[storage]
mirror_sir_files = false

[embeddings]
enabled = true
provider = "qwen3_local"
model = "qwen3-embeddings-4B"

[verify]
commands = ["cargo --version"]
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let before = fs::read_to_string(config_path(workspace)).expect("read before");
        let config = ensure_workspace_config(workspace).expect("ensure config");
        let after = fs::read_to_string(config_path(workspace)).expect("read after");

        assert_eq!(before, after);
        assert_eq!(config.inference.provider, InferenceProviderKind::Mock);
        assert_eq!(config.inference.api_key_env, "CUSTOM_KEY");
        assert!(!config.storage.mirror_sir_files);
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::Qwen3Local
        );
        assert_eq!(config.verify.commands, vec!["cargo --version".to_owned()]);
    }

    #[test]
    fn validate_config_reports_ignored_fields() {
        let config = AetherConfig {
            inference: InferenceConfig {
                provider: InferenceProviderKind::Auto,
                model: None,
                endpoint: Some("http://127.0.0.1:11434".to_owned()),
                api_key_env: DEFAULT_GEMINI_API_KEY_ENV.to_owned(),
            },
            storage: StorageConfig {
                mirror_sir_files: true,
            },
            embeddings: EmbeddingsConfig {
                enabled: false,
                provider: EmbeddingProviderKind::Mock,
                model: Some("mock-x".to_owned()),
                endpoint: Some("http://127.0.0.1:11434/api/embeddings".to_owned()),
            },
            verify: VerifyConfig {
                commands: vec!["cargo test".to_owned()],
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
}
