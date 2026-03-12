use serde::{Deserialize, Serialize};

use crate::{
    constants::{
        DEFAULT_COHERE_API_KEY_ENV, DEFAULT_SEARCH_THRESHOLD_DEFAULT,
        DEFAULT_SEARCH_THRESHOLD_PYTHON, DEFAULT_SEARCH_THRESHOLD_RUST,
        DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT,
    },
    normalize::normalize_threshold_language,
};

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

pub(crate) fn default_rerank_window() -> u32 {
    50
}

pub(crate) fn default_search_threshold_default() -> f32 {
    DEFAULT_SEARCH_THRESHOLD_DEFAULT
}

pub(crate) fn default_search_threshold_rust() -> f32 {
    DEFAULT_SEARCH_THRESHOLD_RUST
}

pub(crate) fn default_search_threshold_typescript() -> f32 {
    DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT
}

pub(crate) fn default_search_threshold_python() -> f32 {
    DEFAULT_SEARCH_THRESHOLD_PYTHON
}

pub(crate) fn default_cohere_api_key_env() -> String {
    DEFAULT_COHERE_API_KEY_ENV.to_owned()
}

#[cfg(test)]
mod tests {
    use super::SearchRerankerKind;

    #[test]
    fn search_reranker_kind_from_str_accepts_all_values() {
        for (raw, expected) in [
            ("none", SearchRerankerKind::None),
            ("candle", SearchRerankerKind::Candle),
            ("cohere", SearchRerankerKind::Cohere),
        ] {
            let parsed: SearchRerankerKind = raw.parse().expect("reranker should parse");
            assert_eq!(parsed, expected);
            assert_eq!(expected.as_str(), raw);
        }
    }
}
