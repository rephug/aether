use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use aether_store::Store;

use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SearchQuery {
    pub q: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct SearchApiResult {
    symbol_id: String,
    symbol_name: String,
    kind: String,
    file_path: String,
    sir_exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sir_excerpt: Option<String>,
}

#[derive(Debug, Serialize)]
struct SearchApiData {
    results: Vec<SearchApiResult>,
    count: usize,
}

pub(crate) async fn search_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
    let q = query.q.unwrap_or_default();
    let limit = query.limit.unwrap_or(20).clamp(1, 100);

    let results = match state.shared.store.search_symbols(&q, limit) {
        Ok(records) => records,
        Err(err) => return support::json_internal_error(err.to_string()),
    };

    let mapped = results
        .into_iter()
        .map(|row| SearchApiResult {
            symbol_id: row.symbol_id.clone(),
            symbol_name: support::symbol_name_from_qualified(&row.qualified_name),
            kind: row.kind,
            file_path: support::normalized_display_path(&row.file_path),
            sir_exists: support::sir_exists_for_symbol(state.shared.as_ref(), &row.symbol_id),
            sir_excerpt: support::sir_excerpt_for_symbol(state.shared.as_ref(), &row.symbol_id),
        })
        .collect::<Vec<_>>();

    support::api_json(
        state.shared.as_ref(),
        SearchApiData {
            count: mapped.len(),
            results: mapped,
        },
    )
    .into_response()
}
