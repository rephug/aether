use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use aether_store::SirStateStore;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Serialize;
use serde_json::Value;

use crate::api::common;
use crate::api::difficulty::DifficultyView;
use crate::narrative::{
    Dep, FileInfo, Layer, LayerAssignmentsCache, SirIntent, SymbolInfo, classify_layer,
    compose_file_summary, compose_layer_narrative, compose_project_summary,
    compute_difficulty_from_fields, layer_catalog,
};
use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_KEY_ACTOR_COUNT: usize = 8;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnatomyData {
    pub project_name: String,
    pub summary: String,
    pub maturity: MaturityBadge,
    pub tech_stack: Vec<TechStackCategory>,
    pub layers: Vec<AnatomyLayer>,
    pub key_actors: Vec<KeyActor>,
    pub simplified_graph: SimplifiedGraph,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MaturityBadge {
    pub dominant_phase: String,
    pub icon: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TechStackCategory {
    pub category: String,
    pub items: Vec<TechStackItem>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TechStackItem {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnatomyLayer {
    pub name: String,
    pub icon: String,
    pub description: String,
    pub narrative: String,
    pub files: Vec<AnatomyFile>,
    pub total_symbol_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnatomyFile {
    pub path: String,
    pub symbol_count: usize,
    pub summary: String,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct KeyActor {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub layer: String,
    pub description: String,
    pub centrality: f64,
    pub dependents_count: usize,
    pub difficulty: DifficultyView,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SimplifiedGraph {
    pub nodes: Vec<SimplifiedGraphNode>,
    pub edges: Vec<SimplifiedGraphEdge>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SimplifiedGraphNode {
    pub id: String,
    pub symbol_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SimplifiedGraphEdge {
    pub source: String,
    pub target: String,
    pub weight: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct AnatomyBuild {
    pub data: AnatomyData,
    pub layers_by_name: HashMap<String, LayerDetail>,
    pub files_by_path: HashMap<String, FileDetail>,
}

#[derive(Debug, Clone)]
pub(crate) struct LayerDetail {
    pub layer: AnatomyLayer,
}

#[derive(Debug, Clone)]
pub(crate) struct FileDetail {
    pub path: String,
    pub layer_name: String,
    pub summary: String,
    pub symbols: Vec<FileSymbolDetail>,
}

#[derive(Debug, Clone)]
pub(crate) struct FileSymbolDetail {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub sir_intent: String,
}

#[derive(Debug, Clone)]
struct RawSymbol {
    id: String,
    name: String,
    qualified_name: String,
    file_path: String,
    kind: String,
    language: String,
    layer: Layer,
    sir: SirData,
}

#[derive(Debug, Clone, Default)]
struct SirData {
    intent: String,
    side_effects: Vec<String>,
    dependencies: Vec<String>,
    error_modes: Vec<String>,
    is_async: bool,
}

pub(crate) async fn anatomy_handler(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || load_anatomy_build(shared.as_ref())).await {
        Ok(build) => support::api_json(state.shared.as_ref(), build.data).into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

pub(crate) fn load_anatomy_build(shared: &SharedState) -> Result<AnatomyBuild, String> {
    let sir_count = read_sir_count(shared)?;
    let tech_deps = discover_tech_deps(shared.workspace.as_path());
    let raw_symbols = load_raw_symbols(shared)?;
    let assignments = layer_assignments(shared, sir_count, raw_symbols.as_slice())?;

    let project_name = shared
        .workspace
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("project")
        .to_owned();

    let dominant_lang = dominant_language(raw_symbols.as_slice());
    let sir_intents = raw_symbols
        .iter()
        .filter(|symbol| !symbol.sir.intent.trim().is_empty())
        .map(|symbol| SirIntent {
            symbol: symbol.name.clone(),
            intent: symbol.sir.intent.clone(),
            side_effects: symbol.sir.side_effects.clone(),
            dependencies: symbol.sir.dependencies.clone(),
            error_modes: symbol.sir.error_modes.clone(),
        })
        .collect::<Vec<_>>();

    let summary = cached_project_summary(
        shared,
        sir_count,
        sir_intents.as_slice(),
        dominant_lang.as_str(),
        tech_deps.as_slice(),
    )?;

    let maturity = build_maturity(raw_symbols.as_slice());
    let tech_stack = build_tech_stack(tech_deps.as_slice());

    let mut by_layer = BTreeMap::<String, Vec<&RawSymbol>>::new();
    for symbol in &raw_symbols {
        by_layer
            .entry(symbol.layer.name.clone())
            .or_default()
            .push(symbol);
    }

    let layer_order = layer_catalog()
        .into_iter()
        .map(|layer| layer.name)
        .collect::<Vec<_>>();

    let mut layers = Vec::<AnatomyLayer>::new();
    let mut layers_by_name = HashMap::<String, LayerDetail>::new();
    let mut files_by_path = HashMap::<String, FileDetail>::new();

    for layer_name in layer_order {
        let Some(symbols_for_layer) = by_layer.get(layer_name.as_str()) else {
            continue;
        };

        let layer = symbols_for_layer
            .first()
            .map(|symbol| symbol.layer.clone())
            .unwrap_or_else(|| classify_layer("", "", None));

        let mut symbols_for_narrative = Vec::<SymbolInfo>::new();
        let mut files_for_narrative = Vec::<FileInfo>::new();
        let mut api_files = Vec::<AnatomyFile>::new();

        let mut by_file = BTreeMap::<String, Vec<&RawSymbol>>::new();
        for symbol in symbols_for_layer {
            by_file
                .entry(symbol.file_path.clone())
                .or_default()
                .push(*symbol);
        }

        for (file_path, file_symbols) in &by_file {
            let mut file_symbol_infos = file_symbols
                .iter()
                .map(|symbol| to_narrative_symbol(symbol, &assignments.symbol_to_layer))
                .collect::<Vec<_>>();
            file_symbol_infos.sort_by(|left, right| left.name.cmp(&right.name));

            let summary_text =
                compose_file_summary(file_path.as_str(), file_symbol_infos.as_slice());
            let normalized_path = support::normalized_display_path(file_path.as_str());

            let symbol_names = file_symbols
                .iter()
                .map(|symbol| symbol.name.clone())
                .collect::<Vec<_>>();

            files_for_narrative.push(FileInfo {
                path: normalized_path.clone(),
                symbol_count: file_symbols.len(),
                summary: summary_text.clone(),
                symbols: symbol_names.clone(),
            });

            api_files.push(AnatomyFile {
                path: normalized_path.clone(),
                symbol_count: file_symbols.len(),
                summary: summary_text.clone(),
                symbols: symbol_names,
            });

            let mut file_detail_symbols = file_symbols
                .iter()
                .map(|symbol| FileSymbolDetail {
                    name: symbol.name.clone(),
                    qualified_name: symbol.qualified_name.clone(),
                    kind: symbol.kind.clone(),
                    sir_intent: common::first_sentence(symbol.sir.intent.as_str()),
                })
                .collect::<Vec<_>>();
            file_detail_symbols.sort_by(|left, right| left.name.cmp(&right.name));

            files_by_path.insert(
                normalized_path.clone(),
                FileDetail {
                    path: normalized_path,
                    layer_name: layer.name.clone(),
                    summary: summary_text,
                    symbols: file_detail_symbols,
                },
            );

            symbols_for_narrative.extend(file_symbol_infos);
        }

        symbols_for_narrative.sort_by(|left, right| left.name.cmp(&right.name));
        files_for_narrative.sort_by(|left, right| left.path.cmp(&right.path));
        api_files.sort_by(|left, right| left.path.cmp(&right.path));

        let narrative = compose_layer_narrative(
            &layer,
            files_for_narrative.as_slice(),
            symbols_for_narrative.as_slice(),
        );

        let layer_entry = AnatomyLayer {
            name: layer.name.clone(),
            icon: layer.icon.clone(),
            description: layer.description.clone(),
            narrative,
            files: api_files,
            total_symbol_count: symbols_for_layer.len(),
        };

        layers_by_name.insert(
            layer_name.to_ascii_lowercase(),
            LayerDetail {
                layer: layer_entry.clone(),
            },
        );
        layers.push(layer_entry);
    }

    let key_actors = build_key_actors(
        shared,
        raw_symbols.as_slice(),
        &assignments.symbol_to_layer,
        DEFAULT_KEY_ACTOR_COUNT,
    )?;

    let simplified_graph =
        build_simplified_graph(shared, layers.as_slice(), &assignments.symbol_to_layer)?;

    Ok(AnatomyBuild {
        data: AnatomyData {
            project_name,
            summary,
            maturity,
            tech_stack,
            layers,
            key_actors,
            simplified_graph,
        },
        layers_by_name,
        files_by_path,
    })
}

fn cached_project_summary(
    shared: &SharedState,
    sir_count: i64,
    sir_intents: &[SirIntent],
    lang: &str,
    deps: &[Dep],
) -> Result<String, String> {
    let mut cache = shared
        .caches
        .project_summary
        .lock()
        .map_err(|err| format!("project summary cache lock poisoned: {err}"))?;

    if let Some((cached_count, ref cached)) = *cache
        && cached_count == sir_count
    {
        return Ok(cached.clone());
    }

    let summary = compose_project_summary(sir_intents, lang, deps);
    *cache = Some((sir_count, summary.clone()));
    Ok(summary)
}

fn layer_assignments(
    shared: &SharedState,
    sir_count: i64,
    symbols: &[RawSymbol],
) -> Result<LayerAssignmentsCache, String> {
    let mut cache = shared
        .caches
        .layer_assignments
        .lock()
        .map_err(|err| format!("layer assignment cache lock poisoned: {err}"))?;

    if let Some((cached_count, ref cached)) = *cache
        && cached_count == sir_count
    {
        return Ok(cached.clone());
    }

    let mut assignments = LayerAssignmentsCache::default();
    for symbol in symbols {
        assignments
            .symbol_to_layer
            .insert(symbol.id.clone(), symbol.layer.name.clone());
        assignments
            .symbol_name_to_layer
            .insert(symbol.name.clone(), symbol.layer.name.clone());
    }

    *cache = Some((sir_count, assignments.clone()));
    Ok(assignments)
}

fn load_raw_symbols(shared: &SharedState) -> Result<Vec<RawSymbol>, String> {
    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|err| err.to_string())?
    else {
        return Ok(Vec::new());
    };

    let mut stmt = conn
        .prepare(
            r#"
            SELECT id, qualified_name, file_path, kind, language
            FROM symbols
            ORDER BY qualified_name ASC, id ASC
            "#,
        )
        .map_err(|err| err.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|err| err.to_string())?;

    let mut symbols = Vec::<RawSymbol>::new();

    for row in rows {
        let (id, qualified_name, file_path, kind, language) = row.map_err(|err| err.to_string())?;
        let sir = shared
            .store
            .read_sir_blob(id.as_str())
            .map_err(|err| err.to_string())?
            .map(|blob| parse_sir(blob.as_str()))
            .unwrap_or_default();
        let layer = classify_layer(
            file_path.as_str(),
            qualified_name.as_str(),
            Some(sir.intent.as_str()),
        );

        symbols.push(RawSymbol {
            id,
            name: support::symbol_name_from_qualified(qualified_name.as_str()),
            qualified_name,
            file_path,
            kind,
            language,
            layer,
            sir,
        });
    }

    Ok(symbols)
}

fn parse_sir(blob: &str) -> SirData {
    let Ok(value) = serde_json::from_str::<Value>(blob) else {
        return SirData::default();
    };

    let intent = value
        .get("intent")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();

    let side_effects = as_string_vec(value.get("side_effects"));
    let dependencies = as_string_vec(value.get("dependencies"));
    let error_modes = as_string_vec(value.get("error_modes"));

    let intent_lower = intent.to_ascii_lowercase();
    let is_async = intent_lower.contains("async")
        || dependencies
            .iter()
            .any(|dep| dep.to_ascii_lowercase().contains("tokio"));

    SirData {
        intent,
        side_effects,
        dependencies,
        error_modes,
        is_async,
    }
}

fn as_string_vec(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn read_sir_count(shared: &SharedState) -> Result<i64, String> {
    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|err| err.to_string())?
    else {
        return Ok(0);
    };

    support::count_nonempty_sir(&conn).map_err(|err| err.to_string())
}

fn dominant_language(symbols: &[RawSymbol]) -> String {
    if symbols.is_empty() {
        return "rust".to_owned();
    }

    let mut counts = HashMap::<String, usize>::new();
    for symbol in symbols {
        let language = symbol.language.trim().to_ascii_lowercase();
        *counts.entry(language).or_insert(0) += 1;
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(language, _)| language)
        .unwrap_or_else(|| "rust".to_owned())
}

fn build_maturity(symbols: &[RawSymbol]) -> MaturityBadge {
    let phases = [
        "Architecture",
        "Implementation",
        "Integration",
        "Testing",
        "Operations",
    ];

    let mut counts = HashMap::<&'static str, usize>::new();
    for phase in phases {
        counts.insert(phase, 0);
    }

    for symbol in symbols {
        let kind = symbol.kind.to_ascii_lowercase();
        let file = symbol.file_path.to_ascii_lowercase();
        let layer = symbol.layer.name.as_str();

        let phase = if kind.contains("trait") {
            "Architecture"
        } else if layer == "Tests" {
            "Testing"
        } else if layer == "Connectors" {
            "Integration"
        } else if file.contains("config") || file.contains("log") || file.contains("tracing") {
            "Operations"
        } else {
            "Implementation"
        };

        *counts.entry(phase).or_insert(0) += 1;
    }

    let dominant = phases
        .into_iter()
        .max_by(|left, right| {
            let left_count = counts.get(left).copied().unwrap_or(0);
            let right_count = counts.get(right).copied().unwrap_or(0);
            left_count
                .cmp(&right_count)
                .then_with(|| phase_priority(left).cmp(&phase_priority(right)).reverse())
        })
        .unwrap_or("Implementation");

    let (icon, description) = match dominant {
        "Architecture" => (
            "🏗️",
            "Focused on structural contracts, abstractions, and system boundaries",
        ),
        "Integration" => (
            "🔌",
            "Focused on connecting internal logic with external systems",
        ),
        "Testing" => (
            "🧪",
            "Focused on validation, assertions, and reliability checks",
        ),
        "Operations" => (
            "🛠️",
            "Focused on configuration, observability, and runtime operations",
        ),
        _ => (
            "⚙️",
            "Focused on concrete functionality with solid test coverage",
        ),
    };

    MaturityBadge {
        dominant_phase: dominant.to_owned(),
        icon: icon.to_owned(),
        description: description.to_owned(),
    }
}

fn phase_priority(phase: &str) -> usize {
    match phase {
        "Implementation" => 5,
        "Architecture" => 4,
        "Integration" => 3,
        "Testing" => 2,
        "Operations" => 1,
        _ => 0,
    }
}

fn discover_tech_deps(workspace: &Path) -> Vec<Dep> {
    let manifest_path = workspace.join("Cargo.toml");
    let Ok(raw) = std::fs::read_to_string(manifest_path) else {
        return Vec::new();
    };

    parse_dependency_names(raw.as_str())
        .into_iter()
        .map(|name| Dep {
            name,
            category: None,
            detail: None,
        })
        .collect()
}

fn parse_dependency_names(raw_manifest: &str) -> Vec<String> {
    let mut names = BTreeSet::<String>::new();
    let mut in_dependency_section = false;

    for line in raw_manifest.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed.trim_matches(['[', ']']);
            let lower = section.to_ascii_lowercase();
            in_dependency_section =
                lower.contains("dependencies") && !lower.contains("dev-dependencies");
            continue;
        }

        if !in_dependency_section {
            continue;
        }

        if trimmed.starts_with('{') || trimmed.starts_with('}') {
            continue;
        }

        if let Some((name, _)) = trimmed.split_once('=') {
            let dep_name = name.trim().trim_matches('"');
            if dep_name.is_empty() {
                continue;
            }

            let normalized = dep_name
                .split('.')
                .next()
                .unwrap_or(dep_name)
                .trim()
                .to_ascii_lowercase();
            if !normalized.is_empty() {
                names.insert(normalized);
            }
        }
    }

    names.into_iter().collect()
}

fn build_tech_stack(deps: &[Dep]) -> Vec<TechStackCategory> {
    let mut grouped = BTreeMap::<String, Vec<TechStackItem>>::new();
    grouped
        .entry("Language & Runtime".to_owned())
        .or_default()
        .push(TechStackItem {
            name: "rust".to_owned(),
            description: "Systems language and compiler toolchain".to_owned(),
        });

    let mut seen = HashSet::<String>::new();
    for dep in deps {
        if let Some((category, description)) = categorize_dependency(dep.name.as_str()) {
            let key = format!("{category}:{}", dep.name);
            if !seen.insert(key) {
                continue;
            }
            grouped
                .entry(category.to_owned())
                .or_default()
                .push(TechStackItem {
                    name: dep.name.clone(),
                    description: description.to_owned(),
                });
        }
    }

    let order = [
        "Language & Runtime",
        "Networking",
        "Serialization",
        "Wire Format",
        "Data Storage",
        "CLI & Config",
        "Observability",
        "Error Handling",
    ];

    let mut out = Vec::<TechStackCategory>::new();
    for category in order {
        let Some(mut items) = grouped.remove(category) else {
            continue;
        };
        items.sort_by(|left, right| left.name.cmp(&right.name));
        out.push(TechStackCategory {
            category: category.to_owned(),
            items,
        });
    }

    for (category, mut items) in grouped {
        items.sort_by(|left, right| left.name.cmp(&right.name));
        out.push(TechStackCategory { category, items });
    }

    out
}

fn categorize_dependency(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "tokio" => Some(("Language & Runtime", "Async runtime")),
        "serde" | "serde_json" => Some(("Serialization", "Data conversion")),
        "clap" | "toml" => Some(("CLI & Config", "Argument and config parsing")),
        "tracing" | "tracing-subscriber" => Some(("Observability", "Logging and diagnostics")),
        "axum" | "hyper" | "reqwest" | "tower" | "tower-http" => {
            Some(("Networking", "HTTP and service middleware"))
        }
        "bytes" | "mime_guess" => Some(("Wire Format", "Byte and payload handling")),
        "anyhow" | "thiserror" => Some(("Error Handling", "Error propagation and typing")),
        "sqlx" | "rusqlite" | "surrealdb" | "lancedb" => {
            Some(("Data Storage", "Persistence and query storage backend"))
        }
        _ => None,
    }
}

fn to_narrative_symbol(symbol: &RawSymbol, assignments: &HashMap<String, String>) -> SymbolInfo {
    SymbolInfo {
        id: symbol.id.clone(),
        name: symbol.name.clone(),
        qualified_name: symbol.qualified_name.clone(),
        kind: symbol.kind.clone(),
        file_path: support::normalized_display_path(symbol.file_path.as_str()),
        sir_intent: symbol.sir.intent.clone(),
        side_effects: symbol.sir.side_effects.clone(),
        dependencies: symbol.sir.dependencies.clone(),
        error_modes: symbol.sir.error_modes.clone(),
        is_async: symbol.sir.is_async,
        layer: assignments
            .get(symbol.id.as_str())
            .cloned()
            .unwrap_or_else(|| symbol.layer.name.clone()),
        dependents_count: 0,
    }
}

fn build_key_actors(
    shared: &SharedState,
    symbols: &[RawSymbol],
    assignments: &HashMap<String, String>,
    limit: usize,
) -> Result<Vec<KeyActor>, String> {
    if symbols.is_empty() {
        return Ok(Vec::new());
    }

    let edges = common::load_dependency_algo_edges(shared)?;
    let pagerank = aether_analysis::page_rank(edges.as_slice(), 0.85, 25);

    let mut dependents_count = HashMap::<String, usize>::new();
    for edge in &edges {
        *dependents_count.entry(edge.target_id.clone()).or_insert(0) += 1;
    }

    let symbol_by_id = symbols
        .iter()
        .map(|symbol| (symbol.id.clone(), symbol))
        .collect::<HashMap<_, _>>();

    let mut scored = symbols
        .iter()
        .map(|symbol| {
            (
                symbol.id.clone(),
                pagerank.get(symbol.id.as_str()).copied().unwrap_or(0.0),
            )
        })
        .collect::<Vec<_>>();

    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut actors = Vec::<KeyActor>::new();
    for (symbol_id, centrality) in scored.into_iter().take(limit) {
        let Some(symbol) = symbol_by_id.get(symbol_id.as_str()) else {
            continue;
        };

        actors.push(KeyActor {
            name: symbol.name.clone(),
            kind: symbol.kind.clone(),
            file: support::normalized_display_path(symbol.file_path.as_str()),
            layer: assignments
                .get(symbol.id.as_str())
                .cloned()
                .unwrap_or_else(|| symbol.layer.name.clone()),
            description: common::first_sentence(symbol.sir.intent.as_str()),
            centrality,
            dependents_count: dependents_count
                .get(symbol.id.as_str())
                .copied()
                .unwrap_or(0),
            difficulty: difficulty_from_sir(
                symbol.sir.intent.as_str(),
                symbol.sir.error_modes.len(),
                symbol.sir.side_effects.len(),
                symbol.sir.dependencies.len(),
                symbol.sir.is_async,
            ),
        });
    }

    Ok(actors)
}

fn build_simplified_graph(
    shared: &SharedState,
    layers: &[AnatomyLayer],
    assignments: &HashMap<String, String>,
) -> Result<SimplifiedGraph, String> {
    let nodes = layers
        .iter()
        .map(|layer| SimplifiedGraphNode {
            id: layer.name.clone(),
            symbol_count: layer.total_symbol_count,
        })
        .collect::<Vec<_>>();

    let edges = common::load_dependency_algo_edges(shared)?;
    let mut aggregate = HashMap::<(String, String), usize>::new();

    for edge in edges {
        let Some(source_layer) = assignments.get(edge.source_id.as_str()) else {
            continue;
        };
        let Some(target_layer) = assignments.get(edge.target_id.as_str()) else {
            continue;
        };
        if source_layer == target_layer {
            continue;
        }

        *aggregate
            .entry((source_layer.clone(), target_layer.clone()))
            .or_insert(0) += 1;
    }

    let mut simplified_edges = aggregate
        .into_iter()
        .map(|((source, target), weight)| SimplifiedGraphEdge {
            source,
            target,
            weight,
        })
        .collect::<Vec<_>>();

    simplified_edges.sort_by(|left, right| {
        right
            .weight
            .cmp(&left.weight)
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.target.cmp(&right.target))
    });

    Ok(SimplifiedGraph {
        nodes,
        edges: simplified_edges,
    })
}

fn difficulty_from_sir(
    intent: &str,
    error_count: usize,
    side_effect_count: usize,
    dep_count: usize,
    is_async: bool,
) -> DifficultyView {
    let score =
        compute_difficulty_from_fields(intent, error_count, side_effect_count, dep_count, is_async);
    DifficultyView {
        score: score.score,
        emoji: score.emoji,
        label: score.label,
        guidance: score.guidance,
        reasons: score.reasons,
    }
}
