use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use aether_store::{SymbolCatalogStore, SymbolRecord};

use crate::api::common;
use crate::api::difficulty::difficulty_for_symbol;
use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_DEPTH: u32 = 2;
const MAX_DEPTH: u32 = 4;
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 500;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GraphQuery {
    pub root: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub file: String,
    pub layer: String,
    pub sir_exists: bool,
    pub difficulty_score: f64,
    pub difficulty_label: String,
    pub difficulty_emoji: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagerank: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub weight: f32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub total_nodes: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub(crate) async fn graph_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<GraphQuery>,
) -> impl IntoResponse {
    let depth = query.depth.unwrap_or(DEFAULT_DEPTH).clamp(1, MAX_DEPTH);
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LIMIT as u32)
        .clamp(1, MAX_LIMIT as u32) as usize;

    let shared = state.shared.clone();
    let root = query.root;
    match support::run_async_with_timeout(move || async move {
        load_graph_data(shared.as_ref(), root, depth, limit).await
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

pub(crate) async fn load_graph_data(
    shared: &SharedState,
    root: Option<String>,
    depth: u32,
    limit: usize,
) -> Result<GraphData, String> {
    let mut nodes_by_id = HashMap::<String, SymbolRecord>::new();
    let mut edges = Vec::<GraphEdge>::new();
    let mut edge_seen = HashSet::<(String, String, String)>::new();
    let mut frontier = VecDeque::<(String, u32)>::new();
    let mut queued = HashSet::<String>::new();
    let mut expanded = HashSet::<String>::new();

    let mut truncated = false;
    let mut dropped_nodes = 0usize;
    let mut message = None;

    if let Some(root_value) = root
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        match resolve_root_symbol(shared, root_value)? {
            Some(symbol) => {
                let root_id = symbol.id.clone();
                if try_insert_node(
                    symbol,
                    &mut nodes_by_id,
                    limit,
                    &mut truncated,
                    &mut dropped_nodes,
                ) {
                    frontier.push_back((root_id.clone(), 0));
                    queued.insert(root_id);
                }
            }
            None => {
                message = Some(format!("Root symbol '{}' not found", root_value));
                return Ok(GraphData {
                    nodes: Vec::new(),
                    edges: Vec::new(),
                    total_nodes: 0,
                    truncated: false,
                    message,
                });
            }
        }
    } else {
        let seed_ids = load_recent_seed_ids(shared, limit.min(50))?;
        if seed_ids.is_empty() {
            message = Some("No indexed symbols available".to_owned());
            return Ok(GraphData {
                nodes: Vec::new(),
                edges: Vec::new(),
                total_nodes: 0,
                truncated: false,
                message,
            });
        }

        for symbol_id in seed_ids {
            let symbol = match shared.store.get_symbol_record(symbol_id.as_str()) {
                Ok(Some(symbol)) => symbol,
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(error = %err, symbol_id = %symbol_id, "dashboard: failed to load seed symbol");
                    continue;
                }
            };
            if try_insert_node(
                symbol.clone(),
                &mut nodes_by_id,
                limit,
                &mut truncated,
                &mut dropped_nodes,
            ) && queued.insert(symbol.id.clone())
            {
                frontier.push_back((symbol.id, 0));
            }
            if nodes_by_id.len() >= limit {
                truncated = true;
                break;
            }
        }
    }

    while let Some((symbol_id, level)) = frontier.pop_front() {
        if level >= depth {
            continue;
        }
        if !expanded.insert(symbol_id.clone()) {
            continue;
        }

        let Some(current) = nodes_by_id.get(symbol_id.as_str()).cloned() else {
            continue;
        };

        let dependencies = match shared.graph.get_dependencies(current.id.as_str()).await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, symbol_id = %current.id, "dashboard: get_dependencies failed");
                Vec::new()
            }
        };

        for dep in dependencies {
            let dep_id = dep.id.clone();
            let dep_visible = try_insert_node(
                dep,
                &mut nodes_by_id,
                limit,
                &mut truncated,
                &mut dropped_nodes,
            );
            if dep_visible {
                if edge_seen.insert((current.id.clone(), dep_id.clone(), "calls".to_owned())) {
                    edges.push(GraphEdge {
                        source: current.id.clone(),
                        target: dep_id.clone(),
                        edge_type: "calls".to_owned(),
                        weight: 1.0,
                    });
                }
                if queued.insert(dep_id.clone()) {
                    frontier.push_back((dep_id, level + 1));
                }
            }
        }

        let callers = match shared
            .graph
            .get_callers(current.qualified_name.as_str())
            .await
        {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, symbol = %current.qualified_name, "dashboard: get_callers failed");
                Vec::new()
            }
        };

        for caller in callers {
            let caller_id = caller.id.clone();
            let caller_visible = try_insert_node(
                caller,
                &mut nodes_by_id,
                limit,
                &mut truncated,
                &mut dropped_nodes,
            );
            if caller_visible {
                if edge_seen.insert((caller_id.clone(), current.id.clone(), "calls".to_owned())) {
                    edges.push(GraphEdge {
                        source: caller_id.clone(),
                        target: current.id.clone(),
                        edge_type: "calls".to_owned(),
                        weight: 1.0,
                    });
                }
                if queued.insert(caller_id.clone()) {
                    frontier.push_back((caller_id, level + 1));
                }
            }
        }
    }

    edges.retain(|edge| {
        nodes_by_id.contains_key(edge.source.as_str())
            && nodes_by_id.contains_key(edge.target.as_str())
    });

    edges.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then_with(|| left.target.cmp(&right.target))
            .then_with(|| left.edge_type.cmp(&right.edge_type))
    });

    let catalog = crate::api::catalog::load_symbol_catalog(shared).ok();
    let mut difficulty_by_id = HashMap::<String, crate::api::difficulty::DifficultyView>::new();
    let mut layer_by_id = HashMap::<String, String>::new();
    if let Some(catalog) = catalog.as_ref() {
        for symbol in &catalog.symbols {
            difficulty_by_id.insert(symbol.id.clone(), difficulty_for_symbol(symbol));
            layer_by_id.insert(symbol.id.clone(), symbol.layer_name.clone());
        }
    }

    let all_edges = common::load_dependency_algo_edges(shared)?;
    let pagerank = common::pagerank_map(shared, &all_edges).await;
    let communities = common::louvain_map(shared, &all_edges).await;
    let drift_map = common::latest_drift_score_by_symbol(shared)?;
    let test_map = common::test_count_by_symbol(shared)?;
    let sir_symbols = common::symbols_with_sir(shared)?;
    let max_pagerank = pagerank.values().copied().fold(0.0f64, f64::max);

    let mut nodes = nodes_by_id
        .values()
        .map(|symbol| {
            let difficulty = difficulty_by_id.get(symbol.id.as_str()).cloned().unwrap_or(
                crate::api::difficulty::DifficultyView {
                    score: 0.0,
                    emoji: "🟢".to_owned(),
                    label: "Easy".to_owned(),
                    guidance: "Minimal prompting needed.".to_owned(),
                    reasons: Vec::new(),
                },
            );

            GraphNode {
                id: symbol.id.clone(),
                label: support::symbol_name_from_qualified(symbol.qualified_name.as_str()),
                kind: symbol.kind.clone(),
                file: support::normalized_display_path(symbol.file_path.as_str()),
                layer: layer_by_id
                    .get(symbol.id.as_str())
                    .cloned()
                    .unwrap_or_else(|| "Core Logic".to_owned()),
                sir_exists: sir_symbols.contains(symbol.id.as_str()),
                difficulty_score: difficulty.score,
                difficulty_label: difficulty.label,
                difficulty_emoji: difficulty.emoji,
                pagerank: Some(pagerank.get(symbol.id.as_str()).copied().unwrap_or(0.0)),
                risk_score: Some(common::risk_score(
                    pagerank.get(symbol.id.as_str()).copied().unwrap_or(0.0),
                    drift_map.get(symbol.id.as_str()).copied().unwrap_or(0.0),
                    sir_symbols.contains(symbol.id.as_str()),
                    test_map.get(symbol.id.as_str()).copied().unwrap_or(0),
                    max_pagerank,
                )),
                community_id: communities.get(symbol.id.as_str()).copied(),
            }
        })
        .collect::<Vec<_>>();

    nodes.sort_by(|left, right| {
        left.label
            .cmp(&right.label)
            .then_with(|| left.id.cmp(&right.id))
    });

    let total_nodes = nodes.len().saturating_add(dropped_nodes);
    if truncated && message.is_none() {
        message = Some(format!("Graph truncated to {} connected components", limit));
    }

    Ok(GraphData {
        nodes,
        edges,
        total_nodes,
        truncated,
        message,
    })
}

fn resolve_root_symbol(shared: &SharedState, root: &str) -> Result<Option<SymbolRecord>, String> {
    if let Some(found) = shared
        .store
        .get_symbol_record(root)
        .map_err(|err| err.to_string())?
    {
        return Ok(Some(found));
    }

    let search = shared
        .store
        .search_symbols(root, 1)
        .map_err(|err| err.to_string())?;
    let Some(first) = search.first() else {
        return Ok(None);
    };

    shared
        .store
        .get_symbol_record(first.symbol_id.as_str())
        .map_err(|err| err.to_string())
}

fn load_recent_seed_ids(shared: &SharedState, limit: usize) -> Result<Vec<String>, String> {
    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };

    let primary_sql = r#"
        SELECT id
        FROM symbols
        ORDER BY COALESCE(last_accessed_at, last_seen_at, 0) DESC,
                 last_seen_at DESC,
                 id ASC
        LIMIT ?1
    "#;

    let fallback_sql = r#"
        SELECT id
        FROM symbols
        ORDER BY last_seen_at DESC, id ASC
        LIMIT ?1
    "#;

    let mut out = Vec::new();
    match conn.prepare(primary_sql) {
        Ok(mut stmt) => {
            let rows = stmt
                .query_map([limit as i64], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for row in rows {
                out.push(row.map_err(|e| e.to_string())?);
            }
        }
        Err(err)
            if support::is_missing_table(&err) || err.to_string().contains("no such column") =>
        {
            let mut stmt = conn.prepare(fallback_sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([limit as i64], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for row in rows {
                out.push(row.map_err(|e| e.to_string())?);
            }
        }
        Err(err) => return Err(err.to_string()),
    }
    Ok(out)
}

fn try_insert_node(
    symbol: SymbolRecord,
    nodes_by_id: &mut HashMap<String, SymbolRecord>,
    limit: usize,
    truncated: &mut bool,
    dropped_nodes: &mut usize,
) -> bool {
    if nodes_by_id.contains_key(symbol.id.as_str()) {
        return true;
    }
    if nodes_by_id.len() >= limit {
        *truncated = true;
        *dropped_nodes = (*dropped_nodes).saturating_add(1);
        return false;
    }
    nodes_by_id.insert(symbol.id.clone(), symbol);
    true
}
