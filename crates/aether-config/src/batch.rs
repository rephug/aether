use serde::{Deserialize, Serialize};

use crate::omp::{
    batch_pricing_providers_list, batch_target_for_omp_route, strip_omp_route_prefix,
};

/// `[batch].provider` value that derives the batch provider from the omp route in
/// `[inference].model` (see [`BatchConfig::resolve_provider`]).
pub const BATCH_PROVIDER_AUTO: &str = "auto";

/// Per-provider overrides for batch models and thinking levels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BatchProviderConfig {
    #[serde(default)]
    pub scan_model: Option<String>,
    #[serde(default)]
    pub triage_model: Option<String>,
    #[serde(default)]
    pub deep_model: Option<String>,
    #[serde(default)]
    pub scan_thinking: Option<String>,
    #[serde(default)]
    pub triage_thinking: Option<String>,
    #[serde(default)]
    pub deep_thinking: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchConfig {
    #[serde(default)]
    pub scan_model: String,
    #[serde(default)]
    pub triage_model: String,
    #[serde(default)]
    pub deep_model: String,
    #[serde(default = "default_scan_thinking")]
    pub scan_thinking: String,
    #[serde(default = "default_triage_thinking")]
    pub triage_thinking: String,
    #[serde(default = "default_deep_thinking")]
    pub deep_thinking: String,
    #[serde(default = "default_triage_neighbor_depth")]
    pub triage_neighbor_depth: u32,
    #[serde(default = "default_deep_neighbor_depth")]
    pub deep_neighbor_depth: u32,
    #[serde(default = "default_scan_max_chars")]
    pub scan_max_chars: usize,
    #[serde(default = "default_triage_max_chars")]
    pub triage_max_chars: usize,
    #[serde(default)]
    pub deep_max_chars: usize,
    #[serde(default = "default_passes")]
    pub passes: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_chain: bool,
    #[serde(default)]
    pub batch_dir: String,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_jsonl_chunk_size")]
    pub jsonl_chunk_size: usize,
    /// Maximum symbols to process per batch pass. 0 = unlimited.
    #[serde(default)]
    pub max_symbols: usize,
    /// Maximum number of batch jobs active simultaneously.
    /// Gemini enforces ~4 concurrent jobs; Anthropic and OpenAI have higher limits.
    #[serde(default = "default_max_concurrent_jobs")]
    pub max_concurrent_jobs: usize,
    /// System prompt tier: "compact", "standard", "full", or "auto" (default).
    /// "auto" selects based on provider: cloud providers get "full", local gets "compact".
    #[serde(default = "default_prompt_tier")]
    pub prompt_tier: String,
    /// Batch provider: "gemini", "openai", "anthropic", or "auto".
    ///
    /// "auto" follows the omp route configured in `[inference].model`: an
    /// `anthropic/...` route submits to the Anthropic Message Batches API, `openai/...` to the
    /// OpenAI Batch API, `google/...` to Gemini Batch Mode. Routes whose provider offers no
    /// batch pricing (subscription-only or local routes) fail with an explicit error.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Provider-specific overrides for Gemini batch.
    #[serde(default)]
    pub gemini: BatchProviderConfig,
    /// Provider-specific overrides for OpenAI batch.
    #[serde(default)]
    pub openai: BatchProviderConfig,
    /// Provider-specific overrides for Anthropic batch (reserved for 10.7b).
    #[serde(default)]
    pub anthropic: BatchProviderConfig,
}

impl BatchConfig {
    /// Resolve the effective batch provider name.
    ///
    /// Precedence: `override_name` (CLI `--provider`), then `[batch].provider`. A value of
    /// `"auto"` derives the provider from `inference_route`, the omp route configured in
    /// `[inference].model`; routes with no batch API are an error naming the providers that
    /// do offer batch pricing.
    pub fn resolve_provider(
        &self,
        override_name: Option<&str>,
        inference_route: Option<&str>,
    ) -> Result<String, String> {
        let selected = override_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| self.provider.trim());
        if !selected.eq_ignore_ascii_case(BATCH_PROVIDER_AUTO) {
            return Ok(selected.to_owned());
        }
        let route = inference_route
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!(
                    "batch.provider=auto needs an omp route in inference.model (provider/model); \
                     set one or choose a batch provider explicitly ({})",
                    batch_pricing_providers_list()
                )
            })?;
        batch_target_for_omp_route(route)
            .map(|(provider, _)| provider.to_owned())
            .ok_or_else(|| {
                format!(
                    "batch.provider=auto: route '{route}' has no batch pricing (only {} offer a \
                     batch API); pick one of those routes or set batch.provider explicitly",
                    batch_pricing_providers_list()
                )
            })
    }

    /// Resolve the model for a given pass, checking the provider subsection first,
    /// then falling back to the top-level flat fields.
    ///
    /// omp-style routes are accepted anywhere a model is configured: an `anthropic/` prefix
    /// is stripped for the anthropic batch provider (and likewise for openai and gemini), so
    /// the same route string can be shared with `[inference].model`.
    pub fn resolve_model(&self, pass: &str, provider: &str) -> &str {
        let provider_config = self.provider_config(provider);
        let override_val = match pass {
            "scan" => provider_config.scan_model.as_deref(),
            "triage" => provider_config.triage_model.as_deref(),
            "deep" => provider_config.deep_model.as_deref(),
            _ => None,
        };
        if let Some(val) = override_val.filter(|s| !s.is_empty()) {
            return strip_omp_route_prefix(val, provider);
        }
        let flat = match pass {
            "scan" => self.scan_model.as_str(),
            "triage" => self.triage_model.as_str(),
            "deep" => self.deep_model.as_str(),
            _ => "",
        };
        strip_omp_route_prefix(flat, provider)
    }

    /// Like [`Self::resolve_model`], but when no batch model is configured for the pass,
    /// fall back to the bare model of `inference_route` if that route maps to `provider`.
    ///
    /// This is what lets `[inference] model = "anthropic/claude-fable-5"` drive both the
    /// online (gateway) path and the batch path without repeating the model name.
    pub fn resolve_model_for_route(
        &self,
        pass: &str,
        provider: &str,
        inference_route: Option<&str>,
    ) -> String {
        let configured = self.resolve_model(pass, provider);
        if !configured.is_empty() {
            return configured.to_owned();
        }
        inference_route
            .and_then(batch_target_for_omp_route)
            .filter(|(route_provider, _)| *route_provider == provider)
            .map(|(_, model)| model.to_owned())
            .unwrap_or_default()
    }

    /// Resolve the thinking level for a given pass, checking the provider subsection first.
    pub fn resolve_thinking(&self, pass: &str, provider: &str) -> &str {
        let provider_config = self.provider_config(provider);
        let override_val = match pass {
            "scan" => provider_config.scan_thinking.as_deref(),
            "triage" => provider_config.triage_thinking.as_deref(),
            "deep" => provider_config.deep_thinking.as_deref(),
            _ => None,
        };
        if let Some(val) = override_val.filter(|s| !s.is_empty()) {
            return val;
        }
        match pass {
            "scan" => self.scan_thinking.as_str(),
            "triage" => self.triage_thinking.as_str(),
            "deep" => self.deep_thinking.as_str(),
            _ => "off",
        }
    }

    /// Resolve the environment variable name holding the API key for a provider.
    pub fn resolve_api_key_env(&self, provider: &str) -> String {
        let provider_config = self.provider_config(provider);
        if let Some(ref env_var) = provider_config.api_key_env
            && !env_var.is_empty()
        {
            return env_var.clone();
        }
        match provider {
            "gemini" => "GEMINI_API_KEY".to_owned(),
            "openai" => "OPENAI_API_KEY".to_owned(),
            "anthropic" => "ANTHROPIC_API_KEY".to_owned(),
            other => format!("{}_API_KEY", other.to_ascii_uppercase()),
        }
    }

    fn provider_config(&self, provider: &str) -> &BatchProviderConfig {
        match provider {
            "gemini" => &self.gemini,
            "openai" => &self.openai,
            "anthropic" => &self.anthropic,
            _ => &self.gemini,
        }
    }
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            scan_model: String::new(),
            triage_model: String::new(),
            deep_model: String::new(),
            scan_thinking: default_scan_thinking(),
            triage_thinking: default_triage_thinking(),
            deep_thinking: default_deep_thinking(),
            triage_neighbor_depth: default_triage_neighbor_depth(),
            deep_neighbor_depth: default_deep_neighbor_depth(),
            scan_max_chars: default_scan_max_chars(),
            triage_max_chars: default_triage_max_chars(),
            deep_max_chars: 0,
            passes: default_passes(),
            auto_chain: default_true(),
            batch_dir: String::new(),
            poll_interval_secs: default_poll_interval(),
            jsonl_chunk_size: default_jsonl_chunk_size(),
            max_symbols: 0,
            max_concurrent_jobs: default_max_concurrent_jobs(),
            prompt_tier: default_prompt_tier(),
            provider: default_provider(),
            gemini: BatchProviderConfig::default(),
            openai: BatchProviderConfig::default(),
            anthropic: BatchProviderConfig::default(),
        }
    }
}

fn default_provider() -> String {
    "gemini".to_owned()
}

fn default_scan_thinking() -> String {
    "low".to_owned()
}

fn default_triage_thinking() -> String {
    "medium".to_owned()
}

fn default_deep_thinking() -> String {
    "high".to_owned()
}

fn default_triage_neighbor_depth() -> u32 {
    1
}

fn default_deep_neighbor_depth() -> u32 {
    2
}

fn default_scan_max_chars() -> usize {
    10_000
}

fn default_triage_max_chars() -> usize {
    10_000
}

fn default_passes() -> Vec<String> {
    vec!["scan".to_owned()]
}

fn default_true() -> bool {
    true
}

fn default_poll_interval() -> u64 {
    60
}

fn default_jsonl_chunk_size() -> usize {
    5_000
}

fn default_max_concurrent_jobs() -> usize {
    4
}

fn default_prompt_tier() -> String {
    "auto".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_provider(provider: &str) -> BatchConfig {
        BatchConfig {
            provider: provider.to_owned(),
            ..BatchConfig::default()
        }
    }

    #[test]
    fn resolve_provider_prefers_cli_override_then_config() {
        let config = config_with_provider("gemini");
        assert_eq!(
            config.resolve_provider(Some("anthropic"), None).unwrap(),
            "anthropic"
        );
        assert_eq!(config.resolve_provider(Some("  "), None).unwrap(), "gemini");
        assert_eq!(config.resolve_provider(None, None).unwrap(), "gemini");
    }

    #[test]
    fn resolve_provider_auto_follows_omp_route() {
        let config = config_with_provider("auto");
        assert_eq!(
            config
                .resolve_provider(None, Some("anthropic/claude-fable-5"))
                .unwrap(),
            "anthropic"
        );
        assert_eq!(
            config
                .resolve_provider(None, Some("google/gemini-3.1-pro"))
                .unwrap(),
            "gemini"
        );
        assert_eq!(
            config
                .resolve_provider(Some("auto"), Some("openai/gpt-5.6-sol"))
                .unwrap(),
            "openai"
        );
    }

    #[test]
    fn resolve_provider_auto_rejects_routes_without_batch_pricing() {
        let config = config_with_provider("auto");
        let error = config
            .resolve_provider(None, Some("openai-codex/gpt-5.6-sol"))
            .unwrap_err();
        assert!(error.contains("no batch pricing"), "{error}");
        assert!(error.contains("anthropic, openai, gemini"), "{error}");

        let error = config.resolve_provider(None, None).unwrap_err();
        assert!(error.contains("needs an omp route"), "{error}");

        let error = config
            .resolve_provider(None, Some("claude-fable-5"))
            .unwrap_err();
        assert!(error.contains("no batch pricing"), "{error}");
    }

    #[test]
    fn resolve_model_strips_matching_omp_prefix() {
        let config = BatchConfig {
            scan_model: "anthropic/claude-haiku-4-5".to_owned(),
            anthropic: BatchProviderConfig {
                deep_model: Some("anthropic/claude-fable-5".to_owned()),
                ..BatchProviderConfig::default()
            },
            ..BatchConfig::default()
        };
        assert_eq!(
            config.resolve_model("scan", "anthropic"),
            "claude-haiku-4-5"
        );
        assert_eq!(config.resolve_model("deep", "anthropic"), "claude-fable-5");
        // A route for another provider is left intact so the provider can reject it.
        assert_eq!(
            config.resolve_model("scan", "openai"),
            "anthropic/claude-haiku-4-5"
        );
    }

    #[test]
    fn resolve_model_for_route_falls_back_to_inference_route() {
        let config = BatchConfig::default();
        assert_eq!(
            config.resolve_model_for_route("scan", "anthropic", Some("anthropic/claude-fable-5")),
            "claude-fable-5"
        );
        // A route for a different provider must not leak into this provider's batch.
        assert_eq!(
            config.resolve_model_for_route("scan", "openai", Some("anthropic/claude-fable-5")),
            ""
        );
        let configured = BatchConfig {
            scan_model: "claude-haiku-4-5".to_owned(),
            ..BatchConfig::default()
        };
        assert_eq!(
            configured.resolve_model_for_route(
                "scan",
                "anthropic",
                Some("anthropic/claude-fable-5")
            ),
            "claude-haiku-4-5"
        );
    }
}
