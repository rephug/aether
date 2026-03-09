use std::collections::BTreeSet;

use crate::models::{ScoreReport, WorkspaceViolation};

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

fn format_workspace_violation(violation: &WorkspaceViolation) -> String {
    format!(
        "  [{}] {}: {} (threshold: {})",
        violation.violation.severity.as_tag(),
        violation.crate_name,
        violation.violation.reason,
        format_number(violation.violation.threshold)
    )
}

fn format_number(value: f64) -> String {
    if (value.fract()).abs() <= f64::EPSILON {
        format!("{}", value as i64)
    } else {
        format!("{value:.1}")
    }
}
