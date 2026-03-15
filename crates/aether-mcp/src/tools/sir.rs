use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aether_core::{
    HoverMarkdownSections, Language, NO_SIR_MESSAGE, SourceRange, format_hover_markdown_sections,
    normalize_path, stable_symbol_id, stale_warning_message,
};
use aether_parse::{SymbolExtractor, language_for_path};
use aether_sir::{
    FileSir, SirAnnotation, SirLevel, canonicalize_file_sir_json, canonicalize_sir_json,
    file_sir_hash, sir_hash, synthetic_file_sir_id, synthetic_module_sir_id, validate_sir,
};
use aether_store::{
    SirHistoryStore, SirMetaRecord, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord,
    SymbolRelationStore,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    AetherMcpServer, SIR_STATUS_GENERATING, current_unix_timestamp, current_unix_timestamp_millis,
    symbol_leaf_name,
};
use crate::AetherMcpError;

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
    pub structural: Option<SirStructuralView>,
    pub last_error: Option<String>,
    pub last_attempt_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SirStructuralView {
    pub symbol_name: String,
    pub kind: String,
    pub file_path: String,
    pub dependencies: Vec<String>,
    pub callers: Vec<String>,
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
    pub method_dependencies: Option<HashMap<String, Vec<String>>>,
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
            method_dependencies: value.method_dependencies,
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

#[derive(Debug, Clone)]
struct ModuleRollupCoverage {
    module_id: String,
    files_with_sir: u32,
    files_total: u32,
}

impl AetherMcpServer {
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

        let store = self.state.store.as_ref();
        store.increment_symbol_access(&[symbol_id.to_owned()], current_unix_timestamp_millis())?;
        let meta = store.get_sir_meta(symbol_id)?;
        let (sir_status, last_error, last_attempt_at) = meta_status_fields(meta.as_ref());
        let sir_blob = store.read_sir_blob(symbol_id)?;

        let Some(sir_blob) = sir_blob else {
            let structural = build_structural_view(store, symbol_id)?;
            if structural.is_some() {
                let _ = store.enqueue_sir_request(symbol_id);
            }
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
                sir_status: structural
                    .as_ref()
                    .map(|_| SIR_STATUS_GENERATING.to_owned())
                    .or(sir_status),
                structural,
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
            structural: None,
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

        let store = self.state.store.as_ref();
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
                structural: None,
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
            structural: None,
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
                structural: None,
                last_error: None,
                last_attempt_at: None,
            });
        }

        let store = self.state.store.as_ref();
        let coverage = self.generate_module_rollup_on_demand(store, &module_path, language)?;
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
                structural: None,
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
            structural: None,
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
            if !path.starts_with(self.workspace()) {
                return Err(AetherMcpError::Message(format!(
                    "{field_name} must be under workspace {}",
                    self.workspace().display()
                )));
            }

            let relative = path.strip_prefix(self.workspace()).map_err(|_| {
                AetherMcpError::Message(format!(
                    "{field_name} must be under workspace {}",
                    self.workspace().display()
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
            generation_pass: "single".to_owned(),
            prompt_hash: None,
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
        if self.sqlite_path().exists() {
            let store = self.state.store.as_ref();
            store.increment_symbol_access(
                std::slice::from_ref(&symbol_id),
                current_unix_timestamp_millis(),
            )?;
        }

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
        structural: None,
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

fn build_structural_view(
    store: &SqliteStore,
    symbol_id: &str,
) -> Result<Option<SirStructuralView>, AetherMcpError> {
    let Some(symbol) = store.get_symbol_record(symbol_id)? else {
        return Ok(None);
    };

    let mut dependencies = store
        .get_dependencies(symbol_id)?
        .into_iter()
        .map(|edge| edge.target_qualified_name)
        .collect::<Vec<_>>();
    sort_and_dedup(&mut dependencies);

    let mut callers = Vec::new();
    for edge in store.get_callers(symbol.qualified_name.as_str())? {
        if let Some(caller) = resolve_caller_name(store, edge.source_id.as_str())? {
            callers.push(caller);
        }
    }
    sort_and_dedup(&mut callers);

    Ok(Some(SirStructuralView {
        symbol_name: symbol_leaf_name(symbol.qualified_name.as_str()).to_owned(),
        kind: symbol.kind,
        file_path: symbol.file_path,
        dependencies,
        callers,
    }))
}

fn resolve_caller_name(
    store: &SqliteStore,
    caller_symbol_id: &str,
) -> Result<Option<String>, AetherMcpError> {
    let caller_symbol_id = caller_symbol_id.trim();
    if caller_symbol_id.is_empty() {
        return Ok(None);
    }
    let display = store
        .get_symbol_record(caller_symbol_id)?
        .map(|record: SymbolRecord| record.qualified_name)
        .unwrap_or_else(|| caller_symbol_id.to_owned());
    Ok(Some(display))
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
