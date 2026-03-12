use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerConfig {
    #[serde(default = "default_planner_semantic_rescue_threshold")]
    pub semantic_rescue_threshold: f32,
    #[serde(default = "default_planner_semantic_rescue_max_k")]
    pub semantic_rescue_max_k: usize,
    #[serde(default = "default_planner_community_resolution")]
    pub community_resolution: f64,
    #[serde(default = "default_planner_min_community_size")]
    pub min_community_size: usize,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            semantic_rescue_threshold: default_planner_semantic_rescue_threshold(),
            semantic_rescue_max_k: default_planner_semantic_rescue_max_k(),
            community_resolution: default_planner_community_resolution(),
            min_community_size: default_planner_min_community_size(),
        }
    }
}

pub(crate) fn default_planner_semantic_rescue_threshold() -> f32 {
    0.70
}

pub(crate) fn default_planner_semantic_rescue_max_k() -> usize {
    3
}

pub(crate) fn default_planner_community_resolution() -> f64 {
    0.5
}

pub(crate) fn default_planner_min_community_size() -> usize {
    3
}
