use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::catalog::{
    CatalogSymbol, SymbolCatalog, load_symbol_catalog, parse_dependency_entry,
};
use crate::api::difficulty::difficulty_for_symbol;
use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DecomposeDifficulty {
    pub emoji: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DecomposeCheckpoint {
    pub check: String,
    pub why: String,
    pub source: String,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DecomposeStep {
    pub number: usize,
    pub title: String,
    pub subtitle: String,
    pub symbol_target: String,
    pub difficulty: String,
    pub prompt: String,
    pub why_this_order: String,
    pub context_needed: Vec<String>,
    pub expected_output: String,
    pub checkpoints: Vec<DecomposeCheckpoint>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DecomposeData {
    pub target: String,
    pub target_kind: String,
    pub target_file: String,
    pub step_count: usize,
    pub difficulty: DecomposeDifficulty,
    pub preamble: String,
    pub steps: Vec<DecomposeStep>,
    pub teaching_summary: String,
}

pub(crate) async fn decompose_handler(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Response {
    let shared = state.shared.clone();
    let selector_for_build = selector.clone();
    match support::run_blocking_with_timeout(move || {
        build_decompose_data(shared.as_ref(), selector_for_build.as_str())
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

pub(crate) async fn decompose_file_handler(
    State(state): State<Arc<DashboardState>>,
    Path(path): Path<String>,
) -> Response {
    let shared = state.shared.clone();
    let path_for_build = path.clone();
    match support::run_blocking_with_timeout(move || {
        build_file_decompose_data(shared.as_ref(), path_for_build.as_str())
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

pub(crate) fn build_decompose_data(
    shared: &SharedState,
    selector: &str,
) -> Result<Option<DecomposeData>, String> {
    let catalog = load_symbol_catalog(shared)?;
    let Some(resolved) = catalog.resolve_symbol_selector(selector) else {
        return Ok(None);
    };
    let Some(symbol) = catalog.symbol(resolved.primary_index) else {
        return Ok(None);
    };

    let steps = if is_function_like(symbol) {
        build_function_steps(symbol, &catalog)
    } else {
        build_type_steps(symbol, &catalog)
    };

    let difficulty = difficulty_for_symbol(symbol);
    let difficulty_badge = DecomposeDifficulty {
        emoji: difficulty.emoji.clone(),
        label: difficulty.label.clone(),
    };

    let preamble = format!(
        "{} is rated {} for LLM generation. Breaking this work into {} step(s) keeps each prompt focused and verifiable.",
        symbol.name,
        difficulty.label,
        steps.len()
    );

    let teaching_summary = "This decomposition follows dependency order: define dependency-free foundations first, then add layers that only depend on already-built pieces. Verify each step before moving to the next prompt.".to_owned();

    Ok(Some(DecomposeData {
        target: symbol.name.clone(),
        target_kind: symbol.kind.clone(),
        target_file: symbol.file_path.clone(),
        step_count: steps.len(),
        difficulty: difficulty_badge,
        preamble,
        steps,
        teaching_summary,
    }))
}

pub(crate) fn build_file_decompose_data(
    shared: &SharedState,
    path: &str,
) -> Result<Option<DecomposeData>, String> {
    let catalog = load_symbol_catalog(shared)?;
    let normalized = support::normalized_display_path(path);

    let mut file_symbols = catalog
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.file_path == normalized
                || symbol.file_path.ends_with(normalized.as_str())
                || normalized.ends_with(symbol.file_path.as_str())
        })
        .collect::<Vec<_>>();

    if file_symbols.is_empty() {
        return Ok(None);
    }

    file_symbols.sort_by(|left, right| left.name.cmp(&right.name));

    let ordered_ids = internal_toposort(file_symbols.as_slice(), &catalog);
    let ordered = ordered_ids
        .iter()
        .filter_map(|id| catalog.symbol_by_id(id.as_str()))
        .collect::<Vec<_>>();

    let mut steps = Vec::<DecomposeStep>::new();
    for (idx, symbol) in ordered.iter().enumerate() {
        let title = if idx == 0 {
            "The Foundation".to_owned()
        } else if idx + 1 == ordered.len() && is_cleanup_symbol(symbol) {
            "The Cleanup".to_owned()
        } else {
            format!("Step {}", idx + 1)
        };

        steps.push(DecomposeStep {
            number: idx + 1,
            title,
            subtitle: format!("Implement {}", symbol.name),
            symbol_target: symbol.name.clone(),
            difficulty: step_difficulty_label(&[symbol]),
            prompt: build_step_prompt(&[symbol], idx + 1),
            why_this_order: if idx == 0 {
                format!(
                    "{} has no internal file dependencies and establishes the foundation.",
                    symbol.name
                )
            } else {
                format!(
                    "{} builds on symbols from earlier steps in this file.",
                    symbol.name
                )
            },
            context_needed: ordered
                .iter()
                .take(idx)
                .map(|item| item.name.clone())
                .collect(),
            expected_output: format!(
                "{} compiles and integrates into {}",
                symbol.name, normalized
            ),
            checkpoints: build_checkpoints(&[symbol], &catalog),
        });
    }

    let max_difficulty = file_symbols
        .iter()
        .map(|symbol| difficulty_for_symbol(symbol))
        .max_by(|left, right| {
            left.score
                .partial_cmp(&right.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(crate::api::difficulty::DifficultyView {
            score: 0.0,
            emoji: "🟢".to_owned(),
            label: "Easy".to_owned(),
            guidance: String::new(),
            reasons: Vec::new(),
        });

    Ok(Some(DecomposeData {
        target: normalized.clone(),
        target_kind: "file".to_owned(),
        target_file: normalized,
        step_count: steps.len(),
        difficulty: DecomposeDifficulty {
            emoji: max_difficulty.emoji,
            label: max_difficulty.label,
        },
        preamble: "This file-level decomposition is ordered by internal dependencies so each prompt builds on already completed symbols.".to_owned(),
        steps,
        teaching_summary: "For file-level prompting, implement dependency roots first, then symbols that depend on them. Keep each prompt scoped to one symbol or one tightly-coupled group.".to_owned(),
    }))
}

fn build_type_steps(target: &CatalogSymbol, catalog: &SymbolCatalog) -> Vec<DecomposeStep> {
    let file_symbols = symbols_for_file(target, catalog);
    let ordered_ids = internal_toposort(file_symbols.as_slice(), catalog);
    let ordered_symbols = ordered_ids
        .iter()
        .filter_map(|id| catalog.symbol_by_id(id.as_str()))
        .collect::<Vec<_>>();

    let mut foundation = Vec::<&CatalogSymbol>::new();
    let mut wrapper = Vec::<&CatalogSymbol>::new();
    let mut core = Vec::<&CatalogSymbol>::new();
    let mut advanced = Vec::<&CatalogSymbol>::new();
    let mut cleanup = Vec::<&CatalogSymbol>::new();

    let foundation_ids = ordered_symbols
        .iter()
        .filter(|symbol| internal_dependency_count(symbol, &ordered_symbols, catalog) == 0)
        .map(|symbol| symbol.id.clone())
        .collect::<BTreeSet<_>>();

    for symbol in &ordered_symbols {
        if is_cleanup_symbol(symbol) {
            cleanup.push(*symbol);
            continue;
        }

        let internal_dep_count = internal_dependency_count(symbol, &ordered_symbols, catalog);
        let depends_only_on_foundation =
            catalog
                .dependency_ids(symbol.id.as_str())
                .iter()
                .all(|dep_id| {
                    !contains_symbol_id(&ordered_symbols, dep_id) || foundation_ids.contains(dep_id)
                });

        let difficulty = difficulty_for_symbol(symbol);

        if internal_dep_count == 0 {
            foundation.push(*symbol);
        } else if depends_only_on_foundation {
            wrapper.push(*symbol);
        } else if difficulty.label == "Hard" || difficulty.label == "Very Hard" {
            advanced.push(*symbol);
        } else {
            core.push(*symbol);
        }
    }

    if !contains_symbol_id(&ordered_symbols, target.id.as_str()) {
        foundation.push(target);
    }

    let mut groups = Vec::<(String, String, Vec<&CatalogSymbol>)>::new();
    if !foundation.is_empty() {
        groups.push((
            "The Foundation".to_owned(),
            "Define dependency-free data and contracts".to_owned(),
            dedup_symbols(foundation),
        ));
    }
    if !wrapper.is_empty() {
        groups.push((
            "The Wrapper".to_owned(),
            "Create interfaces that compose the foundation".to_owned(),
            dedup_symbols(wrapper),
        ));
    }
    if !core.is_empty() {
        groups.push((
            "Core Operations".to_owned(),
            "Add primary behavior and method flow".to_owned(),
            dedup_symbols(core),
        ));
    }
    if !advanced.is_empty() {
        groups.push((
            "Advanced Operations".to_owned(),
            "Handle high-complexity branches and edge cases".to_owned(),
            dedup_symbols(advanced),
        ));
    }
    if !cleanup.is_empty() {
        groups.push((
            "The Cleanup".to_owned(),
            "Finalize lifecycle and shutdown behavior".to_owned(),
            dedup_symbols(cleanup),
        ));
    }

    if groups.is_empty() {
        groups.push((
            "The Foundation".to_owned(),
            "Define the target symbol".to_owned(),
            vec![target],
        ));
    }

    let mut steps = Vec::<DecomposeStep>::new();
    let mut completed_symbols = Vec::<String>::new();

    for (idx, (title, subtitle, symbols)) in groups.into_iter().enumerate() {
        let symbol_names = symbols
            .iter()
            .map(|symbol| symbol.name.clone())
            .collect::<Vec<_>>();
        let prompt = build_step_prompt(symbols.as_slice(), idx + 1);
        let why_this_order = if idx == 0 {
            format!(
                "{} has no internal dependencies, so it is the right starting point.",
                symbol_names.join(", ")
            )
        } else {
            format!(
                "This step depends on symbols implemented in previous steps: {}.",
                completed_symbols.join(", ")
            )
        };

        let context_needed = if completed_symbols.is_empty() {
            symbols
                .iter()
                .flat_map(|symbol| symbol.sir.dependencies.clone())
                .take(4)
                .collect::<Vec<_>>()
        } else {
            completed_symbols.clone()
        };

        let checkpoints = build_checkpoints(symbols.as_slice(), catalog);

        steps.push(DecomposeStep {
            number: idx + 1,
            title,
            subtitle,
            symbol_target: symbol_names.join(", "),
            difficulty: step_difficulty_label(symbols.as_slice()),
            prompt,
            why_this_order,
            context_needed,
            expected_output: format!("{} implemented and compiling", symbol_names.join(", ")),
            checkpoints,
        });

        completed_symbols.extend(symbol_names);
    }

    steps
}

fn build_function_steps(target: &CatalogSymbol, catalog: &SymbolCatalog) -> Vec<DecomposeStep> {
    let diff = difficulty_for_symbol(target);

    let step_specs = vec![
        (
            "The Foundation",
            "Input validation and preconditions",
            format!(
                "Implement input validation for {}. Reject invalid states early and keep failure handling explicit.",
                target.name
            ),
            "Validation first prevents downstream logic from handling malformed state.",
        ),
        (
            "Main Logic",
            "Core control flow",
            format!(
                "Implement the main logic for {}. Follow the existing signature and preserve dependency contracts.",
                target.name
            ),
            "Core behavior can be implemented safely once validation guarantees are in place.",
        ),
        (
            "Error Handling",
            "Explicit failure modes",
            format!(
                "Add explicit handling for error modes in {}: {}.",
                target.name,
                if target.sir.error_modes.is_empty() {
                    "documented failure paths".to_owned()
                } else {
                    target.sir.error_modes.join(", ")
                }
            ),
            "Error handling is isolated so each failure path is deliberate and testable.",
        ),
        (
            "Output Contract",
            "Return values and side effects",
            format!(
                "Finalize return/output behavior for {} and verify side effects: {}.",
                target.name,
                if target.sir.side_effects.is_empty() {
                    "none".to_owned()
                } else {
                    target.sir.side_effects.join(", ")
                }
            ),
            "Finalizing outputs last confirms external callers get the expected contract.",
        ),
    ];

    step_specs
        .into_iter()
        .enumerate()
        .map(|(idx, (title, subtitle, prompt, why))| DecomposeStep {
            number: idx + 1,
            title: title.to_owned(),
            subtitle: subtitle.to_owned(),
            symbol_target: target.name.clone(),
            difficulty: format!("{} {}", diff.emoji, diff.label),
            prompt,
            why_this_order: why.to_owned(),
            context_needed: if idx == 0 {
                target.sir.dependencies.clone()
            } else {
                vec![target.name.clone()]
            },
            expected_output: format!("{} compiles and satisfies its signature", target.name),
            checkpoints: build_checkpoints(&[target], catalog),
        })
        .collect()
}

fn build_step_prompt(symbols: &[&CatalogSymbol], step_number: usize) -> String {
    let names = symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect::<Vec<_>>();

    let intents = symbols
        .iter()
        .map(|symbol| crate::api::common::first_sentence(symbol.sir.intent.as_str()))
        .filter(|intent| !intent.trim().is_empty())
        .collect::<Vec<_>>();

    let constraints = symbols
        .iter()
        .flat_map(|symbol| {
            let mut constraints = Vec::<String>::new();
            if symbol.sir.is_async {
                constraints.push("must preserve async behavior".to_owned());
            }
            if !symbol.sir.error_modes.is_empty() {
                constraints.push(format!("must handle {}", symbol.sir.error_modes.join(", ")));
            }
            if !symbol.sir.side_effects.is_empty() {
                constraints.push(format!(
                    "must preserve side effects: {}",
                    symbol.sir.side_effects.join(", ")
                ));
            }
            constraints
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let mut prompt = format!(
        "Step {}: Implement {} in Rust. Keep signatures compatible with existing code.",
        step_number,
        names.join(", ")
    );

    if !intents.is_empty() {
        prompt.push(' ');
        prompt.push_str(format!("Behavior: {}.", intents.join(" ")).as_str());
    }

    if !constraints.is_empty() {
        prompt.push(' ');
        prompt.push_str(format!("Constraints: {}.", constraints.join("; ")).as_str());
    }

    prompt
}

fn build_checkpoints(
    symbols: &[&CatalogSymbol],
    catalog: &SymbolCatalog,
) -> Vec<DecomposeCheckpoint> {
    let mut checkpoints = Vec::<DecomposeCheckpoint>::new();
    let mut seen = BTreeSet::<String>::new();

    for symbol in symbols {
        let dependents = catalog.dependent_ids(symbol.id.as_str());
        if !dependents.is_empty() {
            let dependent_names = dependents
                .iter()
                .filter_map(|id| catalog.symbol_by_id(id.as_str()))
                .map(|dep| dep.name.clone())
                .collect::<Vec<_>>();

            let check = format!("{} keeps dependent-facing contract stable", symbol.name);
            if seen.insert(check.clone()) {
                checkpoints.push(DecomposeCheckpoint {
                    check,
                    why: format!(
                        "Required by {} dependents: {}.",
                        dependent_names.len(),
                        dependent_names
                            .into_iter()
                            .take(5)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    source: "dependency_graph".to_owned(),
                    severity: "critical".to_owned(),
                });
            }
        }

        let external_dependents = catalog
            .dependent_ids(symbol.id.as_str())
            .iter()
            .filter_map(|id| catalog.symbol_by_id(id.as_str()))
            .any(|dep| dep.file_path != symbol.file_path);

        let visibility_check = if external_dependents {
            DecomposeCheckpoint {
                check: format!("{} visibility is public where required", symbol.name),
                why: "Used by symbols in other files, so public visibility is required for integration.".to_owned(),
                source: "dependency_graph".to_owned(),
                severity: "critical".to_owned(),
            }
        } else {
            DecomposeCheckpoint {
                check: format!("{} remains internal if not externally used", symbol.name),
                why: "No cross-file dependents detected; keeping internals private reduces accidental coupling.".to_owned(),
                source: "dependency_graph".to_owned(),
                severity: "info".to_owned(),
            }
        };
        if seen.insert(visibility_check.check.clone()) {
            checkpoints.push(visibility_check);
        }

        for side_effect in &symbol.sir.side_effects {
            let check = format!("{} preserves side effect: {}", symbol.name, side_effect);
            if seen.insert(check.clone()) {
                checkpoints.push(DecomposeCheckpoint {
                    check,
                    why: "Side effects represent observable runtime behavior and must remain intentional.".to_owned(),
                    source: "sir.side_effects".to_owned(),
                    severity: "warning".to_owned(),
                });
            }
        }

        for error_mode in &symbol.sir.error_modes {
            let check = format!("{} handles error mode: {}", symbol.name, error_mode);
            if seen.insert(check.clone()) {
                checkpoints.push(DecomposeCheckpoint {
                    check,
                    why: "Missing failure handling can create correctness bugs or hidden panics."
                        .to_owned(),
                    source: "sir.error_modes".to_owned(),
                    severity: "critical".to_owned(),
                });
            }
        }

        for dep in &symbol.sir.dependencies {
            let (dep_name, reason) = parse_dependency_entry(dep.as_str());
            if dep_name.trim().is_empty() {
                continue;
            }
            let check = format!("{} uses dependency {} correctly", symbol.name, dep_name);
            if seen.insert(check.clone()) {
                checkpoints.push(DecomposeCheckpoint {
                    check,
                    why: reason
                        .map(|value| format!("Dependency contract: {value}."))
                        .unwrap_or_else(|| {
                            "Dependency call patterns must match existing interfaces.".to_owned()
                        }),
                    source: "sir.dependencies".to_owned(),
                    severity: "info".to_owned(),
                });
            }
        }

        let intent_lower = symbol.sir.intent.to_ascii_lowercase();
        if intent_lower.contains("clone") || intent_lower.contains("arc") {
            let check = format!("{} supports Clone-compatible usage patterns", symbol.name);
            if seen.insert(check.clone()) {
                checkpoints.push(DecomposeCheckpoint {
                    check,
                    why: "Dependents often pass shared handles by clone when Arc/shared-state patterns are used.".to_owned(),
                    source: "sir.intent + dependency_graph".to_owned(),
                    severity: "critical".to_owned(),
                });
            }
        }
    }

    checkpoints
}

fn symbols_for_file<'a>(
    target: &CatalogSymbol,
    catalog: &'a SymbolCatalog,
) -> Vec<&'a CatalogSymbol> {
    let mut symbols = catalog
        .symbol_indices_for_file(target.file_path.as_str())
        .into_iter()
        .filter_map(|idx| catalog.symbol(idx))
        .collect::<Vec<_>>();
    if symbols.is_empty() {
        symbols = catalog
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.file_path.ends_with(target.file_path.as_str())
                    || target.file_path.ends_with(symbol.file_path.as_str())
            })
            .collect();
    }
    symbols
}

fn internal_toposort(file_symbols: &[&CatalogSymbol], catalog: &SymbolCatalog) -> Vec<String> {
    let file_ids = file_symbols
        .iter()
        .map(|symbol| symbol.id.clone())
        .collect::<BTreeSet<_>>();

    let mut indegree = HashMap::<String, usize>::new();
    let mut edges = HashMap::<String, Vec<String>>::new();

    for symbol in file_symbols {
        indegree.entry(symbol.id.clone()).or_insert(0);
    }

    for symbol in file_symbols {
        for dep_id in catalog.dependency_ids(symbol.id.as_str()) {
            if !file_ids.contains(dep_id.as_str()) {
                continue;
            }
            edges
                .entry(dep_id.clone())
                .or_default()
                .push(symbol.id.clone());
            *indegree.entry(symbol.id.clone()).or_insert(0) += 1;
        }
    }

    let mut queue = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
        .collect::<Vec<_>>();
    queue.sort();

    let mut ready = VecDeque::<String>::from(queue);
    let mut ordered = Vec::<String>::new();

    while let Some(id) = ready.pop_front() {
        ordered.push(id.clone());
        let mut next = edges.remove(id.as_str()).unwrap_or_default();
        next.sort();
        for candidate in next {
            let degree = indegree.entry(candidate.clone()).or_insert(0);
            if *degree > 0 {
                *degree -= 1;
            }
            if *degree == 0 {
                ready.push_back(candidate);
            }
        }
    }

    if ordered.len() < file_symbols.len() {
        for symbol in file_symbols {
            if !ordered.contains(&symbol.id) {
                ordered.push(symbol.id.clone());
            }
        }
    }

    ordered
}

fn internal_dependency_count(
    symbol: &CatalogSymbol,
    file_symbols: &[&CatalogSymbol],
    catalog: &SymbolCatalog,
) -> usize {
    catalog
        .dependency_ids(symbol.id.as_str())
        .iter()
        .filter(|dep_id| contains_symbol_id(file_symbols, dep_id))
        .count()
}

fn contains_symbol_id(symbols: &[&CatalogSymbol], symbol_id: &str) -> bool {
    symbols.iter().any(|symbol| symbol.id == symbol_id)
}

fn dedup_symbols(symbols: Vec<&CatalogSymbol>) -> Vec<&CatalogSymbol> {
    let mut seen = BTreeSet::<String>::new();
    let mut out = Vec::<&CatalogSymbol>::new();
    for symbol in symbols {
        if seen.insert(symbol.id.clone()) {
            out.push(symbol);
        }
    }
    out
}

fn step_difficulty_label(symbols: &[&CatalogSymbol]) -> String {
    let hardest = symbols
        .iter()
        .map(|symbol| difficulty_for_symbol(symbol))
        .max_by(|left, right| {
            left.score
                .partial_cmp(&right.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(crate::api::difficulty::DifficultyView {
            score: 0.0,
            emoji: "🟢".to_owned(),
            label: "Easy".to_owned(),
            guidance: String::new(),
            reasons: Vec::new(),
        });

    format!("{} {}", hardest.emoji, hardest.label)
}

fn is_cleanup_symbol(symbol: &CatalogSymbol) -> bool {
    let lower = format!("{} {}", symbol.name, symbol.sir.intent).to_ascii_lowercase();
    ["cleanup", "shutdown", "drop", "purge", "expire"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn is_function_like(symbol: &CatalogSymbol) -> bool {
    let kind = symbol.kind.to_ascii_lowercase();
    kind.contains("fn") || kind.contains("function") || kind.contains("method")
}
