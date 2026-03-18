use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_embedding_pass_threshold")]
    pub embedding_pass_threshold: f64,
    #[serde(default = "default_embedding_fail_threshold")]
    pub embedding_fail_threshold: f64,
    #[serde(default)]
    pub judge_model: String,
    #[serde(default)]
    pub judge_provider: String,
    #[serde(default = "default_streak_threshold")]
    pub streak_threshold: u32,
}

impl Default for ContractsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            embedding_pass_threshold: default_embedding_pass_threshold(),
            embedding_fail_threshold: default_embedding_fail_threshold(),
            judge_model: String::new(),
            judge_provider: String::new(),
            streak_threshold: default_streak_threshold(),
        }
    }
}

fn default_embedding_pass_threshold() -> f64 {
    0.88
}

fn default_embedding_fail_threshold() -> f64 {
    0.50
}

fn default_streak_threshold() -> u32 {
    2
}
