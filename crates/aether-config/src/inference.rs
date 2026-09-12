use serde::{Deserialize, Serialize};

use crate::constants::{
    DEFAULT_GEMINI_API_KEY_ENV, DEFAULT_OMP_BROKER_BIND, DEFAULT_OMP_COMMAND,
    DEFAULT_OMP_GATEWAY_BIND, DEFAULT_OMP_STARTUP_TIMEOUT_SECS, DEFAULT_QWEN_ENDPOINT,
    DEFAULT_QWEN_MODEL, DEFAULT_SIR_CONCURRENCY,
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
    /// Oh My Pi auth gateway (`omp auth-gateway serve`): OpenAI-compatible transport,
    /// models addressed as OMP routes (`provider/model`), billed on the OMP credential.
    Omp,
}

impl InferenceProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Tiered => "tiered",
            Self::Gemini => "gemini",
            Self::Qwen3Local => "qwen3_local",
            Self::OpenAiCompat => "openai_compat",
            Self::Omp => "omp",
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
            "omp" => Ok(Self::Omp),
            other => Err(format!(
                "invalid provider '{other}', expected one of: auto, tiered, gemini, qwen3_local, openai_compat, omp"
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
    /// `[inference.omp]`: how AETHER brings up the Oh My Pi broker and gateway itself.
    #[serde(default)]
    pub omp: OmpConfig,
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
            omp: OmpConfig::default(),
        }
    }
}

/// `[inference.omp]`: autostart settings for the Oh My Pi auth broker + gateway pair.
///
/// After `omp` login the credentials live in `~/.omp/agent`; `omp auth-broker serve` serves
/// them and `omp auth-gateway serve` exposes an OpenAI-compatible endpoint in front of the
/// broker. With `autostart = true` AETHER spawns both on demand when the gateway endpoint is
/// not reachable, so a fresh login is all that is needed before `aetherd index`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmpConfig {
    /// Spawn the broker and gateway when the gateway is unreachable (default `true`).
    #[serde(default = "default_omp_autostart")]
    pub autostart: bool,
    /// The omp CLI to spawn (default `omp`; may be an absolute path).
    #[serde(default = "default_omp_command")]
    pub command: String,
    /// `omp auth-broker serve --bind` value.
    #[serde(default = "default_omp_broker_bind")]
    pub broker_bind: String,
    /// `omp auth-gateway serve --bind` value. Must agree with `inference.endpoint` when that
    /// is set.
    #[serde(default = "default_omp_gateway_bind")]
    pub gateway_bind: String,
    /// Seconds to wait for each process to report healthy.
    #[serde(default = "default_omp_startup_timeout_secs")]
    pub startup_timeout_secs: u64,
}

impl Default for OmpConfig {
    fn default() -> Self {
        Self {
            autostart: default_omp_autostart(),
            command: default_omp_command(),
            broker_bind: default_omp_broker_bind(),
            gateway_bind: default_omp_gateway_bind(),
            startup_timeout_secs: default_omp_startup_timeout_secs(),
        }
    }
}

impl OmpConfig {
    /// `http://<broker_bind>` — what `OMP_AUTH_BROKER_URL` is set to for the gateway.
    pub fn broker_url(&self) -> String {
        format!("http://{}", self.broker_bind.trim())
    }

    /// `http://<gateway_bind>` — the gateway origin (health lives at `/healthz`).
    pub fn gateway_url(&self) -> String {
        format!("http://{}", self.gateway_bind.trim())
    }
}

fn default_omp_autostart() -> bool {
    true
}

fn default_omp_command() -> String {
    DEFAULT_OMP_COMMAND.to_owned()
}

fn default_omp_broker_bind() -> String {
    DEFAULT_OMP_BROKER_BIND.to_owned()
}

fn default_omp_gateway_bind() -> String {
    DEFAULT_OMP_GATEWAY_BIND.to_owned()
}

fn default_omp_startup_timeout_secs() -> u64 {
    DEFAULT_OMP_STARTUP_TIMEOUT_SECS
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
    fn inference_provider_kind_round_trips_omp() {
        let parsed: InferenceProviderKind = "omp".parse().expect("omp should parse");
        assert_eq!(parsed, InferenceProviderKind::Omp);
        assert_eq!(InferenceProviderKind::Omp.as_str(), "omp");
        let toml_value: InferenceProviderKind = toml::from_str::<toml::Value>("kind = \"omp\"")
            .expect("toml")
            .get("kind")
            .cloned()
            .expect("kind")
            .try_into()
            .expect("deserialize omp");
        assert_eq!(toml_value, InferenceProviderKind::Omp);
    }

    #[test]
    fn omp_config_defaults_and_urls() {
        let config = super::OmpConfig::default();
        assert!(config.autostart);
        assert_eq!(config.command, "omp");
        assert_eq!(config.broker_url(), "http://127.0.0.1:8765");
        assert_eq!(config.gateway_url(), "http://127.0.0.1:4000");
        assert_eq!(config.startup_timeout_secs, 20);

        let parsed: super::InferenceConfig = toml::from_str(
            "provider = \"omp\"\nmodel = \"anthropic/claude-fable-5\"\n[omp]\nautostart = false\ngateway_bind = \"127.0.0.1:4100\"\n",
        )
        .expect("parse");
        assert!(!parsed.omp.autostart);
        assert_eq!(parsed.omp.gateway_url(), "http://127.0.0.1:4100");
        assert_eq!(parsed.omp.command, "omp");
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
