use std::cmp::Ordering;
use std::collections::HashMap;

use aether_store::Store;
use serde_json::Value;

use crate::api::common;
use crate::narrative::{LayerAssignmentsCache, classify_layer, layer_by_name};
use crate::state::SharedState;
use crate::support;

#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogSir {
    pub intent: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub is_async: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CatalogSymbol {
    pub id: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub kind: String,
    pub layer_name: String,
    pub layer_icon: String,
    pub sir: CatalogSir,
    pub centrality: f64,
    pub dependents_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSymbolSelector {
    pub primary_index: usize,
    pub alternatives: Vec<usize>,
    pub matched_by: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct SymbolCatalog {
    pub symbols: Vec<CatalogSymbol>,
    pub edges: Vec<aether_analysis::GraphAlgorithmEdge>,
    by_id: HashMap<String, usize>,
    by_qualified: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
    by_name_lower: HashMap<String, Vec<usize>>,
    by_file: HashMap<String, Vec<usize>>,
    dependencies_by_source: HashMap<String, Vec<String>>,
    dependents_by_target: HashMap<String, Vec<String>>,
    ranked_symbol_ids: Vec<String>,
    pagerank_by_id: HashMap<String, f64>,
}

impl SymbolCatalog {
    pub fn symbol(&self, index: usize) -> Option<&CatalogSymbol> {
        self.symbols.get(index)
    }

    pub fn symbol_by_id(&self, symbol_id: &str) -> Option<&CatalogSymbol> {
        let index = self.by_id.get(symbol_id)?;
        self.symbol(*index)
    }

    pub fn symbols_in_layer(&self, layer_name: &str) -> Vec<usize> {
        let target = layer_name.trim();
        if target.is_empty() {
            return Vec::new();
        }

        self.symbols
            .iter()
            .enumerate()
            .filter_map(|(idx, symbol)| {
                symbol
                    .layer_name
                    .eq_ignore_ascii_case(target)
                    .then_some(idx)
            })
            .collect()
    }

    pub fn symbol_indices_for_file(&self, path: &str) -> Vec<usize> {
        self.by_file.get(path).cloned().unwrap_or_default()
    }

    pub fn dependency_ids(&self, source_id: &str) -> Vec<String> {
        self.dependencies_by_source
            .get(source_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn dependent_ids(&self, target_id: &str) -> Vec<String> {
        self.dependents_by_target
            .get(target_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn centrality(&self, symbol_id: &str) -> f64 {
        self.pagerank_by_id.get(symbol_id).copied().unwrap_or(0.0)
    }

    pub fn centrality_rank(&self, symbol_id: &str) -> usize {
        self.ranked_symbol_ids
            .iter()
            .position(|id| id == symbol_id)
            .map(|index| index + 1)
            .unwrap_or_else(|| self.ranked_symbol_ids.len().saturating_add(1))
    }

    pub fn resolve_symbol_selector(&self, selector: &str) -> Option<ResolvedSymbolSelector> {
        let trimmed = selector.trim();
        if trimmed.is_empty() {
            return None;
        }

        if let Some(index) = self.by_id.get(trimmed) {
            return Some(ResolvedSymbolSelector {
                primary_index: *index,
                alternatives: Vec::new(),
                matched_by: "id",
            });
        }

        if let Some(index) = self.by_qualified.get(trimmed) {
            return Some(ResolvedSymbolSelector {
                primary_index: *index,
                alternatives: Vec::new(),
                matched_by: "qualified_name",
            });
        }

        let mut candidates = self
            .by_name
            .get(trimmed)
            .cloned()
            .or_else(|| {
                self.by_name_lower
                    .get(&trimmed.to_ascii_lowercase())
                    .cloned()
            })
            .unwrap_or_default();

        if candidates.is_empty() {
            return None;
        }

        candidates.sort_by(|left, right| {
            let left_symbol = self.symbols.get(*left);
            let right_symbol = self.symbols.get(*right);
            match (left_symbol, right_symbol) {
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
            }
        });

        let primary = candidates[0];
        let alternatives = candidates.into_iter().skip(1).take(6).collect::<Vec<_>>();

        Some(ResolvedSymbolSelector {
            primary_index: primary,
            alternatives,
            matched_by: "name",
        })
    }
}

pub(crate) fn load_symbol_catalog(shared: &SharedState) -> Result<SymbolCatalog, String> {
    let raw_symbols = load_raw_symbols(shared)?;
    let sir_count = read_sir_count(shared)?;
    let assignments = load_or_build_layer_assignments(shared, sir_count, raw_symbols.as_slice())?;

    let edges = common::load_dependency_algo_edges(shared)?;
    let pagerank_by_id = aether_analysis::page_rank(edges.as_slice(), 0.85, 25);
    let mut dependencies_by_source = HashMap::<String, Vec<String>>::new();
    let mut dependents_by_target = HashMap::<String, Vec<String>>::new();
    let mut dependents_count_by_id = HashMap::<String, usize>::new();

    for edge in &edges {
        dependencies_by_source
            .entry(edge.source_id.clone())
            .or_default()
            .push(edge.target_id.clone());
        dependents_by_target
            .entry(edge.target_id.clone())
            .or_default()
            .push(edge.source_id.clone());
        *dependents_count_by_id
            .entry(edge.target_id.clone())
            .or_insert(0) += 1;
    }

    for values in dependencies_by_source.values_mut() {
        values.sort();
        values.dedup();
    }
    for values in dependents_by_target.values_mut() {
        values.sort();
        values.dedup();
    }

    let mut symbols = Vec::<CatalogSymbol>::new();
    let mut by_id = HashMap::<String, usize>::new();
    let mut by_qualified = HashMap::<String, usize>::new();
    let mut by_name = HashMap::<String, Vec<usize>>::new();
    let mut by_name_lower = HashMap::<String, Vec<usize>>::new();
    let mut by_file = HashMap::<String, Vec<usize>>::new();

    for raw in raw_symbols {
        let raw_id = raw.id.clone();
        let layer_name = assignments
            .symbol_to_layer
            .get(raw_id.as_str())
            .cloned()
            .unwrap_or_else(|| {
                classify_layer(
                    raw.file_path.as_str(),
                    raw.qualified_name.as_str(),
                    Some(raw.sir.intent.as_str()),
                )
                .name
            });
        let layer = layer_by_name(layer_name.as_str());

        let symbol = CatalogSymbol {
            id: raw.id,
            name: raw.name,
            qualified_name: raw.qualified_name,
            file_path: support::normalized_display_path(raw.file_path.as_str()),
            kind: raw.kind,
            layer_name: layer.name,
            layer_icon: layer.icon,
            centrality: pagerank_by_id.get(raw_id.as_str()).copied().unwrap_or(0.0),
            dependents_count: dependents_count_by_id
                .get(raw_id.as_str())
                .copied()
                .unwrap_or(0),
            sir: raw.sir,
        };

        let index = symbols.len();
        by_id.insert(symbol.id.clone(), index);
        by_qualified.insert(symbol.qualified_name.clone(), index);
        by_name.entry(symbol.name.clone()).or_default().push(index);
        by_name_lower
            .entry(symbol.name.to_ascii_lowercase())
            .or_default()
            .push(index);
        by_file
            .entry(symbol.file_path.clone())
            .or_default()
            .push(index);
        symbols.push(symbol);
    }

    let mut ranked_symbol_ids = symbols
        .iter()
        .map(|symbol| symbol.id.clone())
        .collect::<Vec<_>>();
    ranked_symbol_ids.sort_by(|left_id, right_id| {
        let left_symbol = symbols.iter().find(|symbol| symbol.id == *left_id);
        let right_symbol = symbols.iter().find(|symbol| symbol.id == *right_id);
        match (left_symbol, right_symbol) {
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
        }
    });

    Ok(SymbolCatalog {
        symbols,
        edges,
        by_id,
        by_qualified,
        by_name,
        by_name_lower,
        by_file,
        dependencies_by_source,
        dependents_by_target,
        ranked_symbol_ids,
        pagerank_by_id,
    })
}

pub(crate) fn parse_dependency_entry(raw: &str) -> (String, Option<String>) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (String::new(), None);
    }

    if let Some((name, reason)) = trimmed.split_once(':') {
        let dep_name = name.trim().to_owned();
        let dep_reason = reason.trim().trim_matches(['(', ')']).trim().to_owned();
        return (dep_name, (!dep_reason.is_empty()).then_some(dep_reason));
    }

    if let Some((name, reason)) = trimmed.split_once(" - ") {
        let dep_name = name.trim().to_owned();
        let dep_reason = reason.trim().to_owned();
        return (dep_name, (!dep_reason.is_empty()).then_some(dep_reason));
    }

    if let Some(open_idx) = trimmed.rfind('(')
        && trimmed.ends_with(')')
        && open_idx > 0
    {
        let dep_name = trimmed[..open_idx].trim().to_owned();
        let dep_reason = trimmed[(open_idx + 1)..(trimmed.len() - 1)]
            .trim()
            .to_owned();
        if !dep_name.is_empty() {
            return (dep_name, (!dep_reason.is_empty()).then_some(dep_reason));
        }
    }

    (trimmed.to_owned(), None)
}

#[derive(Debug, Clone)]
struct RawSymbol {
    id: String,
    name: String,
    qualified_name: String,
    file_path: String,
    kind: String,
    sir: CatalogSir,
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
            SELECT id, qualified_name, file_path, kind
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
            ))
        })
        .map_err(|err| err.to_string())?;

    let mut symbols = Vec::<RawSymbol>::new();

    for row in rows {
        let (id, qualified_name, file_path, kind) = row.map_err(|err| err.to_string())?;
        let sir = shared
            .store
            .read_sir_blob(id.as_str())
            .map_err(|err| err.to_string())?
            .map(|blob| parse_sir(blob.as_str()))
            .unwrap_or_default();

        symbols.push(RawSymbol {
            name: support::symbol_name_from_qualified(qualified_name.as_str()),
            id,
            qualified_name,
            file_path,
            kind,
            sir,
        });
    }

    Ok(symbols)
}

fn parse_sir(blob: &str) -> CatalogSir {
    let Ok(value) = serde_json::from_str::<Value>(blob) else {
        return CatalogSir::default();
    };

    let intent = value
        .get("intent")
        .or_else(|| value.get("purpose"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();

    let inputs = as_string_vec(value.get("inputs"));
    let outputs = as_string_vec(value.get("outputs"));
    let side_effects = as_string_vec(value.get("side_effects"));
    let dependencies = as_string_vec(value.get("dependencies"));
    let error_modes = as_string_vec(value.get("error_modes"));

    let intent_lower = intent.to_ascii_lowercase();
    let is_async = intent_lower.contains("async")
        || dependencies
            .iter()
            .any(|dep| dep.to_ascii_lowercase().contains("tokio"));

    CatalogSir {
        intent,
        inputs,
        outputs,
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

fn load_or_build_layer_assignments(
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
        let layer = classify_layer(
            symbol.file_path.as_str(),
            symbol.qualified_name.as_str(),
            Some(symbol.sir.intent.as_str()),
        );
        assignments
            .symbol_to_layer
            .insert(symbol.id.clone(), layer.name.clone());
        assignments
            .symbol_name_to_layer
            .entry(symbol.name.clone())
            .or_insert(layer.name);
    }

    *cache = Some((sir_count, assignments.clone()));
    Ok(assignments)
}
