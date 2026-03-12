use serde::{Deserialize, Serialize};

use crate::constants::{
    DEFAULT_HEALTH_DRIFT_WEIGHT, DEFAULT_HEALTH_NO_SIR_WEIGHT, DEFAULT_HEALTH_PAGERANK_WEIGHT,
    DEFAULT_HEALTH_RECENCY_WEIGHT, DEFAULT_HEALTH_SCORE_AUTHOR_COUNT_HIGH,
    DEFAULT_HEALTH_SCORE_BLAME_AGE_SPREAD_HIGH_SECS, DEFAULT_HEALTH_SCORE_BOUNDARY_LEAKAGE_HIGH,
    DEFAULT_HEALTH_SCORE_CHURN_30D_HIGH, DEFAULT_HEALTH_SCORE_CHURN_90D_HIGH,
    DEFAULT_HEALTH_SCORE_DEAD_FEATURE_FAIL, DEFAULT_HEALTH_SCORE_DEAD_FEATURE_WARN,
    DEFAULT_HEALTH_SCORE_DRIFT_DENSITY_HIGH, DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL,
    DEFAULT_HEALTH_SCORE_FILE_LOC_WARN, DEFAULT_HEALTH_SCORE_INTERNAL_DEP_FAIL,
    DEFAULT_HEALTH_SCORE_INTERNAL_DEP_WARN, DEFAULT_HEALTH_SCORE_STALE_REF_FAIL,
    DEFAULT_HEALTH_SCORE_STALE_REF_WARN, DEFAULT_HEALTH_SCORE_STALE_SIR_HIGH,
    DEFAULT_HEALTH_SCORE_TEST_GAP_HIGH, DEFAULT_HEALTH_SCORE_TODO_DENSITY_FAIL,
    DEFAULT_HEALTH_SCORE_TODO_DENSITY_WARN, DEFAULT_HEALTH_SCORE_TRAIT_METHOD_FAIL,
    DEFAULT_HEALTH_SCORE_TRAIT_METHOD_WARN, DEFAULT_HEALTH_TEST_GAP_WEIGHT,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub risk_weights: RiskWeights,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: default_health_enabled(),
            risk_weights: RiskWeights::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskWeights {
    #[serde(default = "default_health_pagerank_weight")]
    pub pagerank: f64,
    #[serde(default = "default_health_test_gap_weight")]
    pub test_gap: f64,
    #[serde(default = "default_health_drift_weight")]
    pub drift: f64,
    #[serde(default = "default_health_no_sir_weight")]
    pub no_sir: f64,
    #[serde(default = "default_health_recency_weight")]
    pub recency: f64,
}

impl Default for RiskWeights {
    fn default() -> Self {
        Self {
            pagerank: default_health_pagerank_weight(),
            test_gap: default_health_test_gap_weight(),
            drift: default_health_drift_weight(),
            no_sir: default_health_no_sir_weight(),
            recency: default_health_recency_weight(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthScoreConfig {
    #[serde(default = "default_health_score_file_loc_warn")]
    pub file_loc_warn: usize,
    #[serde(default = "default_health_score_file_loc_fail")]
    pub file_loc_fail: usize,
    #[serde(default = "default_health_score_trait_method_warn")]
    pub trait_method_warn: usize,
    #[serde(default = "default_health_score_trait_method_fail")]
    pub trait_method_fail: usize,
    #[serde(default = "default_health_score_internal_dep_warn")]
    pub internal_dep_warn: usize,
    #[serde(default = "default_health_score_internal_dep_fail")]
    pub internal_dep_fail: usize,
    #[serde(default = "default_health_score_todo_density_warn")]
    pub todo_density_warn: f32,
    #[serde(default = "default_health_score_todo_density_fail")]
    pub todo_density_fail: f32,
    #[serde(default = "default_health_score_dead_feature_warn")]
    pub dead_feature_warn: usize,
    #[serde(default = "default_health_score_dead_feature_fail")]
    pub dead_feature_fail: usize,
    #[serde(default = "default_health_score_stale_ref_warn")]
    pub stale_ref_warn: usize,
    #[serde(default = "default_health_score_stale_ref_fail")]
    pub stale_ref_fail: usize,
    #[serde(default = "default_health_score_stale_ref_patterns")]
    pub stale_ref_patterns: Vec<String>,
    #[serde(default = "default_health_score_churn_30d_high")]
    pub churn_30d_high: usize,
    #[serde(default = "default_health_score_churn_90d_high")]
    pub churn_90d_high: usize,
    #[serde(default = "default_health_score_author_count_high")]
    pub author_count_high: usize,
    #[serde(default = "default_health_score_blame_age_spread_high_secs")]
    pub blame_age_spread_high_secs: u64,
    #[serde(default = "default_health_score_drift_density_high")]
    pub drift_density_high: f32,
    #[serde(default = "default_health_score_stale_sir_high")]
    pub stale_sir_high: f32,
    #[serde(default = "default_health_score_test_gap_high")]
    pub test_gap_high: f32,
    #[serde(default = "default_health_score_boundary_leakage_high")]
    pub boundary_leakage_high: f32,
    #[serde(default)]
    pub structural_weight: Option<f64>,
    #[serde(default)]
    pub git_weight: Option<f64>,
    #[serde(default)]
    pub semantic_weight: Option<f64>,
}

impl Default for HealthScoreConfig {
    fn default() -> Self {
        Self {
            file_loc_warn: default_health_score_file_loc_warn(),
            file_loc_fail: default_health_score_file_loc_fail(),
            trait_method_warn: default_health_score_trait_method_warn(),
            trait_method_fail: default_health_score_trait_method_fail(),
            internal_dep_warn: default_health_score_internal_dep_warn(),
            internal_dep_fail: default_health_score_internal_dep_fail(),
            todo_density_warn: default_health_score_todo_density_warn(),
            todo_density_fail: default_health_score_todo_density_fail(),
            dead_feature_warn: default_health_score_dead_feature_warn(),
            dead_feature_fail: default_health_score_dead_feature_fail(),
            stale_ref_warn: default_health_score_stale_ref_warn(),
            stale_ref_fail: default_health_score_stale_ref_fail(),
            stale_ref_patterns: default_health_score_stale_ref_patterns(),
            churn_30d_high: default_health_score_churn_30d_high(),
            churn_90d_high: default_health_score_churn_90d_high(),
            author_count_high: default_health_score_author_count_high(),
            blame_age_spread_high_secs: default_health_score_blame_age_spread_high_secs(),
            drift_density_high: default_health_score_drift_density_high(),
            stale_sir_high: default_health_score_stale_sir_high(),
            test_gap_high: default_health_score_test_gap_high(),
            boundary_leakage_high: default_health_score_boundary_leakage_high(),
            structural_weight: None,
            git_weight: None,
            semantic_weight: None,
        }
    }
}

fn default_health_enabled() -> bool {
    true
}

pub(crate) fn default_health_pagerank_weight() -> f64 {
    DEFAULT_HEALTH_PAGERANK_WEIGHT
}

pub(crate) fn default_health_test_gap_weight() -> f64 {
    DEFAULT_HEALTH_TEST_GAP_WEIGHT
}

pub(crate) fn default_health_drift_weight() -> f64 {
    DEFAULT_HEALTH_DRIFT_WEIGHT
}

pub(crate) fn default_health_no_sir_weight() -> f64 {
    DEFAULT_HEALTH_NO_SIR_WEIGHT
}

pub(crate) fn default_health_recency_weight() -> f64 {
    DEFAULT_HEALTH_RECENCY_WEIGHT
}

pub(crate) fn default_health_score_file_loc_warn() -> usize {
    DEFAULT_HEALTH_SCORE_FILE_LOC_WARN
}

pub(crate) fn default_health_score_file_loc_fail() -> usize {
    DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL
}

pub(crate) fn default_health_score_trait_method_warn() -> usize {
    DEFAULT_HEALTH_SCORE_TRAIT_METHOD_WARN
}

pub(crate) fn default_health_score_trait_method_fail() -> usize {
    DEFAULT_HEALTH_SCORE_TRAIT_METHOD_FAIL
}

pub(crate) fn default_health_score_internal_dep_warn() -> usize {
    DEFAULT_HEALTH_SCORE_INTERNAL_DEP_WARN
}

pub(crate) fn default_health_score_internal_dep_fail() -> usize {
    DEFAULT_HEALTH_SCORE_INTERNAL_DEP_FAIL
}

pub(crate) fn default_health_score_todo_density_warn() -> f32 {
    DEFAULT_HEALTH_SCORE_TODO_DENSITY_WARN
}

pub(crate) fn default_health_score_todo_density_fail() -> f32 {
    DEFAULT_HEALTH_SCORE_TODO_DENSITY_FAIL
}

pub(crate) fn default_health_score_dead_feature_warn() -> usize {
    DEFAULT_HEALTH_SCORE_DEAD_FEATURE_WARN
}

pub(crate) fn default_health_score_dead_feature_fail() -> usize {
    DEFAULT_HEALTH_SCORE_DEAD_FEATURE_FAIL
}

pub(crate) fn default_health_score_stale_ref_warn() -> usize {
    DEFAULT_HEALTH_SCORE_STALE_REF_WARN
}

pub(crate) fn default_health_score_stale_ref_fail() -> usize {
    DEFAULT_HEALTH_SCORE_STALE_REF_FAIL
}

pub(crate) fn default_health_score_stale_ref_patterns() -> Vec<String> {
    vec![
        "CozoGraphStore".to_owned(),
        "cozo".to_owned(),
        "CozoDB".to_owned(),
    ]
}

pub(crate) fn default_health_score_churn_30d_high() -> usize {
    DEFAULT_HEALTH_SCORE_CHURN_30D_HIGH
}

pub(crate) fn default_health_score_churn_90d_high() -> usize {
    DEFAULT_HEALTH_SCORE_CHURN_90D_HIGH
}

pub(crate) fn default_health_score_author_count_high() -> usize {
    DEFAULT_HEALTH_SCORE_AUTHOR_COUNT_HIGH
}

pub(crate) fn default_health_score_blame_age_spread_high_secs() -> u64 {
    DEFAULT_HEALTH_SCORE_BLAME_AGE_SPREAD_HIGH_SECS
}

pub(crate) fn default_health_score_drift_density_high() -> f32 {
    DEFAULT_HEALTH_SCORE_DRIFT_DENSITY_HIGH
}

pub(crate) fn default_health_score_stale_sir_high() -> f32 {
    DEFAULT_HEALTH_SCORE_STALE_SIR_HIGH
}

pub(crate) fn default_health_score_test_gap_high() -> f32 {
    DEFAULT_HEALTH_SCORE_TEST_GAP_HIGH
}

pub(crate) fn default_health_score_boundary_leakage_high() -> f32 {
    DEFAULT_HEALTH_SCORE_BOUNDARY_LEAKAGE_HIGH
}
