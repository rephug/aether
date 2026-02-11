use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const AETHER_DIR_NAME: &str = ".aether";
pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const DEFAULT_GEMINI_API_KEY_ENV: &str = "GEMINI_API_KEY";
pub const DEFAULT_QWEN_ENDPOINT: &str = "http://127.0.0.1:11434";
pub const DEFAULT_QWEN_MODEL: &str = "qwen3-embeddings-0.6B";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AetherConfig {
    #[serde(default)]
    pub inference: InferenceConfig,
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

fn default_api_key_env() -> String {
    DEFAULT_GEMINI_API_KEY_ENV.to_owned()
}

fn normalize_optional(input: Option<String>) -> Option<String> {
    input
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_config(mut config: AetherConfig) -> AetherConfig {
    config.inference.model = normalize_optional(config.inference.model.take());
    config.inference.endpoint = normalize_optional(config.inference.endpoint.take());

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
        assert!(config_path(workspace).exists());

        let content = fs::read_to_string(config_path(workspace)).expect("read config file");
        assert!(content.contains("[inference]"));
        assert!(content.contains("provider = \"auto\""));
    }

    #[test]
    fn load_workspace_config_parses_inference_values() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[inference]
provider = "qwen3_local"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434"
api_key_env = "CUSTOM_GEMINI_KEY"
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
    }
}
