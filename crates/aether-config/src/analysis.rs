use serde::{Deserialize, Serialize};

use crate::constants::{
    DEFAULT_DRIFT_ANALYSIS_WINDOW, DEFAULT_DRIFT_HUB_PERCENTILE, DEFAULT_DRIFT_THRESHOLD,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CouplingConfig {
    #[serde(default = "default_coupling_enabled")]
    pub enabled: bool,
    #[serde(default = "default_coupling_commit_window")]
    pub commit_window: u32,
    #[serde(default = "default_coupling_min_co_change_count")]
    pub min_co_change_count: u32,
    #[serde(default = "default_coupling_exclude_patterns")]
    pub exclude_patterns: Vec<String>,
    #[serde(default = "default_coupling_bulk_commit_threshold")]
    pub bulk_commit_threshold: u32,
    #[serde(default = "default_coupling_temporal_weight")]
    pub temporal_weight: f32,
    #[serde(default = "default_coupling_static_weight")]
    pub static_weight: f32,
    #[serde(default = "default_coupling_semantic_weight")]
    pub semantic_weight: f32,
}

impl Default for CouplingConfig {
    fn default() -> Self {
        Self {
            enabled: default_coupling_enabled(),
            commit_window: default_coupling_commit_window(),
            min_co_change_count: default_coupling_min_co_change_count(),
            exclude_patterns: default_coupling_exclude_patterns(),
            bulk_commit_threshold: default_coupling_bulk_commit_threshold(),
            temporal_weight: default_coupling_temporal_weight(),
            static_weight: default_coupling_static_weight(),
            semantic_weight: default_coupling_semantic_weight(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftConfig {
    #[serde(default = "default_drift_enabled")]
    pub enabled: bool,
    #[serde(default = "default_drift_threshold")]
    pub drift_threshold: f32,
    #[serde(default = "default_drift_analysis_window")]
    pub analysis_window: String,
    #[serde(default = "default_drift_auto_analyze")]
    pub auto_analyze: bool,
    #[serde(default = "default_drift_hub_percentile")]
    pub hub_percentile: u32,
}

impl Default for DriftConfig {
    fn default() -> Self {
        Self {
            enabled: default_drift_enabled(),
            drift_threshold: default_drift_threshold(),
            analysis_window: default_drift_analysis_window(),
            auto_analyze: default_drift_auto_analyze(),
            hub_percentile: default_drift_hub_percentile(),
        }
    }
}

pub(crate) fn default_coupling_enabled() -> bool {
    true
}

pub(crate) fn default_coupling_commit_window() -> u32 {
    500
}

pub(crate) fn default_coupling_min_co_change_count() -> u32 {
    3
}

pub(crate) fn default_coupling_exclude_patterns() -> Vec<String> {
    vec![
        "*.lock".to_owned(),
        "*.generated.*".to_owned(),
        ".gitignore".to_owned(),
    ]
}

pub(crate) fn default_coupling_bulk_commit_threshold() -> u32 {
    30
}

pub(crate) fn default_coupling_temporal_weight() -> f32 {
    0.5
}

pub(crate) fn default_coupling_static_weight() -> f32 {
    0.3
}

pub(crate) fn default_coupling_semantic_weight() -> f32 {
    0.2
}

fn default_drift_enabled() -> bool {
    true
}

pub(crate) fn default_drift_threshold() -> f32 {
    DEFAULT_DRIFT_THRESHOLD
}

pub(crate) fn default_drift_analysis_window() -> String {
    DEFAULT_DRIFT_ANALYSIS_WINDOW.to_owned()
}

fn default_drift_auto_analyze() -> bool {
    false
}

pub(crate) fn default_drift_hub_percentile() -> u32 {
    DEFAULT_DRIFT_HUB_PERCENTILE
}
