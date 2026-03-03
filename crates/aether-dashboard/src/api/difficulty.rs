use std::cmp::Ordering;
use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::api::catalog::{CatalogSymbol, SymbolCatalog, load_symbol_catalog};
use crate::narrative::compute_difficulty_from_fields;
use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DifficultyView {
    pub score: f64,
    pub emoji: String,
    pub label: String,
    pub guidance: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DifficultyBucketSummary {
    pub count: usize,
    pub percentage: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DifficultySummary {
    pub easy: DifficultyBucketSummary,
    pub moderate: DifficultyBucketSummary,
    pub hard: DifficultyBucketSummary,
    pub very_hard: DifficultyBucketSummary,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DifficultySymbolEntry {
    pub name: String,
    pub difficulty: DifficultyView,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DifficultyData {
    pub summary: DifficultySummary,
    pub symbols: Vec<DifficultySymbolEntry>,
}

pub(crate) async fn difficulty_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || build_difficulty_data(shared.as_ref())).await {
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

pub(crate) fn build_difficulty_data(shared: &SharedState) -> Result<DifficultyData, String> {
    let catalog = load_symbol_catalog(shared)?;
    Ok(build_difficulty_from_catalog(&catalog))
}

pub(crate) fn build_difficulty_from_catalog(catalog: &SymbolCatalog) -> DifficultyData {
    let mut symbols = catalog
        .symbols
        .iter()
        .map(|symbol| DifficultySymbolEntry {
            name: symbol.name.clone(),
            difficulty: difficulty_for_symbol(symbol),
        })
        .collect::<Vec<_>>();

    symbols.sort_by(|left, right| {
        right
            .difficulty
            .score
            .partial_cmp(&left.difficulty.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.name.cmp(&right.name))
    });

    let summary = summarize_symbols(symbols.as_slice());

    DifficultyData { summary, symbols }
}

pub(crate) fn difficulty_for_symbol(symbol: &CatalogSymbol) -> DifficultyView {
    let score = compute_difficulty_from_fields(
        symbol.sir.intent.as_str(),
        symbol.sir.error_modes.len(),
        symbol.sir.side_effects.len(),
        symbol.sir.dependencies.len(),
        symbol.sir.is_async,
    );

    DifficultyView {
        score: round_difficulty_score(score.score),
        emoji: score.emoji,
        label: score.label,
        guidance: score.guidance,
        reasons: score.reasons,
    }
}

pub(crate) fn summarize_catalog(catalog: &SymbolCatalog) -> DifficultySummary {
    let symbols = catalog
        .symbols
        .iter()
        .map(|symbol| DifficultySymbolEntry {
            name: symbol.name.clone(),
            difficulty: difficulty_for_symbol(symbol),
        })
        .collect::<Vec<_>>();
    summarize_symbols(symbols.as_slice())
}

pub(crate) fn summarize_symbols(symbols: &[DifficultySymbolEntry]) -> DifficultySummary {
    let total = symbols.len().max(1);

    let easy = symbols
        .iter()
        .filter(|symbol| symbol.difficulty.label == "Easy")
        .count();
    let moderate = symbols
        .iter()
        .filter(|symbol| symbol.difficulty.label == "Moderate")
        .count();
    let hard = symbols
        .iter()
        .filter(|symbol| symbol.difficulty.label == "Hard")
        .count();
    let very_hard = symbols
        .iter()
        .filter(|symbol| symbol.difficulty.label == "Very Hard")
        .count();

    DifficultySummary {
        easy: DifficultyBucketSummary {
            count: easy,
            percentage: ((easy as f64 / total as f64) * 100.0).round() as usize,
        },
        moderate: DifficultyBucketSummary {
            count: moderate,
            percentage: ((moderate as f64 / total as f64) * 100.0).round() as usize,
        },
        hard: DifficultyBucketSummary {
            count: hard,
            percentage: ((hard as f64 / total as f64) * 100.0).round() as usize,
        },
        very_hard: DifficultyBucketSummary {
            count: very_hard,
            percentage: ((very_hard as f64 / total as f64) * 100.0).round() as usize,
        },
    }
}

fn round_difficulty_score(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}
