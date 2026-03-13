use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::models::{CrateMetricsSnapshot, ScoreReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricChangeKind {
    Improvement,
    Regression,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDelta {
    pub crate_name: String,
    pub metric: String,
    pub before: f64,
    pub after: f64,
    pub delta: f64,
    pub kind: MetricChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateDelta {
    pub name: String,
    pub before_score: Option<u32>,
    pub after_score: Option<u32>,
    pub delta: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareReport {
    pub before_run_at: u64,
    pub before_git_commit: Option<String>,
    pub before_workspace_score: u32,
    pub after_run_at: u64,
    pub after_git_commit: Option<String>,
    pub after_workspace_score: u32,
    pub delta: i32,
    pub crate_deltas: Vec<CrateDelta>,
    pub improvements: Vec<MetricDelta>,
    pub regressions: Vec<MetricDelta>,
}

pub fn compare_reports(before: &ScoreReport, after: &ScoreReport) -> CompareReport {
    let before_by_name = before
        .crates
        .iter()
        .map(|crate_score| (crate_score.name.as_str(), crate_score))
        .collect::<BTreeMap<_, _>>();
    let after_by_name = after
        .crates
        .iter()
        .map(|crate_score| (crate_score.name.as_str(), crate_score))
        .collect::<BTreeMap<_, _>>();

    let mut crate_names = before_by_name
        .keys()
        .chain(after_by_name.keys())
        .copied()
        .collect::<Vec<_>>();
    crate_names.sort();
    crate_names.dedup();

    let mut crate_deltas = Vec::new();
    let mut improvements = Vec::new();
    let mut regressions = Vec::new();

    for crate_name in crate_names {
        let before_crate = before_by_name.get(crate_name).copied();
        let after_crate = after_by_name.get(crate_name).copied();
        let before_score = before_crate.map(|crate_score| crate_score.score);
        let after_score = after_crate.map(|crate_score| crate_score.score);
        crate_deltas.push(CrateDelta {
            name: crate_name.to_owned(),
            before_score,
            after_score,
            delta: after_score.unwrap_or(0) as i32 - before_score.unwrap_or(0) as i32,
        });

        let before_metrics = before_crate.map(|crate_score| &crate_score.metrics);
        let after_metrics = after_crate.map(|crate_score| &crate_score.metrics);
        collect_metric_deltas(
            crate_name,
            before_metrics,
            after_metrics,
            &mut improvements,
            &mut regressions,
        );
    }

    crate_deltas.sort_by(|left, right| {
        let left_worst = left
            .before_score
            .unwrap_or(100)
            .min(left.after_score.unwrap_or(100));
        let right_worst = right
            .before_score
            .unwrap_or(100)
            .min(right.after_score.unwrap_or(100));
        left_worst
            .cmp(&right_worst)
            .then_with(|| left.name.cmp(&right.name))
    });
    improvements.sort_by(metric_delta_sort);
    regressions.sort_by(metric_delta_sort);

    CompareReport {
        before_run_at: before.run_at,
        before_git_commit: before.git_commit.clone(),
        before_workspace_score: before.workspace_score,
        after_run_at: after.run_at,
        after_git_commit: after.git_commit.clone(),
        after_workspace_score: after.workspace_score,
        delta: after.workspace_score as i32 - before.workspace_score as i32,
        crate_deltas,
        improvements,
        regressions,
    }
}

fn collect_metric_deltas(
    crate_name: &str,
    before: Option<&CrateMetricsSnapshot>,
    after: Option<&CrateMetricsSnapshot>,
    improvements: &mut Vec<MetricDelta>,
    regressions: &mut Vec<MetricDelta>,
) {
    push_metric_delta(
        crate_name,
        "max_file_loc",
        before
            .map(|metrics| metrics.max_file_loc as f64)
            .unwrap_or(0.0),
        after
            .map(|metrics| metrics.max_file_loc as f64)
            .unwrap_or(0.0),
        improvements,
        regressions,
    );
    push_metric_delta(
        crate_name,
        "trait_method_max",
        before
            .map(|metrics| metrics.trait_method_max as f64)
            .unwrap_or(0.0),
        after
            .map(|metrics| metrics.trait_method_max as f64)
            .unwrap_or(0.0),
        improvements,
        regressions,
    );
    push_metric_delta(
        crate_name,
        "internal_dep_count",
        before
            .map(|metrics| metrics.internal_dep_count as f64)
            .unwrap_or(0.0),
        after
            .map(|metrics| metrics.internal_dep_count as f64)
            .unwrap_or(0.0),
        improvements,
        regressions,
    );
    push_metric_delta(
        crate_name,
        "todo_density",
        before
            .map(|metrics| metrics.todo_density as f64)
            .unwrap_or(0.0),
        after
            .map(|metrics| metrics.todo_density as f64)
            .unwrap_or(0.0),
        improvements,
        regressions,
    );
    push_metric_delta(
        crate_name,
        "dead_feature_flags",
        before
            .map(|metrics| metrics.dead_feature_flags as f64)
            .unwrap_or(0.0),
        after
            .map(|metrics| metrics.dead_feature_flags as f64)
            .unwrap_or(0.0),
        improvements,
        regressions,
    );
    push_metric_delta(
        crate_name,
        "stale_backend_refs",
        before
            .map(|metrics| metrics.stale_backend_refs as f64)
            .unwrap_or(0.0),
        after
            .map(|metrics| metrics.stale_backend_refs as f64)
            .unwrap_or(0.0),
        improvements,
        regressions,
    );
}

fn push_metric_delta(
    crate_name: &str,
    metric: &str,
    before: f64,
    after: f64,
    improvements: &mut Vec<MetricDelta>,
    regressions: &mut Vec<MetricDelta>,
) {
    let delta = after - before;
    if delta.abs() <= f64::EPSILON {
        return;
    }

    let kind = if delta < 0.0 {
        MetricChangeKind::Improvement
    } else {
        MetricChangeKind::Regression
    };
    let entry = MetricDelta {
        crate_name: crate_name.to_owned(),
        metric: metric.to_owned(),
        before,
        after,
        delta,
        kind,
    };
    match kind {
        MetricChangeKind::Improvement => improvements.push(entry),
        MetricChangeKind::Regression => regressions.push(entry),
        MetricChangeKind::Unchanged => {}
    }
}

fn metric_delta_sort(left: &MetricDelta, right: &MetricDelta) -> std::cmp::Ordering {
    right
        .delta
        .abs()
        .partial_cmp(&left.delta.abs())
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| left.crate_name.cmp(&right.crate_name))
        .then_with(|| left.metric.cmp(&right.metric))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::models::{
        CrateMetricsSnapshot, CrateScore, Severity, SignalAvailability, WorkspaceViolation,
    };
    use crate::{Archetype, Violation, ViolationLevel};

    use super::{MetricChangeKind, compare_reports};

    fn report(
        score: u32,
        crate_score: u32,
        max_file_loc: usize,
        stale_backend_refs: usize,
    ) -> crate::ScoreReport {
        crate::ScoreReport {
            schema_version: 1,
            run_at: 1_700_000_000,
            git_commit: Some("abc123".to_owned()),
            workspace_score: score,
            severity: Severity::from_score(score),
            previous_score: None,
            delta: None,
            crate_count: 1,
            total_loc: 200,
            crates: vec![CrateScore {
                name: "example".to_owned(),
                score: crate_score,
                severity: Severity::from_score(crate_score),
                archetypes: vec![Archetype::GodFile],
                total_loc: 200,
                file_count: 1,
                total_lines: 240,
                metrics: CrateMetricsSnapshot {
                    max_file_loc,
                    max_file_path: Some("crates/example/src/lib.rs".to_owned()),
                    trait_method_max: 12,
                    internal_dep_count: 2,
                    todo_density: 0.5,
                    dead_feature_flags: 1,
                    stale_backend_refs,
                },
                violations: vec![Violation {
                    metric: "max_file_loc".to_owned(),
                    value: max_file_loc as f64,
                    threshold: 50.0,
                    severity: ViolationLevel::Warn,
                    reason: "too big".to_owned(),
                }],
                git_signals: None,
                semantic_signals: None,
                signal_availability: SignalAvailability::default(),
                score_breakdown: None,
            }],
            worst_crate: Some("example".to_owned()),
            top_violations: vec![WorkspaceViolation {
                crate_name: "example".to_owned(),
                violation: Violation {
                    metric: "max_file_loc".to_owned(),
                    value: max_file_loc as f64,
                    threshold: 50.0,
                    severity: ViolationLevel::Warn,
                    reason: "too big".to_owned(),
                },
            }],
            workspace_root: PathBuf::from("/tmp/workspace"),
        }
    }

    #[test]
    fn compare_report_computes_delta() {
        let before = report(40, 30, 300, 8);
        let after = report(50, 36, 220, 3);

        let compare = compare_reports(&before, &after);
        assert_eq!(compare.delta, 10);
        assert_eq!(compare.crate_deltas[0].delta, 6);
    }

    #[test]
    fn compare_identifies_improvements() {
        let before = report(40, 30, 300, 8);
        let after = report(50, 36, 220, 3);

        let compare = compare_reports(&before, &after);
        assert!(compare.improvements.iter().any(|delta| {
            delta.metric == "max_file_loc" && delta.kind == MetricChangeKind::Improvement
        }));
        assert!(compare.improvements.iter().any(|delta| {
            delta.metric == "stale_backend_refs" && delta.kind == MetricChangeKind::Improvement
        }));
        assert!(compare.regressions.is_empty());
    }
}
