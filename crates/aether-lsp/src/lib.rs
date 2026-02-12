use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aether_core::{
    HoverMarkdownSections, NO_SIR_MESSAGE, SourceRange, format_hover_markdown_sections,
    normalize_path, stable_symbol_id, stale_warning_message,
};
use aether_parse::{SymbolExtractor, language_for_path};
use aether_sir::SirAnnotation;
use aether_store::{SqliteStore, Store, StoreError};
use serde_json::Value;
use thiserror::Error;
use tower_lsp::lsp_types::{
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    MarkupContent, MarkupKind, Position, ServerCapabilities,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

#[derive(Debug, Error)]
pub enum HoverResolveError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("invalid SIR JSON: {0}")]
    InvalidSirJson(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum LspServerError {
    #[error("store init error: {0}")]
    Store(#[from] StoreError),
}

pub struct AetherLspBackend {
    client: Client,
    workspace_root: PathBuf,
    store: Arc<Mutex<SqliteStore>>,
}

impl AetherLspBackend {
    pub fn new(client: Client, workspace_root: PathBuf, store: Arc<Mutex<SqliteStore>>) -> Self {
        Self {
            client,
            workspace_root,
            store,
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for AetherLspBackend {
    async fn initialize(
        &self,
        _: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: tower_lsp::lsp_types::InitializedParams) {
        let _ = self
            .client
            .log_message(
                tower_lsp::lsp_types::MessageType::INFO,
                "AETHER LSP initialized",
            )
            .await;
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        Ok(())
    }

    async fn hover(&self, params: HoverParams) -> tower_lsp::jsonrpc::Result<Option<Hover>> {
        let text_doc_pos = params.text_document_position_params;
        let file_path = match text_doc_pos.text_document.uri.to_file_path() {
            Ok(path) => path,
            Err(()) => return Ok(Some(no_sir_hover())),
        };

        let resolution = {
            let guard = match self.store.lock() {
                Ok(guard) => guard,
                Err(_) => return Ok(Some(no_sir_hover())),
            };

            resolve_hover_markdown_for_path(
                &self.workspace_root,
                &guard,
                &file_path,
                text_doc_pos.position,
            )
        };

        let markdown = match resolution {
            Ok(value) => value,
            Err(err) => {
                let _ = self
                    .client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::ERROR,
                        format!("AETHER hover resolution failed: {err}"),
                    )
                    .await;
                None
            }
        };

        Ok(Some(match markdown {
            Some(value) => markdown_hover(value),
            None => no_sir_hover(),
        }))
    }
}

pub async fn run_stdio(workspace_root: PathBuf) -> Result<(), LspServerError> {
    let store = Arc::new(Mutex::new(SqliteStore::open(&workspace_root)?));

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| {
        AetherLspBackend::new(client, workspace_root.clone(), store.clone())
    });

    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}

pub fn resolve_hover_markdown_for_path(
    workspace_root: &Path,
    store: &SqliteStore,
    file_path: &Path,
    position: Position,
) -> Result<Option<String>, HoverResolveError> {
    let language = match language_for_path(file_path) {
        Some(language) => language,
        None => return Ok(None),
    };

    let source = std::fs::read_to_string(file_path)?;

    let display_path = workspace_relative_display_path(workspace_root, file_path);

    let mut extractor =
        SymbolExtractor::new().map_err(|err| HoverResolveError::Parse(err.to_string()))?;
    let symbols = extractor
        .extract_from_source(language, &display_path, &source)
        .map_err(|err| HoverResolveError::Parse(err.to_string()))?;

    let cursor_line = (position.line as usize) + 1;
    let cursor_column = (position.character as usize) + 1;

    let target_symbol = symbols
        .iter()
        .filter(|symbol| position_in_range(symbol.range, cursor_line, cursor_column))
        .min_by_key(|symbol| symbol_span_score(symbol.range));

    let Some(symbol) = target_symbol else {
        return Ok(None);
    };

    let symbol_id = stable_symbol_id(
        symbol.language,
        &symbol.file_path,
        symbol.kind,
        &symbol.qualified_name,
        &symbol.signature_fingerprint,
    );

    let sir_meta = store.get_sir_meta(&symbol_id)?;
    let stale_warning = stale_warning_message(
        sir_meta.as_ref().map(|meta| meta.sir_status.as_str()),
        sir_meta
            .as_ref()
            .and_then(|meta| meta.last_error.as_deref()),
    );

    let sir_json = match store.read_sir_blob(&symbol_id)? {
        Some(json) => json,
        None => {
            if let Some(warning) = stale_warning {
                return Ok(Some(format!("{warning}\n\n{NO_SIR_MESSAGE}")));
            }

            return Ok(None);
        }
    };

    let sir: SirAnnotation = serde_json::from_str(&sir_json)?;
    let mut markdown = format_hover_markdown_sections(
        &HoverMarkdownSections {
            symbol: symbol.qualified_name.clone(),
            intent: sir.intent.clone(),
            confidence: sir.confidence,
            inputs: sir.inputs,
            outputs: sir.outputs,
            side_effects: sir.side_effects,
            dependencies: sir.dependencies,
            error_modes: sir.error_modes,
        },
        stale_warning.as_deref(),
    );
    if let Some(why_hint) = compact_why_hint(store, &symbol_id)? {
        markdown.push_str("\n\n");
        markdown.push_str(&why_hint);
    }

    Ok(Some(markdown))
}

fn compact_why_hint(store: &SqliteStore, symbol_id: &str) -> Result<Option<String>, StoreError> {
    let history = store.list_sir_history(symbol_id)?;
    if history.is_empty() {
        return Ok(None);
    }
    if history.len() == 1 {
        return Ok(Some(
            "> AETHER WHY: only one recorded SIR version.".to_owned(),
        ));
    }

    let to = history.last().expect("len checked above");
    let from = &history[history.len() - 2];

    let (added, removed, modified) = match (
        parse_history_fields(&from.sir_json),
        parse_history_fields(&to.sir_json),
    ) {
        (Some(from_fields), Some(to_fields)) => diff_top_level_fields(&from_fields, &to_fields),
        _ => (Vec::new(), Vec::new(), Vec::new()),
    };
    let summary = format!(
        "> AETHER WHY: latest v{} -> v{}; added: {}; removed: {}; modified: {}.",
        from.version,
        to.version,
        format_compact_field_list(&added),
        format_compact_field_list(&removed),
        format_compact_field_list(&modified),
    );

    Ok(Some(summary))
}

fn parse_history_fields(value: &str) -> Option<serde_json::Map<String, Value>> {
    let parsed: Value = serde_json::from_str(value).ok()?;
    let Value::Object(fields) = parsed else {
        return None;
    };
    Some(fields)
}

fn diff_top_level_fields(
    from: &serde_json::Map<String, Value>,
    to: &serde_json::Map<String, Value>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut added = to
        .keys()
        .filter(|key| !from.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let mut removed = from
        .keys()
        .filter(|key| !to.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let mut modified = from
        .iter()
        .filter_map(|(key, from_value)| {
            let to_value = to.get(key)?;
            (from_value != to_value).then(|| key.clone())
        })
        .collect::<Vec<_>>();

    added.sort_unstable();
    removed.sort_unstable();
    modified.sort_unstable();

    (added, removed, modified)
}

fn format_compact_field_list(fields: &[String]) -> String {
    if fields.is_empty() {
        "none".to_owned()
    } else {
        fields.join(",")
    }
}

fn workspace_relative_display_path(workspace_root: &Path, file_path: &Path) -> String {
    if let Ok(relative) = file_path.strip_prefix(workspace_root) {
        return normalize_path(&relative.to_string_lossy());
    }

    normalize_path(&file_path.to_string_lossy())
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

fn markdown_hover(value: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: None,
    }
}

fn no_sir_hover() -> Hover {
    markdown_hover(NO_SIR_MESSAGE.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_store::{SirMetaRecord, Store};
    use tempfile::tempdir;
    use tower_lsp::lsp_types::Position;

    use super::*;

    #[test]
    fn resolve_hover_formats_sectioned_markdown() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let source_file = write_source_file(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let symbol_id = symbol_id_at(workspace, &source_file, Position::new(0, 4));

        store
            .write_sir_blob(
                &symbol_id,
                r#"{
                    "intent":"Mock summary for alpha",
                    "inputs":["x"],
                    "outputs":["y"],
                    "side_effects":[],
                    "dependencies":["serde"],
                    "error_modes":[],
                    "confidence":0.75
                }"#,
            )
            .expect("write sir blob");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol_id,
                sir_hash: "hash-alpha".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: 1_700_000_000,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_000,
            })
            .expect("upsert sir meta");

        let markdown =
            resolve_hover_markdown_for_path(workspace, &store, &source_file, Position::new(0, 4))
                .expect("resolve hover")
                .expect("hover markdown");

        assert!(markdown.contains("### alpha"));
        assert!(markdown.contains("**Confidence:** 0.75"));
        assert!(markdown.contains("**Intent**\nMock summary for alpha"));
        assert!(markdown.contains("**Inputs**\n- x"));
        assert!(markdown.contains("**Side Effects**\n(none)"));
    }

    #[test]
    fn resolve_hover_includes_stale_warning_when_blob_missing() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let source_file = write_source_file(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let symbol_id = symbol_id_at(workspace, &source_file, Position::new(0, 4));

        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol_id,
                sir_hash: "hash-alpha".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: 1_700_000_000,
                sir_status: "stale".to_owned(),
                last_error: Some("provider timeout".to_owned()),
                last_attempt_at: 1_700_000_001,
            })
            .expect("upsert stale sir meta");

        let markdown =
            resolve_hover_markdown_for_path(workspace, &store, &source_file, Position::new(0, 4))
                .expect("resolve hover")
                .expect("hover markdown");

        assert!(markdown.contains("AETHER WARNING: SIR is stale. Last error: provider timeout"));
        assert!(markdown.contains(NO_SIR_MESSAGE));
    }

    #[test]
    fn resolve_hover_includes_stale_warning_in_sectioned_output() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let source_file = write_source_file(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let symbol_id = symbol_id_at(workspace, &source_file, Position::new(0, 4));

        store
            .write_sir_blob(
                &symbol_id,
                r#"{
                    "intent":"Mock summary for alpha",
                    "inputs":[],
                    "outputs":[],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.80
                }"#,
            )
            .expect("write sir blob");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol_id,
                sir_hash: "hash-alpha".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: 1_700_000_000,
                sir_status: "stale".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_002,
            })
            .expect("upsert stale sir meta");

        let markdown =
            resolve_hover_markdown_for_path(workspace, &store, &source_file, Position::new(0, 4))
                .expect("resolve hover")
                .expect("hover markdown");

        assert!(markdown.contains("> AETHER WARNING: SIR is stale."));
        assert!(markdown.contains("### alpha"));
    }

    #[test]
    fn resolve_hover_includes_compact_why_hint_for_latest_transition() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let source_file = write_source_file(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let symbol_id = symbol_id_at(workspace, &source_file, Position::new(0, 4));

        store
            .write_sir_blob(
                &symbol_id,
                r#"{
                    "intent":"v2",
                    "inputs":["x"],
                    "outputs":["z"],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.90
                }"#,
            )
            .expect("write sir blob");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol_id.clone(),
                sir_hash: "hash-v2".to_owned(),
                sir_version: 2,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: 1_700_700_200,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_700_200,
            })
            .expect("upsert sir meta");

        store
            .record_sir_version_if_changed(
                &symbol_id,
                "hash-v1",
                "mock",
                "mock",
                r#"{
                    "intent":"v1",
                    "inputs":["x"],
                    "outputs":["y"],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.50
                }"#,
                1_700_700_100,
                None,
            )
            .expect("insert history v1");
        store
            .record_sir_version_if_changed(
                &symbol_id,
                "hash-v2",
                "mock",
                "mock",
                r#"{
                    "intent":"v2",
                    "inputs":["x"],
                    "outputs":["z"],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.90
                }"#,
                1_700_700_200,
                None,
            )
            .expect("insert history v2");

        let markdown =
            resolve_hover_markdown_for_path(workspace, &store, &source_file, Position::new(0, 4))
                .expect("resolve hover")
                .expect("hover markdown");

        assert!(markdown.contains("### alpha"));
        assert!(markdown.contains("> AETHER WHY: latest v1 -> v2;"));
        assert!(
            markdown.contains("added: none; removed: none; modified: confidence,intent,outputs.")
        );
    }

    #[test]
    fn resolve_hover_reports_single_history_version_hint() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let source_file = write_source_file(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let symbol_id = symbol_id_at(workspace, &source_file, Position::new(0, 4));

        store
            .write_sir_blob(
                &symbol_id,
                r#"{
                    "intent":"v1",
                    "inputs":["x"],
                    "outputs":["y"],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.50
                }"#,
            )
            .expect("write sir blob");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol_id.clone(),
                sir_hash: "hash-v1".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: 1_700_710_100,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_710_100,
            })
            .expect("upsert sir meta");
        store
            .record_sir_version_if_changed(
                &symbol_id,
                "hash-v1",
                "mock",
                "mock",
                r#"{
                    "intent":"v1",
                    "inputs":["x"],
                    "outputs":["y"],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.50
                }"#,
                1_700_710_100,
                None,
            )
            .expect("insert history v1");

        let markdown =
            resolve_hover_markdown_for_path(workspace, &store, &source_file, Position::new(0, 4))
                .expect("resolve hover")
                .expect("hover markdown");

        assert!(markdown.contains("> AETHER WHY: only one recorded SIR version."));
    }

    fn write_source_file(workspace: &Path) -> std::path::PathBuf {
        let src_dir = workspace.join("src");
        fs::create_dir_all(&src_dir).expect("create src dir");
        let file = src_dir.join("lib.rs");
        fs::write(&file, "fn alpha(x: i32) -> i32 { x + 1 }\n").expect("write source");
        file
    }

    fn symbol_id_at(workspace: &Path, file_path: &Path, position: Position) -> String {
        let language = language_for_path(file_path).expect("language");
        let source = fs::read_to_string(file_path).expect("read source");
        let display_path = workspace_relative_display_path(workspace, file_path);

        let mut extractor = SymbolExtractor::new().expect("extractor");
        let symbols = extractor
            .extract_from_source(language, &display_path, &source)
            .expect("extract symbols");

        let cursor_line = (position.line as usize) + 1;
        let cursor_column = (position.character as usize) + 1;
        let symbol = symbols
            .iter()
            .filter(|symbol| position_in_range(symbol.range, cursor_line, cursor_column))
            .min_by_key(|symbol| symbol_span_score(symbol.range))
            .expect("symbol at cursor");

        stable_symbol_id(
            symbol.language,
            &symbol.file_path,
            symbol.kind,
            &symbol.qualified_name,
            &symbol.signature_fingerprint,
        )
    }
}
