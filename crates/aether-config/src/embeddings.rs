use serde::{Deserialize, Serialize};

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

fn default_embeddings_enabled() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::EmbeddingProviderKind;

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
}
