use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use crate::support::{self, DashboardState};

/// Mirror of `ScoreBands` from aetherd::continuous::monitor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScoreBands {
    #[serde(default)]
    pub critical: usize,
    #[serde(default)]
    pub high: usize,
    #[serde(default)]
    pub medium: usize,
    #[serde(default)]
    pub low: usize,
}

/// Mirror of `ContinuousStatusSymbol` from aetherd::continuous::monitor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MostStaleSymbol {
    #[serde(default)]
    pub symbol_id: String,
    #[serde(default)]
    pub qualified_name: String,
    #[serde(default)]
    pub staleness_score: f64,
}

/// Mirror of `ContinuousStatusSnapshot` from aetherd::continuous::monitor.
/// Every field has `#[serde(default)]` for forward-compatibility.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StatusSnapshot {
    #[serde(default)]
    last_started_at: Option<i64>,
    #[serde(default)]
    last_completed_at: Option<i64>,
    #[serde(default)]
    last_successful_completed_at: Option<i64>,
    #[serde(default)]
    total_symbols: usize,
    #[serde(default)]
    symbols_with_sir: usize,
    #[serde(default)]
    scored_symbols: usize,
    #[serde(default)]
    score_bands: ScoreBands,
    #[serde(default)]
    most_stale_symbol: Option<MostStaleSymbol>,
    #[serde(default)]
    selected_symbols: usize,
    #[serde(default)]
    written_requests: usize,
    #[serde(default)]
    skipped_requests: usize,
    #[serde(default)]
    unresolved_symbols: usize,
    #[serde(default)]
    chunk_count: usize,
    #[serde(default)]
    auto_submit: bool,
    #[serde(default)]
    submitted_chunks: usize,
    #[serde(default)]
    ingested_results: usize,
    #[serde(default)]
    fingerprint_rows: usize,
    #[serde(default)]
    requeue_pass: String,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ContinuousData {
    pub has_data: bool,
    pub last_started_at: Option<i64>,
    pub last_completed_at: Option<i64>,
    pub total_symbols: usize,
    pub symbols_with_sir: usize,
    pub scored_symbols: usize,
    pub score_bands: ScoreBands,
    pub most_stale: Option<MostStaleSymbol>,
    pub selected_symbols: usize,
    pub requeue_pass: String,
    pub last_error: Option<String>,
}

pub(crate) async fn continuous_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || load_continuous_data(shared.as_ref())).await {
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

pub(crate) fn load_continuous_data(shared: &SharedState) -> Result<ContinuousData, String> {
    let aether_dir = aether_config::aether_dir(&shared.workspace);
    let status_path = aether_dir.join("continuous").join("status.json");

    if !status_path.exists() {
        return Ok(ContinuousData {
            has_data: false,
            last_started_at: None,
            last_completed_at: None,
            total_symbols: 0,
            symbols_with_sir: 0,
            scored_symbols: 0,
            score_bands: ScoreBands::default(),
            most_stale: None,
            selected_symbols: 0,
            requeue_pass: String::new(),
            last_error: None,
        });
    }

    let raw = std::fs::read_to_string(&status_path)
        .map_err(|e| format!("failed to read status.json: {e}"))?;
    let snapshot: StatusSnapshot =
        serde_json::from_str(&raw).map_err(|e| format!("failed to parse status.json: {e}"))?;

    Ok(ContinuousData {
        has_data: true,
        last_started_at: snapshot.last_started_at,
        last_completed_at: snapshot.last_completed_at,
        total_symbols: snapshot.total_symbols,
        symbols_with_sir: snapshot.symbols_with_sir,
        scored_symbols: snapshot.scored_symbols,
        score_bands: snapshot.score_bands,
        most_stale: snapshot.most_stale_symbol,
        selected_symbols: snapshot.selected_symbols,
        requeue_pass: snapshot.requeue_pass,
        last_error: snapshot.last_error,
    })
}
