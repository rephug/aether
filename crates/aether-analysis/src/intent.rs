use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{IntentConfig, load_workspace_config};
use aether_core::{content_hash, normalize_path};
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_store::{
    IntentSnapshotRecord, SqliteStore, Store, SymbolRecord, VectorStore, open_vector_store,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::coupling::AnalysisError;
use crate::drift::{build_structured_sir_diff, cosine_similarity};

const INTENT_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentScope {
    Symbol,
    File,
    Directory,
}

impl IntentScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::File => "file",
            Self::Directory => "directory",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "symbol" => Some(Self::Symbol),
            "file" => Some(Self::File),
            "directory" => Some(Self::Directory),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSnapshotRequest {
    pub scope: IntentScope,
    pub target: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSnapshotSkippedSymbol {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSnapshotResult {
    pub schema_version: String,
    pub snapshot_id: String,
    pub label: String,
    pub scope: IntentScope,
    pub target: String,
    pub symbols_captured: u32,
    pub created_at: i64,
    pub commit_hash: Option<String>,
    pub skipped_symbols: Vec<IntentSnapshotSkippedSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyIntentRequest {
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentStatus {
    Preserved,
    ShiftedMinor,
    ShiftedMajor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyIntentSummary {
    pub symbols_checked: u32,
    pub intent_preserved: u32,
    pub intent_shifted: u32,
    pub symbols_removed: u32,
    pub symbols_added: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentSymbolPreservedEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub similarity: f32,
    pub status: IntentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentTestCoverageGap {
    pub existing_tests: Vec<String>,
    pub untested_new_intents: Vec<String>,
    pub recommendation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentSymbolShiftedEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub similarity: f32,
    pub status: IntentStatus,
    pub before_purpose: String,
    pub after_purpose: String,
    pub before_edge_cases: Vec<String>,
    pub after_edge_cases: Vec<String>,
    pub test_coverage_gap: IntentTestCoverageGap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSymbolAddedEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSymbolRemovedEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyIntentResult {
    pub schema_version: String,
    pub snapshot_id: String,
    pub label: String,
    pub verification: VerifyIntentSummary,
    pub preserved: Vec<IntentSymbolPreservedEntry>,
    pub shifted: Vec<IntentSymbolShiftedEntry>,
    pub added: Vec<IntentSymbolAddedEntry>,
    pub removed: Vec<IntentSymbolRemovedEntry>,
    pub embedding_fallback_count: u32,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct IntentAnalyzer {
    workspace: PathBuf,
    config: IntentConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SnapshotSymbol {
    symbol_id: String,
    symbol_name: String,
    file: String,
    sir_hash: String,
    sir_text: String,
    embedding: Option<Vec<f32>>,
    embedding_provider: Option<String>,
    embedding_model: Option<String>,
}

struct SimilarityInputs<'a> {
    runtime: &'a tokio::runtime::Runtime,
    vector_store: Option<&'a std::sync::Arc<dyn VectorStore>>,
    embedding_provider: Option<&'a aether_infer::LoadedEmbeddingProvider>,
    snapshot_embedding: Option<&'a Vec<f32>>,
    snapshot_provider: Option<&'a str>,
    snapshot_model: Option<&'a str>,
    symbol_id: &'a str,
    current_sir: &'a str,
}

impl IntentAnalyzer {
    pub fn new(workspace: impl AsRef<Path>) -> Result<Self, AnalysisError> {
        let workspace = workspace.as_ref().to_path_buf();
        let config = load_workspace_config(&workspace)?;
        Ok(Self {
            workspace,
            config: config.intent,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn config(&self) -> &IntentConfig {
        &self.config
    }

    pub fn snapshot_intent(
        &self,
        request: IntentSnapshotRequest,
    ) -> Result<IntentSnapshotResult, AnalysisError> {
        let target = normalize_path(request.target.trim());
        let label = request.label.trim();
        if target.is_empty() || label.is_empty() {
            return Err(AnalysisError::Message(
                "scope, target, and label are required for snapshot-intent".to_owned(),
            ));
        }

        let store = SqliteStore::open(&self.workspace)?;
        let symbols = resolve_scope_symbols(&store, request.scope, target.as_str())?;
        let created_at = now_millis();
        let commit_hash = resolve_head_commit_hash(&self.workspace);

        let embedding_provider = load_embedding_provider_from_config(
            &self.workspace,
            EmbeddingProviderOverrides::default(),
        )?;
        let runtime = if embedding_provider.is_some() {
            Some(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|err| {
                        AnalysisError::Message(format!(
                            "failed to build async runtime for intent snapshot embeddings: {err}"
                        ))
                    })?,
            )
        } else {
            None
        };

        let mut captured = Vec::new();
        let mut skipped_symbols = Vec::new();

        for symbol in symbols {
            let Some(sir_text) = store.read_sir_blob(symbol.id.as_str())? else {
                skipped_symbols.push(IntentSnapshotSkippedSymbol {
                    symbol_id: symbol.id,
                    symbol_name: symbol_leaf_name(symbol.qualified_name.as_str()),
                    file: symbol.file_path,
                    note: "no SIR at snapshot time".to_owned(),
                });
                continue;
            };

            let sir_hash = store
                .get_sir_meta(symbol.id.as_str())?
                .map(|meta| meta.sir_hash)
                .filter(|hash| !hash.trim().is_empty())
                .unwrap_or_else(|| content_hash(sir_text.as_str()));

            let (embedding, embedding_provider_name, embedding_model_name) =
                if let (Some(runtime), Some(embedding_provider)) =
                    (runtime.as_ref(), embedding_provider.as_ref())
                {
                    match runtime
                        .block_on(embedding_provider.provider.embed_text(sir_text.as_str()))
                    {
                        Ok(embedding) if !embedding.is_empty() => (
                            Some(embedding),
                            Some(embedding_provider.provider_name.clone()),
                            Some(embedding_provider.model_name.clone()),
                        ),
                        _ => (None, None, None),
                    }
                } else {
                    (None, None, None)
                };

            captured.push(SnapshotSymbol {
                symbol_id: symbol.id,
                symbol_name: symbol_leaf_name(symbol.qualified_name.as_str()),
                file: symbol.file_path,
                sir_hash,
                sir_text,
                embedding,
                embedding_provider: embedding_provider_name,
                embedding_model: embedding_model_name,
            });
        }

        let payload = serde_json::to_string(&captured)?;
        let snapshot_id = format!(
            "snap_{}",
            &content_hash(
                format!(
                    "{}\n{}\n{}\n{}\n{}",
                    label,
                    request.scope.as_str(),
                    target,
                    created_at,
                    commit_hash.clone().unwrap_or_default()
                )
                .as_str(),
            )[..12]
        );

        store.insert_intent_snapshot(IntentSnapshotRecord {
            snapshot_id: snapshot_id.clone(),
            label: label.to_owned(),
            scope: request.scope.as_str().to_owned(),
            target: target.clone(),
            symbols_json: payload,
            commit_hash: commit_hash.clone(),
            created_at,
        })?;

        Ok(IntentSnapshotResult {
            schema_version: INTENT_SCHEMA_VERSION.to_owned(),
            snapshot_id,
            label: label.to_owned(),
            scope: request.scope,
            target,
            symbols_captured: captured.len() as u32,
            created_at,
            commit_hash,
            skipped_symbols,
        })
    }

    pub fn verify_intent(
        &self,
        request: VerifyIntentRequest,
    ) -> Result<VerifyIntentResult, AnalysisError> {
        let snapshot_id = request.snapshot_id.trim().to_ascii_lowercase();
        if snapshot_id.is_empty() {
            return Err(AnalysisError::Message("snapshot_id is required".to_owned()));
        }

        let store = SqliteStore::open(&self.workspace)?;
        let Some(snapshot) = store.get_intent_snapshot(snapshot_id.as_str())? else {
            return Err(AnalysisError::Message(
                "no snapshot found, use aether_snapshot_intent first".to_owned(),
            ));
        };

        let scope = IntentScope::parse(snapshot.scope.as_str()).ok_or_else(|| {
            AnalysisError::Message(format!(
                "invalid snapshot scope '{}': expected file/symbol/directory",
                snapshot.scope
            ))
        })?;
        let snapshot_symbols =
            serde_json::from_str::<Vec<SnapshotSymbol>>(snapshot.symbols_json.as_str())?;
        let current_symbols = resolve_scope_symbols(&store, scope, snapshot.target.as_str())?;
        let current_symbols_by_id = current_symbols
            .iter()
            .cloned()
            .map(|symbol| (symbol.id.clone(), symbol))
            .collect::<HashMap<_, _>>();

        let mut notes = Vec::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| {
                AnalysisError::Message(format!(
                    "failed to build async runtime for intent verification: {err}"
                ))
            })?;

        let vector_store = runtime.block_on(open_vector_store(&self.workspace)).ok();
        let embedding_provider = load_embedding_provider_from_config(
            &self.workspace,
            EmbeddingProviderOverrides::default(),
        )?;

        let snapshot_symbol_ids = snapshot_symbols
            .iter()
            .map(|symbol| symbol.symbol_id.clone())
            .collect::<HashSet<_>>();

        let mut added = current_symbols
            .iter()
            .filter(|symbol| !snapshot_symbol_ids.contains(symbol.id.as_str()))
            .map(|symbol| IntentSymbolAddedEntry {
                symbol_id: symbol.id.clone(),
                symbol_name: symbol_leaf_name(symbol.qualified_name.as_str()),
                file: symbol.file_path.clone(),
                note: "New symbol not in original snapshot — verify test coverage".to_owned(),
            })
            .collect::<Vec<_>>();
        added.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));

        let mut removed = Vec::new();
        let mut preserved = Vec::new();
        let mut shifted = Vec::new();
        let mut embedding_fallback_count = 0u32;

        for snapshot_symbol in snapshot_symbols {
            let Some(current_symbol) =
                current_symbols_by_id.get(snapshot_symbol.symbol_id.as_str())
            else {
                removed.push(IntentSymbolRemovedEntry {
                    symbol_id: snapshot_symbol.symbol_id,
                    symbol_name: snapshot_symbol.symbol_name,
                    file: snapshot_symbol.file,
                    note: "Symbol from snapshot is no longer present".to_owned(),
                });
                continue;
            };

            let Some(current_sir_text) = store.read_sir_blob(current_symbol.id.as_str())? else {
                let status = IntentStatus::ShiftedMajor;
                let snapshot_value =
                    serde_json::from_str::<Value>(snapshot_symbol.sir_text.as_str())
                        .unwrap_or(Value::Null);
                shifted.push(IntentSymbolShiftedEntry {
                    symbol_id: current_symbol.id.clone(),
                    symbol_name: symbol_leaf_name(current_symbol.qualified_name.as_str()),
                    similarity: 0.0,
                    status,
                    before_purpose: extract_sir_field(&snapshot_value, &["purpose", "intent"]),
                    after_purpose: String::new(),
                    before_edge_cases: extract_sir_field_list(
                        &snapshot_value,
                        &["edge_cases", "error_modes"],
                    ),
                    after_edge_cases: Vec::new(),
                    test_coverage_gap: IntentTestCoverageGap {
                        existing_tests: Vec::new(),
                        untested_new_intents: Vec::new(),
                        recommendation:
                            "Current symbol has no SIR; regenerate SIR before verifying intent"
                                .to_owned(),
                    },
                });
                notes.push(format!(
                    "symbol {} missing current SIR; classified as shifted_major",
                    current_symbol.id
                ));
                continue;
            };

            let structured_diff = build_structured_sir_diff(
                snapshot_symbol.sir_text.as_str(),
                current_sir_text.as_str(),
            )?;

            let (similarity, used_fallback) = compute_similarity(
                SimilarityInputs {
                    runtime: &runtime,
                    vector_store: vector_store.as_ref(),
                    embedding_provider: embedding_provider.as_ref(),
                    snapshot_embedding: snapshot_symbol.embedding.as_ref(),
                    snapshot_provider: snapshot_symbol.embedding_provider.as_deref(),
                    snapshot_model: snapshot_symbol.embedding_model.as_deref(),
                    symbol_id: current_symbol.id.as_str(),
                    current_sir: current_sir_text.as_str(),
                },
                &structured_diff,
            );
            if used_fallback {
                embedding_fallback_count = embedding_fallback_count.saturating_add(1);
            }

            let status = classify_status(
                similarity,
                self.config.similarity_preserved_threshold,
                self.config.similarity_shifted_threshold,
            );

            match status {
                IntentStatus::Preserved => {
                    preserved.push(IntentSymbolPreservedEntry {
                        symbol_id: current_symbol.id.clone(),
                        symbol_name: symbol_leaf_name(current_symbol.qualified_name.as_str()),
                        similarity,
                        status,
                    });
                }
                IntentStatus::ShiftedMinor | IntentStatus::ShiftedMajor => {
                    let before_purpose = extract_string_field(&structured_diff, "/purpose/before");
                    let after_purpose = extract_string_field(&structured_diff, "/purpose/after");
                    let before_edge_cases =
                        extract_string_list(&structured_diff, "/edge_cases/before");
                    let after_edge_cases =
                        extract_string_list(&structured_diff, "/edge_cases/after");

                    let test_coverage_gap = compute_test_coverage_gap(
                        &store,
                        current_symbol.id.as_str(),
                        current_symbol.file_path.as_str(),
                        before_edge_cases.as_slice(),
                        after_edge_cases.as_slice(),
                    )?;

                    shifted.push(IntentSymbolShiftedEntry {
                        symbol_id: current_symbol.id.clone(),
                        symbol_name: symbol_leaf_name(current_symbol.qualified_name.as_str()),
                        similarity,
                        status,
                        before_purpose,
                        after_purpose,
                        before_edge_cases,
                        after_edge_cases,
                        test_coverage_gap,
                    });
                }
            }
        }

        preserved.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
        shifted.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
        removed.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));

        let verification = VerifyIntentSummary {
            symbols_checked: (preserved.len() + shifted.len()) as u32,
            intent_preserved: preserved.len() as u32,
            intent_shifted: shifted.len() as u32,
            symbols_removed: removed.len() as u32,
            symbols_added: added.len() as u32,
        };

        Ok(VerifyIntentResult {
            schema_version: INTENT_SCHEMA_VERSION.to_owned(),
            snapshot_id: snapshot.snapshot_id,
            label: snapshot.label,
            verification,
            preserved,
            shifted,
            added,
            removed,
            embedding_fallback_count,
            notes,
        })
    }
}

fn compute_similarity(inputs: SimilarityInputs<'_>, structured_diff: &Value) -> (f32, bool) {
    let Some(snapshot_embedding) = inputs.snapshot_embedding else {
        return (structural_similarity_from_diff(structured_diff), true);
    };

    if let (Some(vector_store), Some(provider), Some(model)) = (
        inputs.vector_store,
        inputs.snapshot_provider,
        inputs.snapshot_model,
    ) && let Ok(current) = inputs
        .runtime
        .block_on(vector_store.list_embeddings_for_symbols(
            provider,
            model,
            &[inputs.symbol_id.to_owned()],
        ))
        && let Some(first) = current.first()
        && !first.embedding.is_empty()
        && first.embedding.len() == snapshot_embedding.len()
    {
        return (
            cosine_similarity(snapshot_embedding.as_slice(), first.embedding.as_slice()),
            false,
        );
    }

    if let Some(embedding_provider) = inputs.embedding_provider
        && let Ok(current_embedding) = inputs
            .runtime
            .block_on(embedding_provider.provider.embed_text(inputs.current_sir))
        && !current_embedding.is_empty()
        && current_embedding.len() == snapshot_embedding.len()
    {
        return (
            cosine_similarity(snapshot_embedding.as_slice(), current_embedding.as_slice()),
            false,
        );
    }

    (structural_similarity_from_diff(structured_diff), true)
}

fn structural_similarity_from_diff(structured_diff: &Value) -> f32 {
    let purpose_changed = structured_diff
        .pointer("/purpose/changed")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let edge_delta = extract_string_list(structured_diff, "/edge_cases/added").len()
        + extract_string_list(structured_diff, "/edge_cases/removed").len();
    let constraints_delta = extract_string_list(structured_diff, "/constraints/added").len()
        + extract_string_list(structured_diff, "/constraints/removed").len();

    let purpose_component = if purpose_changed { 1.0 } else { 0.0 };
    let edge_component = (edge_delta as f32).min(4.0) / 4.0;
    let constraints_component = (constraints_delta as f32).min(4.0) / 4.0;

    let magnitude = (0.5 * purpose_component + 0.3 * edge_component + 0.2 * constraints_component)
        .clamp(0.0, 1.0);
    (1.0 - magnitude).clamp(0.0, 1.0)
}

fn classify_status(
    similarity: f32,
    preserved_threshold: f32,
    shifted_threshold: f32,
) -> IntentStatus {
    if similarity >= preserved_threshold {
        IntentStatus::Preserved
    } else if similarity >= shifted_threshold {
        IntentStatus::ShiftedMinor
    } else {
        IntentStatus::ShiftedMajor
    }
}

fn compute_test_coverage_gap(
    store: &SqliteStore,
    symbol_id: &str,
    file_path: &str,
    before_edge_cases: &[String],
    after_edge_cases: &[String],
) -> Result<IntentTestCoverageGap, AnalysisError> {
    let mut intents = store.list_test_intents_for_symbol(symbol_id)?;
    if intents.is_empty() {
        intents = store.list_test_intents_for_file(file_path)?;
    }

    let existing_tests = intents
        .iter()
        .map(|intent| intent.test_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let before_set = before_edge_cases
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut untested_new_intents = Vec::new();

    for edge_case in after_edge_cases {
        let normalized = edge_case.trim().to_ascii_lowercase();
        if normalized.is_empty() || before_set.contains(normalized.as_str()) {
            continue;
        }
        let covered = intents.iter().any(|intent| {
            let text = intent.intent_text.to_ascii_lowercase();
            text.contains(normalized.as_str()) || normalized.contains(text.as_str())
        });
        if !covered {
            untested_new_intents.push(edge_case.clone());
        }
    }

    let recommendation = if untested_new_intents.is_empty() {
        "No additional tests required for newly introduced intents".to_owned()
    } else {
        format!(
            "Add tests for {}",
            untested_new_intents
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    Ok(IntentTestCoverageGap {
        existing_tests,
        untested_new_intents,
        recommendation,
    })
}

fn resolve_scope_symbols(
    store: &SqliteStore,
    scope: IntentScope,
    target: &str,
) -> Result<Vec<SymbolRecord>, AnalysisError> {
    let normalized_target = normalize_path(target.trim());
    if normalized_target.is_empty() {
        return Ok(Vec::new());
    }

    let mut symbols = match scope {
        IntentScope::Symbol => {
            let symbol = store.get_symbol_record(normalized_target.as_str())?;
            let Some(symbol) = symbol else {
                return Err(AnalysisError::Message(format!(
                    "symbol '{}' not found",
                    normalized_target
                )));
            };
            vec![symbol]
        }
        IntentScope::File => store.list_symbols_for_file(normalized_target.as_str())?,
        IntentScope::Directory => {
            let files = store.list_symbol_files_by_directory_prefix(normalized_target.as_str())?;
            let mut symbols = Vec::new();
            for file in files {
                symbols.extend(store.list_symbols_for_file(file.as_str())?);
            }
            symbols
        }
    };

    symbols.sort_by(|left, right| left.id.cmp(&right.id));
    symbols.dedup_by(|left, right| left.id == right.id);
    Ok(symbols)
}

fn extract_string_field(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn extract_string_list(value: &Value, pointer: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn extract_sir_field(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
    }
    String::new()
}

fn extract_sir_field_list(value: &Value, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let Some(items) = value.get(*key).and_then(Value::as_array) else {
            continue;
        };
        let values = items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if !values.is_empty() {
            return values;
        }
    }
    Vec::new()
}

fn resolve_head_commit_hash(workspace: &Path) -> Option<String> {
    let repo = gix::discover(workspace).ok()?;
    let head = repo.head_id().ok()?.detach();
    Some(head.to_string().to_ascii_lowercase())
}

fn symbol_leaf_name(qualified_name: &str) -> String {
    qualified_name
        .rsplit("::")
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(qualified_name)
        .to_owned()
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use aether_store::{SqliteStore, Store, SymbolRecord};
    use tempfile::tempdir;

    use super::{
        IntentAnalyzer, IntentScope, IntentSnapshotRequest, IntentStatus, VerifyIntentRequest,
        classify_status,
    };

    fn symbol(id: &str, file_path: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    #[test]
    fn snapshot_and_verify_detect_shifted_intent() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        let payment = symbol(
            "sym-payment",
            "src/payments/processor.rs",
            "payments::process",
        );
        store.upsert_symbol(payment.clone()).expect("upsert symbol");
        store
            .write_sir_blob(
                payment.id.as_str(),
                r#"{"purpose":"process payment","edge_cases":["timeout"],"constraints":[]}"#,
            )
            .expect("write initial sir");

        let analyzer = IntentAnalyzer::new(workspace).expect("new analyzer");
        let snapshot = analyzer
            .snapshot_intent(IntentSnapshotRequest {
                scope: IntentScope::File,
                target: payment.file_path.clone(),
                label: "pre-refactor".to_owned(),
            })
            .expect("snapshot intent");
        assert_eq!(snapshot.symbols_captured, 1);

        store
            .write_sir_blob(
                payment.id.as_str(),
                r#"{"purpose":"process batch payment","edge_cases":["timeout","partial failure"],"constraints":["idempotent"]}"#,
            )
            .expect("write updated sir");

        let verified = analyzer
            .verify_intent(VerifyIntentRequest {
                snapshot_id: snapshot.snapshot_id,
            })
            .expect("verify intent");

        assert_eq!(verified.verification.symbols_checked, 1);
        assert_eq!(verified.verification.intent_shifted, 1);
        assert!(verified.shifted[0].similarity < 0.90);
    }

    #[test]
    fn verify_reports_removed_and_added_symbols() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        let old_symbol = symbol("sym-old", "src/payments/processor.rs", "payments::old");
        store
            .upsert_symbol(old_symbol.clone())
            .expect("upsert old symbol");
        store
            .write_sir_blob(
                old_symbol.id.as_str(),
                r#"{"purpose":"old","edge_cases":[]}"#,
            )
            .expect("write old sir");

        let analyzer = IntentAnalyzer::new(workspace).expect("new analyzer");
        let snapshot = analyzer
            .snapshot_intent(IntentSnapshotRequest {
                scope: IntentScope::File,
                target: old_symbol.file_path.clone(),
                label: "before".to_owned(),
            })
            .expect("snapshot");

        store
            .mark_removed(old_symbol.id.as_str())
            .expect("mark removed");
        let new_symbol = symbol("sym-new", "src/payments/processor.rs", "payments::new");
        store
            .upsert_symbol(new_symbol.clone())
            .expect("upsert new symbol");
        store
            .write_sir_blob(
                new_symbol.id.as_str(),
                r#"{"purpose":"new","edge_cases":[]}"#,
            )
            .expect("write new sir");

        let verified = analyzer
            .verify_intent(VerifyIntentRequest {
                snapshot_id: snapshot.snapshot_id,
            })
            .expect("verify");

        assert_eq!(verified.verification.symbols_removed, 1);
        assert_eq!(verified.verification.symbols_added, 1);
    }

    #[test]
    fn snapshot_skips_symbols_without_sir() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        let with_sir = symbol("sym-with", "src/lib.rs", "demo::with_sir");
        let without_sir = symbol("sym-without", "src/lib.rs", "demo::without_sir");
        store
            .upsert_symbol(with_sir.clone())
            .expect("upsert with sir");
        store
            .write_sir_blob(
                with_sir.id.as_str(),
                r#"{"purpose":"with","edge_cases":[]}"#,
            )
            .expect("write with sir");
        store
            .upsert_symbol(without_sir)
            .expect("upsert without sir");

        let analyzer = IntentAnalyzer::new(workspace).expect("new analyzer");
        let snapshot = analyzer
            .snapshot_intent(IntentSnapshotRequest {
                scope: IntentScope::File,
                target: "src/lib.rs".to_owned(),
                label: "snapshot".to_owned(),
            })
            .expect("snapshot");

        assert_eq!(snapshot.symbols_captured, 1);
        assert_eq!(snapshot.skipped_symbols.len(), 1);
        assert!(snapshot.skipped_symbols[0].note.contains("no SIR"));
    }

    #[test]
    fn classification_thresholds_match_stage_contract() {
        assert_eq!(classify_status(0.95, 0.90, 0.70), IntentStatus::Preserved);
        assert_eq!(
            classify_status(0.70, 0.90, 0.70),
            IntentStatus::ShiftedMinor
        );
        assert_eq!(
            classify_status(0.69, 0.90, 0.70),
            IntentStatus::ShiftedMajor
        );
    }

    #[test]
    fn verify_uses_structural_fallback_when_embeddings_unavailable() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        let symbol = symbol("sym-fallback", "src/fallback.rs", "demo::fallback");
        store.upsert_symbol(symbol.clone()).expect("upsert symbol");
        store
            .write_sir_blob(
                symbol.id.as_str(),
                r#"{"purpose":"baseline","edge_cases":["timeout"],"constraints":[]}"#,
            )
            .expect("write sir baseline");

        let analyzer = IntentAnalyzer::new(workspace).expect("new analyzer");
        let snapshot = analyzer
            .snapshot_intent(IntentSnapshotRequest {
                scope: IntentScope::File,
                target: symbol.file_path.clone(),
                label: "baseline".to_owned(),
            })
            .expect("snapshot");

        store
            .write_sir_blob(
                symbol.id.as_str(),
                r#"{"purpose":"updated","edge_cases":["timeout","partial"],"constraints":["guarded"]}"#,
            )
            .expect("write sir updated");

        let verified = analyzer
            .verify_intent(VerifyIntentRequest {
                snapshot_id: snapshot.snapshot_id,
            })
            .expect("verify");

        assert!(verified.embedding_fallback_count >= 1);
        assert_eq!(verified.verification.symbols_checked, 1);
    }
}
