use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use aether_store::Store;

use crate::api::common;
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SearchQuery {
    pub q: Option<String>,
    pub limit: Option<u32>,
    pub mode: Option<String>,
    pub lang: Option<String>,
    pub risk: Option<String>,
    pub drift: Option<String>,
    pub has_tests: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct RelatedSymbol {
    symbol_id: String,
    qualified_name: String,
}

#[derive(Debug, Serialize)]
struct SearchApiResult {
    symbol_id: String,
    symbol_name: String,
    qualified_name: String,
    kind: String,
    file_path: String,
    language: String,
    sir_exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sir_excerpt: Option<String>,
    sir_summary: Option<String>,
    risk_score: Option<f64>,
    pagerank: Option<f64>,
    drift_score: Option<f64>,
    test_count: Option<i64>,
    related_symbols: Vec<RelatedSymbol>,
}

#[derive(Debug, Serialize)]
struct SearchApiData {
    mode: String,
    mode_not_computed: bool,
    results: Vec<SearchApiResult>,
    count: usize,
}

pub(crate) async fn search_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
    let q = query.q.unwrap_or_default();
    let requested_mode = query
        .mode
        .unwrap_or_else(|| "hybrid".to_owned())
        .trim()
        .to_ascii_lowercase();
    let mode = match requested_mode.as_str() {
        "lexical" | "semantic" | "hybrid" => requested_mode,
        _ => "hybrid".to_owned(),
    };

    let limit = query.limit.unwrap_or(20).clamp(1, 100);

    // Lexical fallback for all modes at this stage.
    let mode_not_computed = mode != "lexical";
    let results = match state.shared.store.search_symbols(&q, limit) {
        Ok(records) => records,
        Err(err) => return support::json_internal_error(err.to_string()),
    };

    let edges = match common::load_dependency_algo_edges(state.shared.as_ref()) {
        Ok(rows) => rows,
        Err(err) => return support::json_internal_error(err),
    };
    let pagerank = common::pagerank_map(state.shared.as_ref(), &edges).await;
    let drift_map = common::latest_drift_score_by_symbol(state.shared.as_ref()).unwrap_or_default();
    let test_map = common::test_count_by_symbol(state.shared.as_ref()).unwrap_or_default();
    let sir_symbols = common::symbols_with_sir(state.shared.as_ref()).unwrap_or_default();
    let max_pagerank = pagerank.values().copied().fold(0.0f64, f64::max);

    let symbol_records = common::load_symbols(state.shared.as_ref())
        .unwrap_or_default()
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect::<HashMap<_, _>>();

    let mut mapped = Vec::new();
    for row in results {
        if let Some(lang) = query.lang.as_deref().map(str::trim)
            && !lang.is_empty()
            && !row.language.eq_ignore_ascii_case(lang)
        {
            continue;
        }

        let symbol_id = row.symbol_id.clone();
        let pr = pagerank.get(symbol_id.as_str()).copied().unwrap_or(0.0);
        let drift = drift_map.get(symbol_id.as_str()).copied().unwrap_or(0.0);
        let tests = test_map.get(symbol_id.as_str()).copied().unwrap_or(0);
        let has_sir = sir_symbols.contains(symbol_id.as_str());
        let risk = common::risk_score(pr, drift, has_sir, tests, max_pagerank);

        if let Some(flag) = query.has_tests {
            if flag && tests <= 0 {
                continue;
            }
            if !flag && tests > 0 {
                continue;
            }
        }

        if let Some(drift_filter) = query.drift.as_deref().map(str::trim) {
            match drift_filter {
                "drifting" if drift < 0.15 => continue,
                "stable" if drift >= 0.15 => continue,
                _ => {}
            }
        }

        if let Some(risk_filter) = query.risk.as_deref().map(str::trim) {
            match risk_filter {
                "high" if risk < 0.7 => continue,
                "medium" if !(0.4..0.7).contains(&risk) => continue,
                "low" if risk >= 0.4 => continue,
                _ => {}
            }
        }

        let qualified_name = symbol_records
            .get(symbol_id.as_str())
            .map(|value| value.qualified_name.clone())
            .unwrap_or_else(|| row.qualified_name.clone());

        let related = related_symbols(
            state.shared.as_ref(),
            symbol_id.as_str(),
            qualified_name.as_str(),
            &pagerank,
        )
        .await;

        let sir_excerpt =
            support::sir_excerpt_for_symbol(state.shared.as_ref(), symbol_id.as_str());
        let sir_summary = sir_excerpt
            .as_deref()
            .map(common::first_sentence)
            .filter(|value| !value.is_empty());

        mapped.push(SearchApiResult {
            symbol_id,
            symbol_name: support::symbol_name_from_qualified(&qualified_name),
            qualified_name,
            kind: row.kind,
            file_path: support::normalized_display_path(&row.file_path),
            language: row.language,
            sir_exists: has_sir,
            sir_excerpt,
            sir_summary,
            risk_score: Some(risk),
            pagerank: Some(pr),
            drift_score: Some(drift),
            test_count: Some(tests),
            related_symbols: related,
        });
    }

    mapped.sort_by(|left, right| {
        right
            .risk_score
            .unwrap_or(0.0)
            .partial_cmp(&left.risk_score.unwrap_or(0.0))
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                right
                    .pagerank
                    .unwrap_or(0.0)
                    .partial_cmp(&left.pagerank.unwrap_or(0.0))
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
    });

    support::api_json(
        state.shared.as_ref(),
        SearchApiData {
            mode,
            mode_not_computed,
            count: mapped.len(),
            results: mapped,
        },
    )
    .into_response()
}

async fn related_symbols(
    shared: &crate::state::SharedState,
    symbol_id: &str,
    qualified_name: &str,
    pagerank: &HashMap<String, f64>,
) -> Vec<RelatedSymbol> {
    let mut candidates = Vec::<(String, String, f64)>::new();

    for dep in shared
        .graph
        .get_dependencies(symbol_id)
        .await
        .unwrap_or_default()
    {
        let score = pagerank.get(dep.id.as_str()).copied().unwrap_or(0.0);
        candidates.push((dep.id, dep.qualified_name, score));
    }
    for caller in shared
        .graph
        .get_callers(qualified_name)
        .await
        .unwrap_or_default()
    {
        let score = pagerank.get(caller.id.as_str()).copied().unwrap_or(0.0);
        candidates.push((caller.id, caller.qualified_name, score));
    }

    candidates.sort_by(|left, right| {
        right
            .2
            .partial_cmp(&left.2)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.1.cmp(&right.1))
    });
    candidates.dedup_by(|left, right| left.0 == right.0);

    candidates
        .into_iter()
        .take(3)
        .map(|(symbol_id, qualified_name, _)| RelatedSymbol {
            symbol_id,
            qualified_name,
        })
        .collect()
}
