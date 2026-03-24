use std::collections::{HashSet, VecDeque};
use std::fs;

use aether_core::{EdgeKind, Position, SourceRange};
use aether_parse::{SymbolExtractor, language_for_path};
use aether_sir::SirAnnotation;
use aether_store::{
    SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord, SymbolRelationStore,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::AetherMcpServer;
use crate::AetherMcpError;

const CHARS_PER_TOKEN: usize = 4;
const DEFAULT_MAX_SOURCE_TOKENS: usize = 2_000;
const DEFAULT_MAX_SYMBOLS: u32 = 20;
const MAX_REASONING_CHARS: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditCrossSymbolRequest {
    /// Root symbol ID or qualified name
    pub root_symbol: String,
    /// Direction to traverse: "callers", "callees", or "both"
    pub direction: Option<String>,
    /// Traversal depth (1-3, default 2)
    pub depth: Option<u32>,
    /// Include source code for each symbol (default true)
    pub include_source: Option<bool>,
    /// Maximum total symbols in chain (default 20, max 30)
    pub max_symbols: Option<u32>,
    /// Maximum tokens per symbol's source (default 2000)
    pub max_source_tokens: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CrossSymbolNode {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub kind: String,
    /// "root", "caller", or "callee"
    pub role: String,
    /// Distance from root (0 = root)
    pub depth: u32,
    /// Edge kind connecting to parent in chain (e.g., "calls", "implements")
    pub edge_kind: Option<String>,
    /// SIR annotation if available
    pub sir: Option<CrossSymbolSir>,
    /// Source code (truncated if over budget)
    pub source: Option<String>,
    /// Reasoning trace excerpt if available
    pub reasoning_trace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CrossSymbolSir {
    pub intent: String,
    pub side_effects: Vec<String>,
    pub error_modes: Vec<String>,
    pub confidence: f32,
    pub generation_pass: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditCrossSymbolResponse {
    pub root_symbol_id: String,
    pub root_qualified_name: String,
    /// All symbols in the chain, ordered: root first, then by depth
    pub chain: Vec<CrossSymbolNode>,
    /// Total symbols found before max_symbols cap
    pub total_found: u32,
    /// Human-readable summary of the traversal
    pub traversal_summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraversalDirection {
    Callers,
    Callees,
    Both,
}

impl TraversalDirection {
    fn parse(raw: Option<&str>) -> Result<Self, AetherMcpError> {
        match raw.unwrap_or("both").trim().to_ascii_lowercase().as_str() {
            "callers" => Ok(Self::Callers),
            "callees" => Ok(Self::Callees),
            "both" => Ok(Self::Both),
            other => Err(AetherMcpError::Message(format!(
                "invalid direction '{other}', expected callers, callees, or both"
            ))),
        }
    }

    fn includes_callers(self) -> bool {
        matches!(self, Self::Callers | Self::Both)
    }

    fn includes_callees(self) -> bool {
        matches!(self, Self::Callees | Self::Both)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Callers => "callers",
            Self::Callees => "callees",
            Self::Both => "both directions",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeRole {
    Root,
    Caller,
    Callee,
}

impl NodeRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Caller => "caller",
            Self::Callee => "callee",
        }
    }
}

#[derive(Debug, Clone)]
struct QueueEntry {
    symbol_id: String,
    depth: u32,
    role: NodeRole,
    incoming_edge_kind: Option<EdgeKind>,
}

fn resolve_symbol_selector(
    store: &SqliteStore,
    selector: &str,
) -> Result<SymbolRecord, AetherMcpError> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(AetherMcpError::Message(
            "root_symbol must not be empty".to_owned(),
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

fn truncate_by_char_count(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_owned();
    }

    let end = content
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(content.len());

    content[..end].to_owned()
}

fn take_last_chars(content: &str, max_chars: usize) -> String {
    let total_chars = content.chars().count();
    if total_chars <= max_chars {
        return content.to_owned();
    }

    let start = content
        .char_indices()
        .nth(total_chars.saturating_sub(max_chars))
        .map(|(index, _)| index)
        .unwrap_or(0);

    content[start..].to_owned()
}

fn middle_truncate_by_chars(content: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if content.chars().count() <= max_chars {
        return content.to_owned();
    }

    let marker = "[... truncated ...]";
    let marker_chars = marker.chars().count();
    if max_chars <= marker_chars + 8 {
        return truncate_by_char_count(marker, max_chars.max(1));
    }

    let prefix_chars = (max_chars - marker_chars) / 2;
    let suffix_chars = max_chars - marker_chars - prefix_chars;
    let prefix = truncate_by_char_count(content, prefix_chars);
    let suffix = take_last_chars(content, suffix_chars);
    format!("{prefix}{marker}{suffix}")
}

fn middle_truncate_by_tokens(content: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    middle_truncate_by_chars(content, max_chars)
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

fn extract_node_source(
    server: &AetherMcpServer,
    record: &SymbolRecord,
    max_source_tokens: usize,
) -> Option<String> {
    let full_path = server.workspace().join(&record.file_path);
    let source = fs::read_to_string(&full_path).ok()?;
    let truncated_full_file = middle_truncate_by_tokens(&source, max_source_tokens);

    let Some(language) = language_for_path(&full_path) else {
        return Some(truncated_full_file);
    };

    let mut extractor = match SymbolExtractor::new() {
        Ok(extractor) => extractor,
        Err(_) => return Some(truncated_full_file),
    };

    let symbols = match extractor.extract_from_source(language, &record.file_path, &source) {
        Ok(symbols) => symbols,
        Err(_) => return Some(truncated_full_file),
    };

    let symbol_source = symbols
        .iter()
        .find(|symbol| symbol.id == record.id || symbol.qualified_name == record.qualified_name)
        .and_then(|symbol| extract_symbol_source_text(&source, symbol.range))
        .unwrap_or(source);

    Some(middle_truncate_by_tokens(
        symbol_source.as_str(),
        max_source_tokens,
    ))
}

fn read_cross_symbol_sir(
    store: &SqliteStore,
    symbol_id: &str,
) -> Result<Option<CrossSymbolSir>, AetherMcpError> {
    let meta = store.get_sir_meta(symbol_id)?;
    let blob = store.read_sir_blob(symbol_id)?;
    let Some(blob) = blob else {
        return Ok(None);
    };
    let Ok(sir) = serde_json::from_str::<SirAnnotation>(&blob) else {
        return Ok(None);
    };

    Ok(Some(CrossSymbolSir {
        intent: sir.intent,
        side_effects: sir.side_effects,
        error_modes: sir.error_modes,
        confidence: sir.confidence,
        generation_pass: meta.and_then(|entry| {
            let generation_pass = entry.generation_pass.trim();
            (!generation_pass.is_empty()).then(|| generation_pass.to_owned())
        }),
    }))
}

fn read_reasoning_trace(
    store: &SqliteStore,
    symbol_id: &str,
) -> Result<Option<String>, AetherMcpError> {
    let Some(meta) = store.get_sir_meta(symbol_id)? else {
        return Ok(None);
    };
    let reasoning_trace = meta.reasoning_trace.unwrap_or_default();
    let reasoning_trace = reasoning_trace.trim();
    if reasoning_trace.is_empty() {
        return Ok(None);
    }
    Ok(Some(middle_truncate_by_chars(
        reasoning_trace,
        MAX_REASONING_CHARS,
    )))
}

impl AetherMcpServer {
    pub fn aether_audit_cross_symbol_logic(
        &self,
        request: AetherAuditCrossSymbolRequest,
    ) -> Result<AetherAuditCrossSymbolResponse, AetherMcpError> {
        let direction = TraversalDirection::parse(request.direction.as_deref())?;
        let max_depth = request.depth.unwrap_or(2).clamp(1, 3);
        let include_source = request.include_source.unwrap_or(true);
        let max_symbols = request
            .max_symbols
            .unwrap_or(DEFAULT_MAX_SYMBOLS)
            .clamp(1, 30) as usize;
        let max_source_tokens = request
            .max_source_tokens
            .unwrap_or(DEFAULT_MAX_SOURCE_TOKENS);
        if max_source_tokens == 0 {
            return Err(AetherMcpError::Message(
                "max_source_tokens must be greater than 0".to_owned(),
            ));
        }

        let store = self.state.store.as_ref();
        let root_record = resolve_symbol_selector(store, request.root_symbol.as_str())?;

        let mut visited = HashSet::<String>::new();
        let mut queue = VecDeque::<QueueEntry>::new();
        let mut chain = Vec::<CrossSymbolNode>::new();
        let mut caller_count = 0u32;
        let mut callee_count = 0u32;
        let mut max_discovered_depth = 0u32;

        visited.insert(root_record.id.clone());
        queue.push_back(QueueEntry {
            symbol_id: root_record.id.clone(),
            depth: 0,
            role: NodeRole::Root,
            incoming_edge_kind: None,
        });

        while let Some(entry) = queue.pop_front() {
            let Some(record) = store.get_symbol_record(entry.symbol_id.as_str())? else {
                continue;
            };

            if chain.len() < max_symbols {
                chain.push(CrossSymbolNode {
                    symbol_id: record.id.clone(),
                    qualified_name: record.qualified_name.clone(),
                    file_path: record.file_path.clone(),
                    kind: record.kind.clone(),
                    role: entry.role.as_str().to_owned(),
                    depth: entry.depth,
                    edge_kind: entry
                        .incoming_edge_kind
                        .map(|kind| kind.as_str().to_owned()),
                    sir: read_cross_symbol_sir(store, record.id.as_str())?,
                    source: include_source
                        .then(|| extract_node_source(self, &record, max_source_tokens))
                        .flatten(),
                    reasoning_trace: read_reasoning_trace(store, record.id.as_str())?,
                });
            }

            if entry.depth >= max_depth {
                continue;
            }

            if direction.includes_callees() {
                for edge in store.list_symbol_edges_for_source_and_kinds(
                    record.id.as_str(),
                    &[EdgeKind::Calls],
                )? {
                    let Some(target) =
                        store.get_symbol_by_qualified_name(edge.target_qualified_name.as_str())?
                    else {
                        continue;
                    };
                    if !visited.insert(target.id.clone()) {
                        continue;
                    }
                    let next_depth = entry.depth + 1;
                    callee_count += 1;
                    max_discovered_depth = max_discovered_depth.max(next_depth);
                    queue.push_back(QueueEntry {
                        symbol_id: target.id,
                        depth: next_depth,
                        role: NodeRole::Callee,
                        incoming_edge_kind: Some(edge.edge_kind),
                    });
                }
            }

            if direction.includes_callers() {
                for edge in store.get_callers(record.qualified_name.as_str())? {
                    let Some(caller) = store.get_symbol_record(edge.source_id.as_str())? else {
                        continue;
                    };
                    if !visited.insert(caller.id.clone()) {
                        continue;
                    }
                    let next_depth = entry.depth + 1;
                    caller_count += 1;
                    max_discovered_depth = max_discovered_depth.max(next_depth);
                    queue.push_back(QueueEntry {
                        symbol_id: caller.id,
                        depth: next_depth,
                        role: NodeRole::Caller,
                        incoming_edge_kind: Some(edge.edge_kind),
                    });
                }
            }
        }

        let total_found = visited.len().min(u32::MAX as usize) as u32;
        let traversal_summary = format!(
            "Traversed {} from {}: {} callers, {} callees across {} levels ({} symbols returned, {} found before cap)",
            direction.label(),
            root_record.qualified_name,
            caller_count,
            callee_count,
            max_discovered_depth,
            chain.len(),
            total_found,
        );

        Ok(AetherAuditCrossSymbolResponse {
            root_symbol_id: root_record.id,
            root_qualified_name: root_record.qualified_name,
            chain,
            total_found,
            traversal_summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    use aether_core::{EdgeKind, SymbolEdge};
    use aether_parse::SymbolExtractor;
    use aether_store::{
        SirMetaRecord, SirStateStore, SymbolCatalogStore, SymbolRecord, SymbolRelationStore,
    };
    use tempfile::tempdir;

    use super::{
        AetherAuditCrossSymbolRequest, NodeRole, middle_truncate_by_chars, truncate_by_char_count,
    };
    use crate::AetherMcpServer;

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

    fn write_source(workspace: &Path, source: &str) -> String {
        fs::create_dir_all(workspace.join("src")).expect("mkdir src");
        fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"mcp-cross-symbol-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write cargo");
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

    fn leaf_name(qualified_name: &str) -> &str {
        qualified_name.rsplit("::").next().unwrap_or(qualified_name)
    }

    fn seed_workspace(workspace: &Path, source: &str) -> HashMap<String, SymbolRecord> {
        write_test_config(workspace);
        let relative = write_source(workspace, source);
        let symbols = parse_symbols(workspace, &relative);
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        let mut by_leaf = HashMap::new();

        for symbol in symbols {
            let record = symbol_record(&symbol);
            by_leaf.insert(leaf_name(&record.qualified_name).to_owned(), record.clone());
            store.upsert_symbol(record).expect("upsert symbol");
        }

        by_leaf
    }

    fn seed_call_edge(workspace: &Path, source: &SymbolRecord, target: &SymbolRecord) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        store
            .upsert_edges(&[SymbolEdge {
                source_id: source.id.clone(),
                target_qualified_name: target.qualified_name.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: source.file_path.clone(),
            }])
            .expect("upsert edge");
    }

    fn seed_sir(workspace: &Path, symbol: &SymbolRecord) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        store
            .write_sir_blob(
                symbol.id.as_str(),
                r#"{
                    "intent":"seeded root intent",
                    "inputs":[],
                    "outputs":[],
                    "side_effects":["writes state"],
                    "dependencies":[],
                    "error_modes":["io"],
                    "confidence":0.72
                }"#,
            )
            .expect("write sir");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol.id.clone(),
                sir_hash: format!("hash-{}", symbol.id),
                sir_version: 1,
                provider: "test".to_owned(),
                model: "test".to_owned(),
                generation_pass: "triage".to_owned(),
                reasoning_trace: Some(
                    "Model was uncertain about cleanup and rollback interactions across callers."
                        .to_owned(),
                ),
                prompt_hash: None,
                staleness_score: None,
                updated_at: 1_700_000_001,
                sir_status: "ready".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_001,
            })
            .expect("upsert sir meta");
    }

    fn role_count<T: AsRef<str>>(values: &[T], role: NodeRole) -> usize {
        values
            .iter()
            .filter(|value| value.as_ref() == role.as_str())
            .count()
    }

    #[test]
    fn cross_symbol_basic_traversal() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(
            temp.path(),
            r#"
pub fn callee_one() -> i32 { 1 }
pub fn callee_two() -> i32 { 2 }
pub fn root() -> i32 { callee_one() + callee_two() }
"#,
        );
        seed_call_edge(temp.path(), &symbols["root"], &symbols["callee_one"]);
        seed_call_edge(temp.path(), &symbols["root"], &symbols["callee_two"]);

        let server = AetherMcpServer::new(temp.path(), false).expect("server");
        let response = server
            .aether_audit_cross_symbol_logic(AetherAuditCrossSymbolRequest {
                root_symbol: symbols["root"].id.clone(),
                direction: Some("callees".to_owned()),
                depth: Some(1),
                include_source: Some(false),
                max_symbols: Some(10),
                max_source_tokens: None,
            })
            .expect("cross symbol");

        let names = response
            .chain
            .iter()
            .map(|node| leaf_name(&node.qualified_name).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["root", "callee_one", "callee_two"]);
        assert_eq!(response.total_found, 3);
    }

    #[test]
    fn cross_symbol_callers_direction() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(
            temp.path(),
            r#"
pub fn root() -> i32 { 1 }
pub fn caller_one() -> i32 { root() }
pub fn caller_two() -> i32 { root() }
"#,
        );
        seed_call_edge(temp.path(), &symbols["caller_one"], &symbols["root"]);
        seed_call_edge(temp.path(), &symbols["caller_two"], &symbols["root"]);

        let server = AetherMcpServer::new(temp.path(), false).expect("server");
        let response = server
            .aether_audit_cross_symbol_logic(AetherAuditCrossSymbolRequest {
                root_symbol: symbols["root"].qualified_name.clone(),
                direction: Some("callers".to_owned()),
                depth: Some(1),
                include_source: Some(false),
                max_symbols: Some(10),
                max_source_tokens: None,
            })
            .expect("cross symbol");

        assert_eq!(response.chain.len(), 3);
        let roles = response
            .chain
            .iter()
            .map(|node| node.role.as_str())
            .collect::<Vec<_>>();
        assert_eq!(role_count(&roles, NodeRole::Caller), 2);
        assert!(
            response
                .chain
                .iter()
                .skip(1)
                .all(|node| node.role == "caller")
        );
    }

    #[test]
    fn cross_symbol_both_directions() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(
            temp.path(),
            r#"
pub fn callee_one() -> i32 { 1 }
pub fn callee_two() -> i32 { 2 }
pub fn root() -> i32 { callee_one() + callee_two() }
pub fn caller_one() -> i32 { root() }
pub fn caller_two() -> i32 { root() }
"#,
        );
        seed_call_edge(temp.path(), &symbols["root"], &symbols["callee_one"]);
        seed_call_edge(temp.path(), &symbols["root"], &symbols["callee_two"]);
        seed_call_edge(temp.path(), &symbols["caller_one"], &symbols["root"]);
        seed_call_edge(temp.path(), &symbols["caller_two"], &symbols["root"]);

        let server = AetherMcpServer::new(temp.path(), false).expect("server");
        let response = server
            .aether_audit_cross_symbol_logic(AetherAuditCrossSymbolRequest {
                root_symbol: symbols["root"].id.clone(),
                direction: Some("both".to_owned()),
                depth: Some(1),
                include_source: Some(false),
                max_symbols: Some(10),
                max_source_tokens: None,
            })
            .expect("cross symbol");

        let roles = response
            .chain
            .iter()
            .map(|node| node.role.as_str())
            .collect::<Vec<_>>();
        assert_eq!(response.chain.len(), 5);
        assert_eq!(role_count(&roles, NodeRole::Callee), 2);
        assert_eq!(role_count(&roles, NodeRole::Caller), 2);
        let first_neighbors = response
            .chain
            .iter()
            .skip(1)
            .take(2)
            .map(|node| leaf_name(&node.qualified_name).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(first_neighbors, vec!["callee_one", "callee_two"]);
    }

    #[test]
    fn cross_symbol_depth_limit() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(
            temp.path(),
            r#"
pub fn level_three() -> i32 { 3 }
pub fn level_two() -> i32 { level_three() }
pub fn level_one() -> i32 { level_two() }
pub fn root() -> i32 { level_one() }
"#,
        );
        seed_call_edge(temp.path(), &symbols["root"], &symbols["level_one"]);
        seed_call_edge(temp.path(), &symbols["level_one"], &symbols["level_two"]);
        seed_call_edge(temp.path(), &symbols["level_two"], &symbols["level_three"]);

        let server = AetherMcpServer::new(temp.path(), false).expect("server");
        let response = server
            .aether_audit_cross_symbol_logic(AetherAuditCrossSymbolRequest {
                root_symbol: symbols["root"].id.clone(),
                direction: Some("callees".to_owned()),
                depth: Some(1),
                include_source: Some(false),
                max_symbols: Some(10),
                max_source_tokens: None,
            })
            .expect("cross symbol");

        let names = response
            .chain
            .iter()
            .map(|node| leaf_name(&node.qualified_name).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["root", "level_one"]);
        assert_eq!(response.total_found, 2);
    }

    #[test]
    fn cross_symbol_max_symbols_cap() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(
            temp.path(),
            r#"
pub fn a() -> i32 { 1 }
pub fn b() -> i32 { 2 }
pub fn c() -> i32 { 3 }
pub fn d() -> i32 { 4 }
pub fn e() -> i32 { 5 }
pub fn root() -> i32 { a() + b() + c() + d() + e() }
"#,
        );
        for name in ["a", "b", "c", "d", "e"] {
            seed_call_edge(temp.path(), &symbols["root"], &symbols[name]);
        }

        let server = AetherMcpServer::new(temp.path(), false).expect("server");
        let response = server
            .aether_audit_cross_symbol_logic(AetherAuditCrossSymbolRequest {
                root_symbol: symbols["root"].id.clone(),
                direction: Some("callees".to_owned()),
                depth: Some(1),
                include_source: Some(false),
                max_symbols: Some(3),
                max_source_tokens: None,
            })
            .expect("cross symbol");

        assert_eq!(response.chain.len(), 3);
        assert_eq!(response.total_found, 6);
        assert_eq!(leaf_name(&response.chain[0].qualified_name), "root");
    }

    #[test]
    fn cross_symbol_includes_sir_and_source() {
        let temp = tempdir().expect("tempdir");
        let symbols = seed_workspace(
            temp.path(),
            r#"
pub fn helper() -> i32 { 1 }
pub fn root() -> i32 { helper() }
"#,
        );
        seed_call_edge(temp.path(), &symbols["root"], &symbols["helper"]);
        seed_sir(temp.path(), &symbols["root"]);

        let server = AetherMcpServer::new(temp.path(), false).expect("server");
        let response = server
            .aether_audit_cross_symbol_logic(AetherAuditCrossSymbolRequest {
                root_symbol: symbols["root"].id.clone(),
                direction: Some("callees".to_owned()),
                depth: Some(1),
                include_source: Some(true),
                max_symbols: Some(10),
                max_source_tokens: Some(200),
            })
            .expect("cross symbol");

        let root = response.chain.first().expect("root node");
        assert_eq!(root.role, "root");
        assert_eq!(root.edge_kind, None);
        assert_eq!(
            root.sir.as_ref().map(|sir| sir.intent.as_str()),
            Some("seeded root intent")
        );
        assert!(
            root.source
                .as_deref()
                .is_some_and(|source| source.contains("pub fn root()"))
        );
        assert!(
            root.reasoning_trace
                .as_deref()
                .is_some_and(|trace| trace.contains("uncertain"))
        );
    }

    #[test]
    fn cross_symbol_root_not_found() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let err = server
            .aether_audit_cross_symbol_logic(AetherAuditCrossSymbolRequest {
                root_symbol: "missing::symbol".to_owned(),
                direction: None,
                depth: None,
                include_source: None,
                max_symbols: None,
                max_source_tokens: None,
            })
            .expect_err("missing symbol should fail");

        assert!(err.to_string().contains("symbol not found"));
    }

    #[test]
    fn truncate_by_char_count_preserves_multibyte_chars_with_char_limits() {
        let content = "😀😀😀";

        assert_eq!(truncate_by_char_count(content, 8), content);
        assert_eq!(truncate_by_char_count(content, 2), "😀😀");
    }

    #[test]
    fn middle_truncate_by_chars_counts_multibyte_prefix_and_suffix_by_char() {
        let content = format!("{}payload{}", "😀".repeat(12), "界".repeat(12));

        let truncated = middle_truncate_by_chars(&content, 30);

        assert_eq!(truncated.chars().count(), 30);
        assert!(truncated.starts_with(&"😀".repeat(5)));
        assert!(truncated.ends_with(&"界".repeat(6)));
    }
}
