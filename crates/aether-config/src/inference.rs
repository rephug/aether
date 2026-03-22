use serde::{Deserialize, Serialize};

use crate::constants::{
    DEFAULT_GEMINI_API_KEY_ENV, DEFAULT_QWEN_ENDPOINT, DEFAULT_QWEN_MODEL, DEFAULT_SIR_CONCURRENCY,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

impl GeminiThinkingLevel {
    pub fn api_value(self) -> &'static str {
        match self {
            Self::Minimal => "MINIMAL",
            Self::Low => "LOW",
            Self::Medium => "MEDIUM",
            Self::High => "HIGH",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Returns the explicit Gemini 3 thinking level to send.
///
/// `None` means omit `thinkingConfig` and let Gemini use its default dynamic behavior.
pub fn parse_gemini_thinking_level(thinking: Option<&str>) -> Option<GeminiThinkingLevel> {
    match thinking
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("minimal") => Some(GeminiThinkingLevel::Minimal),
        Some("low") => Some(GeminiThinkingLevel::Low),
        Some("medium") => Some(GeminiThinkingLevel::Medium),
        Some("high") => Some(GeminiThinkingLevel::High),
        _ => None,
    }
}

/// Stable label that matches the effective Gemini behavior for hashing / telemetry.
pub fn gemini_thinking_fingerprint(thinking: Option<&str>) -> &'static str {
    parse_gemini_thinking_level(thinking)
        .map(GeminiThinkingLevel::config_value)
        .unwrap_or("dynamic")
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
    pub thinking: Option<String>,
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
            thinking: None,
            tiered: None,
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

pub(crate) fn default_api_key_env() -> String {
    DEFAULT_GEMINI_API_KEY_ENV.to_owned()
}

pub(crate) fn default_sir_concurrency() -> usize {
    DEFAULT_SIR_CONCURRENCY
}

pub(crate) fn default_tiered_primary_threshold() -> f64 {
    0.8
}

pub(crate) fn default_tiered_fallback_model() -> Option<String> {
    Some(DEFAULT_QWEN_MODEL.to_owned())
}

pub(crate) fn default_tiered_fallback_endpoint() -> Option<String> {
    Some(DEFAULT_QWEN_ENDPOINT.to_owned())
}

fn default_tiered_retry_with_fallback() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{
        GeminiThinkingLevel, InferenceProviderKind, gemini_thinking_fingerprint,
        parse_gemini_thinking_level,
    };

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
    fn parse_gemini_thinking_level_accepts_supported_values() {
        assert_eq!(
            parse_gemini_thinking_level(Some("minimal")),
            Some(GeminiThinkingLevel::Minimal)
        );
        assert_eq!(
            parse_gemini_thinking_level(Some(" low ")),
            Some(GeminiThinkingLevel::Low)
        );
        assert_eq!(
            parse_gemini_thinking_level(Some("MEDIUM")),
            Some(GeminiThinkingLevel::Medium)
        );
        assert_eq!(
            parse_gemini_thinking_level(Some("high")),
            Some(GeminiThinkingLevel::High)
        );
    }

    #[test]
    fn parse_gemini_thinking_level_omits_dynamic_and_invalid_values() {
        assert_eq!(parse_gemini_thinking_level(Some("dynamic")), None);
        assert_eq!(parse_gemini_thinking_level(Some("off")), None);
        assert_eq!(parse_gemini_thinking_level(Some("none")), None);
        assert_eq!(parse_gemini_thinking_level(Some("bogus")), None);
        assert_eq!(parse_gemini_thinking_level(None), None);
    }

    #[test]
    fn gemini_thinking_fingerprint_tracks_effective_behavior() {
        assert_eq!(gemini_thinking_fingerprint(Some("minimal")), "minimal");
        assert_eq!(gemini_thinking_fingerprint(Some("dynamic")), "dynamic");
        assert_eq!(gemini_thinking_fingerprint(Some("off")), "dynamic");
        assert_eq!(gemini_thinking_fingerprint(None), "dynamic");
    }
}
