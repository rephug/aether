use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::catalog::{
    CatalogSymbol, SymbolCatalog, load_symbol_catalog, parse_dependency_entry,
};
use crate::narrative::{
    Dependency, Dependent, LayerMap, compose_dependencies_narrative, compose_dependents_narrative,
};
use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DependentsByLayer {
    pub layer: String,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DependentsSection {
    pub count: usize,
    pub narrative: String,
    pub by_layer: Vec<DependentsByLayer>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DependencyItem {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DependenciesSection {
    pub count: usize,
    pub narrative: String,
    pub items: Vec<DependencyItem>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct NarrativeListSection {
    pub narrative: String,
    pub items: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BlastRadiusSection {
    pub risk_level: String,
    pub narrative: String,
    pub affected_files: usize,
    pub affected_symbols: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SymbolAlternative {
    pub name: String,
    pub qualified_name: String,
    pub file: String,
    pub layer: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SymbolDeepDiveData {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub layer: String,
    pub layer_icon: String,

    pub role: String,
    pub context: String,
    pub creation_narrative: String,

    pub dependents: DependentsSection,
    pub dependencies: DependenciesSection,

    pub side_effects: NarrativeListSection,
    pub error_modes: NarrativeListSection,
    pub blast_radius: BlastRadiusSection,

    pub centrality: f64,
    pub centrality_rank: usize,
    pub centrality_narrative: String,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<SymbolAlternative>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub matched_by: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SymbolDeepDiveBuild {
    pub data: SymbolDeepDiveData,
}

pub(crate) async fn symbol_handler(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Response {
    let shared = state.shared.clone();
    let selector_for_build = selector.clone();
    match support::run_blocking_with_timeout(move || {
        build_symbol_deep_dive(shared.as_ref(), selector_for_build.as_str())
    })
    .await
    {
        Ok(Some(build)) => support::api_json(state.shared.as_ref(), build.data).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "not_found",
                "message": format!("symbol '{}' not found", selector)
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

pub(crate) fn build_symbol_deep_dive(
    shared: &SharedState,
    selector: &str,
) -> Result<Option<SymbolDeepDiveBuild>, String> {
    let catalog = load_symbol_catalog(shared)?;
    let Some(resolved) = catalog.resolve_symbol_selector(selector) else {
        return Ok(None);
    };

    let Some(symbol) = catalog.symbol(resolved.primary_index).cloned() else {
        return Ok(None);
    };

    let role = first_sentence_or_fallback(symbol.sir.intent.as_str());
    let project_name = shared
        .workspace
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("project");

    let rank = catalog.centrality_rank(symbol.id.as_str());
    let total = catalog.symbols.len().max(1);
    let percentile = rank as f64 / total as f64;

    let context = if percentile <= 0.10 {
        format!(
            "{} is the heart of {project_name} - it sits in the {} layer and is one of the most depended-upon components in the project.",
            symbol.name, symbol.layer_name
        )
    } else if percentile <= 0.50 {
        format!(
            "{} is an important {} in the {} layer. {}",
            symbol.name,
            normalize_kind(symbol.kind.as_str()),
            symbol.layer_name,
            role
        )
    } else {
        format!(
            "{} is a {} in the {} layer that {}",
            symbol.name,
            normalize_kind(symbol.kind.as_str()),
            symbol.layer_name,
            lowercase_sentence(role.as_str())
        )
    };

    let creation_narrative = creation_narrative(&catalog, &symbol);

    let dependent_ids = catalog.dependent_ids(symbol.id.as_str());
    let mut dependents = dependent_ids
        .into_iter()
        .filter_map(|id| catalog.symbol_by_id(id.as_str()))
        .map(|dep_symbol| Dependent {
            name: dep_symbol.name.clone(),
            layer: dep_symbol.layer_name.clone(),
        })
        .collect::<Vec<_>>();
    dependents.sort_by(|left, right| left.name.cmp(&right.name));

    let mut layer_map = LayerMap::new();
    for dep in &dependents {
        layer_map.insert(dep.name.clone(), dep.layer.clone());
    }

    let dependents_narrative =
        compose_dependents_narrative(symbol.name.as_str(), dependents.as_slice(), &layer_map);

    let mut dependents_by_layer = BTreeMap::<String, Vec<String>>::new();
    for dep in &dependents {
        dependents_by_layer
            .entry(dep.layer.clone())
            .or_default()
            .push(dep.name.clone());
    }

    let dependents_by_layer = dependents_by_layer
        .into_iter()
        .map(|(layer, mut symbols)| {
            symbols.sort();
            symbols.dedup();
            DependentsByLayer { layer, symbols }
        })
        .collect::<Vec<_>>();

    let dependency_items = dependency_items(&catalog, &symbol);
    let dependencies_for_narrative = dependency_items
        .iter()
        .map(|item| Dependency {
            name: item.name.clone(),
            reason: item.reason.clone(),
        })
        .collect::<Vec<_>>();

    let dependencies_narrative =
        compose_dependencies_narrative(symbol.name.as_str(), dependencies_for_narrative.as_slice());

    let side_effects = narrative_list(
        symbol.name.as_str(),
        symbol.sir.side_effects.as_slice(),
        "side effect",
        "This is a pure component with no side effects.",
    );

    let error_modes = narrative_list(
        symbol.name.as_str(),
        symbol.sir.error_modes.as_slice(),
        "failure mode",
        "No documented failure modes.",
    );

    let blast_radius = blast_radius(&catalog, &symbol);

    let centrality_narrative = if percentile <= 0.10 {
        format!(
            "{} is one of the most central components in this project (rank {} of {}).",
            symbol.name, rank, total
        )
    } else if percentile <= 0.25 {
        format!(
            "{} is highly central in this project (rank {} of {}).",
            symbol.name, rank, total
        )
    } else if percentile <= 0.50 {
        format!(
            "{} is moderately connected in this project (rank {} of {}).",
            symbol.name, rank, total
        )
    } else {
        format!(
            "{} is relatively independent in this project (rank {} of {}).",
            symbol.name, rank, total
        )
    };

    let alternatives = resolved
        .alternatives
        .iter()
        .filter_map(|idx| catalog.symbol(*idx))
        .map(|alt| SymbolAlternative {
            name: alt.name.clone(),
            qualified_name: alt.qualified_name.clone(),
            file: alt.file_path.clone(),
            layer: alt.layer_name.clone(),
        })
        .collect::<Vec<_>>();

    let data = SymbolDeepDiveData {
        name: symbol.name,
        kind: symbol.kind,
        file: symbol.file_path,
        layer: symbol.layer_name,
        layer_icon: symbol.layer_icon,
        role,
        context,
        creation_narrative,
        dependents: DependentsSection {
            count: dependents.len(),
            narrative: dependents_narrative,
            by_layer: dependents_by_layer,
        },
        dependencies: DependenciesSection {
            count: dependency_items.len(),
            narrative: dependencies_narrative,
            items: dependency_items,
        },
        side_effects,
        error_modes,
        blast_radius,
        centrality: catalog.centrality(symbol.id.as_str()),
        centrality_rank: rank,
        centrality_narrative,
        alternatives,
        matched_by: resolved.matched_by.to_owned(),
    };

    Ok(Some(SymbolDeepDiveBuild { data }))
}

fn dependency_items(catalog: &SymbolCatalog, symbol: &CatalogSymbol) -> Vec<DependencyItem> {
    let mut parsed_dependency_reasons = HashMap::<String, Option<String>>::new();
    for entry in &symbol.sir.dependencies {
        let (name, reason) = parse_dependency_entry(entry.as_str());
        if name.trim().is_empty() {
            continue;
        }
        parsed_dependency_reasons
            .entry(name.to_ascii_lowercase())
            .or_insert(reason);
    }

    let mut items = Vec::<DependencyItem>::new();
    let mut seen = HashSet::<String>::new();

    for dep_id in catalog.dependency_ids(symbol.id.as_str()) {
        let Some(dep_symbol) = catalog.symbol_by_id(dep_id.as_str()) else {
            continue;
        };

        let key = dep_symbol.name.to_ascii_lowercase();
        if !seen.insert(key.clone()) {
            continue;
        }

        let reason = parsed_dependency_reasons
            .get(key.as_str())
            .cloned()
            .flatten()
            .or_else(|| {
                parsed_dependency_reasons
                    .iter()
                    .find(|(name, _)| {
                        dep_symbol
                            .qualified_name
                            .to_ascii_lowercase()
                            .contains(name.as_str())
                    })
                    .and_then(|(_, reason)| reason.clone())
            });

        items.push(DependencyItem {
            name: dep_symbol.name.clone(),
            reason,
        });
    }

    for entry in &symbol.sir.dependencies {
        let (name, reason) = parse_dependency_entry(entry.as_str());
        let key = name.to_ascii_lowercase();
        if name.trim().is_empty() || !seen.insert(key) {
            continue;
        }

        items.push(DependencyItem { name, reason });
    }

    items.sort_by(|left, right| left.name.cmp(&right.name));
    items
}

fn narrative_list(
    symbol_name: &str,
    items: &[String],
    singular_label: &str,
    empty_message: &str,
) -> NarrativeListSection {
    let narrative = match items.len() {
        0 => empty_message.to_owned(),
        1..=2 => format!(
            "{symbol_name} has a {singular_label} to be aware of: {}.",
            items.join(", ")
        ),
        _ => format!(
            "{symbol_name} has significant {}s to be aware of: {}.",
            singular_label,
            join_human_list(items.iter().map(String::as_str).collect())
        ),
    };

    NarrativeListSection {
        narrative,
        items: items.to_vec(),
    }
}

fn blast_radius(catalog: &SymbolCatalog, symbol: &CatalogSymbol) -> BlastRadiusSection {
    let mut visited = HashSet::<String>::new();
    let mut queue = VecDeque::<String>::new();

    visited.insert(symbol.id.clone());
    queue.push_back(symbol.id.clone());

    let mut affected = HashSet::<String>::new();

    while let Some(current_id) = queue.pop_front() {
        for dependent_id in catalog.dependent_ids(current_id.as_str()) {
            if !visited.insert(dependent_id.clone()) {
                continue;
            }
            affected.insert(dependent_id.clone());
            queue.push_back(dependent_id);
        }
    }

    let affected_files = affected
        .iter()
        .filter_map(|symbol_id| catalog.symbol_by_id(symbol_id.as_str()))
        .map(|symbol| symbol.file_path.clone())
        .collect::<HashSet<_>>()
        .len();

    let affected_symbols = affected.len();

    let mut layer_counts = HashMap::<String, usize>::new();
    for symbol_id in &affected {
        if let Some(affected_symbol) = catalog.symbol_by_id(symbol_id.as_str()) {
            *layer_counts
                .entry(affected_symbol.layer_name.clone())
                .or_insert(0) += 1;
        }
    }

    let mut sorted_layers = layer_counts.into_iter().collect::<Vec<_>>();
    sorted_layers.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let area_text = if sorted_layers.is_empty() {
        String::new()
    } else {
        let areas = sorted_layers
            .iter()
            .take(2)
            .map(|(layer, count)| format!("{layer} ({count})"))
            .collect::<Vec<_>>();
        format!(" Highest-risk areas: {}.", areas.join(", "))
    };

    let (risk_level, narrative) = match affected_symbols {
        0..=2 => (
            "Low",
            format!(
                "Changes to {} are well-contained with {} directly affected component(s).{}",
                symbol.name, affected_symbols, area_text
            ),
        ),
        3..=8 => (
            "Medium",
            format!(
                "Changes to {} affect {} components across {} files.{}",
                symbol.name, affected_symbols, affected_files, area_text
            ),
        ),
        _ => (
            "High",
            format!(
                "Changes to {} ripple across {} components in {} files.{}",
                symbol.name, affected_symbols, affected_files, area_text
            ),
        ),
    };

    BlastRadiusSection {
        risk_level: risk_level.to_owned(),
        narrative,
        affected_files,
        affected_symbols,
    }
}

fn creation_narrative(catalog: &SymbolCatalog, symbol: &CatalogSymbol) -> String {
    let mut callers = catalog
        .dependent_ids(symbol.id.as_str())
        .into_iter()
        .filter_map(|id| catalog.symbol_by_id(id.as_str()))
        .collect::<Vec<_>>();

    callers.sort_by(|left, right| {
        right
            .centrality
            .partial_cmp(&left.centrality)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
    });

    if callers.is_empty() {
        return format!(
            "{} is a foundational component with no upstream callers in the indexed dependency graph.",
            symbol.name
        );
    }

    let primary = callers[0];
    let usage_hint = if callers.len() > 1 {
        format!(
            "It is also referenced by {} additional upstream component(s).",
            callers.len() - 1
        )
    } else {
        "It is used directly in the surrounding control flow.".to_owned()
    };

    format!(
        "The {} layer calls {} in {}. {}",
        primary.layer_name, symbol.name, primary.file_path, usage_hint
    )
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
    match lower.as_str() {
        "fn" => "function".to_owned(),
        "function" => "function".to_owned(),
        "method" => "method".to_owned(),
        "trait" => "trait".to_owned(),
        "enum" => "enum".to_owned(),
        "struct" => "struct".to_owned(),
        _ => {
            if lower.is_empty() {
                "component".to_owned()
            } else {
                lower
            }
        }
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
