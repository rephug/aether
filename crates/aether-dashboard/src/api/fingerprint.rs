use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct FingerprintSummaryQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct FingerprintHistoryQuery {
    pub symbol_id: Option<String>,
}

// ── API data types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FingerprintChangeEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub timestamp: i64,
    pub trigger: String,
    pub source_changed: bool,
    pub neighbor_changed: bool,
    pub config_changed: bool,
    pub delta_sem: Option<f64>,
    pub generation_model: Option<String>,
    pub generation_pass: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TopChangedSymbol {
    pub symbol_id: String,
    pub symbol_name: String,
    pub change_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FingerprintSummaryData {
    pub has_data: bool,
    pub recent_changes: Vec<FingerprintChangeEntry>,
    pub total_recent: usize,
    pub trigger_breakdown: BTreeMap<String, usize>,
    pub top_changed_symbols: Vec<TopChangedSymbol>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FingerprintTimelineEntry {
    pub timestamp: i64,
    pub prompt_hash: String,
    pub prompt_hash_previous: Option<String>,
    pub trigger: String,
    pub source_changed: bool,
    pub neighbor_changed: bool,
    pub config_changed: bool,
    pub delta_sem: Option<f64>,
    pub generation_model: Option<String>,
    pub generation_pass: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FingerprintTimelineData {
    pub symbol_id: String,
    pub symbol_name: String,
    pub entries: Vec<FingerprintTimelineEntry>,
}

// ── Handlers ───────────────────────────────────────────────────────────

pub(crate) async fn fingerprint_summary_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<FingerprintSummaryQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || {
        load_fingerprint_summary(shared.as_ref(), limit)
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

pub(crate) async fn fingerprint_history_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<FingerprintHistoryQuery>,
) -> impl IntoResponse {
    let symbol_id = match query.symbol_id {
        Some(id) if !id.trim().is_empty() => id,
        _ => {
            return support::json_internal_error(
                "symbol_id query parameter is required".to_owned(),
            );
        }
    };
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || {
        load_fingerprint_timeline(shared.as_ref(), &symbol_id)
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

// ── Data loading ───────────────────────────────────────────────────────

pub(crate) fn load_fingerprint_summary(
    shared: &SharedState,
    limit: usize,
) -> Result<FingerprintSummaryData, String> {
    let recent = shared
        .store
        .list_recent_fingerprint_changes(limit)
        .map_err(|e| format!("failed to load recent fingerprint changes: {e}"))?;

    if recent.is_empty() {
        return Ok(FingerprintSummaryData {
            has_data: false,
            recent_changes: Vec::new(),
            total_recent: 0,
            trigger_breakdown: BTreeMap::new(),
            top_changed_symbols: Vec::new(),
        });
    }

    // Resolve symbol names for all unique symbol IDs
    let unique_ids: Vec<String> = recent
        .iter()
        .map(|r| r.symbol_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let name_map = resolve_symbol_names(shared, &unique_ids);

    // Build trigger breakdown
    let mut trigger_breakdown: BTreeMap<String, usize> = BTreeMap::new();
    for record in &recent {
        if record.source_changed {
            *trigger_breakdown.entry("source".to_owned()).or_default() += 1;
        }
        if record.neighbor_changed {
            *trigger_breakdown.entry("neighbor".to_owned()).or_default() += 1;
        }
        if record.config_changed {
            *trigger_breakdown.entry("config".to_owned()).or_default() += 1;
        }
    }

    let recent_changes: Vec<FingerprintChangeEntry> = recent
        .iter()
        .map(|r| {
            let symbol_name = name_map
                .get(&r.symbol_id)
                .cloned()
                .unwrap_or_else(|| r.symbol_id.clone());
            FingerprintChangeEntry {
                symbol_id: r.symbol_id.clone(),
                symbol_name,
                timestamp: r.timestamp,
                trigger: r.trigger.clone(),
                source_changed: r.source_changed,
                neighbor_changed: r.neighbor_changed,
                config_changed: r.config_changed,
                delta_sem: r.delta_sem,
                generation_model: r.generation_model.clone(),
                generation_pass: r.generation_pass.clone(),
            }
        })
        .collect();
    let total_recent = recent_changes.len();

    // Top changed symbols
    let top_counts = shared
        .store
        .count_fingerprint_changes_by_symbol(20)
        .map_err(|e| format!("failed to count fingerprint changes: {e}"))?;
    let top_changed_symbols: Vec<TopChangedSymbol> = top_counts
        .into_iter()
        .map(|(id, count)| {
            let symbol_name = name_map.get(&id).cloned().unwrap_or_else(|| id.clone());
            TopChangedSymbol {
                symbol_id: id,
                symbol_name,
                change_count: count,
            }
        })
        .collect();

    Ok(FingerprintSummaryData {
        has_data: true,
        recent_changes,
        total_recent,
        trigger_breakdown,
        top_changed_symbols,
    })
}

pub(crate) fn load_fingerprint_timeline(
    shared: &SharedState,
    symbol_id: &str,
) -> Result<FingerprintTimelineData, String> {
    let history = shared
        .store
        .list_sir_fingerprint_history(symbol_id)
        .map_err(|e| format!("failed to load fingerprint history: {e}"))?;

    let symbol_name = resolve_symbol_names(shared, &[symbol_id.to_owned()])
        .get(symbol_id)
        .cloned()
        .unwrap_or_else(|| symbol_id.to_owned());

    let entries = history
        .into_iter()
        .map(|r| FingerprintTimelineEntry {
            timestamp: r.timestamp,
            prompt_hash: r.prompt_hash,
            prompt_hash_previous: r.prompt_hash_previous,
            trigger: r.trigger,
            source_changed: r.source_changed,
            neighbor_changed: r.neighbor_changed,
            config_changed: r.config_changed,
            delta_sem: r.delta_sem,
            generation_model: r.generation_model,
            generation_pass: r.generation_pass,
        })
        .collect();

    Ok(FingerprintTimelineData {
        symbol_id: symbol_id.to_owned(),
        symbol_name,
        entries,
    })
}

fn resolve_symbol_names(shared: &SharedState, symbol_ids: &[String]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Ok(Some(conn)) = support::open_meta_sqlite_ro(shared.workspace.as_path()) {
        for id in symbol_ids {
            let result = conn.query_row(
                "SELECT qualified_name FROM symbols WHERE id = ?1 LIMIT 1",
                rusqlite::params![id],
                |row| row.get::<_, String>(0),
            );
            if let Ok(name) = result {
                map.insert(id.clone(), name);
            }
        }
    }
    map
}
