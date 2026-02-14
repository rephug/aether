use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aether_core::{
    HoverMarkdownSections, NO_SIR_MESSAGE, SourceRange, format_hover_markdown_sections,
    normalize_path, stable_symbol_id, stale_warning_message,
};
use aether_parse::{SymbolExtractor, language_for_path};
use aether_sir::{FileSir, SirAnnotation, synthetic_file_sir_id};
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

    if let Some(markdown) = resolve_import_hover_markdown(
        workspace_root,
        store,
        file_path,
        &source,
        cursor_line,
        cursor_column,
    )? {
        return Ok(Some(markdown));
    }

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

fn resolve_import_hover_markdown(
    workspace_root: &Path,
    store: &SqliteStore,
    source_file: &Path,
    source: &str,
    cursor_line: usize,
    cursor_column: usize,
) -> Result<Option<String>, HoverResolveError> {
    let Some(line) = source.lines().nth(cursor_line.saturating_sub(1)) else {
        return Ok(None);
    };
    if !line.contains("import") {
        return Ok(None);
    }

    let Some(import_target) = extract_import_path_literal_at_cursor(line, cursor_column) else {
        return Ok(None);
    };
    if !(import_target.starts_with("./") || import_target.starts_with("../")) {
        return Ok(None);
    }

    let Some(resolved_import_path) = resolve_relative_import_target(source_file, &import_target)
    else {
        return Ok(None);
    };
    let resolved_import_path = resolved_import_path.canonicalize()?;
    if !resolved_import_path.starts_with(workspace_root) {
        return Ok(None);
    }

    let Some(language) = language_for_path(&resolved_import_path) else {
        return Ok(None);
    };
    let relative_path = workspace_relative_display_path(workspace_root, &resolved_import_path);
    let file_rollup_id = synthetic_file_sir_id(language.as_str(), &relative_path);

    if let Some(rollup_blob) = store.read_sir_blob(&file_rollup_id)?
        && let Ok(file_sir) = serde_json::from_str::<FileSir>(&rollup_blob)
    {
        return Ok(Some(format_file_rollup_markdown(&relative_path, &file_sir)));
    }

    let mut imported_symbols = store.list_symbols_for_file(&relative_path)?;
    imported_symbols.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
    let Some(first_symbol) = imported_symbols.first() else {
        return Ok(None);
    };
    let Some(leaf_blob) = store.read_sir_blob(&first_symbol.id)? else {
        return Ok(None);
    };

    let sir: SirAnnotation = serde_json::from_str(&leaf_blob)?;
    Ok(Some(format_hover_markdown_sections(
        &HoverMarkdownSections {
            symbol: first_symbol.qualified_name.clone(),
            intent: sir.intent,
            confidence: sir.confidence,
            inputs: sir.inputs,
            outputs: sir.outputs,
            side_effects: sir.side_effects,
            dependencies: sir.dependencies,
            error_modes: sir.error_modes,
        },
        None,
    )))
}

fn extract_import_path_literal_at_cursor(line: &str, cursor_column: usize) -> Option<String> {
    let bytes = line.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        let quote = bytes[index];
        if quote != b'"' && quote != b'\'' {
            index += 1;
            continue;
        }

        let start = index + 1;
        index += 1;
        while index < bytes.len() && bytes[index] != quote {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }

        let end = index;
        let literal_start_col = start + 1;
        let literal_end_col = end + 1;
        if (literal_start_col..=literal_end_col).contains(&cursor_column) && start <= end {
            let literal = line[start..end].trim();
            if !literal.is_empty() {
                return Some(literal.to_owned());
            }
        }

        index += 1;
    }

    None
}

fn resolve_relative_import_target(source_file: &Path, import_target: &str) -> Option<PathBuf> {
    let base_dir = source_file.parent()?;
    let candidate = base_dir.join(import_target);

    if candidate.extension().is_some() && candidate.is_file() {
        return Some(candidate);
    }

    let mut candidates = Vec::new();
    if candidate.extension().is_some() {
        candidates.push(candidate.clone());
    } else {
        for ext in ["ts", "tsx", "js", "jsx"] {
            candidates.push(candidate.with_extension(ext));
        }
        for index_file in ["index.ts", "index.tsx", "index.js", "index.jsx"] {
            candidates.push(candidate.join(index_file));
        }
    }

    candidates.into_iter().find(|path| path.is_file())
}

fn format_file_rollup_markdown(file_path: &str, file_sir: &FileSir) -> String {
    [
        format!("### {file_path}"),
        format!("**Confidence:** {:.2}", file_sir.confidence),
        format!("**Symbol Count:** {}", file_sir.symbol_count),
        format!("**Intent**\n{}", file_sir.intent),
        format!("**Exports**\n{}", format_markdown_list(&file_sir.exports)),
        format!(
            "**Side Effects**\n{}",
            format_markdown_list(&file_sir.side_effects)
        ),
        format!(
            "**Dependencies**\n{}",
            format_markdown_list(&file_sir.dependencies)
        ),
        format!(
            "**Error Modes**\n{}",
            format_markdown_list(&file_sir.error_modes)
        ),
    ]
    .join("\n\n")
}

fn format_markdown_list(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("- {}", item.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    }
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

    use aether_store::{SirMetaRecord, Store, SymbolRecord};
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

    #[test]
    fn resolve_hover_on_typescript_import_returns_file_rollup_summary() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let src_dir = workspace.join("src");
        fs::create_dir_all(&src_dir).expect("create src dir");

        let dep_file = src_dir.join("dep.ts");
        fs::write(&dep_file, "export function dep(): number { return 1; }\n")
            .expect("write dep source");
        let app_file = src_dir.join("app.ts");
        fs::write(
            &app_file,
            "import { dep } from \"./dep\";\nexport function run(): number { return dep(); }\n",
        )
        .expect("write app source");

        let store = SqliteStore::open(workspace).expect("open store");
        let file_rollup_id = synthetic_file_sir_id("typescript", "src/dep.ts");
        store
            .write_sir_blob(
                &file_rollup_id,
                r#"{
                    "intent":"Mock file summary for dep.ts",
                    "exports":["dep"],
                    "side_effects":["network"],
                    "dependencies":["axios"],
                    "error_modes":[],
                    "symbol_count":1,
                    "confidence":0.88
                }"#,
            )
            .expect("write file rollup");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: file_rollup_id,
                sir_hash: "hash-file".to_owned(),
                sir_version: 1,
                provider: "rollup".to_owned(),
                model: "deterministic".to_owned(),
                updated_at: 1_700_800_100,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_800_100,
            })
            .expect("upsert file rollup meta");

        let first_line = fs::read_to_string(&app_file)
            .expect("read app source")
            .lines()
            .next()
            .expect("import line")
            .to_owned();
        let import_col = first_line.find("./dep").expect("import path");
        let markdown = resolve_hover_markdown_for_path(
            workspace,
            &store,
            &app_file,
            Position::new(0, (import_col + 2) as u32),
        )
        .expect("resolve hover")
        .expect("hover markdown");

        assert!(markdown.contains("### src/dep.ts"));
        assert!(markdown.contains("Mock file summary for dep.ts"));
        assert!(markdown.contains("**Exports**"));
    }

    #[test]
    fn resolve_hover_on_typescript_import_falls_back_to_leaf_sir_when_file_rollup_missing() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let src_dir = workspace.join("src");
        fs::create_dir_all(&src_dir).expect("create src dir");

        let dep_file = src_dir.join("dep.ts");
        fs::write(&dep_file, "export function dep(): number { return 1; }\n")
            .expect("write dep source");
        let app_file = src_dir.join("app.ts");
        fs::write(
            &app_file,
            "import { dep } from \"./dep\";\nexport function run(): number { return dep(); }\n",
        )
        .expect("write app source");

        let store = SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: "dep-symbol".to_owned(),
                file_path: "src/dep.ts".to_owned(),
                language: "typescript".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "dep".to_owned(),
                signature_fingerprint: "sig-dep".to_owned(),
                last_seen_at: 1_700_800_200,
            })
            .expect("upsert dep symbol");
        store
            .write_sir_blob(
                "dep-symbol",
                r#"{
                    "intent":"Leaf summary for dep",
                    "inputs":[],
                    "outputs":["number"],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.77
                }"#,
            )
            .expect("write leaf sir");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: "dep-symbol".to_owned(),
                sir_hash: "hash-leaf".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: 1_700_800_200,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_800_200,
            })
            .expect("upsert leaf meta");

        let first_line = fs::read_to_string(&app_file)
            .expect("read app source")
            .lines()
            .next()
            .expect("import line")
            .to_owned();
        let import_col = first_line.find("./dep").expect("import path");
        let markdown = resolve_hover_markdown_for_path(
            workspace,
            &store,
            &app_file,
            Position::new(0, (import_col + 2) as u32),
        )
        .expect("resolve hover")
        .expect("hover markdown");

        assert!(markdown.contains("### dep"));
        assert!(markdown.contains("Leaf summary for dep"));
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
