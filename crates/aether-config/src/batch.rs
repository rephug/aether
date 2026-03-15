use serde::{Deserialize, Serialize};

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
        }
    }
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
