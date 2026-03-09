use aether_config::HealthScoreConfig;

use crate::models::{
    CrateMetrics, CrateScore, GitSignals, MetricPenalties, ScoreBreakdown, SemanticSignals,
};

const MAX_FILE_LOC_WEIGHT: f64 = 0.20;
const TRAIT_METHOD_WEIGHT: f64 = 0.20;
const INTERNAL_DEP_WEIGHT: f64 = 0.15;
const TODO_DENSITY_WEIGHT: f64 = 0.10;
const DEAD_FEATURE_WEIGHT: f64 = 0.15;
const STALE_REF_WEIGHT: f64 = 0.20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinedScore {
    pub score: u32,
    pub breakdown: ScoreBreakdown,
}

pub fn raw_penalty(value: f64, warn: f64, fail: f64) -> f64 {
    if !value.is_finite() || !warn.is_finite() || !fail.is_finite() || fail <= warn {
        return 0.0;
    }

    let penalty = if value <= warn {
        0.0
    } else if value <= fail {
        (value - warn) / (fail - warn)
    } else {
        1.0 + 0.5 * (value - fail) / fail
    };

    penalty.clamp(0.0, 2.0)
}

pub fn compute_crate_penalty(metrics: &CrateMetrics, config: &HealthScoreConfig) -> f64 {
    compute_metric_penalties(metrics, config).total()
}

pub(crate) fn compute_metric_penalties(
    metrics: &CrateMetrics,
    config: &HealthScoreConfig,
) -> MetricPenalties {
    MetricPenalties {
        max_file_loc: weighted_penalty(
            metrics.max_file_loc as f64,
            config.file_loc_warn as f64,
            config.file_loc_fail as f64,
            MAX_FILE_LOC_WEIGHT,
        ),
        trait_method_max: weighted_penalty(
            metrics.trait_method_max as f64,
            config.trait_method_warn as f64,
            config.trait_method_fail as f64,
            TRAIT_METHOD_WEIGHT,
        ),
        internal_dep_count: weighted_penalty(
            metrics.internal_dep_count as f64,
            config.internal_dep_warn as f64,
            config.internal_dep_fail as f64,
            INTERNAL_DEP_WEIGHT,
        ),
        todo_density: weighted_penalty(
            metrics.todo_density as f64,
            config.todo_density_warn as f64,
            config.todo_density_fail as f64,
            TODO_DENSITY_WEIGHT,
        ),
        dead_feature_flags: weighted_penalty(
            metrics.dead_feature_flags as f64,
            config.dead_feature_warn as f64,
            config.dead_feature_fail as f64,
            DEAD_FEATURE_WEIGHT,
        ),
        stale_backend_refs: weighted_penalty(
            metrics.stale_backend_refs as f64,
            config.stale_ref_warn as f64,
            config.stale_ref_fail as f64,
            STALE_REF_WEIGHT,
        ),
    }
}

pub fn normalize_to_100(penalty: f64) -> u32 {
    if !penalty.is_finite() {
        return 0;
    }

    penalty.round().clamp(0.0, 100.0) as u32
}

pub fn combined_score(
    structural_score: u32,
    git: Option<&GitSignals>,
    semantic: Option<&SemanticSignals>,
    config: &HealthScoreConfig,
) -> CombinedScore {
    let structural_bucket = structural_score;
    let git_bucket = git.map(|signals| normalize_to_100(signals.git_pressure * 100.0));
    let semantic_bucket =
        semantic.map(|signals| normalize_to_100(signals.semantic_pressure * 100.0));

    let structural_weight = config
        .structural_weight
        .unwrap_or(aether_config::DEFAULT_HEALTH_SCORE_STRUCTURAL_WEIGHT);
    let git_weight = config
        .git_weight
        .unwrap_or(aether_config::DEFAULT_HEALTH_SCORE_GIT_WEIGHT);
    let semantic_weight = config
        .semantic_weight
        .unwrap_or(aether_config::DEFAULT_HEALTH_SCORE_SEMANTIC_WEIGHT);

    let mut weighted_sum = structural_bucket as f64 * structural_weight;
    let mut available_weight = structural_weight;

    if let Some(bucket) = git_bucket {
        weighted_sum += bucket as f64 * git_weight;
        available_weight += git_weight;
    }

    if let Some(bucket) = semantic_bucket {
        weighted_sum += bucket as f64 * semantic_weight;
        available_weight += semantic_weight;
    }

    let score = if available_weight <= f64::EPSILON {
        structural_bucket
    } else {
        normalize_to_100(weighted_sum / available_weight)
    };

    CombinedScore {
        score,
        breakdown: ScoreBreakdown {
            structural: structural_bucket,
            git: git_bucket,
            semantic: semantic_bucket,
        },
    }
}

pub fn compute_workspace_aggregate(crate_scores: &[CrateScore]) -> u32 {
    let total_loc: usize = crate_scores.iter().map(|score| score.total_loc).sum();
    if total_loc == 0 {
        return 0;
    }

    let weighted_score = crate_scores.iter().fold(0.0, |acc, crate_score| {
        let weight = crate_score.total_loc as f64 / total_loc as f64;
        acc + crate_score.score as f64 * weight
    });
    normalize_to_100(weighted_score)
}

fn weighted_penalty(value: f64, warn: f64, fail: f64, weight: f64) -> f64 {
    raw_penalty(value, warn, fail) * weight * 100.0
}

#[cfg(test)]
mod tests {
    use crate::models::{CrateMetricsSnapshot, GitSignals, SemanticSignals, Severity};
    use crate::{Archetype, CrateScore};

    use super::{combined_score, compute_workspace_aggregate, normalize_to_100, raw_penalty};

    #[test]
    fn penalty_function_boundary_values() {
        assert_eq!(raw_penalty(10.0, 10.0, 20.0), 0.0);
        assert_eq!(raw_penalty(20.0, 10.0, 20.0), 1.0);
        assert!(raw_penalty(25.0, 10.0, 20.0) > 1.0);
        assert_eq!(raw_penalty(200.0, 10.0, 20.0), 2.0);
    }

    #[test]
    fn score_clamped_to_100() {
        assert_eq!(normalize_to_100(150.0), 100);
    }

    #[test]
    fn workspace_score_is_loc_weighted() {
        let small_bad = CrateScore {
            name: "small-bad".to_owned(),
            score: 100,
            severity: Severity::Critical,
            archetypes: vec![Archetype::GodFile],
            total_loc: 10,
            file_count: 1,
            total_lines: 10,
            metrics: CrateMetricsSnapshot {
                max_file_loc: 10,
                trait_method_max: 0,
                internal_dep_count: 0,
                todo_density: 0.0,
                dead_feature_flags: 0,
                stale_backend_refs: 0,
            },
            violations: Vec::new(),
            git_signals: None,
            semantic_signals: None,
            signal_availability: Default::default(),
            score_breakdown: None,
        };
        let large_good = CrateScore {
            name: "large-good".to_owned(),
            score: 10,
            severity: Severity::Healthy,
            archetypes: Vec::new(),
            total_loc: 1000,
            file_count: 10,
            total_lines: 1000,
            metrics: CrateMetricsSnapshot {
                max_file_loc: 100,
                trait_method_max: 0,
                internal_dep_count: 0,
                todo_density: 0.0,
                dead_feature_flags: 0,
                stale_backend_refs: 0,
            },
            violations: Vec::new(),
            git_signals: None,
            semantic_signals: None,
            signal_availability: Default::default(),
            score_breakdown: None,
        };

        let score = compute_workspace_aggregate(&[small_bad, large_good]);
        assert!(score <= 11);
    }

    #[test]
    fn combined_score_reweights_available_buckets() {
        let config = aether_config::HealthScoreConfig::default();
        let combined = combined_score(
            60,
            Some(&GitSignals {
                git_pressure: 0.8,
                ..GitSignals::default()
            }),
            None,
            &config,
        );

        assert_eq!(combined.breakdown.structural, 60);
        assert_eq!(combined.breakdown.git, Some(80));
        assert_eq!(combined.score, 68);
    }

    #[test]
    fn combined_score_with_all_buckets_uses_default_weights() {
        let config = aether_config::HealthScoreConfig::default();
        let combined = combined_score(
            50,
            Some(&GitSignals {
                git_pressure: 0.4,
                ..GitSignals::default()
            }),
            Some(&SemanticSignals {
                semantic_pressure: 0.8,
                ..SemanticSignals::default()
            }),
            &config,
        );

        assert_eq!(combined.breakdown.structural, 50);
        assert_eq!(combined.breakdown.git, Some(40));
        assert_eq!(combined.breakdown.semantic, Some(80));
        assert_eq!(combined.score, 58);
    }
}
