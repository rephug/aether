use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::catalog::{
    CatalogSymbol, SymbolCatalog, load_symbol_catalog, parse_dependency_entry,
};
use crate::narrative::{SymbolInfo, compose_file_summary};
use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileSymbolEntry {
    pub name: String,
    pub kind: String,
    pub sir_intent: String,
    pub centrality: f64,
    pub dependents_count: usize,
    pub internal_connections: Vec<String>,
    pub role_in_file: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileConnections {
    pub depended_on_by: Vec<String>,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileDeepDiveData {
    pub path: String,
    pub layer: String,
    pub layer_icon: String,
    pub symbol_count: usize,
    pub summary: String,
    pub internal_narrative: String,
    pub external_narrative: String,
    pub symbols: Vec<FileSymbolEntry>,
    pub connections_to_project: FileConnections,
}

pub(crate) async fn file_handler(
    State(state): State<Arc<DashboardState>>,
    Path(path): Path<String>,
) -> Response {
    let shared = state.shared.clone();
    let selector = path.clone();
    match support::run_blocking_with_timeout(move || {
        build_file_deep_dive(shared.as_ref(), selector.as_str())
    })
    .await
    {
        Ok(Some(data)) => support::api_json(state.shared.as_ref(), data).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "not_found",
                "message": format!("file '{}' not found", path)
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

pub(crate) fn build_file_deep_dive(
    shared: &SharedState,
    path: &str,
) -> Result<Option<FileDeepDiveData>, String> {
    let catalog = load_symbol_catalog(shared)?;
    let normalized = support::normalized_display_path(path.trim());
    let indices = resolve_file_indices(&catalog, normalized.as_str());
    if indices.is_empty() {
        return Ok(None);
    }

    let mut symbols = indices
        .iter()
        .filter_map(|idx| catalog.symbol(*idx).cloned())
        .collect::<Vec<_>>();
    symbols.sort_by(|left, right| {
        right
            .centrality
            .partial_cmp(&left.centrality)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.dependents_count.cmp(&left.dependents_count))
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
    });

    let primary = symbols
        .first()
        .cloned()
        .unwrap_or_else(|| catalog.symbol(indices[0]).expect("index exists").clone());

    let layer = crate::narrative::layer_by_name(primary.layer_name.as_str());
    let summary = compose_summary(normalized.as_str(), symbols.as_slice(), &primary);

    let file_symbol_ids = symbols
        .iter()
        .map(|symbol| symbol.id.as_str())
        .collect::<HashSet<_>>();

    let internal_dependency_map = symbols
        .iter()
        .map(|symbol| {
            let mut connections = catalog
                .dependency_ids(symbol.id.as_str())
                .into_iter()
                .filter_map(|dep_id| {
                    if !file_symbol_ids.contains(dep_id.as_str()) {
                        return None;
                    }
                    catalog
                        .symbol_by_id(dep_id.as_str())
                        .map(|dep| dep.name.clone())
                })
                .collect::<Vec<_>>();
            connections.sort();
            connections.dedup();
            (symbol.id.clone(), connections)
        })
        .collect::<HashMap<_, _>>();

    let mut depended_on_by = BTreeSet::<String>::new();
    let mut depended_on_by_layers = BTreeMap::<String, usize>::new();

    for symbol in &symbols {
        for dependent_id in catalog.dependent_ids(symbol.id.as_str()) {
            let Some(dependent) = catalog.symbol_by_id(dependent_id.as_str()) else {
                continue;
            };
            if dependent.file_path == normalized {
                continue;
            }
            depended_on_by.insert(dependent.file_path.clone());
            *depended_on_by_layers
                .entry(dependent.layer_name.clone())
                .or_insert(0) += 1;
        }
    }

    let mut depends_on = BTreeSet::<String>::new();
    for symbol in &symbols {
        for dependency_id in catalog.dependency_ids(symbol.id.as_str()) {
            if let Some(dependency) = catalog.symbol_by_id(dependency_id.as_str())
                && dependency.file_path != normalized
            {
                depends_on.insert(dependency.name.clone());
            }
        }
        for dep in &symbol.sir.dependencies {
            let (name, _) = parse_dependency_entry(dep.as_str());
            if !name.trim().is_empty() {
                depends_on.insert(name);
            }
        }
    }

    let internal_narrative =
        compose_internal_narrative(&primary, symbols.as_slice(), &internal_dependency_map);

    let external_narrative = compose_external_narrative(
        depended_on_by.len(),
        depended_on_by_layers,
        normalized.as_str(),
    );

    let primary_dependencies = catalog.dependency_ids(primary.id.as_str());
    let primary_dependents = catalog.dependent_ids(primary.id.as_str());

    let mut symbol_entries = symbols
        .iter()
        .map(|symbol| {
            let internal_connections = internal_dependency_map
                .get(symbol.id.as_str())
                .cloned()
                .unwrap_or_default();
            let role = role_in_file(
                symbol,
                &primary,
                primary_dependencies.as_slice(),
                primary_dependents.as_slice(),
                normalized.as_str(),
                &catalog,
            );

            FileSymbolEntry {
                name: symbol.name.clone(),
                kind: symbol.kind.clone(),
                sir_intent: first_sentence_or_fallback(symbol.sir.intent.as_str()),
                centrality: symbol.centrality,
                dependents_count: symbol.dependents_count,
                internal_connections,
                role_in_file: role,
            }
        })
        .collect::<Vec<_>>();

    symbol_entries.sort_by(|left, right| {
        right
            .centrality
            .partial_cmp(&left.centrality)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.dependents_count.cmp(&left.dependents_count))
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(Some(FileDeepDiveData {
        path: normalized,
        layer: layer.name,
        layer_icon: layer.icon,
        symbol_count: symbols.len(),
        summary,
        internal_narrative,
        external_narrative,
        symbols: symbol_entries,
        connections_to_project: FileConnections {
            depended_on_by: depended_on_by.into_iter().collect(),
            depends_on: depends_on.into_iter().collect(),
        },
    }))
}

fn resolve_file_indices(catalog: &SymbolCatalog, normalized: &str) -> Vec<usize> {
    let direct = catalog.symbol_indices_for_file(normalized);
    if !direct.is_empty() {
        return direct;
    }

    let mut fallback = catalog
        .symbols
        .iter()
        .enumerate()
        .filter_map(|(idx, symbol)| {
            let matched = symbol.file_path.ends_with(normalized)
                || normalized.ends_with(symbol.file_path.as_str());
            matched.then_some(idx)
        })
        .collect::<Vec<_>>();
    fallback.sort_unstable();
    fallback.dedup();
    fallback
}

fn compose_summary(path: &str, symbols: &[CatalogSymbol], primary: &CatalogSymbol) -> String {
    let narrative_symbols = symbols.iter().map(to_narrative_symbol).collect::<Vec<_>>();
    let base = compose_file_summary(path, narrative_symbols.as_slice());
    let layer_hint = if primary.layer_name.trim().is_empty() {
        "Core Logic"
    } else {
        primary.layer_name.as_str()
    };
    format!(
        "{base} It sits in the {layer_hint} layer and centers on {} as the main entry point for this file.",
        primary.name
    )
}

fn compose_internal_narrative(
    primary: &CatalogSymbol,
    symbols: &[CatalogSymbol],
    internal_dependency_map: &HashMap<String, Vec<String>>,
) -> String {
    let supporting = symbols
        .iter()
        .filter(|symbol| symbol.id != primary.id)
        .collect::<Vec<_>>();

    let supporting_sentence = match supporting.len() {
        0 => "It is mostly centered around a single primary component.".to_owned(),
        1 => format!(
            "{} supports {} inside the same file.",
            supporting[0].name, primary.name
        ),
        2 | 3 => {
            let names = supporting
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>();
            format!(
                "{} provide supporting behavior around {}.",
                join_human_list(names),
                primary.name
            )
        }
        _ => format!(
            "Supporting types include {} ({}), {} ({}), and {} others.",
            supporting[0].name,
            normalize_kind(supporting[0].kind.as_str()),
            supporting[1].name,
            normalize_kind(supporting[1].kind.as_str()),
            supporting.len().saturating_sub(2)
        ),
    };

    let internal_edges = internal_dependency_map
        .values()
        .map(Vec::len)
        .sum::<usize>();

    let relationship_sentence = if internal_edges == 0 {
        "Most coordination for this file happens through external components.".to_owned()
    } else {
        format!(
            "Inside the file there are {internal_edges} internal dependency links connecting these components."
        )
    };

    format!(
        "The file is organized around {}: {}. {} {}",
        normalize_kind(primary.kind.as_str()),
        primary.name,
        supporting_sentence,
        relationship_sentence
    )
}

fn compose_external_narrative(
    depended_on_by_count: usize,
    depended_on_by_layers: BTreeMap<String, usize>,
    path: &str,
) -> String {
    if depended_on_by_count == 0 {
        return format!(
            "{path} is mostly self-contained and currently has no direct dependent files in the indexed graph."
        );
    }

    let layer_summary = if depended_on_by_layers.is_empty() {
        String::new()
    } else {
        let mut grouped = depended_on_by_layers.into_iter().collect::<Vec<_>>();
        grouped.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        let top = grouped
            .iter()
            .take(3)
            .map(|(layer, count)| format!("{layer} ({count})"))
            .collect::<Vec<_>>();
        format!(" The strongest usage comes from {}.", top.join(", "))
    };

    format!("This file is depended upon by {depended_on_by_count} other files.{layer_summary}")
}

fn role_in_file(
    symbol: &CatalogSymbol,
    primary: &CatalogSymbol,
    primary_dependencies: &[String],
    primary_dependents: &[String],
    file_path: &str,
    catalog: &SymbolCatalog,
) -> String {
    if symbol.id == primary.id {
        return "Primary public interface".to_owned();
    }

    let kind = symbol.kind.to_ascii_lowercase();
    let name = symbol.name.to_ascii_lowercase();
    if kind.contains("test") || name.contains("test") || symbol.layer_name == "Tests" {
        return format!("Test for {}", primary.name);
    }

    if primary_dependencies.contains(&symbol.id) || primary_dependents.contains(&symbol.id) {
        return format!("Supporting type for {}", primary.name);
    }

    let external_dependents = catalog
        .dependent_ids(symbol.id.as_str())
        .into_iter()
        .filter_map(|dep_id| catalog.symbol_by_id(dep_id.as_str()))
        .filter(|dependent| dependent.file_path != file_path)
        .count();
    if external_dependents == 0 {
        return "Internal implementation detail".to_owned();
    }

    "Supporting component in this file".to_owned()
}

fn to_narrative_symbol(symbol: &CatalogSymbol) -> SymbolInfo {
    SymbolInfo {
        id: symbol.id.clone(),
        name: symbol.name.clone(),
        qualified_name: symbol.qualified_name.clone(),
        kind: symbol.kind.clone(),
        file_path: symbol.file_path.clone(),
        sir_intent: symbol.sir.intent.clone(),
        side_effects: symbol.sir.side_effects.clone(),
        dependencies: symbol.sir.dependencies.clone(),
        error_modes: symbol.sir.error_modes.clone(),
        is_async: symbol.sir.is_async,
        layer: symbol.layer_name.clone(),
        dependents_count: symbol.dependents_count,
    }
}

fn first_sentence_or_fallback(intent: &str) -> String {
    let first = crate::api::common::first_sentence(intent);
    if first.trim().is_empty() {
        "No SIR intent summary available".to_owned()
    } else {
        first
    }
}

fn normalize_kind(kind: &str) -> String {
    let lower = kind.trim().to_ascii_lowercase();
    if lower.is_empty() {
        "component".to_owned()
    } else if lower == "fn" {
        "function".to_owned()
    } else {
        lower
    }
}

fn join_human_list(items: Vec<&str>) -> String {
    let values = items
        .into_iter()
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();

    match values.len() {
        0 => String::new(),
        1 => values[0].to_owned(),
        2 => format!("{} and {}", values[0], values[1]),
        _ => {
            let head = values[..values.len() - 1].join(", ");
            format!("{head}, and {}", values[values.len() - 1])
        }
    }
}
