use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::api::catalog::{CatalogSymbol, SymbolCatalog, load_symbol_catalog};
use crate::support::{self, DashboardState};

const MAX_HOPS: usize = 10;
const SEARCH_PATH_LIMIT: usize = 2500;

#[derive(Debug, Default, Clone, Deserialize)]
pub(crate) struct FlowQuery {
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FlowStep {
    pub number: usize,
    pub symbol: String,
    pub file: String,
    pub layer: String,
    pub layer_icon: String,
    pub narrative: String,
    pub sir_intent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FlowData {
    pub start: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    pub step_count: usize,
    pub steps: Vec<FlowStep>,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub(crate) enum FlowBuildError {
    MissingStart,
    SymbolNotFound(String),
    NoConnection(String),
    Internal(String),
}

pub(crate) async fn flow_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<FlowQuery>,
) -> Response {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || Ok(build_flow_data(shared.as_ref(), &query)))
        .await
    {
        Ok(Ok(data)) => support::api_json(state.shared.as_ref(), data).into_response(),
        Ok(Err(FlowBuildError::MissingStart)) => (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "invalid_request",
                "message": "query parameter 'start' is required"
            })),
        )
            .into_response(),
        Ok(Err(FlowBuildError::SymbolNotFound(name))) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "not_found",
                "message": format!("symbol '{}' not found", name)
            })),
        )
            .into_response(),
        Ok(Err(FlowBuildError::NoConnection(message))) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "not_found",
                "message": message
            })),
        )
            .into_response(),
        Ok(Err(FlowBuildError::Internal(message))) => support::json_internal_error(message),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

pub(crate) fn build_flow_data(
    shared: &crate::state::SharedState,
    query: &FlowQuery,
) -> Result<FlowData, FlowBuildError> {
    let start_selector = query
        .start
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(FlowBuildError::MissingStart)?;

    let catalog = load_symbol_catalog(shared).map_err(FlowBuildError::Internal)?;
    let start = resolve_symbol(&catalog, start_selector)
        .ok_or_else(|| FlowBuildError::SymbolNotFound(start_selector.to_owned()))?;

    let end = match query
        .end
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(selector) => Some(
            resolve_symbol(&catalog, selector)
                .ok_or_else(|| FlowBuildError::SymbolNotFound(selector.to_owned()))?,
        ),
        None => None,
    };

    let path_ids = if let Some(target) = end.as_ref() {
        let path = aether_analysis::bfs_shortest_path(
            catalog.edges.as_slice(),
            start.id.as_str(),
            target.id.as_str(),
        );
        if path.is_empty() {
            return Err(FlowBuildError::NoConnection(format!(
                "No connection found between {} and {}.",
                start.name, target.name
            )));
        }
        path
    } else {
        most_interesting_path(&catalog, start.id.as_str())
    };

    let mut steps = Vec::<FlowStep>::new();
    let edge_kind_map = edge_kind_map(&catalog);

    for (idx, symbol_id) in path_ids.iter().enumerate() {
        let Some(symbol) = catalog.symbol_by_id(symbol_id.as_str()) else {
            continue;
        };

        let is_first = idx == 0;
        let is_last = idx + 1 == path_ids.len();
        let sir_intent = first_sentence_or_fallback(symbol.sir.intent.as_str());
        let transition = if is_last {
            None
        } else {
            let next_id = &path_ids[idx + 1];
            let next_name = catalog
                .symbol_by_id(next_id.as_str())
                .map(|next| next.name.as_str())
                .unwrap_or("the next step");
            Some(compose_transition(
                edge_kind_map
                    .get(&(symbol_id.clone(), next_id.clone()))
                    .map(String::as_str),
                symbol,
                next_name,
            ))
        };

        steps.push(FlowStep {
            number: idx + 1,
            symbol: symbol.name.clone(),
            file: symbol.file_path.clone(),
            layer: symbol.layer_name.clone(),
            layer_icon: symbol.layer_icon.clone(),
            narrative: compose_step_narrative(symbol, is_first, is_last),
            sir_intent,
            transition,
        });
    }

    if steps.is_empty() {
        return Err(FlowBuildError::NoConnection(format!(
            "No connection found from {}.",
            start.name
        )));
    }

    let summary = compose_flow_summary(steps.as_slice(), start.name.as_str());

    Ok(FlowData {
        start: start.name.clone(),
        end: end.map(|target| target.name.clone()),
        step_count: steps.len(),
        steps,
        summary,
    })
}

fn resolve_symbol<'a>(catalog: &'a SymbolCatalog, selector: &str) -> Option<&'a CatalogSymbol> {
    let resolved = catalog.resolve_symbol_selector(selector)?;
    catalog.symbol(resolved.primary_index)
}

fn edge_kind_map(catalog: &SymbolCatalog) -> HashMap<(String, String), String> {
    let mut map = HashMap::<(String, String), String>::new();
    for edge in &catalog.edges {
        map.entry((edge.source_id.clone(), edge.target_id.clone()))
            .or_insert_with(|| edge.edge_kind.clone());
    }
    map
}

fn most_interesting_path(catalog: &SymbolCatalog, start_id: &str) -> Vec<String> {
    let mut queue = VecDeque::<Vec<String>>::new();
    let mut best_path = vec![start_id.to_owned()];
    let mut best_score = path_score(catalog, best_path.as_slice());
    let mut expanded = 0usize;

    queue.push_back(vec![start_id.to_owned()]);

    while let Some(path) = queue.pop_front() {
        expanded += 1;
        if expanded > SEARCH_PATH_LIMIT {
            break;
        }

        let current = path.last().cloned().unwrap_or_default();
        let next_ids = catalog.dependency_ids(current.as_str());
        if next_ids.is_empty() {
            let score = path_score(catalog, path.as_slice());
            if score > best_score {
                best_score = score;
                best_path = path;
            }
            continue;
        }

        for next_id in next_ids {
            if path.iter().any(|seen| seen == &next_id) {
                continue;
            }
            if path.len() > MAX_HOPS {
                continue;
            }
            let mut next_path = path.clone();
            next_path.push(next_id);

            let score = path_score(catalog, next_path.as_slice());
            if score > best_score {
                best_score = score;
                best_path = next_path.clone();
            }
            queue.push_back(next_path);
        }
    }

    best_path
}

fn path_score(catalog: &SymbolCatalog, path: &[String]) -> f64 {
    if path.is_empty() {
        return 0.0;
    }

    let mut layers = HashSet::<String>::new();
    let mut centrality_sum = 0.0f64;
    let mut boundary_crossings = 0usize;
    let mut previous_layer = None::<String>;

    for symbol_id in path {
        let layer = catalog
            .symbol_by_id(symbol_id.as_str())
            .map(|symbol| symbol.layer_name.clone())
            .unwrap_or_else(|| "Core Logic".to_owned());
        centrality_sum += catalog.centrality(symbol_id.as_str());
        layers.insert(layer.clone());

        if let Some(prev) = previous_layer.as_ref()
            && prev != &layer
        {
            boundary_crossings += 1;
        }
        previous_layer = Some(layer);
    }

    (layers.len() as f64 * 10.0)
        + (boundary_crossings as f64 * 4.0)
        + (centrality_sum * 100.0)
        + path.len() as f64
}

fn compose_step_narrative(symbol: &CatalogSymbol, is_first: bool, is_last: bool) -> String {
    let action = first_sentence_or_fallback(symbol.sir.intent.as_str());
    if is_first {
        return format!(
            "The flow starts at {} in the {} layer. {}",
            symbol.name, symbol.layer_name, action
        );
    }
    if is_last {
        return format!(
            "{} {} and returns the result.",
            symbol.name,
            lowercase_sentence(action.as_str())
        );
    }
    format!("{} {}.", symbol.name, lowercase_sentence(action.as_str()))
}

fn compose_transition(edge_kind: Option<&str>, current: &CatalogSymbol, next_name: &str) -> String {
    let edge_kind = edge_kind.unwrap_or_default().to_ascii_lowercase();
    if edge_kind.contains("call") {
        return format!("The output is passed to {next_name}.");
    }
    if edge_kind.contains("depend") {
        return format!("Control is handed off to {next_name} via a dependency link.");
    }
    if current.layer_name == "Data" {
        return format!(
            "State changes from {} are observed by {next_name}.",
            current.name
        );
    }
    format!("The result from {} is passed to {next_name}.", current.name)
}

fn compose_flow_summary(steps: &[FlowStep], start_name: &str) -> String {
    let mut visited_layers = Vec::<String>::new();
    for step in steps {
        if visited_layers.last() == Some(&step.layer) {
            continue;
        }
        visited_layers.push(step.layer.clone());
    }

    if visited_layers.len() <= 1 {
        return format!(
            "This flow starts at {start_name} and stays within the {} layer.",
            visited_layers
                .first()
                .cloned()
                .unwrap_or_else(|| "Core Logic".to_owned())
        );
    }

    let first_layer = visited_layers.first().cloned().unwrap_or_default();
    let last_layer = visited_layers.last().cloned().unwrap_or_default();
    let middle_layers = if visited_layers.len() > 2 {
        visited_layers[1..visited_layers.len() - 1].join(", ")
    } else {
        String::new()
    };

    let middle_phrase = if middle_layers.is_empty() {
        "to".to_owned()
    } else {
        format!("through {middle_layers} to")
    };

    format!(
        "This flow shows how {start_name} travels from the {first_layer} layer {middle_phrase} the {last_layer} layer."
    )
}

fn first_sentence_or_fallback(intent: &str) -> String {
    let first = crate::api::common::first_sentence(intent);
    if first.trim().is_empty() {
        "Performs its core responsibility in the project".to_owned()
    } else {
        first
    }
}

fn lowercase_sentence(input: &str) -> String {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!(
        "{}{}",
        first.to_ascii_lowercase(),
        chars.collect::<String>()
    )
}
