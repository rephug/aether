use serde::{Deserialize, Serialize};

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
    /// Batch provider: "gemini", "openai", or "anthropic".
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
    /// Resolve the model for a given pass, checking the provider subsection first,
    /// then falling back to the top-level flat fields.
    pub fn resolve_model(&self, pass: &str, provider: &str) -> &str {
        let provider_config = self.provider_config(provider);
        let override_val = match pass {
            "scan" => provider_config.scan_model.as_deref(),
            "triage" => provider_config.triage_model.as_deref(),
            "deep" => provider_config.deep_model.as_deref(),
            _ => None,
        };
        if let Some(val) = override_val.filter(|s| !s.is_empty()) {
            return val;
        }
        match pass {
            "scan" => self.scan_model.as_str(),
            "triage" => self.triage_model.as_str(),
            "deep" => self.deep_model.as_str(),
            _ => "",
        }
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
