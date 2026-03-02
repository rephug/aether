use std::cmp::Ordering;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use aether_analysis::{CausalAnalyzer, TraceCauseRequest};

use crate::api::common;
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CausalChainQuery {
    pub symbol_id: Option<String>,
    pub depth: Option<u32>,
    pub lookback: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CausalTarget {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CausalChainRow {
    pub symbol_id: String,
    pub qualified_name: String,
    pub timestamp: i64,
    pub drift_score: f64,
    pub sir_diff_summary: String,
    pub causal_confidence: f64,
    pub link_type: String,
    pub caused: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CausalChainData {
    pub target: CausalTarget,
    pub chain: Vec<CausalChainRow>,
    pub overall_confidence: f64,
}

pub(crate) async fn causal_chain_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<CausalChainQuery>,
) -> Response {
    let Some(symbol_id) = query
        .symbol_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return support::api_json(
            state.shared.as_ref(),
            CausalChainData {
                target: CausalTarget {
                    symbol_id: String::new(),
                    qualified_name: String::new(),
                    file_path: String::new(),
                },
                chain: Vec::new(),
                overall_confidence: 0.0,
            },
        )
        .into_response();
    };

    let depth = query.depth.unwrap_or(3).clamp(1, 5);
    let lookback = query.lookback.unwrap_or_else(|| "30d".to_owned());

    let shared = state.shared.clone();
    let symbol_id = symbol_id.to_owned();
    match support::run_blocking_with_timeout(move || {
        load_causal_chain_data(
            shared.as_ref(),
            symbol_id.as_str(),
            depth,
            lookback.as_str(),
        )
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

pub(crate) fn load_causal_chain_data(
    shared: &crate::state::SharedState,
    symbol_id: &str,
    depth: u32,
    lookback: &str,
) -> Result<CausalChainData, String> {
    let known_symbol = shared
        .store
        .get_symbol_record(symbol_id)
        .map_err(|e| e.to_string())?;
    let fallback_target = CausalTarget {
        symbol_id: symbol_id.to_owned(),
        qualified_name: known_symbol
            .as_ref()
            .map(|row| row.qualified_name.clone())
            .unwrap_or_else(|| symbol_id.to_owned()),
        file_path: known_symbol
            .as_ref()
            .map(|row| support::normalized_display_path(row.file_path.as_str()))
            .unwrap_or_default(),
    };

    let analyzer = CausalAnalyzer::new(shared.workspace.as_path()).map_err(|e| e.to_string())?;
    let result = match analyzer.trace_cause(TraceCauseRequest {
        target_symbol_id: symbol_id.to_owned(),
        lookback: Some(common::parse_lookback_to_analyzer_input(lookback)),
        max_depth: Some(depth),
        limit: Some(50),
    }) {
        Ok(result) => result,
        Err(_) => {
            return Ok(CausalChainData {
                target: fallback_target.clone(),
                chain: Vec::new(),
                overall_confidence: 0.0,
            });
        }
    };

    let drift_by_symbol = common::latest_drift_score_by_symbol(shared)?;

    let mut rows = result
        .causal_chain
        .into_iter()
        .map(|entry| {
            let sir_summary = summarize_sir_diff(
                entry.change.sir_diff.purpose_before.as_str(),
                entry.change.sir_diff.purpose_after.as_str(),
                entry.change.sir_diff.edge_cases_added.as_slice(),
                entry.change.sir_diff.dependencies_added.as_slice(),
            );

            let link_type = if entry.coupling.coupling_type.contains("struct")
                || entry.coupling.coupling_type.contains("dependency")
            {
                "dependency".to_owned()
            } else {
                "co_change".to_owned()
            };

            CausalChainRow {
                symbol_id: entry.symbol_id.clone(),
                qualified_name: entry.symbol_name,
                timestamp: parse_rfc3339_to_millis(entry.change.date.as_str())
                    .unwrap_or_else(|| support::current_unix_timestamp() * 1000),
                drift_score: drift_by_symbol
                    .get(entry.symbol_id.as_str())
                    .copied()
                    .unwrap_or(0.0),
                sir_diff_summary: if sir_summary.is_empty() {
                    "No SIR diff available".to_owned()
                } else {
                    sir_summary
                },
                causal_confidence: (entry.causal_score as f64).clamp(0.0, 1.0),
                link_type,
                caused: entry.dependency_path,
            }
        })
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        right
            .causal_confidence
            .partial_cmp(&left.causal_confidence)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.timestamp.cmp(&left.timestamp))
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });

    let overall_confidence = if rows.is_empty() {
        0.0
    } else {
        rows.iter().map(|row| row.causal_confidence).sum::<f64>() / rows.len() as f64
    };

    let mut file_path = support::normalized_display_path(result.target.file.as_str());
    if file_path.is_empty() {
        file_path = fallback_target.file_path.clone();
    }

    Ok(CausalChainData {
        target: CausalTarget {
            symbol_id: if result.target.symbol_id.trim().is_empty() {
                fallback_target.symbol_id
            } else {
                result.target.symbol_id
            },
            qualified_name: if result.target.symbol_name.trim().is_empty() {
                fallback_target.qualified_name
            } else {
                result.target.symbol_name
            },
            file_path,
        },
        chain: rows,
        overall_confidence: overall_confidence.clamp(0.0, 1.0),
    })
}

fn summarize_sir_diff(
    before: &str,
    after: &str,
    edge_cases_added: &[String],
    deps_added: &[String],
) -> String {
    let before = before.trim();
    let after = after.trim();

    if !before.is_empty() || !after.is_empty() {
        if before != after {
            return format!("Purpose shifted from '{before}' to '{after}'");
        }
        return format!("Purpose unchanged: {after}");
    }

    if !edge_cases_added.is_empty() {
        return format!("Added edge cases: {}", edge_cases_added.join(", "));
    }
    if !deps_added.is_empty() {
        return format!("Added dependencies: {}", deps_added.join(", "));
    }

    String::new()
}

fn parse_rfc3339_to_millis(input: &str) -> Option<i64> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.with_timezone(&Utc).timestamp_millis())
}
