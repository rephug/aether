use crate::GEMINI_DEFAULT_CONCURRENCY;
use serde::{Deserialize, Serialize};

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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage_thinking: Option<String>,

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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deep_thinking: Option<String>,

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
            triage_thinking: None,
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
            deep_thinking: None,
            deep_max_symbols: default_deep_max_symbols(),
            deep_max_neighbors: default_deep_max_neighbors(),
            deep_concurrency: default_deep_concurrency(),
            deep_timeout_secs: default_deep_timeout_secs(),
        }
    }
}

pub(crate) fn default_triage_priority_threshold() -> f64 {
    0.7
}

pub(crate) fn default_triage_confidence_threshold() -> f64 {
    0.85
}

pub(crate) fn default_triage_max_symbols() -> usize {
    0
}

pub(crate) fn default_triage_concurrency() -> usize {
    GEMINI_DEFAULT_CONCURRENCY
}

pub(crate) fn default_triage_timeout_secs() -> u64 {
    180
}

pub(crate) fn default_deep_priority_threshold() -> f64 {
    0.9
}

pub(crate) fn default_deep_confidence_threshold() -> f64 {
    0.85
}

pub(crate) fn default_deep_max_symbols() -> usize {
    20
}

pub(crate) fn default_deep_max_neighbors() -> usize {
    10
}

pub(crate) fn default_deep_concurrency() -> usize {
    GEMINI_DEFAULT_CONCURRENCY
}

pub(crate) fn default_deep_timeout_secs() -> u64 {
    180
}
