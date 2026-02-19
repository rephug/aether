use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_config::{SearchRerankerKind, VerifyMode, load_workspace_config};
pub use aether_core::SearchMode;
use aether_core::{
    HoverMarkdownSections, Language, NO_SIR_MESSAGE, SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR,
    SEARCH_FALLBACK_EMBEDDINGS_DISABLED, SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED,
    SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY, SearchEnvelope, SourceRange,
    format_hover_markdown_sections, normalize_path, stable_symbol_id, stale_warning_message,
};
use aether_infer::{
    EmbeddingProviderOverrides, RerankCandidate, RerankerProvider, RerankerProviderOverrides,
    load_embedding_provider_from_config, load_reranker_provider_from_config,
};
use aether_memory::{
    EntityRef as MemoryEntityRef, NoteEmbeddingRequest as MemoryNoteEmbeddingRequest,
    NoteSourceType as MemoryNoteSourceType, ProjectMemoryService,
    RecallRequest as MemoryRecallRequest, RememberRequest as MemoryRememberRequest,
    SemanticQuery as MemorySemanticQuery, truncate_content_for_embedding,
};
use aether_parse::{SymbolExtractor, language_for_path};
use aether_sir::{
    FileSir, SirAnnotation, SirError, SirLevel, canonicalize_file_sir_json, canonicalize_sir_json,
    file_sir_hash, sir_hash, synthetic_file_sir_id, synthetic_module_sir_id, validate_sir,
};
use aether_store::{
    SirHistorySelector, SirMetaRecord, SqliteStore, Store, StoreError, SymbolRecord,
    SymbolSearchResult, open_graph_store, open_vector_store,
};
use aetherd::verification::{VerificationRequest, run_verification};
use anyhow::Result;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{
    ErrorData as McpError, Json, ServerHandler, ServiceExt, tool, tool_handler, tool_router,
};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const SERVER_NAME: &str = "aether";
pub const SERVER_VERSION: &str = "0.1.0";
pub const SERVER_DESCRIPTION: &str = "AETHER local symbol/SIR lookup from .aether store";
pub const MCP_SCHEMA_VERSION: u32 = 1;
pub const MEMORY_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Error)]
pub enum AetherMcpError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("inference error: {0}")]
    Infer(#[from] aether_infer::InferError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("memory error: {0}")]
    Memory(#[from] aether_memory::MemoryError),
    #[error("sir validation error: {0}")]
    Sir(#[from] SirError),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherStatusResponse {
    pub schema_version: u32,
    pub generated_at: i64,
    pub workspace: String,
    pub store_present: bool,
    pub sqlite_path: String,
    pub sir_dir: String,
    pub symbol_count: i64,
    pub sir_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolLookupRequest {
    pub query: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolLookupMatch {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub semantic_score: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolLookupResponse {
    pub query: String,
    pub limit: u32,
    pub mode_requested: SearchMode,
    pub mode_used: SearchMode,
    pub fallback_reason: Option<String>,
    pub result_count: u32,
    pub matches: Vec<AetherSymbolLookupMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDependenciesRequest {
    pub symbol_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDependenciesResponse {
    pub symbol_id: String,
    pub found: bool,
    pub caller_count: u32,
    pub dependency_count: u32,
    pub callers: Vec<AetherSymbolLookupMatch>,
    pub dependencies: Vec<AetherSymbolLookupMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherCallChainRequest {
    pub symbol_id: Option<String>,
    pub qualified_name: Option<String>,
    pub max_depth: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherCallChainResponse {
    pub found: bool,
    pub symbol_id: String,
    pub qualified_name: String,
    pub max_depth: u32,
    pub depth_count: u32,
    pub levels: Vec<Vec<AetherSymbolLookupMatch>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSearchRequest {
    pub query: String,
    pub limit: Option<u32>,
    pub mode: Option<SearchMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSearchResponse {
    pub query: String,
    pub limit: u32,
    pub mode_requested: SearchMode,
    pub mode_used: SearchMode,
    pub fallback_reason: Option<String>,
    pub result_count: u32,
    pub matches: Vec<AetherSymbolLookupMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherMemoryEntityRef {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRememberRequest {
    pub content: String,
    pub tags: Option<Vec<String>>,
    pub entity_refs: Option<Vec<AetherMemoryEntityRef>>,
    pub file_refs: Option<Vec<String>>,
    pub symbol_refs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRememberResponse {
    pub schema_version: String,
    pub note_id: String,
    pub action: String,
    pub content_hash: String,
    pub tags: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRecallRequest {
    pub query: String,
    pub mode: Option<SearchMode>,
    pub limit: Option<u32>,
    pub include_archived: Option<bool>,
    pub tags_filter: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRecallNote {
    pub note_id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub file_refs: Vec<String>,
    pub symbol_refs: Vec<String>,
    pub source_type: String,
    pub created_at: i64,
    pub access_count: i64,
    pub relevance_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRecallResponse {
    pub schema_version: String,
    pub query: String,
    pub mode_requested: SearchMode,
    pub mode_used: SearchMode,
    pub fallback_reason: Option<String>,
    pub result_count: u32,
    pub notes: Vec<AetherRecallNote>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherVerifyRequest {
    pub commands: Option<Vec<String>>,
    pub mode: Option<AetherVerifyMode>,
    pub fallback_to_host_on_unavailable: Option<bool>,
    pub fallback_to_container_on_unavailable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherVerifyCommandResult {
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub passed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherVerifyMode {
    Host,
    Container,
    Microvm,
}

impl From<AetherVerifyMode> for VerifyMode {
    fn from(value: AetherVerifyMode) -> Self {
        match value {
            AetherVerifyMode::Host => VerifyMode::Host,
            AetherVerifyMode::Container => VerifyMode::Container,
            AetherVerifyMode::Microvm => VerifyMode::Microvm,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherVerifyResponse {
    pub schema_version: u32,
    pub workspace: String,
    pub mode: String,
    pub mode_requested: String,
    pub mode_used: String,
    pub fallback_reason: Option<String>,
    pub allowlisted_commands: Vec<String>,
    pub requested_commands: Vec<String>,
    pub passed: bool,
    pub error: Option<String>,
    pub result_count: u32,
    pub results: Vec<AetherVerifyCommandResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolTimelineRequest {
    pub symbol_id: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolTimelineEntry {
    pub version: i64,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub created_at: i64,
    pub commit_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolTimelineResponse {
    pub symbol_id: String,
    pub limit: u32,
    pub found: bool,
    pub result_count: u32,
    pub timeline: Vec<AetherSymbolTimelineEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherWhySelectorMode {
    Auto,
    Version,
    Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherWhyChangedReason {
    NoHistory,
    SingleVersionOnly,
    SelectorNotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherWhyChangedRequest {
    pub symbol_id: String,
    pub from_version: Option<i64>,
    pub to_version: Option<i64>,
    pub from_created_at: Option<i64>,
    pub to_created_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AetherWhySnapshot {
    pub version: i64,
    pub created_at: i64,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub commit_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AetherWhyChangedResponse {
    pub symbol_id: String,
    pub found: bool,
    pub reason: Option<AetherWhyChangedReason>,
    pub selector_mode: AetherWhySelectorMode,
    pub from: Option<AetherWhySnapshot>,
    pub to: Option<AetherWhySnapshot>,
    pub prior_summary: Option<String>,
    pub current_summary: Option<String>,
    pub fields_added: Vec<String>,
    pub fields_removed: Vec<String>,
    pub fields_modified: Vec<String>,
}

impl AetherSymbolLookupResponse {
    fn from_search_envelope(
        query: String,
        limit: u32,
        envelope: SearchEnvelope<AetherSymbolLookupMatch>,
    ) -> Self {
        let SearchEnvelope {
            mode_requested,
            mode_used,
            fallback_reason,
            matches,
        } = envelope;
        let result_count = matches.len() as u32;

        Self {
            query,
            limit,
            mode_requested,
            mode_used,
            fallback_reason,
            result_count,
            matches,
        }
    }
}

impl AetherSearchResponse {
    fn from_search_envelope(
        query: String,
        limit: u32,
        envelope: SearchEnvelope<AetherSymbolLookupMatch>,
    ) -> Self {
        let SearchEnvelope {
            mode_requested,
            mode_used,
            fallback_reason,
            matches,
        } = envelope;
        let result_count = matches.len() as u32;

        Self {
            query,
            limit,
            mode_requested,
            mode_used,
            fallback_reason,
            result_count,
            matches,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherGetSirRequest {
    pub level: Option<SirLevelRequest>,
    pub symbol_id: Option<String>,
    pub file_path: Option<String>,
    pub module_path: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherGetSirResponse {
    pub found: bool,
    pub level: SirLevelRequest,
    pub symbol_id: String,
    pub sir: Option<SirAnnotationView>,
    pub rollup: Option<FileSirView>,
    pub files_with_sir: Option<u32>,
    pub files_total: Option<u32>,
    pub sir_json: String,
    pub sir_hash: String,
    pub sir_status: Option<String>,
    pub last_error: Option<String>,
    pub last_attempt_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SirLevelRequest {
    #[default]
    Leaf,
    File,
    Module,
}

impl From<SirLevelRequest> for SirLevel {
    fn from(value: SirLevelRequest) -> Self {
        match value {
            SirLevelRequest::Leaf => SirLevel::Leaf,
            SirLevelRequest::File => SirLevel::File,
            SirLevelRequest::Module => SirLevel::Module,
        }
    }
}

impl From<SirLevel> for SirLevelRequest {
    fn from(value: SirLevel) -> Self {
        match value {
            SirLevel::Leaf => SirLevelRequest::Leaf,
            SirLevel::File => SirLevelRequest::File,
            SirLevel::Module => SirLevelRequest::Module,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherExplainRequest {
    pub file_path: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherExplainPosition {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherExplainResponse {
    pub found: bool,
    pub file_path: String,
    pub position: AetherExplainPosition,
    pub symbol_id: String,
    pub qualified_name: String,
    pub hover_markdown: String,
    pub sir: Option<SirAnnotationView>,
    pub sir_status: Option<String>,
    pub last_error: Option<String>,
    pub last_attempt_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SirAnnotationView {
    pub intent: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileSirView {
    pub intent: String,
    pub exports: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub symbol_count: usize,
    pub confidence: f32,
}

impl From<aether_sir::SirAnnotation> for SirAnnotationView {
    fn from(value: aether_sir::SirAnnotation) -> Self {
        Self {
            intent: value.intent,
            inputs: value.inputs,
            outputs: value.outputs,
            side_effects: value.side_effects,
            dependencies: value.dependencies,
            error_modes: value.error_modes,
            confidence: value.confidence,
        }
    }
}

impl From<FileSir> for FileSirView {
    fn from(value: FileSir) -> Self {
        Self {
            intent: value.intent,
            exports: value.exports,
            side_effects: value.side_effects,
            dependencies: value.dependencies,
            error_modes: value.error_modes,
            symbol_count: value.symbol_count,
            confidence: value.confidence,
        }
    }
}

impl From<SymbolSearchResult> for AetherSymbolLookupMatch {
    fn from(value: SymbolSearchResult) -> Self {
        Self {
            symbol_id: value.symbol_id,
            qualified_name: value.qualified_name,
            file_path: value.file_path,
            language: value.language,
            kind: value.kind,
            semantic_score: None,
        }
    }
}

impl From<SymbolRecord> for AetherSymbolLookupMatch {
    fn from(value: SymbolRecord) -> Self {
        Self {
            symbol_id: value.id,
            qualified_name: value.qualified_name,
            file_path: value.file_path,
            language: value.language,
            kind: value.kind,
            semantic_score: None,
        }
    }
}

impl AetherWhySnapshot {
    fn from_history_record(record: &aether_store::SirHistoryRecord) -> Self {
        Self {
            version: record.version,
            created_at: record.created_at,
            sir_hash: record.sir_hash.clone(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            commit_hash: record.commit_hash.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WhySelector {
    Auto,
    Version {
        from_version: i64,
        to_version: i64,
    },
    Timestamp {
        from_created_at: i64,
        to_created_at: i64,
    },
}

impl WhySelector {
    fn mode(self) -> AetherWhySelectorMode {
        match self {
            Self::Auto => AetherWhySelectorMode::Auto,
            Self::Version { .. } => AetherWhySelectorMode::Version,
            Self::Timestamp { .. } => AetherWhySelectorMode::Timestamp,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AetherMcpServer {
    workspace: PathBuf,
    verbose: bool,
    tool_router: ToolRouter<Self>,
}

impl AetherMcpServer {
    pub fn new(workspace: impl AsRef<Path>, verbose: bool) -> Result<Self, AetherMcpError> {
        let workspace = workspace.as_ref().canonicalize()?;

        Ok(Self {
            workspace,
            verbose,
            tool_router: Self::tool_router(),
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn aether_status_logic(&self) -> Result<AetherStatusResponse, AetherMcpError> {
        let sqlite_path = self.sqlite_path();
        let sir_dir = self.sir_dir();
        let store_present = sqlite_path.exists() && sir_dir.is_dir();

        let (symbol_count, sir_count) = if store_present {
            let conn = self.open_sqlite_connection(&sqlite_path)?;
            (
                count_table_rows(&conn, "symbols")?,
                count_table_rows(&conn, "sir")?,
            )
        } else {
            (0, 0)
        };

        Ok(AetherStatusResponse {
            schema_version: MCP_SCHEMA_VERSION,
            generated_at: current_unix_timestamp(),
            workspace: normalize_path(&self.workspace.to_string_lossy()),
            store_present,
            sqlite_path: normalize_path(&sqlite_path.to_string_lossy()),
            sir_dir: normalize_path(&sir_dir.to_string_lossy()),
            symbol_count,
            sir_count,
        })
    }

    pub fn aether_symbol_lookup_logic(
        &self,
        request: AetherSymbolLookupRequest,
    ) -> Result<AetherSymbolLookupResponse, AetherMcpError> {
        let limit = effective_limit(request.limit);
        let matches = self.lexical_search_matches(&request.query, limit)?;
        let envelope = SearchEnvelope {
            mode_requested: SearchMode::Lexical,
            mode_used: SearchMode::Lexical,
            fallback_reason: None,
            matches,
        };

        Ok(AetherSymbolLookupResponse::from_search_envelope(
            request.query,
            limit,
            envelope,
        ))
    }

    pub fn aether_dependencies_logic(
        &self,
        request: AetherDependenciesRequest,
    ) -> Result<AetherDependenciesResponse, AetherMcpError> {
        let symbol_id = request.symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(AetherDependenciesResponse {
                symbol_id: String::new(),
                found: false,
                caller_count: 0,
                dependency_count: 0,
                callers: Vec::new(),
                dependencies: Vec::new(),
            });
        }

        if !self.sqlite_path().exists() {
            return Ok(AetherDependenciesResponse {
                symbol_id: symbol_id.to_owned(),
                found: false,
                caller_count: 0,
                dependency_count: 0,
                callers: Vec::new(),
                dependencies: Vec::new(),
            });
        }

        let store = SqliteStore::open(&self.workspace)?;
        let Some(symbol) = store.get_symbol_record(symbol_id)? else {
            return Ok(AetherDependenciesResponse {
                symbol_id: symbol_id.to_owned(),
                found: false,
                caller_count: 0,
                dependency_count: 0,
                callers: Vec::new(),
                dependencies: Vec::new(),
            });
        };

        let graph_store = open_graph_store(&self.workspace)?;
        let callers = graph_store
            .get_callers(&symbol.qualified_name)?
            .into_iter()
            .map(AetherSymbolLookupMatch::from)
            .collect::<Vec<_>>();
        let dependencies = graph_store
            .get_dependencies(&symbol.id)?
            .into_iter()
            .map(AetherSymbolLookupMatch::from)
            .collect::<Vec<_>>();

        Ok(AetherDependenciesResponse {
            symbol_id: symbol_id.to_owned(),
            found: true,
            caller_count: callers.len() as u32,
            dependency_count: dependencies.len() as u32,
            callers,
            dependencies,
        })
    }

    pub fn aether_call_chain_logic(
        &self,
        request: AetherCallChainRequest,
    ) -> Result<AetherCallChainResponse, AetherMcpError> {
        let symbol_id_input = request.symbol_id.as_deref().unwrap_or("").trim().to_owned();
        let qualified_name_input = request
            .qualified_name
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_owned();
        let max_depth = request.max_depth.unwrap_or(3).clamp(1, 10);

        if symbol_id_input.is_empty() && qualified_name_input.is_empty() {
            return Ok(AetherCallChainResponse {
                found: false,
                symbol_id: String::new(),
                qualified_name: String::new(),
                max_depth,
                depth_count: 0,
                levels: Vec::new(),
            });
        }

        if !self.sqlite_path().exists() {
            return Ok(AetherCallChainResponse {
                found: false,
                symbol_id: symbol_id_input,
                qualified_name: qualified_name_input,
                max_depth,
                depth_count: 0,
                levels: Vec::new(),
            });
        }

        let store = SqliteStore::open(&self.workspace)?;
        let mut start_symbol = None;
        if !symbol_id_input.is_empty() {
            start_symbol = store.get_symbol_record(&symbol_id_input)?;
        }
        if start_symbol.is_none() && !qualified_name_input.is_empty() {
            start_symbol = store.get_symbol_by_qualified_name(&qualified_name_input)?;
        }
        let Some(start_symbol) = start_symbol else {
            return Ok(AetherCallChainResponse {
                found: false,
                symbol_id: symbol_id_input,
                qualified_name: qualified_name_input,
                max_depth,
                depth_count: 0,
                levels: Vec::new(),
            });
        };

        let graph_store = open_graph_store(&self.workspace)?;
        let levels = graph_store
            .get_call_chain(&start_symbol.id, max_depth)?
            .into_iter()
            .map(|rows| {
                rows.into_iter()
                    .map(AetherSymbolLookupMatch::from)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        Ok(AetherCallChainResponse {
            found: true,
            symbol_id: start_symbol.id,
            qualified_name: start_symbol.qualified_name,
            max_depth,
            depth_count: levels.len() as u32,
            levels,
        })
    }

    pub async fn aether_search_logic(
        &self,
        request: AetherSearchRequest,
    ) -> Result<AetherSearchResponse, AetherMcpError> {
        let mode_requested = request.mode.unwrap_or_default();
        let limit = effective_limit(request.limit);
        let lexical = self.lexical_search_matches(&request.query, limit)?;
        let search_config = load_workspace_config(&self.workspace).map_err(|err| {
            AetherMcpError::Message(format!("failed to load workspace config: {err}"))
        })?;
        let reranker_kind = search_config.search.reranker;
        let rerank_window = search_config.search.rerank_window;

        let envelope = match mode_requested {
            SearchMode::Lexical => SearchEnvelope {
                mode_requested: SearchMode::Lexical,
                mode_used: SearchMode::Lexical,
                fallback_reason: None,
                matches: lexical,
            },
            SearchMode::Semantic => {
                let (semantic, fallback_reason) =
                    self.semantic_search_matches(&request.query, limit).await?;
                if semantic.is_empty() {
                    SearchEnvelope {
                        mode_requested: SearchMode::Semantic,
                        mode_used: SearchMode::Lexical,
                        fallback_reason,
                        matches: lexical,
                    }
                } else {
                    SearchEnvelope {
                        mode_requested: SearchMode::Semantic,
                        mode_used: SearchMode::Semantic,
                        fallback_reason: None,
                        matches: semantic,
                    }
                }
            }
            SearchMode::Hybrid => {
                let (semantic, fallback_reason) =
                    self.semantic_search_matches(&request.query, limit).await?;
                if semantic.is_empty() {
                    SearchEnvelope {
                        mode_requested: SearchMode::Hybrid,
                        mode_used: SearchMode::Lexical,
                        fallback_reason,
                        matches: lexical,
                    }
                } else {
                    let fuse_limit = if matches!(reranker_kind, SearchRerankerKind::None) {
                        limit
                    } else {
                        rerank_window.max(limit).clamp(1, 200)
                    };
                    let fused = fuse_hybrid_matches(&lexical, &semantic, fuse_limit);
                    let matches = self
                        .maybe_rerank_hybrid_matches(
                            &request.query,
                            limit,
                            fused,
                            reranker_kind,
                            rerank_window,
                        )
                        .await?;
                    SearchEnvelope {
                        mode_requested: SearchMode::Hybrid,
                        mode_used: SearchMode::Hybrid,
                        fallback_reason: None,
                        matches,
                    }
                }
            }
        };

        Ok(AetherSearchResponse::from_search_envelope(
            request.query,
            limit,
            envelope,
        ))
    }

    pub async fn aether_remember_logic(
        &self,
        request: AetherRememberRequest,
    ) -> Result<AetherRememberResponse, AetherMcpError> {
        let memory = ProjectMemoryService::new(&self.workspace);
        let entity_refs = request
            .entity_refs
            .unwrap_or_default()
            .into_iter()
            .map(|entity| MemoryEntityRef {
                kind: entity.kind,
                id: entity.id,
            })
            .collect::<Vec<_>>();

        let remember = memory.remember(MemoryRememberRequest {
            content: request.content,
            source_type: MemoryNoteSourceType::Agent,
            source_agent: Some("aether_mcp".to_owned()),
            tags: request.tags.unwrap_or_default(),
            entity_refs,
            file_refs: request.file_refs.unwrap_or_default(),
            symbol_refs: request.symbol_refs.unwrap_or_default(),
            now_ms: None,
        })?;

        if remember.action == aether_memory::RememberAction::Created {
            match load_embedding_provider_from_config(
                &self.workspace,
                EmbeddingProviderOverrides::default(),
            ) {
                Ok(Some(loaded)) => {
                    let content = truncate_content_for_embedding(remember.note.content.as_str());
                    match loaded.provider.embed_text(content.as_str()).await {
                        Ok(embedding) if !embedding.is_empty() => {
                            if let Err(err) = memory
                                .upsert_note_embedding(MemoryNoteEmbeddingRequest {
                                    note_id: remember.note.note_id.clone(),
                                    provider: loaded.provider_name,
                                    model: loaded.model_name,
                                    embedding,
                                    content: remember.note.content.clone(),
                                    created_at: remember.note.created_at,
                                    updated_at: Some(remember.note.updated_at),
                                })
                                .await
                            {
                                tracing::warn!(
                                    error = %err,
                                    "failed to persist note embedding after remember"
                                );
                            }
                        }
                        Ok(_) => tracing::warn!(
                            "embedding provider returned empty vector while indexing project note"
                        ),
                        Err(err) => tracing::warn!(
                            error = %err,
                            "embedding provider error while indexing project note"
                        ),
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::warn!(
                    error = %err,
                    "failed to load embedding provider for project note indexing"
                ),
            }
        }

        Ok(AetherRememberResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            note_id: remember.note.note_id,
            action: remember.action.as_str().to_owned(),
            content_hash: remember.note.content_hash,
            tags: remember.note.tags,
            created_at: remember.note.created_at,
        })
    }

    pub async fn aether_recall_logic(
        &self,
        request: AetherRecallRequest,
    ) -> Result<AetherRecallResponse, AetherMcpError> {
        let mode = request.mode.unwrap_or(SearchMode::Hybrid);
        let limit = request.limit.unwrap_or(5).clamp(1, 100);

        let mut semantic_query = None;
        let mut semantic_fallback_reason = None;
        if !matches!(mode, SearchMode::Lexical) {
            match load_embedding_provider_from_config(
                &self.workspace,
                EmbeddingProviderOverrides::default(),
            ) {
                Ok(Some(loaded)) => {
                    match loaded.provider.embed_text(request.query.as_str()).await {
                        Ok(embedding) if !embedding.is_empty() => {
                            semantic_query = Some(MemorySemanticQuery {
                                provider: loaded.provider_name,
                                model: loaded.model_name,
                                embedding,
                            });
                        }
                        Ok(_) => {
                            semantic_fallback_reason =
                                Some(SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR.to_owned())
                        }
                        Err(err) => {
                            semantic_fallback_reason =
                                Some(format!("embedding provider error: {err}"))
                        }
                    }
                }
                Ok(None) => {
                    semantic_fallback_reason = Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED.to_owned())
                }
                Err(err) => {
                    semantic_fallback_reason =
                        Some(format!("failed to load embedding provider: {err}"))
                }
            }
        }

        let memory = ProjectMemoryService::new(&self.workspace);
        let result = memory
            .recall(MemoryRecallRequest {
                query: request.query.clone(),
                mode,
                limit,
                include_archived: request.include_archived.unwrap_or(false),
                tags_filter: request.tags_filter.unwrap_or_default(),
                now_ms: None,
                semantic: semantic_query,
                semantic_fallback_reason,
            })
            .await?;

        let notes = result
            .notes
            .into_iter()
            .map(|entry| AetherRecallNote {
                note_id: entry.note.note_id,
                content: entry.note.content,
                tags: entry.note.tags,
                file_refs: entry.note.file_refs,
                symbol_refs: entry.note.symbol_refs,
                source_type: entry.note.source_type,
                created_at: entry.note.created_at,
                access_count: entry.note.access_count,
                relevance_score: entry.relevance_score,
            })
            .collect::<Vec<_>>();
        let result_count = notes.len() as u32;

        Ok(AetherRecallResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            query: request.query,
            mode_requested: result.mode_requested,
            mode_used: result.mode_used,
            fallback_reason: result.fallback_reason,
            result_count,
            notes,
        })
    }

    pub fn aether_symbol_timeline_logic(
        &self,
        request: AetherSymbolTimelineRequest,
    ) -> Result<AetherSymbolTimelineResponse, AetherMcpError> {
        let symbol_id = request.symbol_id.trim();
        let limit = effective_limit(request.limit);
        if symbol_id.is_empty() {
            return Ok(AetherSymbolTimelineResponse {
                symbol_id: String::new(),
                limit,
                found: false,
                result_count: 0,
                timeline: Vec::new(),
            });
        }

        if !self.sqlite_path().exists() {
            return Ok(AetherSymbolTimelineResponse {
                symbol_id: symbol_id.to_owned(),
                limit,
                found: false,
                result_count: 0,
                timeline: Vec::new(),
            });
        }

        let store = SqliteStore::open(&self.workspace)?;
        let mut history = store.list_sir_history(symbol_id)?;
        if history.len() > limit as usize {
            let split_idx = history.len().saturating_sub(limit as usize);
            history = history.split_off(split_idx);
        }

        let timeline = history
            .into_iter()
            .map(|record| AetherSymbolTimelineEntry {
                version: record.version,
                sir_hash: record.sir_hash,
                provider: record.provider,
                model: record.model,
                created_at: record.created_at,
                commit_hash: record.commit_hash,
            })
            .collect::<Vec<_>>();
        let result_count = timeline.len() as u32;

        Ok(AetherSymbolTimelineResponse {
            symbol_id: symbol_id.to_owned(),
            limit,
            found: result_count > 0,
            result_count,
            timeline,
        })
    }

    pub fn aether_verify_logic(
        &self,
        request: AetherVerifyRequest,
    ) -> Result<AetherVerifyResponse, AetherMcpError> {
        let config = load_workspace_config(&self.workspace).map_err(|err| {
            AetherMcpError::Message(format!("failed to load workspace config: {err}"))
        })?;

        let execution = run_verification(
            &self.workspace,
            &config,
            VerificationRequest {
                commands: request.commands,
                mode: request.mode.map(Into::into),
                fallback_to_host_on_unavailable: request.fallback_to_host_on_unavailable,
                fallback_to_container_on_unavailable: request.fallback_to_container_on_unavailable,
            },
        )
        .map_err(|err| AetherMcpError::Message(format!("failed to run verification: {err}")))?;

        let results = execution
            .command_results
            .into_iter()
            .map(|item| AetherVerifyCommandResult {
                command: item.command,
                exit_code: item.exit_code,
                stdout: item.stdout,
                stderr: item.stderr,
                passed: item.passed,
            })
            .collect::<Vec<_>>();
        let result_count = results.len() as u32;

        Ok(AetherVerifyResponse {
            schema_version: MCP_SCHEMA_VERSION,
            workspace: normalize_path(&self.workspace.to_string_lossy()),
            mode: execution.mode,
            mode_requested: execution.mode_requested,
            mode_used: execution.mode_used,
            fallback_reason: execution.fallback_reason,
            allowlisted_commands: execution.allowlisted_commands,
            requested_commands: execution.requested_commands,
            passed: execution.passed,
            error: execution.error,
            result_count,
            results,
        })
    }

    pub fn aether_why_changed_logic(
        &self,
        request: AetherWhyChangedRequest,
    ) -> Result<AetherWhyChangedResponse, AetherMcpError> {
        let selector = parse_why_selector(&request)?;
        let selector_mode = selector.mode();
        let symbol_id = request.symbol_id.trim();

        if symbol_id.is_empty() {
            return Ok(empty_why_changed_response(
                String::new(),
                selector_mode,
                AetherWhyChangedReason::NoHistory,
            ));
        }

        if !self.sqlite_path().exists() {
            return Ok(empty_why_changed_response(
                symbol_id.to_owned(),
                selector_mode,
                AetherWhyChangedReason::NoHistory,
            ));
        }

        let store = SqliteStore::open(&self.workspace)?;
        let history = store.list_sir_history(symbol_id)?;
        if history.is_empty() {
            return Ok(empty_why_changed_response(
                symbol_id.to_owned(),
                selector_mode,
                AetherWhyChangedReason::NoHistory,
            ));
        }

        let pair = match selector {
            WhySelector::Auto => store.latest_sir_history_pair(symbol_id)?,
            WhySelector::Version {
                from_version,
                to_version,
            } => store.resolve_sir_history_pair(
                symbol_id,
                SirHistorySelector::Version(from_version),
                SirHistorySelector::Version(to_version),
            )?,
            WhySelector::Timestamp {
                from_created_at,
                to_created_at,
            } => store.resolve_sir_history_pair(
                symbol_id,
                SirHistorySelector::CreatedAt(from_created_at),
                SirHistorySelector::CreatedAt(to_created_at),
            )?,
        };

        let Some(pair) = pair else {
            return Ok(empty_why_changed_response(
                symbol_id.to_owned(),
                selector_mode,
                AetherWhyChangedReason::SelectorNotFound,
            ));
        };

        let from_fields = parse_sir_history_json_fields(&pair.from.sir_json)?;
        let to_fields = parse_sir_history_json_fields(&pair.to.sir_json)?;
        let (fields_added, fields_removed, fields_modified) =
            diff_top_level_field_names(&from_fields, &to_fields);

        let reason = (selector_mode == AetherWhySelectorMode::Auto && history.len() == 1)
            .then_some(AetherWhyChangedReason::SingleVersionOnly);

        Ok(AetherWhyChangedResponse {
            symbol_id: symbol_id.to_owned(),
            found: true,
            reason,
            selector_mode,
            from: Some(AetherWhySnapshot::from_history_record(&pair.from)),
            to: Some(AetherWhySnapshot::from_history_record(&pair.to)),
            prior_summary: extract_intent_field(&from_fields),
            current_summary: extract_intent_field(&to_fields),
            fields_added,
            fields_removed,
            fields_modified,
        })
    }

    fn lexical_search_matches(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        let sqlite_path = self.sqlite_path();
        if !sqlite_path.exists() {
            return Ok(Vec::new());
        }

        let store = SqliteStore::open(&self.workspace)?;
        let matches = store.search_symbols(query, limit)?;

        Ok(matches
            .into_iter()
            .map(AetherSymbolLookupMatch::from)
            .collect())
    }

    async fn semantic_search_matches(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<(Vec<AetherSymbolLookupMatch>, Option<String>), AetherMcpError> {
        let sqlite_path = self.sqlite_path();
        if !sqlite_path.exists() {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED.to_owned()),
            ));
        }

        let loaded = load_embedding_provider_from_config(
            &self.workspace,
            EmbeddingProviderOverrides::default(),
        )?;
        let Some(loaded) = loaded else {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED.to_owned()),
            ));
        };

        let query_embedding = match loaded.provider.embed_text(query).await {
            Ok(embedding) => embedding,
            Err(err) => {
                return Ok((Vec::new(), Some(format!("embedding provider error: {err}"))));
            }
        };

        if query_embedding.is_empty() {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR.to_owned()),
            ));
        }

        let vector_store = open_vector_store(&self.workspace).await?;
        let candidates = vector_store
            .search_nearest(
                &query_embedding,
                &loaded.provider_name,
                &loaded.model_name,
                limit,
            )
            .await?;
        if candidates.is_empty() {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY.to_owned()),
            ));
        }

        let store = SqliteStore::open(&self.workspace)?;
        let mut matches = Vec::new();
        for candidate in candidates {
            let Some(symbol) = store.get_symbol_search_result(&candidate.symbol_id)? else {
                continue;
            };

            matches.push(AetherSymbolLookupMatch {
                symbol_id: symbol.symbol_id,
                qualified_name: symbol.qualified_name,
                file_path: symbol.file_path,
                language: symbol.language,
                kind: symbol.kind,
                semantic_score: Some(candidate.semantic_score),
            });
        }
        if matches.is_empty() {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY.to_owned()),
            ));
        }

        Ok((matches, None))
    }

    async fn maybe_rerank_hybrid_matches(
        &self,
        query: &str,
        limit: u32,
        fused_matches: Vec<AetherSymbolLookupMatch>,
        reranker_kind: SearchRerankerKind,
        rerank_window: u32,
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        if fused_matches.is_empty() {
            return Ok(Vec::new());
        }

        let limit = limit.clamp(1, 100) as usize;
        if matches!(reranker_kind, SearchRerankerKind::None) {
            return Ok(fused_matches.into_iter().take(limit).collect());
        }

        let fallback = fused_matches
            .iter()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let loaded = match load_reranker_provider_from_config(
            &self.workspace,
            RerankerProviderOverrides::default(),
        ) {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return Ok(fallback),
            Err(err) => {
                tracing::warn!(error = %err, "reranker unavailable, falling back to RRF matches");
                return Ok(fallback);
            }
        };

        match self
            .rerank_matches_with_provider(
                query,
                limit,
                rerank_window,
                &fused_matches,
                loaded.provider.as_ref(),
            )
            .await
        {
            Ok(matches) => Ok(matches),
            Err(err) => {
                tracing::warn!(
                    provider = %loaded.provider_name,
                    error = %err,
                    "reranker failed, falling back to RRF matches"
                );
                Ok(fallback)
            }
        }
    }

    async fn rerank_matches_with_provider(
        &self,
        query: &str,
        limit: usize,
        rerank_window: u32,
        fused_matches: &[AetherSymbolLookupMatch],
        provider: &dyn RerankerProvider,
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        if fused_matches.is_empty() || query.trim().is_empty() || limit == 0 {
            return Ok(fused_matches.iter().take(limit).cloned().collect());
        }

        let store = SqliteStore::open(&self.workspace)?;
        let window = rerank_window.max(limit as u32).clamp(1, 200) as usize;
        let candidate_matches = fused_matches
            .iter()
            .take(window.min(fused_matches.len()))
            .cloned()
            .collect::<Vec<_>>();

        let mut rerank_candidates = Vec::with_capacity(candidate_matches.len());
        for candidate in &candidate_matches {
            rerank_candidates.push(RerankCandidate {
                id: candidate.symbol_id.clone(),
                text: self.rerank_candidate_text(&store, candidate)?,
            });
        }

        let reranked = provider.rerank(query, &rerank_candidates, limit).await?;

        let mut resolved = Vec::with_capacity(limit.min(candidate_matches.len()));
        let mut used = HashSet::new();

        for result in &reranked {
            if let Some(row) = candidate_matches.get(result.original_rank)
                && row.symbol_id == result.id
                && used.insert(row.symbol_id.clone())
            {
                resolved.push(row.clone());
                if resolved.len() >= limit {
                    break;
                }
                continue;
            }

            if let Some(row) = candidate_matches
                .iter()
                .find(|row| row.symbol_id == result.id && !used.contains(&row.symbol_id))
            {
                used.insert(row.symbol_id.clone());
                resolved.push(row.clone());
                if resolved.len() >= limit {
                    break;
                }
            }
        }

        for row in fused_matches {
            if resolved.len() >= limit {
                break;
            }
            if used.insert(row.symbol_id.clone()) {
                resolved.push(row.clone());
            }
        }

        Ok(resolved)
    }

    fn rerank_candidate_text(
        &self,
        store: &SqliteStore,
        row: &AetherSymbolLookupMatch,
    ) -> Result<String, AetherMcpError> {
        let fallback = format!(
            "qualified_name: {}\nkind: {}\nfile_path: {}",
            row.qualified_name, row.kind, row.file_path
        );

        let Some(blob) = store.read_sir_blob(&row.symbol_id)? else {
            return Ok(fallback);
        };

        let Ok(value) = serde_json::from_str::<Value>(&blob) else {
            return Ok(fallback);
        };

        let Some(intent) = value
            .get("intent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(fallback);
        };

        Ok(format!("{intent}\n{fallback}"))
    }

    pub fn aether_get_sir_logic(
        &self,
        request: AetherGetSirRequest,
    ) -> Result<AetherGetSirResponse, AetherMcpError> {
        let level = request.level.unwrap_or_default();
        match level {
            SirLevelRequest::Leaf => self.aether_get_sir_leaf(&request),
            SirLevelRequest::File => self.aether_get_sir_file(&request),
            SirLevelRequest::Module => self.aether_get_sir_module(&request),
        }
    }

    fn aether_get_sir_leaf(
        &self,
        request: &AetherGetSirRequest,
    ) -> Result<AetherGetSirResponse, AetherMcpError> {
        let symbol_id = required_request_field(request.symbol_id.as_deref(), "symbol_id")?;
        if !self.sqlite_path().exists() {
            return Ok(empty_get_sir_response(
                SirLevel::Leaf.into(),
                symbol_id.to_owned(),
            ));
        }

        let store = SqliteStore::open(&self.workspace)?;
        let meta = store.get_sir_meta(symbol_id)?;
        let (sir_status, last_error, last_attempt_at) = meta_status_fields(meta.as_ref());
        let sir_blob = store.read_sir_blob(symbol_id)?;

        let Some(sir_blob) = sir_blob else {
            return Ok(AetherGetSirResponse {
                found: false,
                level: SirLevel::Leaf.into(),
                symbol_id: symbol_id.to_owned(),
                sir: None,
                rollup: None,
                files_with_sir: None,
                files_total: None,
                sir_json: String::new(),
                sir_hash: String::new(),
                sir_status,
                last_error,
                last_attempt_at,
            });
        };

        let sir: SirAnnotation = serde_json::from_str(&sir_blob)?;
        validate_sir(&sir)?;

        let canonical_json = canonicalize_sir_json(&sir);
        let hash = meta
            .as_ref()
            .map(|record| record.sir_hash.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| sir_hash(&sir));

        Ok(AetherGetSirResponse {
            found: true,
            level: SirLevel::Leaf.into(),
            symbol_id: symbol_id.to_owned(),
            sir: Some(sir.into()),
            rollup: None,
            files_with_sir: None,
            files_total: None,
            sir_json: canonical_json,
            sir_hash: hash,
            sir_status,
            last_error,
            last_attempt_at,
        })
    }

    fn aether_get_sir_file(
        &self,
        request: &AetherGetSirRequest,
    ) -> Result<AetherGetSirResponse, AetherMcpError> {
        let file_path = self
            .normalize_workspace_relative_request_path(request.file_path.as_deref(), "file_path")?;
        let language = language_for_path(Path::new(&file_path)).ok_or_else(|| {
            AetherMcpError::Message(format!(
                "unable to infer language for file path: {file_path}"
            ))
        })?;
        let rollup_id = synthetic_file_sir_id(language.as_str(), &file_path);

        if !self.sqlite_path().exists() {
            return Ok(empty_get_sir_response(SirLevel::File.into(), rollup_id));
        }

        let store = SqliteStore::open(&self.workspace)?;
        let meta = store.get_sir_meta(&rollup_id)?;
        let (sir_status, last_error, last_attempt_at) = meta_status_fields(meta.as_ref());
        let blob = store.read_sir_blob(&rollup_id)?;

        let Some(blob) = blob else {
            return Ok(AetherGetSirResponse {
                found: false,
                level: SirLevel::File.into(),
                symbol_id: rollup_id,
                sir: None,
                rollup: None,
                files_with_sir: None,
                files_total: None,
                sir_json: String::new(),
                sir_hash: String::new(),
                sir_status,
                last_error,
                last_attempt_at,
            });
        };

        let file_sir: FileSir = serde_json::from_str(&blob)?;
        let canonical_json = canonicalize_file_sir_json(&file_sir);
        let hash = meta
            .as_ref()
            .map(|record| record.sir_hash.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| file_sir_hash(&file_sir));

        Ok(AetherGetSirResponse {
            found: true,
            level: SirLevel::File.into(),
            symbol_id: rollup_id,
            sir: None,
            rollup: Some(file_sir.into()),
            files_with_sir: None,
            files_total: None,
            sir_json: canonical_json,
            sir_hash: hash,
            sir_status,
            last_error,
            last_attempt_at,
        })
    }

    fn aether_get_sir_module(
        &self,
        request: &AetherGetSirRequest,
    ) -> Result<AetherGetSirResponse, AetherMcpError> {
        let module_path = self.normalize_workspace_relative_request_path(
            request.module_path.as_deref(),
            "module_path",
        )?;
        let language = parse_language_field(request.language.as_deref())?;
        let module_id = synthetic_module_sir_id(language.as_str(), &module_path);

        if !self.sqlite_path().exists() {
            return Ok(AetherGetSirResponse {
                found: false,
                level: SirLevel::Module.into(),
                symbol_id: module_id,
                sir: None,
                rollup: None,
                files_with_sir: Some(0),
                files_total: Some(0),
                sir_json: String::new(),
                sir_hash: String::new(),
                sir_status: None,
                last_error: None,
                last_attempt_at: None,
            });
        }

        let store = SqliteStore::open(&self.workspace)?;
        let coverage = self.generate_module_rollup_on_demand(&store, &module_path, language)?;
        let meta = store.get_sir_meta(&coverage.module_id)?;
        let (sir_status, last_error, last_attempt_at) = meta_status_fields(meta.as_ref());
        let blob = store.read_sir_blob(&coverage.module_id)?;
        let Some(blob) = blob else {
            return Ok(AetherGetSirResponse {
                found: false,
                level: SirLevel::Module.into(),
                symbol_id: coverage.module_id,
                sir: None,
                rollup: None,
                files_with_sir: Some(coverage.files_with_sir),
                files_total: Some(coverage.files_total),
                sir_json: String::new(),
                sir_hash: String::new(),
                sir_status,
                last_error,
                last_attempt_at,
            });
        };

        let module_sir: FileSir = serde_json::from_str(&blob)?;
        let canonical_json = canonicalize_file_sir_json(&module_sir);
        let hash = meta
            .as_ref()
            .map(|record| record.sir_hash.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| file_sir_hash(&module_sir));

        Ok(AetherGetSirResponse {
            found: true,
            level: SirLevel::Module.into(),
            symbol_id: coverage.module_id,
            sir: None,
            rollup: Some(module_sir.into()),
            files_with_sir: Some(coverage.files_with_sir),
            files_total: Some(coverage.files_total),
            sir_json: canonical_json,
            sir_hash: hash,
            sir_status,
            last_error,
            last_attempt_at,
        })
    }

    fn normalize_workspace_relative_request_path(
        &self,
        raw: Option<&str>,
        field_name: &str,
    ) -> Result<String, AetherMcpError> {
        let value = required_request_field(raw, field_name)?;
        let path = PathBuf::from(value);
        let normalized = if path.is_absolute() {
            if !path.starts_with(&self.workspace) {
                return Err(AetherMcpError::Message(format!(
                    "{field_name} must be under workspace {}",
                    self.workspace.display()
                )));
            }

            let relative = path.strip_prefix(&self.workspace).map_err(|_| {
                AetherMcpError::Message(format!(
                    "{field_name} must be under workspace {}",
                    self.workspace.display()
                ))
            })?;
            normalize_path(&relative.to_string_lossy())
        } else {
            normalize_path(value)
        };

        let mut trimmed = normalized.trim().to_owned();
        while trimmed.starts_with("./") {
            trimmed = trimmed[2..].to_owned();
        }
        if trimmed != "/" {
            trimmed = trimmed.trim_end_matches('/').to_owned();
        }
        if trimmed.is_empty() {
            return Err(AetherMcpError::Message(format!(
                "{field_name} must not be empty"
            )));
        }
        Ok(trimmed)
    }

    fn generate_module_rollup_on_demand(
        &self,
        store: &SqliteStore,
        module_path: &str,
        language: Language,
    ) -> Result<ModuleRollupCoverage, AetherMcpError> {
        let module_id = synthetic_module_sir_id(language.as_str(), module_path);
        let file_paths = store.list_module_file_paths(module_path, language.as_str())?;
        let files_total = file_paths.len() as u32;

        let mut file_rollups = Vec::new();
        for file_path in file_paths {
            let file_rollup_id = synthetic_file_sir_id(language.as_str(), &file_path);
            let Some(file_blob) = store.read_sir_blob(&file_rollup_id)? else {
                continue;
            };
            let parsed = serde_json::from_str::<FileSir>(&file_blob);
            let Ok(file_sir) = parsed else {
                tracing::warn!(
                    file_path = %file_path,
                    rollup_id = %file_rollup_id,
                    "invalid file rollup JSON while building module rollup"
                );
                continue;
            };
            file_rollups.push((file_path, file_sir));
        }

        let files_with_sir = file_rollups.len() as u32;
        if file_rollups.is_empty() {
            store.mark_removed(&module_id)?;
            return Ok(ModuleRollupCoverage {
                module_id,
                files_with_sir,
                files_total,
            });
        }

        let module_sir = aggregate_module_rollup(&file_rollups);
        let canonical_json = canonicalize_file_sir_json(&module_sir);
        let hash = file_sir_hash(&module_sir);
        let attempted_at = current_unix_timestamp();
        let version_write = store.record_sir_version_if_changed(
            &module_id,
            &hash,
            "rollup",
            "deterministic",
            &canonical_json,
            attempted_at,
            None,
        )?;

        if version_write.changed {
            store.write_sir_blob(&module_id, &canonical_json)?;
        }

        store.upsert_sir_meta(SirMetaRecord {
            id: module_id.clone(),
            sir_hash: hash,
            sir_version: version_write.version,
            provider: "rollup".to_owned(),
            model: "deterministic".to_owned(),
            updated_at: version_write.updated_at,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: attempted_at,
        })?;

        Ok(ModuleRollupCoverage {
            module_id,
            files_with_sir,
            files_total,
        })
    }

    pub fn aether_explain_logic(
        &self,
        request: AetherExplainRequest,
    ) -> Result<AetherExplainResponse, AetherMcpError> {
        if request.line == 0 || request.column == 0 {
            return Err(AetherMcpError::Message(
                "line and column must be >= 1".to_owned(),
            ));
        }

        let absolute_path = self.resolve_workspace_file_path(&request.file_path)?;
        let language = language_for_path(&absolute_path).ok_or_else(|| {
            AetherMcpError::Message(format!(
                "unsupported file extension: {}",
                absolute_path.display()
            ))
        })?;

        let source = fs::read_to_string(&absolute_path)?;
        let display_path = self.workspace_relative_display_path(&absolute_path);

        let mut extractor =
            SymbolExtractor::new().map_err(|err| AetherMcpError::Message(err.to_string()))?;
        let symbols = extractor
            .extract_from_source(language, &display_path, &source)
            .map_err(|err| AetherMcpError::Message(err.to_string()))?;

        let line = request.line as usize;
        let column = request.column as usize;

        let target_symbol = symbols
            .iter()
            .filter(|symbol| position_in_range(symbol.range, line, column))
            .min_by_key(|symbol| symbol_span_score(symbol.range));

        let normalized_file_path = normalize_path(&absolute_path.to_string_lossy());

        let Some(symbol) = target_symbol else {
            return Ok(AetherExplainResponse {
                found: false,
                file_path: normalized_file_path,
                position: AetherExplainPosition {
                    line: request.line,
                    column: request.column,
                },
                symbol_id: String::new(),
                qualified_name: String::new(),
                hover_markdown: NO_SIR_MESSAGE.to_owned(),
                sir: None,
                sir_status: None,
                last_error: None,
                last_attempt_at: None,
            });
        };

        let symbol_id = stable_symbol_id(
            symbol.language,
            &symbol.file_path,
            symbol.kind,
            &symbol.qualified_name,
            &symbol.signature_fingerprint,
        );

        let meta = self.read_sir_meta(&symbol_id)?;
        let (sir_status, last_error, last_attempt_at) = meta_status_fields(meta.as_ref());
        let stale_warning = stale_warning_message(sir_status.as_deref(), last_error.as_deref());
        let sir = self.read_valid_sir_blob(&symbol_id)?;

        let (found, hover_markdown, sir) = match sir {
            Some(sir) => (
                true,
                format_hover_markdown_sections(
                    &HoverMarkdownSections {
                        symbol: symbol.qualified_name.clone(),
                        intent: sir.intent.clone(),
                        confidence: sir.confidence,
                        inputs: sir.inputs.clone(),
                        outputs: sir.outputs.clone(),
                        side_effects: sir.side_effects.clone(),
                        dependencies: sir.dependencies.clone(),
                        error_modes: sir.error_modes.clone(),
                    },
                    stale_warning.as_deref(),
                ),
                Some(SirAnnotationView::from(sir)),
            ),
            None => {
                let markdown = match stale_warning {
                    Some(warning) => format!("{warning}\n\n{NO_SIR_MESSAGE}"),
                    None => NO_SIR_MESSAGE.to_owned(),
                };
                (false, markdown, None)
            }
        };

        Ok(AetherExplainResponse {
            found,
            file_path: normalized_file_path,
            position: AetherExplainPosition {
                line: request.line,
                column: request.column,
            },
            symbol_id,
            qualified_name: symbol.qualified_name.clone(),
            hover_markdown,
            sir,
            sir_status,
            last_error,
            last_attempt_at,
        })
    }

    fn sqlite_path(&self) -> PathBuf {
        self.workspace.join(".aether").join("meta.sqlite")
    }

    fn sir_dir(&self) -> PathBuf {
        self.workspace.join(".aether").join("sir")
    }

    fn open_sqlite_connection(&self, sqlite_path: &Path) -> Result<Connection, AetherMcpError> {
        let conn = Connection::open(sqlite_path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(conn)
    }

    fn resolve_workspace_file_path(&self, file_path: &str) -> Result<PathBuf, AetherMcpError> {
        let path = PathBuf::from(file_path);
        let joined = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };

        let absolute = joined.canonicalize()?;
        if !absolute.starts_with(&self.workspace) {
            return Err(AetherMcpError::Message(format!(
                "file_path must be under workspace {}",
                self.workspace.display()
            )));
        }

        Ok(absolute)
    }

    fn workspace_relative_display_path(&self, absolute_path: &Path) -> String {
        if let Ok(relative) = absolute_path.strip_prefix(&self.workspace) {
            return normalize_path(&relative.to_string_lossy());
        }

        normalize_path(&absolute_path.to_string_lossy())
    }

    fn read_valid_sir_blob(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SirAnnotation>, AetherMcpError> {
        if !self.sqlite_path().exists() {
            return Ok(None);
        }

        let store = SqliteStore::open(&self.workspace)?;
        let blob = store.read_sir_blob(symbol_id)?;

        let Some(blob) = blob else {
            return Ok(None);
        };

        let sir: SirAnnotation = serde_json::from_str(&blob)?;
        validate_sir(&sir)?;
        Ok(Some(sir))
    }

    fn read_sir_meta(&self, symbol_id: &str) -> Result<Option<SirMetaRecord>, AetherMcpError> {
        if !self.sqlite_path().exists() {
            return Ok(None);
        }

        let store = SqliteStore::open(&self.workspace)?;
        store.get_sir_meta(symbol_id).map_err(Into::into)
    }

    fn verbose_log(&self, message: &str) {
        if self.verbose {
            tracing::debug!(message = %message, "aether-mcp verbose");
        }
    }
}

#[tool_router(router = tool_router)]
impl AetherMcpServer {
    #[tool(name = "aether_status", description = "Get AETHER local store status")]
    pub async fn aether_status(&self) -> Result<Json<AetherStatusResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_status");
        self.aether_status_logic().map(Json).map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_symbol_lookup",
        description = "Lookup symbols by qualified name or file path"
    )]
    pub async fn aether_symbol_lookup(
        &self,
        Parameters(request): Parameters<AetherSymbolLookupRequest>,
    ) -> Result<Json<AetherSymbolLookupResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_symbol_lookup");
        self.aether_symbol_lookup_logic(request)
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_dependencies",
        description = "Get resolved callers and call dependencies for a symbol"
    )]
    pub async fn aether_dependencies(
        &self,
        Parameters(request): Parameters<AetherDependenciesRequest>,
    ) -> Result<Json<AetherDependenciesResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_dependencies");
        self.aether_dependencies_logic(request)
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_call_chain",
        description = "Get transitive call-chain levels for a symbol"
    )]
    pub async fn aether_call_chain(
        &self,
        Parameters(request): Parameters<AetherCallChainRequest>,
    ) -> Result<Json<AetherCallChainResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_call_chain");
        self.aether_call_chain_logic(request)
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_search",
        description = "Search symbols by name, path, language, or kind"
    )]
    pub async fn aether_search(
        &self,
        Parameters(request): Parameters<AetherSearchRequest>,
    ) -> Result<Json<AetherSearchResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_search");
        self.aether_search_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_remember",
        description = "Store project memory note content with deterministic deduplication"
    )]
    pub async fn aether_remember(
        &self,
        Parameters(request): Parameters<AetherRememberRequest>,
    ) -> Result<Json<AetherRememberResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_remember");
        self.aether_remember_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_recall",
        description = "Recall project memory notes using lexical, semantic, or hybrid retrieval"
    )]
    pub async fn aether_recall(
        &self,
        Parameters(request): Parameters<AetherRecallRequest>,
    ) -> Result<Json<AetherRecallResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_recall");
        self.aether_recall_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_verify",
        description = "Run allowlisted verification commands in host, container, or microvm mode"
    )]
    pub async fn aether_verify(
        &self,
        Parameters(request): Parameters<AetherVerifyRequest>,
    ) -> Result<Json<AetherVerifyResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_verify");
        self.aether_verify_logic(request)
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_symbol_timeline",
        description = "Get ordered SIR timeline entries for a symbol"
    )]
    pub async fn aether_symbol_timeline(
        &self,
        Parameters(request): Parameters<AetherSymbolTimelineRequest>,
    ) -> Result<Json<AetherSymbolTimelineResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_symbol_timeline");
        self.aether_symbol_timeline_logic(request)
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_why_changed",
        description = "Explain why a symbol changed between two SIR versions or timestamps"
    )]
    pub async fn aether_why_changed(
        &self,
        Parameters(request): Parameters<AetherWhyChangedRequest>,
    ) -> Result<Json<AetherWhyChangedResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_why_changed");
        self.aether_why_changed_logic(request)
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_get_sir",
        description = "Get SIR for leaf/file/module level"
    )]
    pub async fn aether_get_sir(
        &self,
        Parameters(request): Parameters<AetherGetSirRequest>,
    ) -> Result<Json<AetherGetSirResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_get_sir");
        self.aether_get_sir_logic(request)
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_explain",
        description = "Explain symbol at a file position using local SIR"
    )]
    pub async fn aether_explain(
        &self,
        Parameters(request): Parameters<AetherExplainRequest>,
    ) -> Result<Json<AetherExplainResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_explain");
        self.aether_explain_logic(request)
            .map(Json)
            .map_err(to_mcp_error)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AetherMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: SERVER_NAME.to_owned(),
                title: None,
                version: SERVER_VERSION.to_owned(),
                icons: None,
                website_url: None,
            },
            instructions: Some(SERVER_DESCRIPTION.to_owned()),
            ..Default::default()
        }
    }
}

pub async fn run_stdio_server(workspace: impl AsRef<Path>, verbose: bool) -> Result<()> {
    let server = AetherMcpServer::new(workspace, verbose)?;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn to_mcp_error(err: AetherMcpError) -> McpError {
    McpError::internal_error(err.to_string(), None)
}

fn count_table_rows(conn: &Connection, table_name: &str) -> Result<i64, AetherMcpError> {
    let sql = format!("SELECT COUNT(*) FROM {table_name}");
    match conn.query_row(&sql, [], |row| row.get::<_, i64>(0)) {
        Ok(count) => Ok(count),
        Err(err) if err.to_string().contains("no such table") => Ok(0),
        Err(err) => Err(err.into()),
    }
}

#[derive(Debug, Clone)]
struct ModuleRollupCoverage {
    module_id: String,
    files_with_sir: u32,
    files_total: u32,
}

fn empty_get_sir_response(level: SirLevelRequest, symbol_id: String) -> AetherGetSirResponse {
    let (files_with_sir, files_total) = if level == SirLevelRequest::Module {
        (Some(0), Some(0))
    } else {
        (None, None)
    };

    AetherGetSirResponse {
        found: false,
        level,
        symbol_id,
        sir: None,
        rollup: None,
        files_with_sir,
        files_total,
        sir_json: String::new(),
        sir_hash: String::new(),
        sir_status: None,
        last_error: None,
        last_attempt_at: None,
    }
}

fn required_request_field<'a>(
    value: Option<&'a str>,
    field_name: &str,
) -> Result<&'a str, AetherMcpError> {
    let value = value.unwrap_or("").trim();
    if value.is_empty() {
        return Err(AetherMcpError::Message(format!(
            "{field_name} is required for this level"
        )));
    }

    Ok(value)
}

fn parse_language_field(language: Option<&str>) -> Result<Language, AetherMcpError> {
    let value = required_request_field(language, "language")?;
    match value.to_ascii_lowercase().as_str() {
        "rust" => Ok(Language::Rust),
        "typescript" => Ok(Language::TypeScript),
        "tsx" => Ok(Language::Tsx),
        "javascript" => Ok(Language::JavaScript),
        "jsx" => Ok(Language::Jsx),
        "python" => Ok(Language::Python),
        _ => Err(AetherMcpError::Message(format!(
            "unsupported language: {value}"
        ))),
    }
}

fn aggregate_module_rollup(file_rollups: &[(String, FileSir)]) -> FileSir {
    let mut sorted_rollups = file_rollups.to_vec();
    sorted_rollups.sort_by(|left, right| left.0.cmp(&right.0));

    let mut intents = Vec::new();
    let mut exports = Vec::new();
    let mut side_effects = Vec::new();
    let mut dependencies = Vec::new();
    let mut error_modes = Vec::new();
    let mut symbol_count = 0usize;
    let mut confidence = 1.0f32;

    for (_, rollup) in &sorted_rollups {
        let intent = rollup.intent.trim();
        if !intent.is_empty() {
            intents.push(intent.to_owned());
        }
        exports.extend(rollup.exports.clone());
        side_effects.extend(rollup.side_effects.clone());
        dependencies.extend(rollup.dependencies.clone());
        error_modes.extend(rollup.error_modes.clone());
        symbol_count += rollup.symbol_count;
        confidence = confidence.min(rollup.confidence);
    }

    sort_and_dedup(&mut exports);
    sort_and_dedup(&mut side_effects);
    sort_and_dedup(&mut dependencies);
    sort_and_dedup(&mut error_modes);

    FileSir {
        intent: if intents.is_empty() {
            "No summarized intent available".to_owned()
        } else {
            intents.join("; ")
        },
        exports,
        side_effects,
        dependencies,
        error_modes,
        symbol_count,
        confidence,
    }
}

fn sort_and_dedup(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn effective_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(20).clamp(1, 100)
}

fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn position_in_range(range: SourceRange, line: usize, column: usize) -> bool {
    let pos = (line, column);
    let start = (range.start.line, range.start.column);
    let end = (range.end.line, range.end.column);

    start <= pos && pos < end
}

fn symbol_span_score(range: SourceRange) -> (usize, usize) {
    let line_span = range.end.line.saturating_sub(range.start.line);
    let col_span = if line_span == 0 {
        range.end.column.saturating_sub(range.start.column)
    } else {
        range.end.column
    };

    (line_span, col_span)
}

fn meta_status_fields(
    meta: Option<&SirMetaRecord>,
) -> (Option<String>, Option<String>, Option<i64>) {
    let Some(meta) = meta else {
        return (None, None, None);
    };

    let sir_status = (!meta.sir_status.trim().is_empty()).then(|| meta.sir_status.clone());
    let last_error = meta
        .last_error
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned();
    let last_attempt_at = (meta.last_attempt_at > 0).then_some(meta.last_attempt_at);

    (sir_status, last_error, last_attempt_at)
}

fn parse_why_selector(request: &AetherWhyChangedRequest) -> Result<WhySelector, AetherMcpError> {
    let has_any_version = request.from_version.is_some() || request.to_version.is_some();
    let has_any_timestamp = request.from_created_at.is_some() || request.to_created_at.is_some();

    if has_any_version && has_any_timestamp {
        return Err(AetherMcpError::Message(
            "provide either version selectors or timestamp selectors, not both".to_owned(),
        ));
    }

    if has_any_version {
        let from_version = request.from_version.ok_or_else(|| {
            AetherMcpError::Message(
                "from_version is required when using version selectors".to_owned(),
            )
        })?;
        let to_version = request.to_version.ok_or_else(|| {
            AetherMcpError::Message(
                "to_version is required when using version selectors".to_owned(),
            )
        })?;
        if from_version < 1 || to_version < 1 {
            return Err(AetherMcpError::Message(
                "version selectors must be >= 1".to_owned(),
            ));
        }

        return Ok(WhySelector::Version {
            from_version,
            to_version,
        });
    }

    if has_any_timestamp {
        let from_created_at = request.from_created_at.ok_or_else(|| {
            AetherMcpError::Message(
                "from_created_at is required when using timestamp selectors".to_owned(),
            )
        })?;
        let to_created_at = request.to_created_at.ok_or_else(|| {
            AetherMcpError::Message(
                "to_created_at is required when using timestamp selectors".to_owned(),
            )
        })?;
        if from_created_at < 0 || to_created_at < 0 {
            return Err(AetherMcpError::Message(
                "timestamp selectors must be >= 0".to_owned(),
            ));
        }

        return Ok(WhySelector::Timestamp {
            from_created_at,
            to_created_at,
        });
    }

    Ok(WhySelector::Auto)
}

fn empty_why_changed_response(
    symbol_id: String,
    selector_mode: AetherWhySelectorMode,
    reason: AetherWhyChangedReason,
) -> AetherWhyChangedResponse {
    AetherWhyChangedResponse {
        symbol_id,
        found: false,
        reason: Some(reason),
        selector_mode,
        from: None,
        to: None,
        prior_summary: None,
        current_summary: None,
        fields_added: Vec::new(),
        fields_removed: Vec::new(),
        fields_modified: Vec::new(),
    }
}

fn parse_sir_history_json_fields(
    value: &str,
) -> Result<serde_json::Map<String, Value>, AetherMcpError> {
    let parsed: Value = serde_json::from_str(value)?;
    let Value::Object(fields) = parsed else {
        return Err(AetherMcpError::Message(
            "sir_history row contains non-object sir_json".to_owned(),
        ));
    };
    Ok(fields)
}

fn extract_intent_field(fields: &serde_json::Map<String, Value>) -> Option<String> {
    fields
        .get("intent")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn diff_top_level_field_names(
    from: &serde_json::Map<String, Value>,
    to: &serde_json::Map<String, Value>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut fields_added = to
        .keys()
        .filter(|key| !from.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let mut fields_removed = from
        .keys()
        .filter(|key| !to.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let mut fields_modified = from
        .iter()
        .filter_map(|(key, from_value)| {
            let to_value = to.get(key)?;
            (from_value != to_value).then(|| key.clone())
        })
        .collect::<Vec<_>>();

    fields_added.sort_unstable();
    fields_removed.sort_unstable();
    fields_modified.sort_unstable();

    (fields_added, fields_removed, fields_modified)
}

fn fuse_hybrid_matches(
    lexical: &[AetherSymbolLookupMatch],
    semantic: &[AetherSymbolLookupMatch],
    limit: u32,
) -> Vec<AetherSymbolLookupMatch> {
    const RRF_K: f32 = 60.0;

    let mut by_id: HashMap<String, AetherSymbolLookupMatch> = HashMap::new();
    let mut score_by_id: HashMap<String, f32> = HashMap::new();

    for (rank, row) in lexical.iter().enumerate() {
        let id = row.symbol_id.clone();
        by_id.entry(id.clone()).or_insert_with(|| row.clone());
        *score_by_id.entry(id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
    }

    for (rank, row) in semantic.iter().enumerate() {
        let id = row.symbol_id.clone();
        by_id
            .entry(id.clone())
            .and_modify(|existing| {
                if existing.semantic_score.is_none() && row.semantic_score.is_some() {
                    existing.semantic_score = row.semantic_score;
                }
            })
            .or_insert_with(|| row.clone());
        *score_by_id.entry(id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
    }

    let mut ranked: Vec<(String, f32)> = score_by_id.into_iter().collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });

    ranked
        .into_iter()
        .take(limit.clamp(1, 100) as usize)
        .filter_map(|(symbol_id, _)| by_id.remove(&symbol_id))
        .collect()
}
