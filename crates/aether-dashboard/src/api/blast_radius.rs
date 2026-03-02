use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::api::common;
use crate::support::{self, DashboardState};

const DEFAULT_DEPTH: u32 = 3;
const MAX_DEPTH: u32 = 5;
const PER_DEPTH_CAP: usize = 500;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct BlastRadiusQuery {
    pub symbol_id: Option<String>,
    pub depth: Option<u32>,
    pub min_coupling: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BlastSignalBreakdown {
    pub temporal: f64,
    pub structural: f64,
    pub semantic: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BlastCouplingToParent {
    pub strength: Option<f64>,
    #[serde(rename = "type")]
    pub coupling_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signals: Option<BlastSignalBreakdown>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BlastNode {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sir_intent: Option<String>,
    pub pagerank: f64,
    pub risk_score: f64,
    pub has_tests: bool,
    pub is_drifting: bool,
    pub drift_score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_symbol_id: Option<String>,
    pub coupling_to_parent: BlastCouplingToParent,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BlastRing {
    pub hop: u32,
    pub count: usize,
    pub nodes: Vec<BlastNode>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BlastRadiusData {
    pub center: BlastNode,
    pub rings: Vec<BlastRing>,
    pub total_impacted: usize,
}

pub(crate) async fn blast_radius_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<BlastRadiusQuery>,
) -> Response {
    let Some(symbol_id) = query
        .symbol_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "bad_request",
                "message": "symbol_id is required"
            })),
        )
            .into_response();
    };

    let depth = query.depth.unwrap_or(DEFAULT_DEPTH).clamp(1, MAX_DEPTH);
    let min_coupling = query.min_coupling.unwrap_or(0.2).clamp(0.0, 1.0);

    let shared = state.shared.clone();
    let symbol_id_owned = symbol_id.to_owned();
    match support::run_async_with_timeout(move || async move {
        load_blast_radius_data(
            shared.as_ref(),
            symbol_id_owned.as_str(),
            depth,
            min_coupling,
        )
        .await
    })
    .await
    {
        Ok(Some(data)) => support::api_json(state.shared.as_ref(), data).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "not_found",
                "message": format!("symbol '{symbol_id}' not found")
            })),
        )
            .into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

pub(crate) async fn load_blast_radius_data(
    shared: &crate::state::SharedState,
    symbol_id: &str,
    depth: u32,
    min_coupling: f64,
) -> Result<Option<BlastRadiusData>, String> {
    let Some(center_symbol) = shared
        .store
        .get_symbol_record(symbol_id)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };

    let symbols = common::load_symbols(shared)?;
    let edges = common::load_dependency_algo_edges(shared)?;
    let pagerank = common::pagerank_map(shared, &edges).await;
    let drift_by_symbol = common::latest_drift_score_by_symbol(shared)?;
    let tests_by_symbol = common::test_count_by_symbol(shared)?;
    let sir_symbols = common::symbols_with_sir(shared)?;

    let max_pagerank = pagerank.values().copied().fold(0.0f64, f64::max);

    let symbol_lookup = symbols
        .iter()
        .map(|row| (row.id.clone(), row.clone()))
        .collect::<HashMap<_, _>>();

    let mut parent_by_symbol = HashMap::<String, String>::new();
    let mut depth_by_symbol = HashMap::<String, u32>::new();
    let mut queue = VecDeque::<String>::new();

    depth_by_symbol.insert(center_symbol.id.clone(), 0);
    queue.push_back(center_symbol.id.clone());

    while let Some(current_id) = queue.pop_front() {
        let current_depth = *depth_by_symbol.get(current_id.as_str()).unwrap_or(&0);
        if current_depth >= depth {
            continue;
        }

        let deps = shared
            .graph
            .get_dependencies(current_id.as_str())
            .await
            .unwrap_or_default();

        let mut inserted_at_level = 0usize;
        for dep in deps {
            let next_depth = current_depth + 1;
            if next_depth > depth {
                continue;
            }
            if depth_by_symbol.contains_key(dep.id.as_str()) {
                continue;
            }

            let existing_in_level = depth_by_symbol
                .values()
                .filter(|value| **value == next_depth)
                .count();
            if existing_in_level + inserted_at_level >= PER_DEPTH_CAP {
                continue;
            }

            depth_by_symbol.insert(dep.id.clone(), next_depth);
            parent_by_symbol.insert(dep.id.clone(), current_id.clone());
            queue.push_back(dep.id);
            inserted_at_level += 1;
        }
    }

    let center = build_node(
        shared,
        &center_symbol.id,
        None,
        &symbol_lookup,
        &pagerank,
        &drift_by_symbol,
        &tests_by_symbol,
        &sir_symbols,
        max_pagerank,
        0.0,
        min_coupling,
    )
    .await;

    let mut rings = Vec::<BlastRing>::new();
    for hop in 1..=depth {
        let ids = depth_by_symbol
            .iter()
            .filter_map(|(id, level)| (*level == hop).then_some(id.clone()))
            .collect::<Vec<_>>();

        let mut nodes = Vec::<BlastNode>::new();
        for id in ids {
            let parent = parent_by_symbol.get(id.as_str()).cloned();
            let mut node = build_node(
                shared,
                id.as_str(),
                parent.clone(),
                &symbol_lookup,
                &pagerank,
                &drift_by_symbol,
                &tests_by_symbol,
                &sir_symbols,
                max_pagerank,
                min_coupling,
                min_coupling,
            )
            .await;

            if node
                .coupling_to_parent
                .strength
                .is_some_and(|strength| strength < min_coupling)
            {
                continue;
            }
            node.parent_symbol_id = parent;
            nodes.push(node);
        }

        nodes.sort_by(|left, right| {
            right
                .risk_score
                .partial_cmp(&left.risk_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| {
                    right
                        .pagerank
                        .partial_cmp(&left.pagerank)
                        .unwrap_or(Ordering::Equal)
                })
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });

        rings.push(BlastRing {
            hop,
            count: nodes.len(),
            nodes,
        });
    }

    let total_impacted = rings.iter().map(|ring| ring.count).sum();
    Ok(Some(BlastRadiusData {
        center,
        rings,
        total_impacted,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn build_node(
    shared: &crate::state::SharedState,
    symbol_id: &str,
    parent_symbol_id: Option<String>,
    symbol_lookup: &HashMap<String, common::SymbolInfo>,
    pagerank: &HashMap<String, f64>,
    drift_by_symbol: &HashMap<String, f64>,
    tests_by_symbol: &HashMap<String, i64>,
    sir_symbols: &HashSet<String>,
    max_pagerank: f64,
    default_strength: f64,
    min_coupling: f64,
) -> BlastNode {
    let symbol = symbol_lookup.get(symbol_id);
    let qualified_name = symbol
        .map(|s| s.qualified_name.clone())
        .or_else(|| {
            shared
                .store
                .get_symbol_record(symbol_id)
                .ok()
                .flatten()
                .map(|s| s.qualified_name)
        })
        .unwrap_or_else(|| symbol_id.to_owned());

    let file_path = symbol
        .map(|s| support::normalized_display_path(s.file_path.as_str()))
        .unwrap_or_default();

    let drift_score = drift_by_symbol.get(symbol_id).copied().unwrap_or(0.0);
    let pr = pagerank.get(symbol_id).copied().unwrap_or(0.0);
    let test_count = tests_by_symbol.get(symbol_id).copied().unwrap_or(0);
    let has_sir = sir_symbols.contains(symbol_id);
    let risk = common::risk_score(pr, drift_score, has_sir, test_count, max_pagerank);

    let sir_intent = support::sir_excerpt_for_symbol(shared, symbol_id)
        .map(|excerpt| common::first_sentence(excerpt.as_str()))
        .filter(|value| !value.is_empty());

    let mut coupling = BlastCouplingToParent {
        strength: Some(default_strength),
        coupling_type: "structural".to_owned(),
        signals: None,
    };

    if let Some(parent_id) = parent_symbol_id.as_deref() {
        let parent_file = symbol_lookup
            .get(parent_id)
            .map(|s| s.file_path.clone())
            .or_else(|| {
                shared
                    .store
                    .get_symbol_record(parent_id)
                    .ok()
                    .flatten()
                    .map(|s| s.file_path)
            })
            .unwrap_or_default();

        if let Some(edge) =
            common::co_change_between_files(shared, parent_file.as_str(), file_path.as_str()).await
        {
            coupling = BlastCouplingToParent {
                strength: Some(edge.fused_score),
                coupling_type: edge.coupling_type,
                signals: Some(BlastSignalBreakdown {
                    temporal: edge.signals.temporal,
                    structural: edge.signals.structural,
                    semantic: edge.signals.semantic,
                }),
            };
        } else {
            coupling = BlastCouplingToParent {
                strength: Some(default_strength.max(min_coupling)),
                coupling_type: "structural".to_owned(),
                signals: None,
            };
        }
    }

    BlastNode {
        symbol_id: symbol_id.to_owned(),
        qualified_name,
        file_path,
        sir_intent,
        pagerank: pr,
        risk_score: risk,
        has_tests: test_count > 0,
        is_drifting: drift_score >= 0.15,
        drift_score,
        parent_symbol_id,
        coupling_to_parent: coupling,
    }
}
