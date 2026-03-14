mod archetypes;
pub mod compare;
mod explanations;
pub mod git_signals;
pub mod history;
pub mod metrics;
pub mod models;
pub mod output;
pub mod planner;
pub mod planner_communities;
mod scanner;
pub mod scoring;
pub mod semantic_signals;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_core::{GitContext, normalize_path};

pub use aether_config::HealthScoreConfig;
pub use compare::{CompareReport, CrateDelta, MetricChangeKind, MetricDelta, compare_reports};
pub use models::{
    Archetype, CrateScore, GitSignals, HealthError, ScoreBreakdown, ScoreReport, SemanticSignals,
    Severity, SignalAvailability, Violation, WorkspaceViolation,
};
pub use output::{
    format_compare_json, format_compare_table, format_crate_explanation, format_hotspots_text,
    format_json, format_table,
};
pub use planner::{
    ConsumerMethodUsage, CrossCuttingMethod, SplitConfidence, SplitSuggestion, SuggestedModule,
    SuggestedSubTrait, TraitMethod, TraitSplitSuggestion, suggest_split, suggest_trait_split,
};
pub use planner_communities::{
    FileCommunityConfig, FileSymbol, PlannerDiagnostics, detect_file_communities,
};
pub use semantic_signals::{SemanticFileInput, SemanticInput};

use crate::archetypes::{assign_archetypes, assign_combined_archetypes};
use crate::explanations::explain_violation;
use crate::git_signals::{GitSignalAnalysis, analyze_crate_git_signals};
use crate::metrics::{
    count_feature_flags, count_internal_deps, count_loc, count_stale_refs, count_todo_markers,
    trait_method_max,
};
use crate::models::{CrateMetrics, MetricPenalties, ViolationLevel};
use crate::scanner::{WorkspaceCrate, scan_crate, scan_workspace};
use crate::scoring::{
    combined_score, compute_crate_penalty, compute_metric_penalties, compute_workspace_aggregate,
    normalize_to_100, raw_penalty,
};
use crate::semantic_signals::{SemanticSignalAnalysis, analyze_semantic_signals};

pub type Result<T> = std::result::Result<T, HealthError>;

const STRUCTURAL_SCHEMA_VERSION: u32 = 1;
const SEMANTIC_SCHEMA_VERSION: u32 = 2;
const LEGACY_FEATURE_PATTERN: &str = "feature = \"legacy-";
const TOP_VIOLATION_LIMIT: usize = 5;

pub fn workspace_health_config_or_default(path: &Path) -> HealthScoreConfig {
    aether_config::load_workspace_config(path)
        .map(|config| config.health_score)
        .unwrap_or_default()
}

pub fn compute_workspace_score(path: &Path, config: &HealthScoreConfig) -> Result<ScoreReport> {
    compute_workspace_score_filtered(path, config, &[])
}

pub fn compute_workspace_score_filtered(
    path: &Path,
    config: &HealthScoreConfig,
    crate_filter: &[String],
) -> Result<ScoreReport> {
    compute_workspace_score_internal(
        path,
        config,
        crate_filter,
        None,
        None,
        STRUCTURAL_SCHEMA_VERSION,
    )
}

pub fn compute_workspace_score_with_signals(
    path: &Path,
    config: &HealthScoreConfig,
    crate_filter: &[String],
    git: Option<&GitContext>,
    semantic_input: Option<&SemanticInput>,
) -> Result<ScoreReport> {
    compute_workspace_score_internal(
        path,
        config,
        crate_filter,
        git,
        semantic_input,
        SEMANTIC_SCHEMA_VERSION,
    )
}

fn compute_workspace_score_internal(
    path: &Path,
    config: &HealthScoreConfig,
    crate_filter: &[String],
    git: Option<&GitContext>,
    semantic_input: Option<&SemanticInput>,
    schema_version: u32,
) -> Result<ScoreReport> {
    let workspace_root = path.canonicalize().map_err(|err| {
        HealthError::Message(format!(
            "failed to resolve workspace path {}: {err}",
            path.display()
        ))
    })?;
    let scanned_crates = scan_workspace(&workspace_root)?;
    let selected_crates = filter_crates(scanned_crates, crate_filter)?;
    let mut crate_scores = Vec::new();
    for crate_info in &selected_crates {
        crate_scores.push(score_workspace_crate(
            crate_info,
            &workspace_root,
            config,
            git,
            semantic_input,
            schema_version >= SEMANTIC_SCHEMA_VERSION,
        )?);
    }
    crate_scores.sort_by(|left, right| {
        left.score
            .cmp(&right.score)
            .then_with(|| left.name.cmp(&right.name))
    });

    let workspace_score = compute_workspace_aggregate(&crate_scores);
    let total_loc = crate_scores.iter().map(|score| score.total_loc).sum();
    let worst_crate = crate_scores.first().map(|score| score.name.clone());
    let top_violations = collect_top_violations(&crate_scores);

    Ok(ScoreReport {
        schema_version,
        run_at: current_unix_time(),
        git_commit: detect_git_commit(&workspace_root),
        workspace_score,
        severity: Severity::from_score(workspace_score),
        previous_score: None,
        delta: None,
        crate_count: crate_scores.len(),
        total_loc,
        crates: crate_scores,
        worst_crate,
        top_violations,
        workspace_root,
    })
}

pub fn compute_crate_score(path: &Path, config: &HealthScoreConfig) -> Result<CrateScore> {
    let crate_info = scan_crate(path)?;
    let workspace_root = crate_info
        .root
        .parent()
        .and_then(Path::parent)
        .unwrap_or(crate_info.root.as_path())
        .to_path_buf();
    score_workspace_crate(&crate_info, &workspace_root, config, None, None, false)
}

fn score_workspace_crate(
    crate_info: &WorkspaceCrate,
    workspace_root: &Path,
    config: &HealthScoreConfig,
    git: Option<&GitContext>,
    semantic_input: Option<&SemanticInput>,
    extended_mode: bool,
) -> Result<CrateScore> {
    let metrics = collect_crate_metrics(crate_info, workspace_root, config)?;
    let penalties = compute_metric_penalties(&metrics, config);
    let structural_score = normalize_to_100(compute_crate_penalty(&metrics, config));
    let git_analysis = git
        .map(|context| analyze_crate_git_signals(crate_info, workspace_root, context, config))
        .transpose()?;
    let git_signals = git_analysis
        .as_ref()
        .map(|analysis| analysis.signals.clone());
    let semantic_analysis = semantic_input.map(|input| {
        analyze_semantic_signals(
            &filter_semantic_input(input, crate_info, workspace_root),
            config,
        )
    });
    let semantic_signals = semantic_analysis
        .as_ref()
        .map(|analysis| analysis.signals.clone());
    let score_data = if extended_mode {
        Some(combined_score(
            structural_score,
            git_signals.as_ref(),
            semantic_signals.as_ref(),
            config,
        ))
    } else {
        None
    };
    let score = score_data
        .as_ref()
        .map(|combined| combined.score)
        .unwrap_or(structural_score);
    let archetypes = if extended_mode {
        assign_combined_archetypes(
            &metrics,
            &penalties,
            git_signals.as_ref(),
            semantic_signals.as_ref(),
        )
    } else {
        assign_archetypes(&metrics, &penalties)
    };
    let availability = if extended_mode {
        build_signal_availability(git.is_some(), semantic_input.is_some())
    } else {
        SignalAvailability::default()
    };
    let violations = build_violations(
        crate_info,
        &metrics,
        &penalties,
        config,
        git_analysis.as_ref(),
        semantic_analysis.as_ref(),
        &archetypes,
    );

    Ok(CrateScore {
        name: crate_info.name.clone(),
        score,
        severity: Severity::from_score(score),
        archetypes,
        total_loc: metrics.total_loc,
        file_count: metrics.file_count,
        total_lines: metrics.total_lines,
        metrics: metrics.snapshot(),
        violations,
        git_signals,
        semantic_signals,
        signal_availability: availability,
        score_breakdown: score_data.map(|combined| combined.breakdown),
    })
}

fn collect_crate_metrics(
    crate_info: &WorkspaceCrate,
    workspace_root: &Path,
    config: &HealthScoreConfig,
) -> Result<CrateMetrics> {
    let mut metrics = CrateMetrics {
        file_count: crate_info.source_files.len(),
        internal_dep_count: count_internal_deps(&crate_info.cargo_toml),
        dead_feature_flags: count_feature_flags(&crate_info.cargo_toml_raw, LEGACY_FEATURE_PATTERN),
        ..CrateMetrics::default()
    };
    let mut todo_markers = 0usize;

    for source_file in &crate_info.source_files {
        let content = fs::read_to_string(source_file).map_err(|err| {
            HealthError::Message(format!("failed to read {}: {err}", source_file.display()))
        })?;
        let (loc, total_lines) = count_loc(&content);
        metrics.total_loc += loc;
        metrics.total_lines += total_lines;
        metrics.dead_feature_flags += count_feature_flags(&content, LEGACY_FEATURE_PATTERN);
        todo_markers += count_todo_markers(&content);

        let (trait_methods, trait_name) = trait_method_max(&content);
        if trait_methods > metrics.trait_method_max {
            metrics.trait_method_max = trait_methods;
            metrics.trait_name = trait_name;
        }

        if loc > metrics.max_file_loc {
            metrics.max_file_loc = loc;
            metrics.max_file_path = relative_path(workspace_root, source_file)
                .or_else(|| relative_path(&crate_info.root, source_file));
        }

        if !is_legacy_cozo_file(source_file) {
            metrics.stale_backend_refs += count_stale_refs(&content, &config.stale_ref_patterns);
        }
    }

    if metrics.total_loc > 0 {
        metrics.todo_density = (todo_markers as f32 * 1000.0) / metrics.total_loc as f32;
    }

    Ok(metrics)
}

fn build_violations(
    crate_info: &WorkspaceCrate,
    metrics: &CrateMetrics,
    penalties: &MetricPenalties,
    config: &HealthScoreConfig,
    git_analysis: Option<&GitSignalAnalysis>,
    semantic_analysis: Option<&SemanticSignalAnalysis>,
    archetypes: &[Archetype],
) -> Vec<Violation> {
    let mut violations = Vec::new();
    push_violation(
        &mut violations,
        "max_file_loc",
        metrics.max_file_loc as f64,
        penalties.max_file_loc,
        config.file_loc_warn as f64,
        config.file_loc_fail as f64,
        metrics
            .max_file_path
            .clone()
            .unwrap_or_else(|| format!("{}/src", crate_info.name)),
    );
    push_violation(
        &mut violations,
        "trait_method_max",
        metrics.trait_method_max as f64,
        penalties.trait_method_max,
        config.trait_method_warn as f64,
        config.trait_method_fail as f64,
        metrics
            .trait_name
            .clone()
            .unwrap_or_else(|| format!("{} trait", crate_info.name)),
    );
    push_violation(
        &mut violations,
        "internal_dep_count",
        metrics.internal_dep_count as f64,
        penalties.internal_dep_count,
        config.internal_dep_warn as f64,
        config.internal_dep_fail as f64,
        crate_info.name.clone(),
    );
    push_violation(
        &mut violations,
        "todo_density",
        metrics.todo_density as f64,
        penalties.todo_density,
        config.todo_density_warn as f64,
        config.todo_density_fail as f64,
        crate_info.name.clone(),
    );
    push_violation(
        &mut violations,
        "dead_feature_flags",
        metrics.dead_feature_flags as f64,
        penalties.dead_feature_flags,
        config.dead_feature_warn as f64,
        config.dead_feature_fail as f64,
        crate_info.name.clone(),
    );
    push_violation(
        &mut violations,
        "stale_backend_refs",
        metrics.stale_backend_refs as f64,
        penalties.stale_backend_refs,
        config.stale_ref_warn as f64,
        config.stale_ref_fail as f64,
        crate_info.name.clone(),
    );
    if let Some(analysis) = git_analysis {
        if let Some(hotspot) = analysis
            .files
            .iter()
            .max_by(|left, right| left.raw.commits_30d.cmp(&right.raw.commits_30d))
            && hotspot.raw.commits_30d > 0
        {
            push_violation(
                &mut violations,
                "git_churn_30d",
                hotspot.raw.commits_30d as f64,
                raw_penalty(
                    hotspot.raw.commits_30d as f64,
                    (config.churn_30d_high as f64 / 2.0).max(1.0),
                    config.churn_30d_high as f64,
                ) * 100.0,
                (config.churn_30d_high as f64 / 2.0).max(1.0),
                config.churn_30d_high as f64,
                hotspot.path.clone(),
            );
        }

        if let Some(hotspot) = analysis
            .files
            .iter()
            .max_by(|left, right| left.raw.author_count.cmp(&right.raw.author_count))
            && hotspot.raw.author_count > 1
        {
            push_violation(
                &mut violations,
                "git_author_count",
                hotspot.raw.author_count as f64,
                raw_penalty(
                    hotspot.raw.author_count as f64,
                    (config.author_count_high as f64 / 2.0).max(2.0),
                    config.author_count_high as f64,
                ) * 100.0,
                (config.author_count_high as f64 / 2.0).max(2.0),
                config.author_count_high as f64,
                hotspot.path.clone(),
            );
        }
    }

    if let Some(analysis) = semantic_analysis {
        if let Some(hotspot) = analysis
            .files
            .iter()
            .max_by(|left, right| left.community_count.cmp(&right.community_count))
            && hotspot.community_count > 1
        {
            push_violation(
                &mut violations,
                "boundary_leakage",
                hotspot.community_count as f64,
                raw_penalty(hotspot.community_count as f64, 2.0, 3.0) * 100.0,
                2.0,
                3.0,
                hotspot.path.clone(),
            );
        }

        if archetypes.contains(&Archetype::FalseStable) && analysis.total_symbols > 0 {
            let percentage =
                analysis.drifted_symbols as f64 * 100.0 / analysis.total_symbols as f64;
            violations.push(Violation {
                metric: "false_stable".to_owned(),
                value: percentage,
                threshold: 50.0,
                severity: ViolationLevel::Warn,
                reason: explain_violation("false_stable", percentage, 50.0, &crate_info.name),
            });
        }

        if archetypes.contains(&Archetype::ZombieFile) {
            let commits = git_analysis
                .zip(semantic_analysis)
                .and_then(|(git_analysis, semantic_analysis)| {
                    let candidate_path = semantic_analysis.files.first()?.path.as_str();
                    git_analysis
                        .files
                        .iter()
                        .find(|entry| entry.path == candidate_path)
                        .map(|entry| entry.raw.commits_30d)
                })
                .unwrap_or_default();

            violations.push(Violation {
                metric: "zombie_file".to_owned(),
                value: commits as f64,
                threshold: (config.churn_30d_high as f64 * 0.1).max(1.0),
                severity: ViolationLevel::Warn,
                reason: explain_violation("zombie_file", commits as f64, 0.0, &crate_info.name),
            });
        }
    }

    violations.sort_by(|left, right| {
        severity_sort_key(right.severity)
            .cmp(&severity_sort_key(left.severity))
            .then_with(|| {
                right
                    .value
                    .partial_cmp(&left.value)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    violations
}

fn push_violation(
    violations: &mut Vec<Violation>,
    metric: &str,
    value: f64,
    contribution: f64,
    warn: f64,
    fail: f64,
    context: String,
) {
    if contribution <= f64::EPSILON {
        return;
    }

    let severity = if value > fail {
        ViolationLevel::Fail
    } else {
        ViolationLevel::Warn
    };
    let threshold = if matches!(severity, ViolationLevel::Fail) {
        fail
    } else {
        warn
    };

    violations.push(Violation {
        metric: metric.to_owned(),
        value,
        threshold,
        severity,
        reason: explain_violation(metric, value, threshold, &context),
    });
}

fn collect_top_violations(crate_scores: &[CrateScore]) -> Vec<WorkspaceViolation> {
    let mut top_violations = Vec::new();
    for crate_score in crate_scores {
        for violation in &crate_score.violations {
            top_violations.push(WorkspaceViolation {
                crate_name: crate_score.name.clone(),
                violation: violation.clone(),
            });
        }
    }

    top_violations.sort_by(|left, right| {
        severity_sort_key(right.violation.severity)
            .cmp(&severity_sort_key(left.violation.severity))
            .then_with(|| {
                right
                    .violation
                    .value
                    .partial_cmp(&left.violation.value)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.crate_name.cmp(&right.crate_name))
    });
    top_violations.truncate(TOP_VIOLATION_LIMIT);
    top_violations
}

fn filter_semantic_input(
    semantic_input: &SemanticInput,
    crate_info: &WorkspaceCrate,
    workspace_root: &Path,
) -> SemanticInput {
    let mut files = HashMap::new();
    for source_file in &crate_info.source_files {
        let Ok(relative) = source_file.strip_prefix(workspace_root) else {
            continue;
        };
        let key = normalize_path(relative.to_string_lossy().as_ref());
        if let Some(entry) = semantic_input.files.get(&key) {
            files.insert(key, entry.clone());
        }
    }

    SemanticInput {
        workspace_max_pagerank: semantic_input.workspace_max_pagerank,
        files,
    }
}

fn build_signal_availability(git_available: bool, semantic_available: bool) -> SignalAvailability {
    let mut notes = Vec::new();
    if !git_available {
        notes.push("Git data unavailable - git signals skipped".to_owned());
    }
    if !semantic_available {
        notes.push(
            "Indexed workspace not found - semantic signals skipped. Run aetherd --index-once first."
                .to_owned(),
        );
    }

    SignalAvailability {
        git_available,
        semantic_available,
        notes,
    }
}

fn filter_crates(
    scanned_crates: Vec<WorkspaceCrate>,
    crate_filter: &[String],
) -> Result<Vec<WorkspaceCrate>> {
    if crate_filter.is_empty() {
        return Ok(scanned_crates);
    }

    let requested: HashSet<&str> = crate_filter.iter().map(String::as_str).collect();
    let matched: Vec<_> = scanned_crates
        .into_iter()
        .filter(|crate_info| requested.contains(crate_info.name.as_str()))
        .collect();

    let known_names: HashSet<_> = matched
        .iter()
        .map(|crate_info| crate_info.name.as_str())
        .collect();
    let mut unknown: Vec<_> = requested
        .into_iter()
        .filter(|name| !known_names.contains(name))
        .map(str::to_owned)
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        return Err(HealthError::Message(format!(
            "unknown crate filter(s): {}",
            unknown.join(", ")
        )));
    }

    Ok(matched)
}

fn relative_path(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.display().to_string())
}

fn is_legacy_cozo_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|file_name| file_name.contains("_cozo"))
}

fn detect_git_commit(workspace_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(workspace_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let commit = String::from_utf8(output.stdout).ok()?;
    let trimmed = commit.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn current_unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn severity_sort_key(level: ViolationLevel) -> u8 {
    match level {
        ViolationLevel::Warn => 0,
        ViolationLevel::Fail => 1,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use aether_config::HealthScoreConfig;
    use tempfile::tempdir;

    use crate::{compute_workspace_score, compute_workspace_score_filtered};

    fn write_workspace_file(path: &Path, content: &str) {
        fs::create_dir_all(
            path.parent()
                .expect("workspace test path should have parent directory"),
        )
        .expect("create parent");
        fs::write(path, content).expect("write file");
    }

    use std::path::Path;

    fn create_workspace(manifest_members: &str) -> tempfile::TempDir {
        let temp = tempdir().expect("tempdir");
        write_workspace_file(
            &temp.path().join("Cargo.toml"),
            &format!("[workspace]\nmembers = [{manifest_members}]\nresolver = \"2\"\n"),
        );
        temp
    }

    fn create_crate(root: &Path, relative_path: &str, name: &str, source: &str) {
        let crate_root = root.join(relative_path);
        write_workspace_file(
            &crate_root.join("Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        );
        write_workspace_file(&crate_root.join("src/lib.rs"), source);
    }

    #[test]
    fn score_100_for_clean_crate() {
        let workspace = create_workspace("\"crates/clean\"");
        create_crate(
            workspace.path(),
            "crates/clean",
            "clean",
            "pub fn alpha() {}\n",
        );

        let report = compute_workspace_score(workspace.path(), &HealthScoreConfig::default())
            .expect("workspace score");

        assert_eq!(report.workspace_score, 100);
        assert_eq!(report.crates[0].score, 100);
    }

    #[test]
    fn stale_ref_excludes_legacy_files() {
        let workspace = create_workspace("\"crates/legacy\"");
        let crate_root = workspace.path().join("crates/legacy");
        create_crate(
            workspace.path(),
            "crates/legacy",
            "legacy",
            "pub fn alpha() { let _ = \"ok\"; }\n",
        );
        write_workspace_file(
            &crate_root.join("src/graph_cozo.rs"),
            "pub fn legacy() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );

        let report = compute_workspace_score(workspace.path(), &HealthScoreConfig::default())
            .expect("workspace score");

        assert_eq!(report.crates[0].metrics.stale_backend_refs, 0);
    }

    #[test]
    fn filtered_workspace_errors_on_unknown_crate() {
        let workspace = create_workspace("\"crates/clean\"");
        create_crate(
            workspace.path(),
            "crates/clean",
            "clean",
            "pub fn alpha() {}\n",
        );

        let error = compute_workspace_score_filtered(
            workspace.path(),
            &HealthScoreConfig::default(),
            &[String::from("missing")],
        )
        .expect_err("expected unknown crate error");

        assert!(error.to_string().contains("missing"));
    }
}
