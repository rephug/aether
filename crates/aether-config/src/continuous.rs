use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContinuousConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_schedule")]
    pub schedule: String,
    #[serde(default = "default_half_life")]
    pub staleness_half_life_days: f64,
    #[serde(default = "default_sigmoid_k")]
    pub staleness_sigmoid_k: f64,
    #[serde(default = "default_neighbor_decay")]
    pub neighbor_decay: f64,
    #[serde(default = "default_neighbor_cutoff")]
    pub neighbor_cutoff: f64,
    #[serde(default = "default_coupling_threshold")]
    pub coupling_predict_threshold: f64,
    #[serde(default = "default_pr_alpha")]
    pub priority_pagerank_alpha: f64,
    #[serde(default = "default_max_requeue")]
    pub max_requeue_per_run: usize,
    #[serde(default)]
    pub auto_submit: bool,
    #[serde(default = "default_requeue_pass")]
    pub requeue_pass: String,
}

impl Default for ContinuousConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: default_schedule(),
            staleness_half_life_days: default_half_life(),
            staleness_sigmoid_k: default_sigmoid_k(),
            neighbor_decay: default_neighbor_decay(),
            neighbor_cutoff: default_neighbor_cutoff(),
            coupling_predict_threshold: default_coupling_threshold(),
            priority_pagerank_alpha: default_pr_alpha(),
            max_requeue_per_run: default_max_requeue(),
            auto_submit: false,
            requeue_pass: default_requeue_pass(),
        }
    }
}

fn default_schedule() -> String {
    "nightly".to_owned()
}

fn default_half_life() -> f64 {
    15.0
}

fn default_sigmoid_k() -> f64 {
    0.3
}

fn default_neighbor_decay() -> f64 {
    0.5
}

fn default_neighbor_cutoff() -> f64 {
    0.1
}

fn default_coupling_threshold() -> f64 {
    0.85
}

fn default_pr_alpha() -> f64 {
    0.2
}

fn default_max_requeue() -> usize {
    500
}

fn default_requeue_pass() -> String {
    "triage".to_owned()
}
