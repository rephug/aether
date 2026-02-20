use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_core::{
    HoverMarkdownSections, Language, NO_SIR_MESSAGE, SourceRange, format_hover_markdown_sections,
    normalize_path, stable_symbol_id, stale_warning_message,
};
use aether_parse::{RustUsePrefix, SymbolExtractor, language_for_path, rust_use_path_at_cursor};
use aether_sir::{FileSir, SirAnnotation, synthetic_file_sir_id};
use aether_store::{CouplingEdgeRecord, CozoGraphStore, SqliteStore, Store, StoreError};
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
    store.increment_symbol_access_debounced(
        std::slice::from_ref(&symbol_id),
        current_unix_timestamp_millis(),
    )?;

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

    let context_lines = collect_project_context_lines(
        workspace_root,
        store,
        symbol.file_path.as_str(),
        symbol_id.as_str(),
        current_unix_timestamp_millis(),
    )?;
    if !context_lines.is_empty() {
        markdown.push_str("\n\n---\n");
        markdown.push_str(context_lines.join("\n").as_str());
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
    if matches!(language_for_path(source_file), Some(Language::Rust)) {
        return resolve_rust_import_hover_markdown(
            workspace_root,
            store,
            source_file,
            source,
            cursor_line,
            cursor_column,
        );
    }

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

fn resolve_rust_import_hover_markdown(
    workspace_root: &Path,
    store: &SqliteStore,
    source_file: &Path,
    source: &str,
    cursor_line: usize,
    cursor_column: usize,
) -> Result<Option<String>, HoverResolveError> {
    let Some(use_path) = rust_use_path_at_cursor(
        source,
        cursor_line.saturating_sub(1),
        cursor_column.saturating_sub(1),
    ) else {
        return Ok(None);
    };
    let Some((resolved_path, file_segment_index)) =
        resolve_rust_use_target_file(source_file, use_path.prefix, &use_path.segments)
    else {
        return Ok(None);
    };
    if use_path
        .cursor_segment_index
        .is_some_and(|index| index > file_segment_index)
    {
        return Ok(None);
    }

    let resolved_path = resolved_path.canonicalize()?;
    if !resolved_path.starts_with(workspace_root) {
        return Ok(None);
    }

    let relative_path = workspace_relative_display_path(workspace_root, &resolved_path);
    let file_rollup_id = synthetic_file_sir_id("rust", &relative_path);
    if let Some(rollup_blob) = store.read_sir_blob(&file_rollup_id)?
        && let Ok(file_sir) = serde_json::from_str::<FileSir>(&rollup_blob)
    {
        return Ok(Some(format_file_rollup_markdown(&relative_path, &file_sir)));
    }

    Ok(None)
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

fn resolve_rust_use_target_file(
    source_file: &Path,
    prefix: RustUsePrefix,
    segments: &[String],
) -> Option<(PathBuf, usize)> {
    let mut base = match prefix {
        RustUsePrefix::Crate => find_crate_src_root(source_file)?,
        RustUsePrefix::Self_ => rust_self_base_dir(source_file)?,
        RustUsePrefix::Super => rust_self_base_dir(source_file)?.parent()?.to_path_buf(),
    };
    if segments.is_empty() {
        return None;
    }

    let mut resolved_file = None;
    let mut resolved_segment_index = 0usize;

    for (index, segment) in segments.iter().enumerate() {
        let mod_candidate = base.join(segment).join("mod.rs");
        let file_candidate = base.join(format!("{segment}.rs"));
        let has_mod = mod_candidate.is_file();
        let has_file = file_candidate.is_file();

        if has_mod {
            resolved_file = Some(mod_candidate);
            resolved_segment_index = index;
            base = base.join(segment);
            continue;
        }
        if has_file {
            resolved_file = Some(file_candidate);
            resolved_segment_index = index;
            break;
        }
        if resolved_file.is_some() {
            break;
        }
        return None;
    }

    resolved_file.map(|path| (path, resolved_segment_index))
}

fn rust_self_base_dir(source_file: &Path) -> Option<PathBuf> {
    Some(source_file.parent()?.to_path_buf())
}

fn find_crate_src_root(source_file: &Path) -> Option<PathBuf> {
    for dir in source_file.ancestors() {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.is_file() {
            let src = dir.join("src");
            if src.is_dir() {
                return Some(src);
            }
            return None;
        }
    }
    None
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

fn collect_project_context_lines(
    workspace_root: &Path,
    store: &SqliteStore,
    file_path: &str,
    symbol_id: &str,
    now_ms: i64,
) -> Result<Vec<String>, HoverResolveError> {
    let mut lines = Vec::new();

    if let Some(line) = top_project_note_line(store, file_path, symbol_id, now_ms)? {
        lines.push(line);
    }
    if let Some(line) = top_coupling_line(workspace_root, file_path)? {
        lines.push(line);
    }
    let test_lines = top_test_intent_lines(store, file_path, symbol_id)?;
    lines.extend(test_lines);

    Ok(lines)
}

fn top_project_note_line(
    store: &SqliteStore,
    file_path: &str,
    symbol_id: &str,
    now_ms: i64,
) -> Result<Option<String>, HoverResolveError> {
    let notes = store.list_project_notes_for_file_ref(file_path, 20)?;
    if notes.is_empty() {
        return Ok(None);
    }

    let selected = notes
        .iter()
        .find(|note| note.symbol_refs.iter().any(|value| value == symbol_id))
        .unwrap_or(&notes[0]);

    let age = format_relative_age(now_ms, selected.updated_at);
    let snippet = compact_text(selected.content.as_str(), 110);
    Ok(Some(format!("📝 \"{snippet}\" ({age})")))
}

fn top_coupling_line(
    workspace_root: &Path,
    file_path: &str,
) -> Result<Option<String>, HoverResolveError> {
    let cozo = match CozoGraphStore::open(workspace_root) {
        Ok(store) => store,
        Err(_) => return Ok(None),
    };

    let mut edges = match cozo.list_co_change_edges_for_file(file_path, 0.0) {
        Ok(edges) => edges,
        Err(_) => return Ok(None),
    };
    if edges.is_empty() {
        return Ok(None);
    }
    edges.sort_by(|left, right| {
        right
            .fused_score
            .partial_cmp(&left.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.file_a.cmp(&right.file_a))
            .then_with(|| left.file_b.cmp(&right.file_b))
    });

    let top = &edges[0];
    let coupled_file = coupled_file_for_edge(file_path, top);
    if coupled_file.is_empty() {
        return Ok(None);
    }

    Ok(Some(format!(
        "⚠️ Co-changes with {} ({:.0}%, {})",
        coupled_file,
        (top.git_coupling.clamp(0.0, 1.0) * 100.0),
        risk_label(top.fused_score)
    )))
}

fn top_test_intent_lines(
    store: &SqliteStore,
    file_path: &str,
    symbol_id: &str,
) -> Result<Vec<String>, HoverResolveError> {
    let mut intents = store.list_test_intents_for_symbol(symbol_id)?;
    if intents.is_empty() {
        intents = store.list_test_intents_for_file(file_path)?;
    }

    let mut lines = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();
    for intent in intents {
        if lines.len() >= 3 {
            break;
        }
        if !seen.insert(intent.intent_id.clone()) {
            continue;
        }
        lines.push(format!(
            "🧪 {}",
            compact_text(intent.intent_text.as_str(), 110)
        ));
    }
    Ok(lines)
}

fn coupled_file_for_edge(file_path: &str, edge: &CouplingEdgeRecord) -> String {
    if edge.file_a == file_path {
        return edge.file_b.clone();
    }
    if edge.file_b == file_path {
        return edge.file_a.clone();
    }
    String::new()
}

fn risk_label(score: f32) -> &'static str {
    if score >= 0.7 {
        return "Critical";
    }
    if score >= 0.4 {
        return "High";
    }
    if score >= 0.2 {
        return "Medium";
    }
    "Low"
}

fn format_relative_age(now_ms: i64, ts_ms: i64) -> String {
    let age_ms = now_ms.saturating_sub(ts_ms).max(0);
    const MINUTE_MS: i64 = 60 * 1000;
    const HOUR_MS: i64 = 60 * MINUTE_MS;
    const DAY_MS: i64 = 24 * HOUR_MS;

    if age_ms < HOUR_MS {
        let minutes = (age_ms / MINUTE_MS).max(1);
        return format!("{minutes}m ago");
    }
    if age_ms < DAY_MS {
        return format!("{}h ago", (age_ms / HOUR_MS).max(1));
    }
    format!("{}d ago", (age_ms / DAY_MS).max(1))
}

fn compact_text(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= limit {
        return trimmed.to_owned();
    }

    let mut end = limit;
    while !trimmed.is_char_boundary(end) {
        end = end.saturating_sub(1);
        if end == 0 {
            break;
        }
    }
    if end == 0 {
        return String::new();
    }
    format!("{}...", &trimmed[..end])
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

fn current_unix_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
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

    use aether_store::{
        CouplingEdgeRecord, CozoGraphStore, ProjectNoteRecord, SirMetaRecord, Store, SymbolRecord,
        TestIntentRecord,
    };
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
    fn resolve_hover_enriches_with_project_context_when_available() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let source_file = write_source_file(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let symbol_id = symbol_id_at(workspace, &source_file, Position::new(0, 4));

        store
            .write_sir_blob(
                &symbol_id,
                r#"{
                    "intent":"Processes payments with retry",
                    "inputs":["x"],
                    "outputs":["y"],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.92
                }"#,
            )
            .expect("write sir blob");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol_id.clone(),
                sir_hash: "hash-context".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: 1_700_000_000,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_000,
            })
            .expect("upsert sir meta");

        store
            .upsert_project_note(ProjectNoteRecord {
                note_id: "note-context".to_owned(),
                content: "Refactored payment retries to cap timeout budget".to_owned(),
                content_hash: "hash-note-context".to_owned(),
                source_type: "session".to_owned(),
                source_agent: None,
                tags: vec!["refactor".to_owned()],
                entity_refs: Vec::new(),
                file_refs: vec!["src/lib.rs".to_owned()],
                symbol_refs: vec![symbol_id.clone()],
                created_at: 1_700_000_000_000,
                updated_at: 1_700_000_000_000,
                access_count: 0,
                last_accessed_at: None,
                is_archived: false,
            })
            .expect("insert context note");

        store
            .replace_test_intents_for_file(
                "src/lib.rs",
                &[
                    TestIntentRecord {
                        intent_id: "intent-1".to_owned(),
                        file_path: "src/lib.rs".to_owned(),
                        test_name: "test_retry_1".to_owned(),
                        intent_text: "retries on timeout".to_owned(),
                        group_label: None,
                        language: "rust".to_owned(),
                        symbol_id: Some(symbol_id.clone()),
                        created_at: 1_700_000_000_000,
                        updated_at: 1_700_000_000_100,
                    },
                    TestIntentRecord {
                        intent_id: "intent-2".to_owned(),
                        file_path: "src/lib.rs".to_owned(),
                        test_name: "test_retry_2".to_owned(),
                        intent_text: "logs retry attempts".to_owned(),
                        group_label: None,
                        language: "rust".to_owned(),
                        symbol_id: Some(symbol_id.clone()),
                        created_at: 1_700_000_000_000,
                        updated_at: 1_700_000_000_100,
                    },
                    TestIntentRecord {
                        intent_id: "intent-3".to_owned(),
                        file_path: "src/lib.rs".to_owned(),
                        test_name: "test_retry_3".to_owned(),
                        intent_text: "guards negative balance".to_owned(),
                        group_label: None,
                        language: "rust".to_owned(),
                        symbol_id: Some(symbol_id.clone()),
                        created_at: 1_700_000_000_000,
                        updated_at: 1_700_000_000_100,
                    },
                    TestIntentRecord {
                        intent_id: "intent-4".to_owned(),
                        file_path: "src/lib.rs".to_owned(),
                        test_name: "test_retry_4".to_owned(),
                        intent_text: "fourth intent should be trimmed".to_owned(),
                        group_label: None,
                        language: "rust".to_owned(),
                        symbol_id: Some(symbol_id.clone()),
                        created_at: 1_700_000_000_000,
                        updated_at: 1_700_000_000_100,
                    },
                ],
            )
            .expect("insert test intents");

        let cozo = CozoGraphStore::open(workspace).expect("open cozo");
        cozo.upsert_co_change_edges(&[CouplingEdgeRecord {
            file_a: "src/lib.rs".to_owned(),
            file_b: "src/gateway.rs".to_owned(),
            co_change_count: 8,
            total_commits_a: 10,
            total_commits_b: 9,
            git_coupling: 0.89,
            static_signal: 0.8,
            semantic_signal: 0.6,
            fused_score: 0.82,
            coupling_type: "multi".to_owned(),
            last_co_change_commit: "abc123".to_owned(),
            last_co_change_at: 1_700_000_000,
            mined_at: 1_700_000_200,
        }])
        .expect("insert coupling edge");
        drop(cozo);
        assert!(
            top_coupling_line(workspace, "src/lib.rs")
                .expect("resolve coupling line")
                .is_some()
        );

        let markdown =
            resolve_hover_markdown_for_path(workspace, &store, &source_file, Position::new(0, 4))
                .expect("resolve hover")
                .expect("hover markdown");

        assert!(markdown.contains("📝"));
        assert!(markdown.contains("Co-changes"));
        assert!(markdown.contains("🧪"));
        assert!(!markdown.contains("fourth intent should be trimmed"));
    }

    #[test]
    fn resolve_hover_does_not_append_context_when_none_exists() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let source_file = write_source_file(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let symbol_id = symbol_id_at(workspace, &source_file, Position::new(0, 4));

        store
            .write_sir_blob(
                &symbol_id,
                r#"{
                    "intent":"No context summary",
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
                sir_hash: "hash-none".to_owned(),
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

        assert!(!markdown.contains("📝"));
        assert!(!markdown.contains("⚠️"));
        assert!(!markdown.contains("🧪"));
        assert!(!markdown.contains("---\n📝"));
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

    #[test]
    fn resolve_hover_on_rust_use_crate_loader_returns_file_rollup_summary() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace_manifest(workspace);
        let src_dir = workspace.join("src");
        fs::create_dir_all(src_dir.join("config")).expect("create config dir");

        let source_file = src_dir.join("lib.rs");
        fs::write(
            &source_file,
            "use crate::config::loader;\nfn alpha() -> i32 { 1 }\n",
        )
        .expect("write source");
        fs::write(
            src_dir.join("config/loader.rs"),
            "pub fn parse_toml() -> bool { true }\n",
        )
        .expect("write loader");
        fs::write(src_dir.join("config/mod.rs"), "pub mod loader;\n").expect("write config mod");

        let store = SqliteStore::open(workspace).expect("open store");
        write_file_rollup(
            &store,
            "rust",
            "src/config/loader.rs",
            "Loader rollup summary",
        );

        let import_line = fs::read_to_string(&source_file)
            .expect("read source")
            .lines()
            .next()
            .expect("import line")
            .to_owned();
        let col = import_line.find("config").expect("config segment");
        let markdown = resolve_hover_markdown_for_path(
            workspace,
            &store,
            &source_file,
            Position::new(0, col as u32),
        )
        .expect("resolve hover")
        .expect("hover markdown");

        assert!(markdown.contains("### src/config/loader.rs"));
        assert!(markdown.contains("Loader rollup summary"));
    }

    #[test]
    fn resolve_hover_on_rust_use_crate_prefers_mod_rs_when_both_exist() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace_manifest(workspace);
        let src_dir = workspace.join("src");
        fs::create_dir_all(src_dir.join("config")).expect("create config dir");

        let source_file = src_dir.join("lib.rs");
        fs::write(&source_file, "use crate::config;\n").expect("write source");
        fs::write(src_dir.join("config.rs"), "pub fn from_file() {}\n").expect("write config.rs");
        fs::write(src_dir.join("config/mod.rs"), "pub fn from_mod() {}\n").expect("write mod.rs");

        let store = SqliteStore::open(workspace).expect("open store");
        write_file_rollup(&store, "rust", "src/config/mod.rs", "Config mod summary");

        let import_line = fs::read_to_string(&source_file)
            .expect("read source")
            .lines()
            .next()
            .expect("import line")
            .to_owned();
        let col = import_line.find("config").expect("config segment");
        let markdown = resolve_hover_markdown_for_path(
            workspace,
            &store,
            &source_file,
            Position::new(0, col as u32),
        )
        .expect("resolve hover")
        .expect("hover markdown");

        assert!(markdown.contains("### src/config/mod.rs"));
        assert!(markdown.contains("Config mod summary"));
    }

    #[test]
    fn resolve_hover_on_rust_use_super_resolves_parent_relative_file() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace_manifest(workspace);
        let src_dir = workspace.join("src");
        fs::create_dir_all(src_dir.join("config")).expect("create config dir");

        let source_file = src_dir.join("config/loader.rs");
        fs::write(&source_file, "use super::utils;\n").expect("write source");
        fs::write(src_dir.join("utils.rs"), "pub fn shared() {}\n").expect("write utils");

        let store = SqliteStore::open(workspace).expect("open store");
        write_file_rollup(&store, "rust", "src/utils.rs", "Utils rollup summary");

        let import_line = fs::read_to_string(&source_file)
            .expect("read source")
            .lines()
            .next()
            .expect("import line")
            .to_owned();
        let col = import_line.find("utils").expect("utils segment");
        let markdown = resolve_hover_markdown_for_path(
            workspace,
            &store,
            &source_file,
            Position::new(0, col as u32),
        )
        .expect("resolve hover")
        .expect("hover markdown");

        assert!(markdown.contains("### src/utils.rs"));
        assert!(markdown.contains("Utils rollup summary"));
    }

    #[test]
    fn resolve_hover_on_rust_unresolvable_use_falls_through_without_error() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace_manifest(workspace);
        let src_dir = workspace.join("src");
        fs::create_dir_all(&src_dir).expect("create src dir");

        let source_file = src_dir.join("lib.rs");
        fs::write(&source_file, "use crate::nonexistent;\n").expect("write source");
        let store = SqliteStore::open(workspace).expect("open store");

        let import_line = fs::read_to_string(&source_file)
            .expect("read source")
            .lines()
            .next()
            .expect("import line")
            .to_owned();
        let col = import_line
            .find("nonexistent")
            .expect("nonexistent segment");
        let markdown = resolve_hover_markdown_for_path(
            workspace,
            &store,
            &source_file,
            Position::new(0, col as u32),
        )
        .expect("resolve hover");

        assert!(markdown.is_none());
    }

    #[test]
    fn resolve_hover_on_rust_use_without_file_rollup_falls_back_to_leaf_hover() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_workspace_manifest(workspace);
        let src_dir = workspace.join("src");
        fs::create_dir_all(src_dir.join("config")).expect("create config dir");

        let source_file = src_dir.join("lib.rs");
        fs::write(
            &source_file,
            "use crate::config::loader;\nfn alpha(x: i32) -> i32 { x + 1 }\n",
        )
        .expect("write source");
        fs::write(
            src_dir.join("config/loader.rs"),
            "pub fn parse_toml() -> bool { true }\n",
        )
        .expect("write loader");

        let store = SqliteStore::open(workspace).expect("open store");
        let alpha_symbol_id = symbol_id_at(workspace, &source_file, Position::new(1, 4));
        store
            .write_sir_blob(
                &alpha_symbol_id,
                r#"{
                    "intent":"Leaf summary for alpha",
                    "inputs":["x"],
                    "outputs":["i32"],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.81
                }"#,
            )
            .expect("write alpha sir");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: alpha_symbol_id,
                sir_hash: "hash-alpha".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: 1_700_810_100,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_810_100,
            })
            .expect("upsert alpha meta");

        let import_line = fs::read_to_string(&source_file)
            .expect("read source")
            .lines()
            .next()
            .expect("import line")
            .to_owned();
        let import_col = import_line.find("config").expect("config segment");
        let import_markdown = resolve_hover_markdown_for_path(
            workspace,
            &store,
            &source_file,
            Position::new(0, import_col as u32),
        )
        .expect("resolve import hover");
        assert!(import_markdown.is_none());

        let leaf_markdown =
            resolve_hover_markdown_for_path(workspace, &store, &source_file, Position::new(1, 4))
                .expect("resolve leaf hover")
                .expect("leaf hover markdown");
        assert!(leaf_markdown.contains("### alpha"));
        assert!(leaf_markdown.contains("Leaf summary for alpha"));
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

    fn write_workspace_manifest(workspace: &Path) {
        fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"hover-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("write manifest");
    }

    fn write_file_rollup(store: &SqliteStore, language: &str, file_path: &str, intent: &str) {
        let rollup_id = synthetic_file_sir_id(language, file_path);
        store
            .write_sir_blob(
                &rollup_id,
                &format!(
                    r#"{{
                        "intent":"{intent}",
                        "exports":[],
                        "side_effects":[],
                        "dependencies":[],
                        "error_modes":[],
                        "symbol_count":1,
                        "confidence":0.90
                    }}"#
                ),
            )
            .expect("write rollup");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: rollup_id,
                sir_hash: "hash-rollup".to_owned(),
                sir_version: 1,
                provider: "rollup".to_owned(),
                model: "deterministic".to_owned(),
                updated_at: 1_700_800_100,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_800_100,
            })
            .expect("upsert rollup meta");
    }
}
