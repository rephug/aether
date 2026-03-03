use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::api::catalog::{SymbolCatalog, load_symbol_catalog};
use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_PAGE: usize = 1;
const DEFAULT_PER_PAGE: usize = 50;
const MAX_PER_PAGE: usize = 200;

#[derive(Debug, Default, Clone, Deserialize)]
pub(crate) struct GlossaryQuery {
    pub search: Option<String>,
    pub layer: Option<String>,
    pub kind: Option<String>,
    pub page: Option<usize>,
    pub per_page: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GlossaryTerm {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub layer: String,
    pub layer_icon: String,
    pub definition: String,
    pub related: Vec<String>,
    pub dependents_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GlossaryData {
    pub terms: Vec<GlossaryTerm>,
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
}

pub(crate) async fn glossary_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<GlossaryQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || build_glossary_data(shared.as_ref(), &query))
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

pub(crate) fn build_glossary_data(
    shared: &SharedState,
    query: &GlossaryQuery,
) -> Result<GlossaryData, String> {
    let catalog = load_symbol_catalog(shared)?;
    Ok(build_glossary_from_catalog(&catalog, query))
}

pub(crate) fn build_glossary_from_catalog(
    catalog: &SymbolCatalog,
    query: &GlossaryQuery,
) -> GlossaryData {
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());

    let layer_filter = query
        .layer
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let kind_filter = query
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());

    let mut terms = catalog
        .symbols
        .iter()
        .map(|symbol| {
            let definition = first_sentence_or_fallback(symbol.sir.intent.as_str());
            GlossaryTerm {
                name: symbol.name.clone(),
                kind: symbol.kind.clone(),
                file: symbol.file_path.clone(),
                layer: symbol.layer_name.clone(),
                layer_icon: symbol.layer_icon.clone(),
                definition,
                related: related_terms(catalog, symbol.id.as_str(), 4),
                dependents_count: symbol.dependents_count,
            }
        })
        .filter(|term| {
            if let Some(layer) = layer_filter.as_deref()
                && !term.layer.eq_ignore_ascii_case(layer)
            {
                return false;
            }

            if let Some(kind) = kind_filter.as_deref()
                && term.kind.to_ascii_lowercase() != kind
            {
                return false;
            }

            if let Some(search) = search.as_deref() {
                let searchable = format!(
                    "{}\n{}\n{}\n{}",
                    term.name, term.kind, term.file, term.definition
                )
                .to_ascii_lowercase();
                return searchable.contains(search);
            }

            true
        })
        .collect::<Vec<_>>();

    terms.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.kind.cmp(&right.kind))
    });

    let total = terms.len();
    let per_page = query
        .per_page
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = query.page.unwrap_or(DEFAULT_PAGE).max(1);

    let start = per_page.saturating_mul(page.saturating_sub(1));
    let end = start.saturating_add(per_page).min(total);
    let paged_terms = if start >= total {
        Vec::new()
    } else {
        terms[start..end].to_vec()
    };

    GlossaryData {
        terms: paged_terms,
        total,
        page,
        per_page,
    }
}

fn related_terms(catalog: &SymbolCatalog, symbol_id: &str, limit: usize) -> Vec<String> {
    let mut related_ids = catalog.dependency_ids(symbol_id);
    related_ids.extend(catalog.dependent_ids(symbol_id));
    related_ids.sort();
    related_ids.dedup();

    let mut names = related_ids
        .into_iter()
        .filter(|id| id != symbol_id)
        .filter_map(|id| catalog.symbol_by_id(id.as_str()))
        .map(|symbol| {
            (
                symbol.name.clone(),
                symbol.centrality,
                symbol.qualified_name.clone(),
            )
        })
        .collect::<Vec<_>>();

    names.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.2.cmp(&right.2))
    });

    let mut dedup = HashSet::<String>::new();
    names
        .into_iter()
        .filter_map(|(name, _, _)| dedup.insert(name.clone()).then_some(name))
        .take(limit)
        .collect()
}

fn first_sentence_or_fallback(intent: &str) -> String {
    let first = crate::api::common::first_sentence(intent);
    if first.trim().is_empty() {
        "No SIR intent summary available".to_owned()
    } else {
        first
    }
}
