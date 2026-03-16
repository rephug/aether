use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use aether_analysis::{CommunitiesRequest, DriftAnalyzer};
use aether_config::GraphBackend;

use crate::api::common;
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ArchitectureQuery {
    pub granularity: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArchitectureCommunity {
    pub community_id: i64,
    pub label: String,
    pub symbol_count: usize,
    pub misplaced_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArchitectureSymbol {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub directory: String,
    pub community_id: i64,
    pub misplaced: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArchitectureData {
    pub granularity: String,
    pub not_computed: bool,
    pub community_count: usize,
    pub misplaced_count: usize,
    pub communities: Vec<ArchitectureCommunity>,
    pub symbols: Vec<ArchitectureSymbol>,
}

pub(crate) async fn architecture_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<ArchitectureQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    let granularity = query.granularity;
    match support::run_blocking_with_timeout(move || {
        load_architecture_data(shared.as_ref(), granularity.as_deref())
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

pub(crate) fn load_architecture_data(
    shared: &crate::state::SharedState,
    granularity: Option<&str>,
) -> Result<ArchitectureData, String> {
    let granularity = granularity.unwrap_or("symbol").trim().to_ascii_lowercase();
    if shared.config.storage.graph_backend != GraphBackend::Surreal {
        return Ok(not_computed_architecture_data(granularity));
    }

    let analyzer = DriftAnalyzer::new(shared.workspace.as_path()).map_err(|e| e.to_string())?;
    let result = match analyzer.communities(CommunitiesRequest { format: None }) {
        Ok(result) => result,
        Err(_) => return Ok(not_computed_architecture_data(granularity)),
    };

    if result.communities.is_empty() {
        return Ok(not_computed_architecture_data(granularity));
    }

    let symbol_rows = common::load_symbols(shared)?;
    let by_id = symbol_rows
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect::<HashMap<_, _>>();

    let mut dir_community_counts = HashMap::<String, HashMap<i64, usize>>::new();
    for row in &result.communities {
        let source_path = by_id
            .get(row.symbol_id.as_str())
            .map(|value| value.file_path.as_str())
            .unwrap_or(row.file_path.as_str());
        let dir = directory_of(source_path);
        *dir_community_counts
            .entry(dir)
            .or_default()
            .entry(row.community_id)
            .or_insert(0) += 1;
    }

    let mut dominant_by_dir = HashMap::<String, (i64, f64)>::new();
    for (dir, counts) in &dir_community_counts {
        let total = counts.values().copied().sum::<usize>();
        if total == 0 {
            continue;
        }
        if let Some((community_id, count)) = counts.iter().max_by_key(|entry| *entry.1) {
            dominant_by_dir.insert(dir.clone(), (*community_id, *count as f64 / total as f64));
        }
    }

    let mut symbols = Vec::<ArchitectureSymbol>::new();
    for row in &result.communities {
        let source_path = by_id
            .get(row.symbol_id.as_str())
            .map(|value| value.file_path.as_str())
            .unwrap_or(row.file_path.as_str());
        let directory = directory_of(source_path);

        let misplaced = dominant_by_dir
            .get(directory.as_str())
            .is_some_and(|(dominant, ratio)| *ratio > 0.6 && *dominant != row.community_id);

        symbols.push(ArchitectureSymbol {
            symbol_id: row.symbol_id.clone(),
            qualified_name: row.symbol_name.clone(),
            file_path: support::normalized_display_path(source_path),
            directory,
            community_id: row.community_id,
            misplaced,
        });
    }

    symbols.sort_by(|left, right| {
        left.community_id
            .cmp(&right.community_id)
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
    });

    let mut by_community = HashMap::<i64, Vec<&ArchitectureSymbol>>::new();
    for symbol in &symbols {
        by_community
            .entry(symbol.community_id)
            .or_default()
            .push(symbol);
    }

    let mut communities = by_community
        .into_iter()
        .map(|(community_id, members)| {
            let mut dir_counts = HashMap::<String, usize>::new();
            let mut misplaced_count = 0usize;
            for member in &members {
                *dir_counts.entry(member.directory.clone()).or_insert(0) += 1;
                if member.misplaced {
                    misplaced_count += 1;
                }
            }
            let label = dir_counts
                .iter()
                .max_by_key(|entry| *entry.1)
                .map(|entry| entry.0.clone())
                .unwrap_or_else(|| format!("community-{community_id}"));

            ArchitectureCommunity {
                community_id,
                label,
                symbol_count: members.len(),
                misplaced_count,
            }
        })
        .collect::<Vec<_>>();

    communities.sort_by(|left, right| left.community_id.cmp(&right.community_id));

    let misplaced_count = symbols.iter().filter(|symbol| symbol.misplaced).count();

    Ok(ArchitectureData {
        granularity,
        not_computed: false,
        community_count: communities.len(),
        misplaced_count,
        communities,
        symbols,
    })
}

fn not_computed_architecture_data(granularity: String) -> ArchitectureData {
    ArchitectureData {
        granularity,
        not_computed: true,
        community_count: 0,
        misplaced_count: 0,
        communities: Vec::new(),
        symbols: Vec::new(),
    }
}

fn directory_of(file_path: &str) -> String {
    let normalized = support::normalized_display_path(file_path);
    if normalized.is_empty() {
        return "<root>".to_owned();
    }

    let mut parts = normalized.rsplitn(2, '/');
    let _file = parts.next();
    match parts.next() {
        Some(dir) if !dir.trim().is_empty() => dir.to_owned(),
        _ => "<root>".to_owned(),
    }
}
