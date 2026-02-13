use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_config::{VerifyMode, load_workspace_config};
pub use aether_core::SearchMode;
use aether_core::{
    HoverMarkdownSections, NO_SIR_MESSAGE, SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR,
    SEARCH_FALLBACK_EMBEDDINGS_DISABLED, SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED,
    SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY, SearchEnvelope, SourceRange,
    format_hover_markdown_sections, normalize_path, stable_symbol_id, stale_warning_message,
};
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_parse::{SymbolExtractor, language_for_path};
use aether_sir::{SirAnnotation, SirError, canonicalize_sir_json, sir_hash, validate_sir};
use aether_store::{
    SirHistorySelector, SirMetaRecord, SqliteStore, Store, StoreError, SymbolSearchResult,
    open_vector_store,
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
    pub symbol_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherGetSirResponse {
    pub found: bool,
    pub symbol_id: String,
    pub sir: Option<SirAnnotationView>,
    pub sir_json: String,
    pub sir_hash: String,
    pub sir_status: Option<String>,
    pub last_error: Option<String>,
    pub last_attempt_at: Option<i64>,
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

impl From<SirAnnotation> for SirAnnotationView {
    fn from(value: SirAnnotation) -> Self {
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

    pub async fn aether_search_logic(
        &self,
        request: AetherSearchRequest,
    ) -> Result<AetherSearchResponse, AetherMcpError> {
        let mode_requested = request.mode.unwrap_or_default();
        let limit = effective_limit(request.limit);
        let lexical = self.lexical_search_matches(&request.query, limit)?;

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
                    SearchEnvelope {
                        mode_requested: SearchMode::Hybrid,
                        mode_used: SearchMode::Hybrid,
                        fallback_reason: None,
                        matches: fuse_hybrid_matches(&lexical, &semantic, limit),
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

    pub fn aether_get_sir_logic(
        &self,
        symbol_id: &str,
    ) -> Result<AetherGetSirResponse, AetherMcpError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(AetherGetSirResponse {
                found: false,
                symbol_id: String::new(),
                sir: None,
                sir_json: String::new(),
                sir_hash: String::new(),
                sir_status: None,
                last_error: None,
                last_attempt_at: None,
            });
        }

        if !self.sqlite_path().exists() {
            return Ok(AetherGetSirResponse {
                found: false,
                symbol_id: symbol_id.to_owned(),
                sir: None,
                sir_json: String::new(),
                sir_hash: String::new(),
                sir_status: None,
                last_error: None,
                last_attempt_at: None,
            });
        }

        let store = SqliteStore::open(&self.workspace)?;
        let meta = store.get_sir_meta(symbol_id)?;
        let (sir_status, last_error, last_attempt_at) = meta_status_fields(meta.as_ref());
        let sir_blob = store.read_sir_blob(symbol_id)?;

        let Some(sir_blob) = sir_blob else {
            return Ok(AetherGetSirResponse {
                found: false,
                symbol_id: symbol_id.to_owned(),
                sir: None,
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
        // Stage-3 compatibility: keep serving SIR when metadata is stale or partially empty.
        let hash = meta
            .as_ref()
            .map(|record| record.sir_hash.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| sir_hash(&sir));

        Ok(AetherGetSirResponse {
            found: true,
            symbol_id: symbol_id.to_owned(),
            sir: Some(sir.into()),
            sir_json: canonical_json,
            sir_hash: hash,
            sir_status,
            last_error,
            last_attempt_at,
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

    #[tool(name = "aether_get_sir", description = "Get SIR for a symbol ID")]
    pub async fn aether_get_sir(
        &self,
        Parameters(request): Parameters<AetherGetSirRequest>,
    ) -> Result<Json<AetherGetSirResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_get_sir");
        self.aether_get_sir_logic(&request.symbol_id)
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
