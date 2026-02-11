use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aether_core::{SourceRange, Symbol, normalize_path, stable_symbol_id};
use aether_parse::{SymbolExtractor, language_for_path};
use aether_sir::{SirAnnotation, SirError, canonicalize_sir_json, sir_hash, validate_sir};
use aether_store::{SirMetaRecord, SqliteStore, Store, StoreError, SymbolSearchResult};
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
use thiserror::Error;

pub const SERVER_NAME: &str = "aether";
pub const SERVER_VERSION: &str = "0.1.0";
pub const SERVER_DESCRIPTION: &str = "AETHER local symbol/SIR lookup from .aether store";
pub const NO_SIR_MESSAGE: &str =
    "AETHER: No SIR yet for this symbol. Run aetherd indexing and try again.";

#[derive(Debug, Error)]
pub enum AetherMcpError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sir validation error: {0}")]
    Sir(#[from] SirError),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherStatusResponse {
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
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolLookupResponse {
    pub matches: Vec<AetherSymbolLookupMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSearchRequest {
    pub query: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSearchResponse {
    pub matches: Vec<AetherSymbolLookupMatch>,
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
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        self.search_matches(&request.query, request.limit)
    }

    pub fn aether_search_logic(
        &self,
        request: AetherSearchRequest,
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        self.search_matches(&request.query, request.limit)
    }

    fn search_matches(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        let sqlite_path = self.sqlite_path();
        if !sqlite_path.exists() {
            return Ok(Vec::new());
        }

        let store = SqliteStore::open(&self.workspace)?;
        let matches = store.search_symbols(query, limit.unwrap_or(20).min(100))?;

        Ok(matches
            .into_iter()
            .map(AetherSymbolLookupMatch::from)
            .collect())
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
        let sir = self.read_valid_sir_blob(&symbol_id)?;

        let (found, hover_markdown, sir) = match sir {
            Some(sir) => (
                true,
                format_hover_markdown(symbol, &sir),
                Some(SirAnnotationView::from(sir)),
            ),
            None => (false, NO_SIR_MESSAGE.to_owned(), None),
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
            eprintln!("{message}");
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
            .map(|matches| Json(AetherSymbolLookupResponse { matches }))
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
            .map(|matches| Json(AetherSearchResponse { matches }))
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

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_owned()
    } else {
        items.join(", ")
    }
}

fn format_hover_markdown(symbol: &Symbol, sir: &SirAnnotation) -> String {
    format!(
        "{}\n\n**symbol:** {}\n**confidence:** {:.2}\n**inputs:** {}\n**outputs:** {}\n**side_effects:** {}\n**dependencies:** {}\n**error_modes:** {}",
        sir.intent,
        symbol.qualified_name,
        sir.confidence,
        list_or_none(&sir.inputs),
        list_or_none(&sir.outputs),
        list_or_none(&sir.side_effects),
        list_or_none(&sir.dependencies),
        list_or_none(&sir.error_modes),
    )
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
