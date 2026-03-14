use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tracing::warn;

use aether_analysis::HealthAnalyzer;
use aether_core::{GitContext, SIR_STATUS_STALE, normalize_path};
use aether_health::history::read_recent_reports;
use aether_health::{
    ScoreReport, SemanticFileInput, SemanticInput, Severity, compute_workspace_score,
    compute_workspace_score_with_signals,
};
use aether_store::{DriftStore, SirStateStore, SqliteStore, SymbolCatalogStore, TestIntentStore};

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 200;
const DEFAULT_MAX_SCORE: u32 = 75;
const HEALTH_SCORE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct HealthScoreQuery {
    pub limit: Option<usize>,
    pub max_score: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HealthScoreCrateRow {
    pub name: String,
    pub score: u32,
    pub severity: Severity,
    pub archetypes: Vec<String>,
    pub top_violation: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HealthScoreData {
    pub workspace_score: u32,
    pub severity: Severity,
    pub delta: i32,
    pub crates: Vec<HealthScoreCrateRow>,
    pub archetype_distribution: BTreeMap<String, usize>,
    pub trend: Vec<u32>,
}

pub(crate) async fn health_score_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<HealthScoreQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    let query = query.clone();
    match support::run_async_with_timeout(move || {
        let shared = shared.clone();
        let query = query.clone();
        async move { load_health_score_data(shared.as_ref(), &query).await }
    })
    .await
    {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

pub(crate) async fn load_health_score_data(
    shared: &SharedState,
    query: &HealthScoreQuery,
) -> Result<HealthScoreData, String> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let max_score = query.max_score.unwrap_or(DEFAULT_MAX_SCORE).min(100);
    let report = load_health_score_report(shared).await?;
    let recent_reports = read_score_history(shared)?;
    let delta = recent_reports
        .first()
        .map(|previous| report.workspace_score as i32 - previous.workspace_score as i32)
        .unwrap_or(0);
    let trend = build_trend(&report, recent_reports.as_slice());

    let filtered_crates = report
        .crates
        .iter()
        .filter(|crate_score| crate_score.score <= max_score)
        .collect::<Vec<_>>();
    let mut archetype_distribution = BTreeMap::new();
    for crate_score in &filtered_crates {
        for archetype in &crate_score.archetypes {
            *archetype_distribution
                .entry(archetype.as_str().to_owned())
                .or_insert(0) += 1;
        }
    }

    let crates = filtered_crates
        .into_iter()
        .take(limit)
        .map(|crate_score| HealthScoreCrateRow {
            name: crate_score.name.clone(),
            score: crate_score.score,
            severity: crate_score.severity,
            archetypes: crate_score
                .archetypes
                .iter()
                .map(|archetype| archetype.as_str().to_owned())
                .collect(),
            top_violation: crate_score
                .violations
                .first()
                .map(|violation| violation.reason.clone())
                .unwrap_or_else(|| "No active violations".to_owned()),
        })
        .collect();

    Ok(HealthScoreData {
        workspace_score: report.workspace_score,
        severity: report.severity,
        delta,
        crates,
        archetype_distribution,
        trend,
    })
}

pub(crate) async fn load_health_score_report(shared: &SharedState) -> Result<ScoreReport, String> {
    if let Ok(guard) = shared.caches.health_score_report.read()
        && let Some((captured_at, report)) = guard.as_ref()
        && captured_at.elapsed() < HEALTH_SCORE_CACHE_TTL
    {
        return Ok(report.clone());
    }

    let report = compute_health_score_report(shared).await?;
    if let Ok(mut guard) = shared.caches.health_score_report.write() {
        *guard = Some((Instant::now(), report.clone()));
    }
    Ok(report)
}

async fn compute_health_score_report(shared: &SharedState) -> Result<ScoreReport, String> {
    let git = GitContext::open(shared.workspace.as_path());
    let semantic_input = match load_semantic_input(shared).await {
        Ok(input) => input,
        Err(err) => {
            warn!("health-score semantic bridge unavailable: {err}");
            None
        }
    };
    if git.is_some() || semantic_input.is_some() {
        compute_workspace_score_with_signals(
            shared.workspace.as_path(),
            &shared.config.health_score,
            &[],
            git.as_ref(),
            semantic_input.as_ref(),
        )
        .map_err(|err| format!("failed to compute semantic workspace health score: {err}"))
    } else {
        compute_workspace_score(shared.workspace.as_path(), &shared.config.health_score)
            .map_err(|err| format!("failed to compute workspace health score: {err}"))
    }
}

async fn load_semantic_input(shared: &SharedState) -> Result<Option<SemanticInput>, String> {
    let analyzer = HealthAnalyzer::new(shared.workspace.as_path())
        .map_err(|err| format!("failed to initialize health analyzer: {err}"))?;
    let centrality = analyzer
        .centrality_by_file()
        .await
        .map_err(|err| format!("failed to collect centrality by file: {err}"))?;
    if centrality.files.is_empty() && !centrality.notes.is_empty() {
        return Ok(None);
    }

    let drift_by_symbol = latest_semantic_drift_by_symbol(shared.store.as_ref())?;
    let community_by_symbol = shared
        .store
        .list_latest_community_snapshot()
        .map_err(|err| format!("failed to list latest community snapshot: {err}"))?
        .into_iter()
        .map(|entry| (entry.symbol_id, entry.community_id))
        .collect::<HashMap<_, _>>();

    let mut files = HashMap::new();
    for entry in centrality.files {
        let path = normalize_path(entry.file.as_str());
        let symbols = shared
            .store
            .list_symbols_for_file(path.as_str())
            .map_err(|err| format!("failed to list symbols for {path}: {err}"))?;
        if symbols.is_empty() {
            continue;
        }

        let drifted_symbol_count = symbols
            .iter()
            .filter(|symbol| {
                drift_by_symbol
                    .get(symbol.id.as_str())
                    .is_some_and(|magnitude| *magnitude > 0.3)
            })
            .count();
        let stale_or_missing_sir_count = symbols
            .iter()
            .filter(|symbol| {
                shared
                    .store
                    .get_sir_meta(symbol.id.as_str())
                    .ok()
                    .flatten()
                    .is_none_or(|meta| {
                        meta.sir_status
                            .trim()
                            .eq_ignore_ascii_case(SIR_STATUS_STALE)
                    })
            })
            .count();
        let mut community_freq = HashMap::new();
        for symbol in &symbols {
            if let Some(community_id) = community_by_symbol.get(symbol.id.as_str()).copied() {
                *community_freq.entry(community_id).or_insert(0usize) += 1;
            }
        }
        let threshold = 3_usize.max((symbols.len() as f64 * 0.2).ceil() as usize);
        let community_count = community_freq
            .values()
            .filter(|&&count| count >= threshold)
            .count();
        let has_test_coverage = symbols.iter().any(|symbol| {
            shared
                .store
                .list_test_intents_for_symbol(symbol.id.as_str())
                .map(|records| !records.is_empty())
                .unwrap_or(false)
        });

        files.insert(
            path,
            SemanticFileInput {
                max_pagerank: entry.max_pagerank,
                symbol_count: symbols.len(),
                drifted_symbol_count,
                stale_or_missing_sir_count,
                community_count,
                has_test_coverage,
            },
        );
    }

    if files.is_empty() {
        return Ok(None);
    }

    Ok(Some(SemanticInput {
        workspace_max_pagerank: centrality.workspace_max_pagerank,
        files,
    }))
}

fn latest_semantic_drift_by_symbol(store: &SqliteStore) -> Result<HashMap<String, f64>, String> {
    let mut drift_by_symbol = HashMap::new();
    for record in store
        .list_drift_results(true)
        .map_err(|err| format!("failed to list semantic drift results: {err}"))?
    {
        if record.drift_type != "semantic" {
            continue;
        }
        let Some(magnitude) = record.drift_magnitude else {
            continue;
        };
        drift_by_symbol
            .entry(record.symbol_id)
            .or_insert((magnitude as f64).clamp(0.0, 1.0));
    }
    Ok(drift_by_symbol)
}

fn read_score_history(shared: &SharedState) -> Result<Vec<ScoreReport>, String> {
    let Some(conn) = support::open_meta_sqlite_ro(shared.workspace.as_path())
        .map_err(|err| format!("failed to open meta sqlite: {err}"))?
    else {
        return Ok(Vec::new());
    };
    read_recent_reports(&conn, 10)
        .map_err(|err| format!("failed to read health score history: {err}"))
}

fn build_trend(current: &ScoreReport, recent_reports: &[ScoreReport]) -> Vec<u32> {
    let latest_matches_current = recent_reports.first().is_some_and(|report| {
        report.workspace_score == current.workspace_score && report.git_commit == current.git_commit
    });
    let mut trend = if latest_matches_current {
        recent_reports
            .iter()
            .take(10)
            .map(|report| report.workspace_score)
            .collect::<Vec<_>>()
    } else {
        let mut values = recent_reports
            .iter()
            .take(9)
            .map(|report| report.workspace_score)
            .collect::<Vec<_>>();
        values.push(current.workspace_score);
        values
    };
    trend.reverse();
    if trend.is_empty() {
        trend.push(current.workspace_score);
    }
    trend
}
