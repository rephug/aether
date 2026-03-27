use std::collections::HashSet;
use std::fs;

use aether_analysis::{HealthAnalyzer, HealthInclude, HealthRequest};
use aether_config::GraphBackend;
use aether_core::{EdgeKind, Position, SourceRange};
use aether_parse::{SymbolExtractor, language_for_path};
use aether_sir::SirAnnotation;
use aether_store::{
    SirStateStore, SymbolCatalogStore, SymbolRecord, SymbolRelationStore, TestIntentStore,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::AetherMcpServer;
use crate::AetherMcpError;

const DEFAULT_MAX_TOKENS: usize = 8_000;
const CHARS_PER_TOKEN: usize = 4;
const MAX_GRAPH_NEIGHBORS: usize = 20;
const MAX_TEST_SUMMARIES: usize = 5;
const DEFAULT_DEPTH: u32 = 1;

const LAYER_ORDER: [&str; 6] = ["source", "sir", "graph", "health", "reasoning", "tests"];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSirContextRequest {
    /// Symbol ID or qualified name selector
    pub symbol: String,
    /// Maximum token budget (default 8000)
    pub max_tokens: Option<usize>,
    /// Output format: "markdown" (default) or "json"
    pub format: Option<String>,
    /// Layers to include. Default: all available.
    /// Valid layers: "source", "sir", "graph", "tests", "health", "reasoning"
    pub include_layers: Option<Vec<String>>,
    /// Dependency traversal depth (1-3, default 1)
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SirContextLayer {
    /// Layer name (e.g., "source", "sir", "graph")
    pub name: String,
    /// Layer content
    pub content: String,
    /// Approximate token count for this layer
    pub token_estimate: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSirContextResponse {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    /// Assembled context document (markdown or JSON depending on format)
    pub context: String,
    /// Individual layers with their token estimates
    pub layers: Vec<SirContextLayer>,
    /// Total estimated tokens used
    pub total_tokens: usize,
    /// Token budget remaining
    pub budget_remaining: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextFormat {
    Markdown,
    Json,
}

#[derive(Debug, Clone)]
struct GraphEntry {
    section: &'static str,
    relationship: String,
    depth: u32,
    qualified_name: String,
    file_path: String,
    intent_summary: String,
}

fn estimate_tokens(content: &str) -> usize {
    content.len() / CHARS_PER_TOKEN
}

fn parse_context_format(raw: Option<&str>) -> Result<ContextFormat, AetherMcpError> {
    match raw
        .unwrap_or("markdown")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "markdown" => Ok(ContextFormat::Markdown),
        "json" => Ok(ContextFormat::Json),
        other => Err(AetherMcpError::Message(format!(
            "invalid format '{other}', expected markdown or json"
        ))),
    }
}

fn parse_include_layers(raw: Option<Vec<String>>) -> Result<HashSet<String>, AetherMcpError> {
    let mut include = HashSet::new();
    for layer in raw.unwrap_or_else(|| {
        LAYER_ORDER
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    }) {
        let normalized = layer.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if !LAYER_ORDER.contains(&normalized.as_str()) {
            return Err(AetherMcpError::Message(format!(
                "invalid layer '{normalized}', expected one of: {}",
                LAYER_ORDER.join(", ")
            )));
        }
        include.insert(normalized);
    }
    Ok(include)
}

fn parse_depth(raw: Option<u32>) -> Result<u32, AetherMcpError> {
    let depth = raw.unwrap_or(DEFAULT_DEPTH);
    if !(1..=3).contains(&depth) {
        return Err(AetherMcpError::Message(
            "depth must be between 1 and 3".to_owned(),
        ));
    }
    Ok(depth)
}

fn parse_max_tokens(raw: Option<usize>) -> Result<usize, AetherMcpError> {
    let max_tokens = raw.unwrap_or(DEFAULT_MAX_TOKENS);
    if max_tokens == 0 {
        return Err(AetherMcpError::Message(
            "max_tokens must be greater than 0".to_owned(),
        ));
    }
    Ok(max_tokens)
}

fn first_sentence(value: &str) -> String {
    value
        .split(['\n', '.', '!', '?'])
        .map(str::trim)
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn relation_label(kind: EdgeKind) -> String {
    kind.as_str().to_owned()
}

fn inverse_relation_label(kind: EdgeKind) -> String {
    match kind {
        EdgeKind::Calls => "called_by".to_owned(),
        EdgeKind::DependsOn => "depended_on_by".to_owned(),
        EdgeKind::TypeRef => "type_ref_by".to_owned(),
        EdgeKind::Implements => "implemented_by".to_owned(),
    }
}

fn truncate_by_char_count(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_owned();
    }

    let mut end = 0usize;
    for (index, ch) in content.char_indices() {
        let next = index + ch.len_utf8();
        if next > max_chars {
            break;
        }
        end = next;
    }

    content[..end].to_owned()
}

fn middle_truncate(content: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return String::new();
    }

    let max_chars = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    if content.len() <= max_chars {
        return content.to_owned();
    }

    let marker = "[... truncated ...]";
    if max_chars <= marker.len() + 8 {
        return truncate_by_char_count(marker, max_chars.max(1));
    }

    let prefix_chars = (max_chars - marker.len()) / 2;
    let suffix_chars = max_chars - marker.len() - prefix_chars;

    let prefix = truncate_by_char_count(content, prefix_chars);

    let mut suffix_start = content.len();
    let mut remaining = suffix_chars;
    for (index, ch) in content.char_indices().rev() {
        let char_len = ch.len_utf8();
        if remaining < char_len {
            break;
        }
        remaining -= char_len;
        suffix_start = index;
        if remaining == 0 {
            break;
        }
    }

    let suffix = &content[suffix_start..];
    format!("{prefix}{marker}{suffix}")
}

fn allocation_for_layer(layer: &str, max_tokens: usize) -> usize {
    let weight = match layer {
        "source" => 40,
        "sir" => 15,
        "graph" => 20,
        "health" => 5,
        "reasoning" => 10,
        "tests" => 10,
        _ => 0,
    };
    ((max_tokens * weight) / 100).max(1)
}

fn trim_layers_to_budget(layers: Vec<SirContextLayer>, max_tokens: usize) -> Vec<SirContextLayer> {
    let total_tokens = layers
        .iter()
        .map(|layer| layer.token_estimate)
        .sum::<usize>();
    if total_tokens <= max_tokens {
        return layers;
    }

    layers
        .into_iter()
        .map(|mut layer| {
            let allocation = allocation_for_layer(layer.name.as_str(), max_tokens);
            if layer.token_estimate > allocation {
                layer.content = middle_truncate(layer.content.as_str(), allocation);
                layer.token_estimate = estimate_tokens(layer.content.as_str());
            }
            layer
        })
        .collect()
}

fn resolve_symbol_selector(
    store: &aether_store::SqliteStore,
    selector: &str,
) -> Result<SymbolRecord, AetherMcpError> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(AetherMcpError::Message(
            "symbol selector must not be empty".to_owned(),
        ));
    }

    if let Some(record) = store.get_symbol_record(selector)? {
        return Ok(record);
    }

    let exact_matches = store.find_symbol_search_results_by_qualified_name(selector)?;
    match exact_matches.as_slice() {
        [only] => {
            return store
                .get_symbol_record(only.symbol_id.as_str())?
                .ok_or_else(|| {
                    AetherMcpError::Message(format!(
                        "symbol search returned missing record: {}",
                        only.symbol_id
                    ))
                });
        }
        [] => {}
        many => {
            let candidates = many
                .iter()
                .map(|candidate| {
                    format!(
                        "{} [{}]",
                        candidate.qualified_name.trim(),
                        candidate.file_path.trim()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n  - ");
            return Err(AetherMcpError::Message(format!(
                "ambiguous symbol selector '{selector}'. Candidates:\n  - {candidates}"
            )));
        }
    }

    let matches = store.search_symbols(selector, 10)?;
    match matches.as_slice() {
        [] => Err(AetherMcpError::Message(format!(
            "symbol not found: {selector}"
        ))),
        [only] => store
            .get_symbol_record(only.symbol_id.as_str())?
            .ok_or_else(|| {
                AetherMcpError::Message(format!(
                    "symbol search returned missing record: {}",
                    only.symbol_id
                ))
            }),
        many => {
            let candidates = many
                .iter()
                .map(|candidate| {
                    format!(
                        "{} [{}]",
                        candidate.qualified_name.trim(),
                        candidate.file_path.trim()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n  - ");
            Err(AetherMcpError::Message(format!(
                "ambiguous symbol selector '{selector}'. Candidates:\n  - {candidates}"
            )))
        }
    }
}

fn byte_offset_for_position(source: &str, position: Position) -> Option<usize> {
    let mut line = 1usize;
    let mut column = 1usize;
    if position.line == 1 && position.column == 1 {
        return Some(0);
    }

    for (index, ch) in source.char_indices() {
        if line == position.line && column == position.column {
            return Some(index);
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += ch.len_utf8();
        }
    }

    if line == position.line && column == position.column {
        Some(source.len())
    } else {
        None
    }
}

fn extract_symbol_source_text(source: &str, range: SourceRange) -> Option<String> {
    let start = range
        .start_byte
        .or_else(|| byte_offset_for_position(source, range.start))?;
    let end = range
        .end_byte
        .or_else(|| byte_offset_for_position(source, range.end))?;
    if start > end || end > source.len() {
        return None;
    }
    source.get(start..end).map(str::to_owned)
}

fn read_neighbor_intent_summary(store: &aether_store::SqliteStore, symbol_id: &str) -> String {
    match store.read_sir_blob(symbol_id) {
        Ok(Some(blob)) => match serde_json::from_str::<SirAnnotation>(&blob) {
            Ok(sir) => {
                let summary = first_sentence(sir.intent.as_str());
                if summary.is_empty() {
                    "No SIR recorded.".to_owned()
                } else {
                    summary
                }
            }
            Err(err) => {
                tracing::warn!(
                    symbol_id,
                    error = %err,
                    "failed to parse SIR blob for context neighbor summary"
                );
                "Unreadable SIR.".to_owned()
            }
        },
        Ok(None) => "No SIR recorded.".to_owned(),
        Err(err) => {
            tracing::warn!(
                symbol_id,
                error = %err,
                "failed to read SIR blob for context neighbor summary"
            );
            "SIR unavailable.".to_owned()
        }
    }
}

fn build_source_layer(server: &AetherMcpServer, record: &SymbolRecord) -> String {
    let full_path = server.workspace().join(&record.file_path);
    let source = match fs::read_to_string(&full_path) {
        Ok(source) => source,
        Err(err) => {
            return format!(
                "Unavailable: failed to read source file {}: {err}",
                full_path.display()
            );
        }
    };

    let language = match language_for_path(&full_path) {
        Some(language) => language,
        None => {
            return source;
        }
    };
    let display_path = record.file_path.clone();

    let mut extractor = match SymbolExtractor::new() {
        Ok(extractor) => extractor,
        Err(err) => {
            return format!("Unavailable: failed to initialize symbol extractor: {err}");
        }
    };

    let symbols = match extractor.extract_from_source(language, &display_path, &source) {
        Ok(symbols) => symbols,
        Err(err) => {
            return format!("Unavailable: failed to parse source file {display_path}: {err}");
        }
    };

    symbols
        .iter()
        .find(|symbol| symbol.id == record.id || symbol.qualified_name == record.qualified_name)
        .and_then(|symbol| extract_symbol_source_text(&source, symbol.range))
        .unwrap_or(source)
}

fn build_sir_layer(server: &AetherMcpServer, record: &SymbolRecord) -> String {
    let meta = match server.read_sir_meta(record.id.as_str()) {
        Ok(meta) => meta,
        Err(err) => return format!("Unavailable: failed to read SIR metadata: {err}"),
    };
    let sir = match server.read_valid_sir_blob(record.id.as_str()) {
        Ok(sir) => sir,
        Err(err) => return format!("Unavailable: failed to read SIR annotation: {err}"),
    };

    match sir {
        Some(sir) => {
            let side_effects = if sir.side_effects.is_empty() {
                "none".to_owned()
            } else {
                sir.side_effects.join("; ")
            };
            let error_modes = if sir.error_modes.is_empty() {
                "none".to_owned()
            } else {
                sir.error_modes.join("; ")
            };
            let generation_pass = meta
                .as_ref()
                .map(|entry| entry.generation_pass.as_str())
                .unwrap_or("unknown");
            format!(
                "**Intent:** {}\n**Side Effects:** {}\n**Error Modes:** {}\n**Confidence:** {:.2}\n**Generation Pass:** {}",
                sir.intent, side_effects, error_modes, sir.confidence, generation_pass
            )
        }
        None => "No SIR recorded.".to_owned(),
    }
}

fn build_reasoning_layer(server: &AetherMcpServer, record: &SymbolRecord) -> String {
    match server.read_sir_meta(record.id.as_str()) {
        Ok(Some(meta)) => meta
            .reasoning_trace
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Unavailable: no reasoning trace recorded.".to_owned()),
        Ok(None) => "Unavailable: no SIR metadata recorded.".to_owned(),
        Err(err) => format!("Unavailable: failed to read SIR metadata: {err}"),
    }
}

fn build_tests_layer(store: &aether_store::SqliteStore, record: &SymbolRecord) -> String {
    let intents = match store.list_test_intents_for_symbol(record.id.as_str()) {
        Ok(intents) if !intents.is_empty() => intents,
        Ok(_) => match store.list_test_intents_for_file(record.file_path.as_str()) {
            Ok(intents) => intents,
            Err(err) => {
                return format!(
                    "Unavailable: failed to list test intents for {}: {err}",
                    record.file_path
                );
            }
        },
        Err(err) => {
            return format!(
                "Unavailable: failed to list test intents for {}: {err}",
                record.id
            );
        }
    };

    if intents.is_empty() {
        return "No test intents recorded.".to_owned();
    }

    intents
        .into_iter()
        .take(MAX_TEST_SUMMARIES)
        .map(|intent| format!("- {}: {}", intent.test_name, intent.intent_text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_graph_entries(
    store: &aether_store::SqliteStore,
    record: &SymbolRecord,
    depth: u32,
) -> Result<Vec<GraphEntry>, AetherMcpError> {
    let mut entries = Vec::<GraphEntry>::new();
    let mut seen = HashSet::<String>::new();

    for edge in store.get_callers(record.qualified_name.as_str())? {
        let Some(caller) = store.get_symbol_record(edge.source_id.as_str())? else {
            continue;
        };
        if !seen.insert(format!("caller::{}", caller.id)) {
            continue;
        }
        entries.push(GraphEntry {
            section: "callers",
            relationship: inverse_relation_label(edge.edge_kind),
            depth: 1,
            qualified_name: caller.qualified_name.clone(),
            file_path: caller.file_path.clone(),
            intent_summary: read_neighbor_intent_summary(store, caller.id.as_str()),
        });
        if entries.len() >= MAX_GRAPH_NEIGHBORS {
            return Ok(entries);
        }
    }

    let direct_dependencies = store.get_dependencies(record.id.as_str())?;
    let mut frontier = Vec::<String>::new();
    for edge in direct_dependencies {
        let Some(target) =
            store.get_symbol_by_qualified_name(edge.target_qualified_name.as_str())?
        else {
            continue;
        };
        if seen.insert(format!("dependency::{}", target.id)) {
            frontier.push(target.id.clone());
            entries.push(GraphEntry {
                section: "callees",
                relationship: relation_label(edge.edge_kind),
                depth: 1,
                qualified_name: target.qualified_name.clone(),
                file_path: target.file_path.clone(),
                intent_summary: read_neighbor_intent_summary(store, target.id.as_str()),
            });
            if entries.len() >= MAX_GRAPH_NEIGHBORS {
                return Ok(entries);
            }
        }
    }

    if depth <= 1 {
        return Ok(entries);
    }

    let mut expanded = HashSet::<String>::new();
    for current_depth in 2..=depth {
        let mut next_frontier = Vec::<String>::new();
        for source_id in frontier {
            for edge in store.get_dependencies(source_id.as_str())? {
                let Some(target) =
                    store.get_symbol_by_qualified_name(edge.target_qualified_name.as_str())?
                else {
                    continue;
                };
                if target.id == record.id || !expanded.insert(target.id.clone()) {
                    continue;
                }
                next_frontier.push(target.id.clone());
                if seen.insert(format!("transitive::{}", target.id)) {
                    entries.push(GraphEntry {
                        section: "transitive_dependencies",
                        relationship: relation_label(edge.edge_kind),
                        depth: current_depth,
                        qualified_name: target.qualified_name.clone(),
                        file_path: target.file_path.clone(),
                        intent_summary: read_neighbor_intent_summary(store, target.id.as_str()),
                    });
                    if entries.len() >= MAX_GRAPH_NEIGHBORS {
                        return Ok(entries);
                    }
                }
            }
        }
        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }

    Ok(entries)
}

fn build_graph_layer(
    store: &aether_store::SqliteStore,
    record: &SymbolRecord,
    depth: u32,
) -> String {
    let entries = match build_graph_entries(store, record, depth) {
        Ok(entries) => entries,
        Err(err) => return format!("Unavailable: failed to build graph context: {err}"),
    };

    if entries.is_empty() {
        return "No callers or dependencies recorded.".to_owned();
    }

    let callers = entries
        .iter()
        .filter(|entry| entry.section == "callers")
        .collect::<Vec<_>>();
    let callees = entries
        .iter()
        .filter(|entry| entry.section == "callees")
        .collect::<Vec<_>>();
    let transitive = entries
        .iter()
        .filter(|entry| entry.section == "transitive_dependencies")
        .collect::<Vec<_>>();

    let mut sections = Vec::<String>::new();
    sections.push(format!("### Callers ({})", callers.len()));
    if callers.is_empty() {
        sections.push("- none".to_owned());
    } else {
        sections.extend(callers.into_iter().map(|entry| {
            format!(
                "- `{}` ({}) [{}] - {}",
                entry.qualified_name, entry.file_path, entry.relationship, entry.intent_summary
            )
        }));
    }

    sections.push(String::new());
    sections.push(format!("### Callees ({})", callees.len()));
    if callees.is_empty() {
        sections.push("- none".to_owned());
    } else {
        sections.extend(callees.into_iter().map(|entry| {
            format!(
                "- `{}` ({}) [{}] - {}",
                entry.qualified_name, entry.file_path, entry.relationship, entry.intent_summary
            )
        }));
    }

    if !transitive.is_empty() {
        sections.push(String::new());
        sections.push(format!(
            "### Transitive Dependencies ({})",
            transitive.len()
        ));
        sections.extend(transitive.into_iter().map(|entry| {
            format!(
                "- `{}` ({}) [{} depth {}] - {}",
                entry.qualified_name,
                entry.file_path,
                entry.relationship,
                entry.depth,
                entry.intent_summary
            )
        }));
    }

    sections.join("\n")
}

fn build_health_layer(server: &AetherMcpServer, record: &SymbolRecord) -> String {
    let total_symbols = server
        .state
        .store
        .list_all_symbol_ids()
        .map(|ids| ids.len())
        .unwrap_or(0);
    if total_symbols == 0 {
        return "Unavailable: no indexed symbols available for health analysis.".to_owned();
    }

    let request = HealthRequest {
        include: vec![
            HealthInclude::CriticalSymbols,
            HealthInclude::Cycles,
            HealthInclude::RiskHotspots,
        ],
        limit: total_symbols as u32,
        min_risk: 0.0,
    };
    let analyzer = match HealthAnalyzer::new(server.workspace()) {
        Ok(analyzer) => analyzer,
        Err(err) => return format!("Unavailable: failed to initialize health analyzer: {err}"),
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            return format!("Unavailable: failed to build runtime for health analysis: {err}");
        }
    };

    let report = runtime.block_on(async {
        if server.state.config.storage.graph_backend == GraphBackend::Surreal {
            match server.state.surreal_graph_for_health().await {
                Ok(graph) => analyzer.analyze_with_graph(&request, graph.as_ref()).await,
                Err(_) => analyzer.analyze(&request).await,
            }
        } else {
            analyzer.analyze(&request).await
        }
    });

    let report = match report {
        Ok(report) => report,
        Err(err) => return format!("Unavailable: health analysis failed: {err}"),
    };

    let Some(entry) = report
        .critical_symbols
        .iter()
        .find(|entry| entry.symbol_id == record.id)
    else {
        let note = report
            .notes
            .first()
            .cloned()
            .unwrap_or_else(|| "symbol metrics unavailable".to_owned());
        return format!("Unavailable: {note}");
    };

    let in_cycle = report
        .cycles
        .iter()
        .any(|cycle| cycle.symbols.iter().any(|symbol| symbol.id == record.id));

    format!(
        "- Risk Score: {:.2}\n- PageRank: {:.4}\n- Betweenness: {:.4}\n- In Cycle: {}\n- Test Count: {}",
        entry.risk_score,
        entry.pagerank,
        entry.betweenness,
        if in_cycle { "yes" } else { "no" },
        entry.test_count
    )
}

fn markdown_title(record: &SymbolRecord) -> String {
    format!("# Context: {}\n", record.qualified_name)
}

fn render_markdown_context(record: &SymbolRecord, layers: &[SirContextLayer]) -> String {
    let mut parts = vec![markdown_title(record)];
    for layer in layers {
        let title = match layer.name.as_str() {
            "source" => "Source",
            "sir" => "SIR Annotation",
            "graph" => "Dependencies",
            "health" => "Health",
            "reasoning" => "Reasoning Trace",
            "tests" => "Test Intents",
            _ => layer.name.as_str(),
        };
        parts.push(format!("## {title}"));
        if layer.name == "source" {
            parts.push(format!("```{}\n{}\n```", record.language, layer.content));
        } else {
            parts.push(layer.content.clone());
        }
        parts.push(String::new());
    }
    parts.join("\n")
}

fn render_json_context(
    record: &SymbolRecord,
    layers: &[SirContextLayer],
    total_tokens: usize,
    budget_remaining: usize,
) -> Result<String, AetherMcpError> {
    let mut object = Map::<String, Value>::new();
    object.insert("symbol_id".to_owned(), Value::String(record.id.clone()));
    object.insert(
        "qualified_name".to_owned(),
        Value::String(record.qualified_name.clone()),
    );
    object.insert(
        "file_path".to_owned(),
        Value::String(record.file_path.clone()),
    );
    object.insert("total_tokens".to_owned(), json!(total_tokens));
    object.insert("budget_remaining".to_owned(), json!(budget_remaining));
    for layer in layers {
        object.insert(layer.name.clone(), Value::String(layer.content.clone()));
    }
    serde_json::to_string_pretty(&Value::Object(object)).map_err(Into::into)
}

impl AetherMcpServer {
    pub fn aether_sir_context_logic(
        &self,
        request: AetherSirContextRequest,
    ) -> Result<AetherSirContextResponse, AetherMcpError> {
        let max_tokens = parse_max_tokens(request.max_tokens)?;
        let format = parse_context_format(request.format.as_deref())?;
        let include_layers = parse_include_layers(request.include_layers)?;
        let depth = parse_depth(request.depth)?;

        let store = self.state.store.as_ref();
        let record = resolve_symbol_selector(store, request.symbol.as_str())?;

        let mut layers = Vec::<SirContextLayer>::new();
        for layer_name in LAYER_ORDER {
            if !include_layers.contains(layer_name) {
                continue;
            }

            let content = match layer_name {
                "source" => build_source_layer(self, &record),
                "sir" => build_sir_layer(self, &record),
                "graph" => build_graph_layer(store, &record, depth),
                "health" => build_health_layer(self, &record),
                "reasoning" => build_reasoning_layer(self, &record),
                "tests" => build_tests_layer(store, &record),
                _ => continue,
            };
            layers.push(SirContextLayer {
                name: layer_name.to_owned(),
                token_estimate: estimate_tokens(content.as_str()),
                content,
            });
        }

        let layers = trim_layers_to_budget(layers, max_tokens);
        let total_tokens = layers
            .iter()
            .map(|layer| layer.token_estimate)
            .sum::<usize>();
        let budget_remaining = max_tokens.saturating_sub(total_tokens);
        let context = match format {
            ContextFormat::Markdown => render_markdown_context(&record, &layers),
            ContextFormat::Json => {
                render_json_context(&record, &layers, total_tokens, budget_remaining)?
            }
        };

        Ok(AetherSirContextResponse {
            symbol_id: record.id,
            qualified_name: record.qualified_name,
            file_path: record.file_path,
            context,
            layers,
            total_tokens,
            budget_remaining,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Write};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use aether_core::{EdgeKind, SymbolEdge};
    use aether_parse::SymbolExtractor;
    use aether_store::{
        SirMetaRecord, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord,
        SymbolRelationStore, TestIntentRecord, TestIntentStore,
    };
    use serde_json::Value;
    use tempfile::tempdir;
    use tracing::dispatcher::{self, Dispatch};
    use tracing_subscriber::fmt::MakeWriter;

    use super::{AetherSirContextRequest, AetherSirContextResponse, read_neighbor_intent_summary};
    use crate::AetherMcpServer;

    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter(self.0.clone())
        }
    }

    impl Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn capture_logs<T>(run: impl FnOnce() -> T) -> (T, String) {
        let buffer = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_writer(buffer.clone())
            .finish();
        let result = dispatcher::with_default(&Dispatch::new(subscriber), run);
        let logs = String::from_utf8(buffer.0.lock().expect("log buffer lock").clone())
            .expect("utf8 logs");
        (result, logs)
    }

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn write_demo_workspace(workspace: &Path, large_target: bool) -> String {
        fs::create_dir_all(workspace.join("src")).expect("mkdir src");
        fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"mcp-context-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write cargo");
        let target_body = if large_target {
            (0..160)
                .map(|index| format!("    let value_{index} = {index};\n"))
                .collect::<String>()
        } else {
            "    helper();\n    callee();\n".to_owned()
        };
        let source = format!(
            "pub fn helper() -> i32 {{\n    1\n}}\n\npub fn callee() -> i32 {{\n    helper()\n}}\n\npub fn target() -> i32 {{\n{target_body}    callee()\n}}\n\npub fn caller() -> i32 {{\n    target()\n}}\n"
        );
        let relative = "src/lib.rs";
        fs::write(workspace.join(relative), source).expect("write source");
        relative.to_owned()
    }

    fn parse_symbols(workspace: &Path, relative: &str) -> Vec<aether_core::Symbol> {
        let source = fs::read_to_string(workspace.join(relative)).expect("read source");
        let mut extractor = SymbolExtractor::new().expect("extractor");
        extractor
            .extract_from_source(aether_core::Language::Rust, relative, &source)
            .expect("parse symbols")
    }

    fn symbol_record(symbol: &aether_core::Symbol) -> SymbolRecord {
        SymbolRecord {
            id: symbol.id.clone(),
            file_path: symbol.file_path.clone(),
            language: symbol.language.as_str().to_owned(),
            kind: symbol.kind.as_str().to_owned(),
            qualified_name: symbol.qualified_name.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            last_seen_at: 1_700_000_000,
        }
    }

    fn seed_workspace(workspace: &Path, large_target: bool) -> Vec<aether_core::Symbol> {
        write_test_config(workspace);
        let relative = write_demo_workspace(workspace, large_target);
        let symbols = parse_symbols(workspace, &relative);
        let store = aether_store::SqliteStore::open(workspace).expect("open store");

        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            let leaf = symbol
                .qualified_name
                .rsplit("::")
                .next()
                .unwrap_or(symbol.qualified_name.as_str());
            let sir_json = format!(
                "{{\"intent\":\"{} intent summary\",\"inputs\":[],\"outputs\":[],\"side_effects\":[\"writes state\"],\"dependencies\":[],\"error_modes\":[\"io\"],\"confidence\":0.72}}",
                leaf
            );
            store
                .write_sir_blob(symbol.id.as_str(), sir_json.as_str())
                .expect("write sir");
            store
                .upsert_sir_meta(SirMetaRecord {
                    id: symbol.id.clone(),
                    sir_hash: format!("hash-{}", symbol.id),
                    sir_version: 1,
                    provider: "mock".to_owned(),
                    model: "mock".to_owned(),
                    generation_pass: "triage".to_owned(),
                    reasoning_trace: if leaf == "target" {
                        Some("Model expressed uncertainty about rollback behavior.".to_owned())
                    } else {
                        None
                    },
                    prompt_hash: None,
                    staleness_score: Some(0.12),
                    updated_at: 1_700_000_100,
                    sir_status: "fresh".to_owned(),
                    last_error: None,
                    last_attempt_at: 1_700_000_100,
                })
                .expect("upsert meta");
        }

        let target = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("target"))
            .expect("target symbol");
        let caller = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("caller"))
            .expect("caller symbol");
        let callee = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("callee"))
            .expect("callee symbol");

        store
            .upsert_edges(&[
                SymbolEdge {
                    source_id: caller.id.clone(),
                    target_qualified_name: target.qualified_name.clone(),
                    edge_kind: EdgeKind::Calls,
                    file_path: caller.file_path.clone(),
                },
                SymbolEdge {
                    source_id: target.id.clone(),
                    target_qualified_name: callee.qualified_name.clone(),
                    edge_kind: EdgeKind::Calls,
                    file_path: target.file_path.clone(),
                },
            ])
            .expect("upsert edges");
        store
            .replace_test_intents_for_file(
                target.file_path.as_str(),
                &[TestIntentRecord {
                    intent_id: "intent-target".to_owned(),
                    file_path: target.file_path.clone(),
                    test_name: "test_target_path".to_owned(),
                    intent_text: "verifies target handles normal execution".to_owned(),
                    group_label: None,
                    language: "rust".to_owned(),
                    symbol_id: Some(target.id.clone()),
                    created_at: 1_700_000_000_000,
                    updated_at: 1_700_000_000_100,
                }],
            )
            .expect("replace test intents");

        symbols
    }

    fn context_response(
        server: &AetherMcpServer,
        request: AetherSirContextRequest,
    ) -> AetherSirContextResponse {
        server
            .aether_sir_context_logic(request)
            .expect("sir context should succeed")
    }

    #[test]
    fn basic_context_assembly_includes_source_sir_graph_and_tests() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(temp.path(), false);
        let target = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("target"))
            .expect("target");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = context_response(
            &server,
            AetherSirContextRequest {
                symbol: target.id.clone(),
                max_tokens: Some(8_000),
                format: None,
                include_layers: None,
                depth: Some(2),
            },
        );

        assert_eq!(response.symbol_id, target.id);
        assert!(response.context.contains("# Context:"));
        assert!(response.context.contains("## Source"));
        assert!(response.context.contains("pub fn target()"));
        assert!(response.context.contains("## SIR Annotation"));
        assert!(
            response
                .context
                .contains("**Intent:** target intent summary")
        );
        assert!(response.context.contains("## Dependencies"));
        assert!(response.context.contains("caller"));
        assert!(response.context.contains("callee"));
        assert!(response.context.contains("## Reasoning Trace"));
        assert!(response.context.contains("rollback behavior"));
        assert!(response.context.contains("## Test Intents"));
        assert!(response.context.contains("test_target_path"));
        assert!(response.layers.iter().any(|layer| layer.name == "health"));
    }

    #[test]
    fn token_budget_trimming_truncates_large_source() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(temp.path(), true);
        let target = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("target"))
            .expect("target");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = context_response(
            &server,
            AetherSirContextRequest {
                symbol: target.qualified_name.clone(),
                max_tokens: Some(120),
                format: None,
                include_layers: Some(vec!["source".to_owned(), "sir".to_owned()]),
                depth: Some(1),
            },
        );

        let source_layer = response
            .layers
            .iter()
            .find(|layer| layer.name == "source")
            .expect("source layer");
        assert!(source_layer.content.contains("[... truncated ...]"));
        assert!(response.total_tokens <= 120 || response.budget_remaining == 0);
    }

    #[test]
    fn include_layers_filter_only_returns_requested_layers() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(temp.path(), false);
        let target = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("target"))
            .expect("target");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = context_response(
            &server,
            AetherSirContextRequest {
                symbol: target.id.clone(),
                max_tokens: None,
                format: None,
                include_layers: Some(vec!["sir".to_owned(), "reasoning".to_owned()]),
                depth: None,
            },
        );

        let names = response
            .layers
            .iter()
            .map(|layer| layer.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["sir", "reasoning"]);
        assert!(!response.context.contains("## Source"));
        assert!(response.context.contains("## SIR Annotation"));
        assert!(response.context.contains("## Reasoning Trace"));
    }

    #[test]
    fn json_format_returns_structured_context_string() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(temp.path(), false);
        let target = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("target"))
            .expect("target");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = context_response(
            &server,
            AetherSirContextRequest {
                symbol: target.id.clone(),
                max_tokens: None,
                format: Some("json".to_owned()),
                include_layers: Some(vec!["source".to_owned(), "sir".to_owned()]),
                depth: None,
            },
        );

        let value: Value = serde_json::from_str(&response.context).expect("parse json context");
        assert_eq!(value["symbol_id"], target.id);
        assert!(value["source"].as_str().is_some());
        assert!(value["sir"].as_str().is_some());
        assert!(response.context.contains("\"qualified_name\""));
        assert!(!response.context.contains("## Source"));
    }

    #[test]
    fn symbol_not_found_returns_error() {
        let temp = tempdir().expect("tempdir");
        seed_workspace(temp.path(), false);
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let err = server
            .aether_sir_context_logic(AetherSirContextRequest {
                symbol: "missing::symbol".to_owned(),
                max_tokens: None,
                format: None,
                include_layers: None,
                depth: None,
            })
            .expect_err("missing symbol should fail");
        assert!(err.to_string().contains("symbol not found"));
    }

    #[test]
    fn invalid_format_layers_and_depth_are_rejected() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(temp.path(), false);
        let target = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("target"))
            .expect("target");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let invalid_format = server
            .aether_sir_context_logic(AetherSirContextRequest {
                symbol: target.id.clone(),
                max_tokens: None,
                format: Some("text".to_owned()),
                include_layers: None,
                depth: None,
            })
            .expect_err("invalid format should fail");
        assert!(invalid_format.to_string().contains("invalid format"));

        let invalid_layer = server
            .aether_sir_context_logic(AetherSirContextRequest {
                symbol: target.id.clone(),
                max_tokens: None,
                format: None,
                include_layers: Some(vec!["memory".to_owned()]),
                depth: None,
            })
            .expect_err("invalid layer should fail");
        assert!(invalid_layer.to_string().contains("invalid layer"));

        let invalid_depth = server
            .aether_sir_context_logic(AetherSirContextRequest {
                symbol: target.id.clone(),
                max_tokens: None,
                format: None,
                include_layers: None,
                depth: Some(4),
            })
            .expect_err("invalid depth should fail");
        assert!(
            invalid_depth
                .to_string()
                .contains("depth must be between 1 and 3")
        );
    }

    #[test]
    fn read_neighbor_intent_summary_returns_placeholder_and_logs_on_parse_error() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .write_sir_blob("sym-invalid", "{invalid")
            .expect("write invalid sir");

        let (summary, logs) = capture_logs(|| read_neighbor_intent_summary(&store, "sym-invalid"));

        assert_eq!(summary, "Unreadable SIR.");
        assert!(logs.contains("failed to parse SIR blob for context neighbor summary"));
        assert!(logs.contains("sym-invalid"));
    }
}
