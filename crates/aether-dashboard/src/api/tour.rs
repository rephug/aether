use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::api::catalog::{CatalogSymbol, SymbolCatalog, load_symbol_catalog};
use crate::narrative::{SymbolInfo, compose_file_summary};
use crate::state::SharedState;
use crate::support::{self, DashboardState};

const ALL_STOP_TITLES: [&str; 8] = [
    "The Front Door",
    "What It Accepts",
    "How It Thinks",
    "Where It Stores Things",
    "How It Talks",
    "How It Handles Problems",
    "How It Gets Tested",
    "The Utilities",
];

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TourSymbol {
    pub name: String,
    pub file: String,
    pub sir_intent: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TourStop {
    pub number: usize,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub symbols: Vec<TourSymbol>,
    pub layer: String,
    pub file_count: usize,
    pub symbol_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TourData {
    pub stop_count: usize,
    pub stops: Vec<TourStop>,
    pub skipped_stops: Vec<String>,
}

#[derive(Debug, Clone)]
struct TourStopDraft {
    title: &'static str,
    subtitle: &'static str,
    layer: String,
    symbol_indices: Vec<usize>,
}

pub(crate) async fn tour_handler(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || build_tour_data(shared.as_ref())).await {
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

pub(crate) fn build_tour_data(shared: &SharedState) -> Result<TourData, String> {
    let catalog = load_symbol_catalog(shared)?;
    Ok(build_tour_from_catalog(&catalog))
}

pub(crate) fn build_tour_from_catalog(catalog: &SymbolCatalog) -> TourData {
    let mut drafts = Vec::<TourStopDraft>::new();

    let interface_symbols = catalog.symbols_in_layer("Interface");
    let front_door_symbols = interface_symbols
        .iter()
        .copied()
        .filter(|idx| catalog.symbol(*idx).map(is_entry_symbol).unwrap_or(false))
        .collect::<Vec<_>>();
    if !front_door_symbols.is_empty() {
        drafts.push(TourStopDraft {
            title: "The Front Door",
            subtitle: "Entry Point",
            layer: "Interface".to_owned(),
            symbol_indices: front_door_symbols,
        });
    }

    let input_symbols = interface_symbols
        .iter()
        .copied()
        .filter(|idx| {
            catalog
                .symbol(*idx)
                .map(|symbol| !is_entry_symbol(symbol) && is_input_symbol(symbol))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    if !input_symbols.is_empty() {
        drafts.push(TourStopDraft {
            title: "What It Accepts",
            subtitle: "Input Processing",
            layer: "Interface".to_owned(),
            symbol_indices: input_symbols,
        });
    }

    let core_logic_symbols = catalog.symbols_in_layer("Core Logic");
    if !core_logic_symbols.is_empty() {
        drafts.push(TourStopDraft {
            title: "How It Thinks",
            subtitle: "Core Logic",
            layer: "Core Logic".to_owned(),
            symbol_indices: core_logic_symbols,
        });
    }

    let data_symbols = catalog.symbols_in_layer("Data");
    if !data_symbols.is_empty() {
        drafts.push(TourStopDraft {
            title: "Where It Stores Things",
            subtitle: "Data",
            layer: "Data".to_owned(),
            symbol_indices: data_symbols,
        });
    }

    let mut output_symbols = catalog.symbols_in_layer("Wire Format");
    output_symbols.extend(catalog.symbols_in_layer("Connectors"));
    output_symbols.sort_unstable();
    output_symbols.dedup();
    if !output_symbols.is_empty() {
        drafts.push(TourStopDraft {
            title: "How It Talks",
            subtitle: "Wire Format + Connectors",
            layer: "Wire Format + Connectors".to_owned(),
            symbol_indices: output_symbols,
        });
    }

    let error_symbols = catalog
        .symbols
        .iter()
        .enumerate()
        .filter_map(|(idx, symbol)| (!symbol.sir.error_modes.is_empty()).then_some(idx))
        .collect::<Vec<_>>();
    if !error_symbols.is_empty() {
        drafts.push(TourStopDraft {
            title: "How It Handles Problems",
            subtitle: "Error Handling",
            layer: "Cross-cutting".to_owned(),
            symbol_indices: error_symbols,
        });
    }

    let test_symbols = catalog.symbols_in_layer("Tests");
    if !test_symbols.is_empty() {
        drafts.push(TourStopDraft {
            title: "How It Gets Tested",
            subtitle: "Testing",
            layer: "Tests".to_owned(),
            symbol_indices: test_symbols,
        });
    }

    let utility_symbols = catalog.symbols_in_layer("Utilities");
    if utility_symbols.len() >= 3 {
        drafts.push(TourStopDraft {
            title: "The Utilities",
            subtitle: "Utilities",
            layer: "Utilities".to_owned(),
            symbol_indices: utility_symbols,
        });
    }

    let mut skipped = BTreeSet::<String>::new();

    if drafts.len() > 8 {
        if let Some(index) = drafts
            .iter()
            .position(|draft| draft.title == "The Utilities")
        {
            drafts.remove(index);
            skipped.insert("The Utilities".to_owned());
        }

        if drafts.len() > 8 {
            let front_door_index = drafts
                .iter()
                .position(|draft| draft.title == "The Front Door");
            let input_index = drafts
                .iter()
                .position(|draft| draft.title == "What It Accepts");

            if let (Some(front_door_index), Some(input_index)) = (front_door_index, input_index)
                && front_door_index != input_index
            {
                let input_symbols = drafts[input_index].symbol_indices.clone();
                drafts[front_door_index]
                    .symbol_indices
                    .extend(input_symbols);
                drafts[front_door_index].symbol_indices.sort_unstable();
                drafts[front_door_index].symbol_indices.dedup();
                drafts.remove(input_index);
            }
        }
    }

    if drafts.len() < 2 {
        let all_indices = (0..catalog.symbols.len()).collect::<Vec<_>>();
        let summary = describe_stop("Overview", all_indices.as_slice(), catalog);
        let overview_symbols = collect_stop_symbols(all_indices.as_slice(), catalog, 8);

        let mut skipped_stops = ALL_STOP_TITLES
            .iter()
            .map(|title| (*title).to_owned())
            .collect::<Vec<_>>();
        skipped_stops.sort();

        return TourData {
            stop_count: 1,
            stops: vec![TourStop {
                number: 1,
                title: "Overview".to_owned(),
                subtitle: "Project Walkthrough".to_owned(),
                description: summary,
                symbols: overview_symbols,
                layer: "Cross-cutting".to_owned(),
                file_count: catalog
                    .symbols
                    .iter()
                    .map(|symbol| symbol.file_path.as_str())
                    .collect::<HashSet<_>>()
                    .len(),
                symbol_count: catalog.symbols.len(),
            }],
            skipped_stops,
        };
    }

    let included_titles = drafts
        .iter()
        .map(|draft| draft.title)
        .collect::<HashSet<_>>();

    for title in ALL_STOP_TITLES {
        if !included_titles.contains(title) {
            skipped.insert(title.to_owned());
        }
    }

    let stops = drafts
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            let file_count = draft
                .symbol_indices
                .iter()
                .filter_map(|idx| catalog.symbol(*idx))
                .map(|symbol| symbol.file_path.as_str())
                .collect::<HashSet<_>>()
                .len();

            TourStop {
                number: index + 1,
                title: draft.title.to_owned(),
                subtitle: draft.subtitle.to_owned(),
                description: describe_stop(draft.title, draft.symbol_indices.as_slice(), catalog),
                symbols: collect_stop_symbols(draft.symbol_indices.as_slice(), catalog, 8),
                layer: draft.layer.clone(),
                file_count,
                symbol_count: draft.symbol_indices.len(),
            }
        })
        .collect::<Vec<_>>();

    TourData {
        stop_count: stops.len(),
        stops,
        skipped_stops: skipped.into_iter().collect(),
    }
}

fn collect_stop_symbols(
    indices: &[usize],
    catalog: &SymbolCatalog,
    limit: usize,
) -> Vec<TourSymbol> {
    ranked_indices(indices, catalog)
        .into_iter()
        .take(limit)
        .filter_map(|idx| catalog.symbol(idx))
        .map(|symbol| TourSymbol {
            name: symbol.name.clone(),
            file: symbol.file_path.clone(),
            sir_intent: first_sentence_or_fallback(symbol.sir.intent.as_str()),
        })
        .collect()
}

fn describe_stop(title: &str, indices: &[usize], catalog: &SymbolCatalog) -> String {
    if indices.is_empty() {
        return format!("{title} currently has no indexed symbols.");
    }

    let ranked = ranked_indices(indices, catalog);
    let featured = ranked.into_iter().take(3).collect::<Vec<_>>();

    let mut file_summaries = Vec::<String>::new();
    let mut seen_files = HashSet::<String>::new();

    for idx in &featured {
        let Some(symbol) = catalog.symbol(*idx) else {
            continue;
        };
        if !seen_files.insert(symbol.file_path.clone()) {
            continue;
        }

        let file_symbols = catalog
            .symbol_indices_for_file(symbol.file_path.as_str())
            .into_iter()
            .filter_map(|file_idx| catalog.symbol(file_idx))
            .map(to_narrative_symbol)
            .collect::<Vec<_>>();
        file_summaries.push(compose_file_summary(
            symbol.file_path.as_str(),
            file_symbols.as_slice(),
        ));
    }

    let featured_names = featured
        .iter()
        .filter_map(|idx| catalog.symbol(*idx))
        .map(|symbol| symbol.name.as_str())
        .collect::<Vec<_>>();

    let file_count = indices
        .iter()
        .filter_map(|idx| catalog.symbol(*idx))
        .map(|symbol| symbol.file_path.as_str())
        .collect::<HashSet<_>>()
        .len();

    let mut sentences = vec![format!(
        "{title} covers {} components across {file_count} files.",
        indices.len()
    )];

    for summary in file_summaries.into_iter().take(2) {
        sentences.push(summary);
    }

    if !featured_names.is_empty() {
        sentences.push(format!(
            "Key components here include {}.",
            join_human_list(featured_names)
        ));
    }

    sentences.join(" ")
}

fn ranked_indices(indices: &[usize], catalog: &SymbolCatalog) -> Vec<usize> {
    let mut ranked = indices.to_vec();
    ranked.sort_by(
        |left, right| match (catalog.symbol(*left), catalog.symbol(*right)) {
            (Some(left_symbol), Some(right_symbol)) => right_symbol
                .centrality
                .partial_cmp(&left_symbol.centrality)
                .unwrap_or(Ordering::Equal)
                .then_with(|| {
                    right_symbol
                        .dependents_count
                        .cmp(&left_symbol.dependents_count)
                })
                .then_with(|| left_symbol.qualified_name.cmp(&right_symbol.qualified_name)),
            _ => Ordering::Equal,
        },
    );
    ranked
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

fn is_entry_symbol(symbol: &CatalogSymbol) -> bool {
    let name = symbol.name.to_ascii_lowercase();
    let qualified = symbol.qualified_name.to_ascii_lowercase();
    let file = symbol.file_path.to_ascii_lowercase();

    name == "main"
        || qualified.ends_with("::main")
        || file.ends_with("/main.rs")
        || file == "main.rs"
        || file.contains("/bin/")
}

fn is_input_symbol(symbol: &CatalogSymbol) -> bool {
    let name = symbol.name.to_ascii_lowercase();
    let qualified = symbol.qualified_name.to_ascii_lowercase();
    let file = symbol.file_path.to_ascii_lowercase();

    [
        "server", "handler", "route", "listener", "accept", "request", "http",
    ]
    .iter()
    .any(|needle| name.contains(needle) || qualified.contains(needle) || file.contains(needle))
}

fn first_sentence_or_fallback(intent: &str) -> String {
    let first = crate::api::common::first_sentence(intent);
    if first.trim().is_empty() {
        "No SIR intent summary available".to_owned()
    } else {
        first
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
