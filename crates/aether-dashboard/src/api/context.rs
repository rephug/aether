use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::catalog::{
    CatalogSymbol, SymbolCatalog, load_symbol_catalog, parse_dependency_entry,
};
use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ContextFileItem {
    pub file: String,
    pub symbols: Vec<String>,
    pub reason: String,
    pub estimated_lines: usize,
    pub priority: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ContextNotNeededGroup {
    pub reason: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ContextData {
    pub symbol: String,
    pub context_type: String,
    pub required: Vec<ContextFileItem>,
    pub helpful_but_optional: Vec<ContextFileItem>,
    pub not_needed: Vec<ContextNotNeededGroup>,
    pub total_required_lines: usize,
    pub total_with_optional_lines: usize,
    pub full_codebase_lines: usize,
    pub context_reduction: String,
    pub teaching_note: String,
}

pub(crate) async fn context_handler(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Response {
    let shared = state.shared.clone();
    let selector_for_build = selector.clone();
    match support::run_blocking_with_timeout(move || {
        build_context_data(shared.as_ref(), selector_for_build.as_str())
    })
    .await
    {
        Ok(Some(data)) => support::api_json(state.shared.as_ref(), data).into_response(),
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

pub(crate) fn build_context_data(
    shared: &SharedState,
    selector: &str,
) -> Result<Option<ContextData>, String> {
    let catalog = load_symbol_catalog(shared)?;
    let Some(resolved) = catalog.resolve_symbol_selector(selector) else {
        return Ok(None);
    };
    let Some(symbol) = catalog.symbol(resolved.primary_index) else {
        return Ok(None);
    };

    let dependency_reason_map = dependency_reason_map(symbol);

    let mut required = Vec::<ContextFileItem>::new();
    let mut seen_required = BTreeSet::<String>::new();
    for dep_id in catalog.dependency_ids(symbol.id.as_str()) {
        let Some(dep) = catalog.symbol_by_id(dep_id.as_str()) else {
            continue;
        };
        let file_key = dep.file_path.clone();
        if !seen_required.insert(file_key.clone()) {
            continue;
        }

        let reason = dependency_reason_map
            .get(dep.name.to_ascii_lowercase().as_str())
            .cloned()
            .unwrap_or_else(|| {
                format!(
                    "{} must integrate with {}'s public interface",
                    symbol.name, dep.name
                )
            });

        required.push(ContextFileItem {
            file: dep.file_path.clone(),
            symbols: vec![dep.name.clone()],
            reason,
            estimated_lines: estimate_lines(dep),
            priority: "essential".to_owned(),
        });
    }

    required.sort_by(|left, right| left.file.cmp(&right.file));

    let optional = choose_optional_peer(&catalog, symbol)
        .map(|peer| ContextFileItem {
            file: peer.file_path.clone(),
            symbols: vec![peer.name.clone()],
            reason: format!(
                "{} is a similar {} in the {} layer and is a useful implementation reference",
                peer.name, peer.kind, peer.layer_name
            ),
            estimated_lines: estimate_lines(peer),
            priority: "helpful".to_owned(),
        })
        .into_iter()
        .collect::<Vec<_>>();

    let mut not_needed = Vec::<ContextNotNeededGroup>::new();

    let other_same_layer = catalog
        .symbols
        .iter()
        .filter(|candidate| {
            candidate.id != symbol.id
                && candidate.layer_name == symbol.layer_name
                && !required
                    .iter()
                    .any(|entry| entry.file == candidate.file_path)
        })
        .map(|candidate| candidate.file_path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(6)
        .collect::<Vec<_>>();
    if !other_same_layer.is_empty() {
        not_needed.push(ContextNotNeededGroup {
            reason: "Other components in the same layer are useful examples but add noise to the generation prompt".to_owned(),
            files: other_same_layer,
        });
    }

    let test_files = catalog
        .symbols
        .iter()
        .filter(|candidate| {
            candidate.layer_name == "Tests"
                || candidate.file_path.to_ascii_lowercase().contains("/tests/")
                || candidate
                    .file_path
                    .to_ascii_lowercase()
                    .starts_with("tests/")
        })
        .map(|candidate| candidate.file_path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(6)
        .collect::<Vec<_>>();
    if !test_files.is_empty() {
        not_needed.push(ContextNotNeededGroup {
            reason: "Test files are useful for verification later but are usually not required for initial generation".to_owned(),
            files: test_files,
        });
    }

    let internal_impl_files = catalog
        .symbols
        .iter()
        .filter(|candidate| {
            candidate.file_path == symbol.file_path
                && candidate.id != symbol.id
                && !required
                    .iter()
                    .any(|entry| entry.file == candidate.file_path)
        })
        .map(|candidate| candidate.file_path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(3)
        .collect::<Vec<_>>();
    if !internal_impl_files.is_empty() {
        not_needed.push(ContextNotNeededGroup {
            reason:
                "Internal implementation details outside direct dependencies can dilute the prompt"
                    .to_owned(),
            files: internal_impl_files,
        });
    }

    let total_required_lines = required
        .iter()
        .map(|item| item.estimated_lines)
        .sum::<usize>();
    let total_with_optional_lines = total_required_lines
        + optional
            .iter()
            .map(|item| item.estimated_lines)
            .sum::<usize>();

    let full_codebase_lines = estimate_project_lines(shared.workspace.as_path(), &catalog);
    let reduction_pct = if full_codebase_lines > 0 {
        (((full_codebase_lines.saturating_sub(total_required_lines)) as f64
            / full_codebase_lines as f64)
            * 100.0)
            .round() as usize
    } else {
        0
    };

    let teaching_note = format!(
        "Context selection is a core prompting skill. The {} lines identified here contain the direct interfaces needed to work on {}. The remaining {} lines are mostly surrounding detail that can dilute the signal. Include direct dependencies first, then only add optional examples if needed.",
        total_required_lines,
        symbol.name,
        full_codebase_lines.saturating_sub(total_required_lines)
    );

    Ok(Some(ContextData {
        symbol: symbol.name.clone(),
        context_type: "generation".to_owned(),
        required,
        helpful_but_optional: optional,
        not_needed,
        total_required_lines,
        total_with_optional_lines,
        full_codebase_lines,
        context_reduction: format!("{}% smaller than providing everything", reduction_pct),
        teaching_note,
    }))
}

fn dependency_reason_map(symbol: &CatalogSymbol) -> BTreeMap<String, String> {
    let mut out = BTreeMap::<String, String>::new();
    for dep in &symbol.sir.dependencies {
        let (name, reason) = parse_dependency_entry(dep.as_str());
        if name.trim().is_empty() {
            continue;
        }
        let reason = reason.unwrap_or_else(|| format!("{} is a direct dependency", name));
        out.insert(name.to_ascii_lowercase(), reason);
    }
    out
}

fn choose_optional_peer<'a>(
    catalog: &'a SymbolCatalog,
    symbol: &CatalogSymbol,
) -> Option<&'a CatalogSymbol> {
    let mut candidates = catalog
        .symbols
        .iter()
        .filter(|candidate| {
            candidate.id != symbol.id
                && candidate.kind.eq_ignore_ascii_case(symbol.kind.as_str())
                && candidate
                    .layer_name
                    .eq_ignore_ascii_case(symbol.layer_name.as_str())
                && candidate.file_path != symbol.file_path
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        right
            .centrality
            .partial_cmp(&left.centrality)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.name.cmp(&right.name))
    });
    candidates.into_iter().next()
}

fn estimate_lines(symbol: &CatalogSymbol) -> usize {
    let kind = symbol.kind.to_ascii_lowercase();
    if kind.contains("trait") {
        16
    } else if kind.contains("struct") || kind.contains("enum") {
        10
    } else if kind.contains("fn") || kind.contains("function") || kind.contains("method") {
        if symbol.sir.error_modes.len() + symbol.sir.side_effects.len() >= 3 {
            8
        } else {
            4
        }
    } else {
        6
    }
}

fn estimate_project_lines(workspace: &std::path::Path, catalog: &SymbolCatalog) -> usize {
    let mut total = 0usize;
    let mut seen = BTreeSet::<String>::new();

    for symbol in &catalog.symbols {
        if !seen.insert(symbol.file_path.clone()) {
            continue;
        }
        let path = workspace.join(symbol.file_path.as_str());
        let lines = std::fs::read_to_string(path)
            .map(|content| content.lines().count())
            .unwrap_or(0);
        total = total.saturating_add(lines);
    }

    total
}
