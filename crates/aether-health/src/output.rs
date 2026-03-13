use std::collections::BTreeSet;

use crate::compare::{CompareReport, MetricChangeKind};
use crate::models::{CrateScore, ScoreReport, WorkspaceViolation};
use crate::planner::SplitSuggestion;

pub fn format_table(report: &ScoreReport) -> String {
    let workspace_name = report
        .workspace_root
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_else(|| "WORKSPACE".to_owned());
    let git_commit = report.git_commit.as_deref().unwrap_or("-");
    let delta = match report.delta {
        Some(delta) => delta.to_string(),
        None => "-".to_owned(),
    };

    let mut lines = Vec::new();
    lines.push(format!(
        "{workspace_name} Health Score - {}",
        report.workspace_root.display()
    ));
    lines.push(format!(
        "Run: {} | Git: {git_commit} | Score: {}/100 ({}) | Delta: {delta}",
        report.run_at,
        report.workspace_score,
        report.severity.as_label()
    ));
    let extended_mode = report
        .crates
        .iter()
        .any(|crate_score| crate_score.score_breakdown.is_some());
    if extended_mode {
        lines.push("Mode: structural + git + semantic".to_owned());
    }
    lines.push(String::new());
    if extended_mode {
        lines.push(format!(
            "{:<22} {:>5} {:>6} {:>5} {:>8}  {}",
            "Crate", "Score", "Struct", "Git", "Semantic", "Archetype"
        ));
        lines.push("-".repeat(82));
    } else {
        lines.push(format!(
            "{:<22} {:>5} {:>6} {:>6}  {}",
            "Crate", "Score", "LOC", "Files", "Archetype"
        ));
        lines.push("-".repeat(74));
    }

    for crate_score in &report.crates {
        let archetypes = if crate_score.archetypes.is_empty() {
            "-".to_owned()
        } else {
            crate_score
                .archetypes
                .iter()
                .map(|archetype| archetype.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(breakdown) = &crate_score.score_breakdown {
            let git = breakdown
                .git
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_owned());
            let semantic = breakdown
                .semantic
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_owned());
            lines.push(format!(
                "{:<22} {:>5} {:>6} {:>5} {:>8}  {}",
                crate_score.name,
                crate_score.score,
                breakdown.structural,
                git,
                semantic,
                archetypes
            ));
        } else {
            lines.push(format!(
                "{:<22} {:>5} {:>6} {:>6}  {}",
                crate_score.name,
                crate_score.score,
                crate_score.total_loc,
                crate_score.file_count,
                archetypes
            ));
        }
    }

    if extended_mode {
        let notes = report
            .crates
            .iter()
            .flat_map(|crate_score| crate_score.signal_availability.notes.iter().cloned())
            .collect::<BTreeSet<_>>();
        if !notes.is_empty() {
            lines.push(String::new());
            lines.push("Notes:".to_owned());
            for note in notes {
                lines.push(format!("  - {note}"));
            }
        }
    }

    lines.push(String::new());
    lines.push("Top issues:".to_owned());
    if report.top_violations.is_empty() {
        lines.push("  none".to_owned());
    } else {
        for violation in &report.top_violations {
            lines.push(format_workspace_violation(violation));
        }
    }

    lines.join("\n")
}

pub fn format_json(report: &ScoreReport) -> String {
    match serde_json::to_string_pretty(report) {
        Ok(json) => json,
        Err(err) => format!("{{\"error\":\"failed to serialize health report: {err}\"}}"),
    }
}

pub fn format_compare_table(report: &CompareReport) -> String {
    let before_commit = report.before_git_commit.as_deref().unwrap_or("-");
    let after_commit = report.after_git_commit.as_deref().unwrap_or("-");
    let delta = format_signed_delta(report.delta);
    let mut lines = vec![
        "AETHER Health Score - Before/After Comparison".to_owned(),
        format!(
            "Before: {before_commit} @ {} - Score: {}/100",
            report.before_run_at, report.before_workspace_score
        ),
        format!(
            "After:  {after_commit} @ {} - Score: {}/100",
            report.after_run_at, report.after_workspace_score
        ),
        format!("Delta:  {delta}"),
        String::new(),
        format!(
            "{:<22} {:>6} {:>6} {:>7}",
            "Crate", "Before", "After", "Delta"
        ),
        "-".repeat(45),
    ];

    for crate_delta in &report.crate_deltas {
        lines.push(format!(
            "{:<22} {:>6} {:>6} {:>7}",
            crate_delta.name,
            format_optional_u32(crate_delta.before_score),
            format_optional_u32(crate_delta.after_score),
            format_signed_delta(crate_delta.delta),
        ));
    }

    lines.push(String::new());
    lines.push("Improvements:".to_owned());
    if report.improvements.is_empty() {
        lines.push("  none".to_owned());
    } else {
        for delta in &report.improvements {
            lines.push(format_metric_delta(delta));
        }
    }

    lines.push(String::new());
    lines.push("Regressions:".to_owned());
    if report.regressions.is_empty() {
        lines.push("  none".to_owned());
    } else {
        for delta in &report.regressions {
            lines.push(format_metric_delta(delta));
        }
    }

    lines.join("\n")
}

pub fn format_compare_json(report: &CompareReport) -> String {
    match serde_json::to_string_pretty(report) {
        Ok(json) => json,
        Err(err) => format!("{{\"error\":\"failed to serialize compare report: {err}\"}}"),
    }
}

pub fn format_hotspots_text(report: &ScoreReport, limit: usize, max_score: u32) -> String {
    let crates = report
        .crates
        .iter()
        .filter(|crate_score| crate_score.score <= max_score)
        .take(limit.max(1))
        .collect::<Vec<_>>();

    let mut lines = vec![format!(
        "Workspace Health: {}/100 ({})",
        report.workspace_score,
        report.severity.as_label()
    )];
    if crates.is_empty() {
        lines.push("No hotspot crates matched the current filter.".to_owned());
        return lines.join("\n");
    }

    for crate_score in crates {
        lines.push(String::new());
        lines.push(format!(
            "{} - {}/100 ({})",
            crate_score.name,
            crate_score.score,
            crate_score.severity.as_label()
        ));
        lines.push(format!(
            "Archetypes: {}",
            format_archetypes(crate_score).unwrap_or_else(|| "none".to_owned())
        ));
        lines.push(format!(
            "Top violation: {}",
            crate_score
                .violations
                .first()
                .map(|violation| violation.reason.clone())
                .unwrap_or_else(|| "No active violations".to_owned())
        ));
    }

    lines.join("\n")
}

pub fn format_crate_explanation(
    crate_score: &CrateScore,
    split_suggestion: Option<&SplitSuggestion>,
) -> String {
    let mut lines = vec![format!(
        "Health Score: {} - {}/100 ({})",
        crate_score.name,
        crate_score.score,
        crate_score.severity.as_label()
    )];

    lines.push(format!(
        "Archetypes: {}",
        format_archetypes(crate_score).unwrap_or_else(|| "none".to_owned())
    ));
    lines.push(String::new());
    lines.push("Structural metrics:".to_owned());
    lines.push(format!(
        "  max_file_loc: {}{}",
        crate_score.metrics.max_file_loc,
        crate_score
            .metrics
            .max_file_path
            .as_ref()
            .map(|path| format!(" ({path})"))
            .unwrap_or_default()
    ));
    lines.push(format!(
        "  trait_method_max: {}",
        crate_score.metrics.trait_method_max
    ));
    lines.push(format!(
        "  internal_dep_count: {}",
        crate_score.metrics.internal_dep_count
    ));
    lines.push(format!(
        "  todo_density: {:.2}",
        crate_score.metrics.todo_density
    ));
    lines.push(format!(
        "  dead_feature_flags: {}",
        crate_score.metrics.dead_feature_flags
    ));
    lines.push(format!(
        "  stale_backend_refs: {}",
        crate_score.metrics.stale_backend_refs
    ));

    if let Some(git) = &crate_score.git_signals {
        lines.push(String::new());
        lines.push("Git signals:".to_owned());
        lines.push(format!("  churn_30d: {:.2}", git.churn_30d));
        lines.push(format!("  churn_90d: {:.2}", git.churn_90d));
        lines.push(format!("  author_count: {:.2}", git.author_count));
        lines.push(format!("  blame_age_spread: {:.2}", git.blame_age_spread));
        lines.push(format!("  git_pressure: {:.2}", git.git_pressure));
    }

    if let Some(semantic) = &crate_score.semantic_signals {
        lines.push(String::new());
        lines.push("Semantic signals:".to_owned());
        lines.push(format!("  max_centrality: {:.2}", semantic.max_centrality));
        lines.push(format!("  drift_density: {:.2}", semantic.drift_density));
        lines.push(format!(
            "  stale_sir_ratio: {:.2}",
            semantic.stale_sir_ratio
        ));
        lines.push(format!("  test_gap: {:.2}", semantic.test_gap));
        lines.push(format!(
            "  boundary_leakage: {:.2}",
            semantic.boundary_leakage
        ));
        lines.push(format!(
            "  semantic_pressure: {:.2}",
            semantic.semantic_pressure
        ));
    }

    if let Some(breakdown) = &crate_score.score_breakdown {
        lines.push(String::new());
        lines.push("Score breakdown:".to_owned());
        lines.push(format!("  structural: {}", breakdown.structural));
        if let Some(git) = breakdown.git {
            lines.push(format!("  git: {}", git));
        }
        if let Some(semantic) = breakdown.semantic {
            lines.push(format!("  semantic: {}", semantic));
        }
    }

    lines.push(String::new());
    lines.push("Violations:".to_owned());
    if crate_score.violations.is_empty() {
        lines.push("  none".to_owned());
    } else {
        for violation in &crate_score.violations {
            lines.push(format!(
                "  [{}] {}",
                violation.severity.as_tag(),
                violation.reason
            ));
        }
    }

    if let Some(split) = split_suggestion {
        lines.push(String::new());
        lines.push("Split suggestion:".to_owned());
        lines.push(format!("  target_file: {}", split.target_file));
        lines.push(format!(
            "  confidence: {}",
            match split.confidence {
                crate::planner::SplitConfidence::High => "high",
                crate::planner::SplitConfidence::Medium => "medium",
                crate::planner::SplitConfidence::Low => "low",
            }
        ));
        lines.push(format!(
            "  expected impact: {}",
            split.expected_score_impact
        ));
        for module in &split.suggested_modules {
            lines.push(format!(
                "  - {}: {} ({})",
                module.name,
                module.symbols.join(", "),
                module.reason
            ));
        }
    }

    lines.join("\n")
}

fn format_workspace_violation(violation: &WorkspaceViolation) -> String {
    format!(
        "  [{}] {}: {} (threshold: {})",
        violation.violation.severity.as_tag(),
        violation.crate_name,
        violation.violation.reason,
        format_number(violation.violation.threshold)
    )
}

fn format_archetypes(crate_score: &CrateScore) -> Option<String> {
    if crate_score.archetypes.is_empty() {
        None
    } else {
        Some(
            crate_score
                .archetypes
                .iter()
                .map(|archetype| archetype.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

fn format_optional_u32(value: Option<u32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_owned())
}

fn format_signed_delta(delta: i32) -> String {
    match delta.cmp(&0) {
        std::cmp::Ordering::Greater => format!("+{delta}"),
        std::cmp::Ordering::Equal => "0".to_owned(),
        std::cmp::Ordering::Less => delta.to_string(),
    }
}

fn format_metric_delta(delta: &crate::compare::MetricDelta) -> String {
    let direction = match delta.kind {
        MetricChangeKind::Improvement => "improved",
        MetricChangeKind::Regression => "regressed",
        MetricChangeKind::Unchanged => "unchanged",
    };
    format!(
        "  {}: {} {} -> {} ({direction})",
        delta.crate_name,
        delta.metric,
        format_number(delta.before),
        format_number(delta.after),
    )
}

fn format_number(value: f64) -> String {
    if (value.fract()).abs() <= f64::EPSILON {
        format!("{}", value as i64)
    } else {
        format!("{value:.1}")
    }
}
