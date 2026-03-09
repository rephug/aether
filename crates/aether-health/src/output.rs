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
    lines.push(String::new());
    lines.push(format!(
        "{:<22} {:>5} {:>6} {:>6}  {}",
        "Crate", "Score", "LOC", "Files", "Archetype"
    ));
    lines.push("-".repeat(74));

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
        lines.push(format!(
            "{:<22} {:>5} {:>6} {:>6}  {}",
            crate_score.name,
            crate_score.score,
            crate_score.total_loc,
            crate_score.file_count,
            archetypes
        ));
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
