use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use aether_analysis::{
    PreparedRefactorCandidate, RefactorPreparationRequest, RefactorScope, collect_intent_snapshot,
    prepare_refactor_prep, verify_intent_snapshot,
};
use aether_config::InferenceProviderKind;
use aether_core::{Position, SourceRange, normalize_path};
use aether_infer::{
    EmbeddingProvider, EmbeddingProviderOverrides, EmbeddingPurpose, InferenceProvider,
    ProviderOverrides, SirContext, load_embedding_provider_from_config,
    load_provider_from_env_or_mock, sir_prompt,
};
use aether_sir::{canonicalize_sir_json, sir_hash, validate_sir};
use aether_store::{
    SirHistoryStore, SirMetaRecord, SirStateStore, SnapshotStore, SymbolEmbeddingRecord,
};
use anyhow::{Result as AnyResult, anyhow};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tokio::time::{sleep, timeout};

use super::{AetherMcpServer, MCP_SCHEMA_VERSION, current_unix_timestamp};
use crate::AetherMcpError;

const INFERENCE_MAX_RETRIES: usize = 2;
const INFERENCE_BACKOFF_BASE_MS: u64 = 200;
const INFERENCE_BACKOFF_MAX_MS: u64 = 2_000;
const DEFAULT_TIMEOUT_SECS: u64 = 90;
const MAX_SYMBOL_TEXT_CHARS: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRefactorPrepRequest {
    pub file: Option<String>,
    pub crate_name: Option<String>,
    pub top_n: Option<usize>,
    pub local: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRefactorCandidate {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub refactor_risk: f64,
    pub risk_factors: Vec<String>,
    pub needs_deep_scan: bool,
    pub deep_scan_completed: bool,
    pub in_cycle: bool,
    pub generation_pass: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRefactorPrepResponse {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub scope: String,
    pub total_in_scope_symbols: u32,
    pub selected_count: u32,
    pub deep_requested: u32,
    pub deep_completed: u32,
    pub deep_failed: u32,
    pub deep_failed_symbol_ids: Vec<String>,
    pub forced_cycle_members: u32,
    pub skipped_fresh: u32,
    pub notes: Vec<String>,
    pub candidates: Vec<AetherRefactorCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherVerifyIntentRequest {
    pub snapshot: String,
    pub threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherIntentSymbolSummary {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherIntentVerificationEntry {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub similarity: f64,
    pub threshold: f64,
    pub passed: bool,
    pub method: String,
    pub issue: Option<String>,
    pub generation_pass: Option<String>,
    pub was_deep_scanned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherVerifyIntentResponse {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub scope: String,
    pub threshold: f64,
    pub passed: bool,
    pub compared_entries: u32,
    pub failed_entries: u32,
    pub disappeared_symbols: Vec<AetherIntentSymbolSummary>,
    pub new_symbols: Vec<AetherIntentSymbolSummary>,
    pub entries: Vec<AetherIntentVerificationEntry>,
    pub used_embeddings: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Default)]
struct McpDeepScanOutcome {
    requested: usize,
    succeeded_ids: HashSet<String>,
    failed_symbol_ids: Vec<String>,
}

impl AetherMcpServer {
    pub fn aether_refactor_prep_logic(
        &self,
        request: AetherRefactorPrepRequest,
    ) -> Result<AetherRefactorPrepResponse, AetherMcpError> {
        self.state.require_writable()?;
        let scope = refactor_scope_from_request(&request)?;
        let top_n = request.top_n.unwrap_or(20).max(1);
        let prep = prepare_refactor_prep(
            self.workspace(),
            self.state.store.as_ref(),
            RefactorPreparationRequest {
                scope: scope.clone(),
                top_n,
            },
        )?;
        let deep_candidates = prep
            .candidates
            .iter()
            .filter(|candidate| candidate.needs_deep_scan)
            .cloned()
            .collect::<Vec<_>>();
        let deep_outcome =
            self.run_local_deep_scan(deep_candidates.as_slice(), request.local.unwrap_or(false))?;
        let snapshot = collect_intent_snapshot(
            self.workspace(),
            self.state.store.as_ref(),
            &prep.scope,
            prep.scope_symbols.as_slice(),
            &deep_outcome.succeeded_ids,
        )?;
        self.state.store.create_snapshot(&snapshot)?;

        let mut notes = prep.notes;
        if !deep_outcome.failed_symbol_ids.is_empty() {
            notes.push(format!(
                "{} deep scans did not complete successfully.",
                deep_outcome.failed_symbol_ids.len()
            ));
        }
        notes.push(
            "Inference cost tracking is unavailable in the current provider abstraction; counts are reported instead.".to_owned(),
        );

        let candidates = prep
            .candidates
            .iter()
            .map(|candidate| AetherRefactorCandidate {
                symbol_id: candidate.symbol.id.clone(),
                qualified_name: candidate.symbol.qualified_name.clone(),
                file_path: normalize_path(candidate.symbol.file_path.as_str()),
                refactor_risk: candidate.refactor_risk,
                risk_factors: candidate.risk_factors.clone(),
                needs_deep_scan: candidate.needs_deep_scan,
                deep_scan_completed: deep_outcome
                    .succeeded_ids
                    .contains(candidate.symbol.id.as_str()),
                in_cycle: candidate.in_cycle,
                generation_pass: if deep_outcome
                    .succeeded_ids
                    .contains(candidate.symbol.id.as_str())
                {
                    Some("deep".to_owned())
                } else {
                    candidate.current_generation_pass.clone()
                },
            })
            .collect::<Vec<_>>();

        Ok(AetherRefactorPrepResponse {
            schema_version: MCP_SCHEMA_VERSION,
            snapshot_id: snapshot.snapshot_id,
            scope: prep.scope.label(),
            total_in_scope_symbols: prep.scope_symbols.len() as u32,
            selected_count: candidates.len() as u32,
            deep_requested: deep_outcome.requested as u32,
            deep_completed: deep_outcome.succeeded_ids.len() as u32,
            deep_failed: deep_outcome.failed_symbol_ids.len() as u32,
            deep_failed_symbol_ids: deep_outcome.failed_symbol_ids,
            forced_cycle_members: prep.forced_cycle_members as u32,
            skipped_fresh: prep.skipped_fresh as u32,
            notes,
            candidates,
        })
    }

    pub fn aether_verify_intent_logic(
        &self,
        request: AetherVerifyIntentRequest,
    ) -> Result<AetherVerifyIntentResponse, AetherMcpError> {
        let report = verify_intent_snapshot(
            self.workspace(),
            self.state.store.as_ref(),
            request.snapshot.as_str(),
            request.threshold.unwrap_or(0.85),
        )?;

        Ok(AetherVerifyIntentResponse {
            schema_version: MCP_SCHEMA_VERSION,
            snapshot_id: report.snapshot_id,
            scope: report.scope,
            threshold: report.threshold,
            passed: report.passed,
            compared_entries: report.compared_entries as u32,
            failed_entries: report.failed_entries as u32,
            disappeared_symbols: report
                .disappeared_symbols
                .into_iter()
                .map(|symbol| AetherIntentSymbolSummary {
                    symbol_id: symbol.symbol_id,
                    qualified_name: symbol.qualified_name,
                    file_path: normalize_path(symbol.file_path.as_str()),
                })
                .collect(),
            new_symbols: report
                .new_symbols
                .into_iter()
                .map(|symbol| AetherIntentSymbolSummary {
                    symbol_id: symbol.symbol_id,
                    qualified_name: symbol.qualified_name,
                    file_path: normalize_path(symbol.file_path.as_str()),
                })
                .collect(),
            entries: report
                .entries
                .into_iter()
                .map(|entry| AetherIntentVerificationEntry {
                    symbol_id: entry.symbol_id,
                    qualified_name: entry.qualified_name,
                    file_path: normalize_path(entry.file_path.as_str()),
                    similarity: entry.similarity,
                    threshold: entry.threshold,
                    passed: entry.passed,
                    method: entry.method,
                    issue: entry.issue,
                    generation_pass: entry.generation_pass,
                    was_deep_scanned: entry.was_deep_scanned,
                })
                .collect(),
            used_embeddings: report.used_embeddings,
            notes: report.notes,
        })
    }

    fn run_local_deep_scan(
        &self,
        candidates: &[PreparedRefactorCandidate],
        local: bool,
    ) -> Result<McpDeepScanOutcome, AetherMcpError> {
        if candidates.is_empty() {
            return Ok(McpDeepScanOutcome::default());
        }

        let provider = load_provider_from_env_or_mock(
            self.workspace(),
            if local {
                ProviderOverrides {
                    provider: Some(InferenceProviderKind::Qwen3Local),
                    ..ProviderOverrides::default()
                }
            } else {
                ProviderOverrides {
                    provider: parse_deep_provider(
                        self.state.config.sir_quality.deep_provider.as_deref(),
                    )?,
                    model: self.state.config.sir_quality.deep_model.clone(),
                    endpoint: self.state.config.sir_quality.deep_endpoint.clone(),
                    api_key_env: self.state.config.sir_quality.deep_api_key_env.clone(),
                }
            },
        )?;
        let provider_name = provider.provider_name.clone();
        let model_name = provider.model_name.clone();
        let provider = Arc::<dyn InferenceProvider>::from(provider.provider);
        let use_cot = provider_name == InferenceProviderKind::Qwen3Local.as_str();
        let timeout_secs = self
            .state
            .config
            .sir_quality
            .deep_timeout_secs
            .max(DEFAULT_TIMEOUT_SECS);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| {
                AetherMcpError::Message(format!("failed to build tokio runtime: {err}"))
            })?;

        let embedding_loaded = if self.state.vector_store.is_some() {
            load_embedding_provider_from_config(
                self.workspace(),
                EmbeddingProviderOverrides::default(),
            )?
        } else {
            None
        };
        let embedding_provider = embedding_loaded.map(|loaded| {
            (
                Arc::<dyn EmbeddingProvider>::from(loaded.provider),
                loaded.provider_name,
                loaded.model_name,
            )
        });

        let commit_hash = aether_core::GitContext::open(self.workspace())
            .and_then(|context| context.head_commit_hash());
        let mut outcome = McpDeepScanOutcome {
            requested: candidates.len(),
            ..McpDeepScanOutcome::default()
        };

        for candidate in candidates {
            match self.generate_and_persist_deep_sir(
                &runtime,
                provider.clone(),
                provider_name.as_str(),
                model_name.as_str(),
                embedding_provider.as_ref(),
                commit_hash.as_deref(),
                candidate,
                use_cot,
                timeout_secs,
            ) {
                Ok(()) => {
                    outcome.succeeded_ids.insert(candidate.symbol.id.clone());
                }
                Err(err) => {
                    outcome.failed_symbol_ids.push(candidate.symbol.id.clone());
                    self.record_failed_deep_attempt(
                        candidate.symbol.id.as_str(),
                        provider_name.as_str(),
                        model_name.as_str(),
                        err.to_string().as_str(),
                    )?;
                }
            }
        }

        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_and_persist_deep_sir(
        &self,
        runtime: &Runtime,
        provider: Arc<dyn InferenceProvider>,
        provider_name: &str,
        model_name: &str,
        embedding_provider: Option<&(Arc<dyn EmbeddingProvider>, String, String)>,
        commit_hash: Option<&str>,
        candidate: &PreparedRefactorCandidate,
        use_cot: bool,
        timeout_secs: u64,
    ) -> Result<(), AetherMcpError> {
        let symbol_text = extract_symbol_text(self.workspace(), &candidate.symbol)?;
        let context = build_sir_context(
            &candidate.symbol,
            candidate.refactor_risk,
            symbol_text.as_str(),
        );
        let prompt = if use_cot {
            sir_prompt::build_enriched_sir_prompt_with_cot(
                symbol_text.as_str(),
                &context,
                &candidate.enrichment,
            )
        } else {
            sir_prompt::build_enriched_sir_prompt(
                symbol_text.as_str(),
                &context,
                &candidate.enrichment,
            )
        };
        let generated = runtime
            .block_on(generate_sir_from_prompt_with_retries(
                provider,
                prompt,
                context,
                use_cot,
                timeout_secs,
            ))
            .map_err(|err| AetherMcpError::Message(err.to_string()))?;
        validate_sir(&generated.sir)?;

        let canonical_json = canonicalize_sir_json(&generated.sir);
        let sir_hash_value = sir_hash(&generated.sir);
        let attempted_at = current_unix_timestamp();
        let version = self.state.store.record_sir_version_if_changed(
            candidate.symbol.id.as_str(),
            sir_hash_value.as_str(),
            provider_name,
            model_name,
            canonical_json.as_str(),
            attempted_at,
            commit_hash,
        )?;

        if version.changed {
            self.state
                .store
                .write_sir_blob(candidate.symbol.id.as_str(), canonical_json.as_str())?;
        }
        self.state.store.upsert_sir_meta(SirMetaRecord {
            id: candidate.symbol.id.clone(),
            sir_hash: sir_hash_value.clone(),
            sir_version: version.version,
            provider: provider_name.to_owned(),
            model: model_name.to_owned(),
            generation_pass: "deep".to_owned(),
            updated_at: version.updated_at,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: attempted_at,
        })?;
        self.refresh_embedding_if_needed(
            runtime,
            embedding_provider,
            candidate.symbol.id.as_str(),
            sir_hash_value.as_str(),
            canonical_json.as_str(),
        )?;

        Ok(())
    }

    fn record_failed_deep_attempt(
        &self,
        symbol_id: &str,
        provider_name: &str,
        model_name: &str,
        error_message: &str,
    ) -> Result<(), AetherMcpError> {
        let current = self.state.store.get_sir_meta(symbol_id)?;
        let updated_at = current_unix_timestamp();
        self.state.store.upsert_sir_meta(SirMetaRecord {
            id: symbol_id.to_owned(),
            sir_hash: current
                .as_ref()
                .map(|meta| meta.sir_hash.clone())
                .unwrap_or_default(),
            sir_version: current.as_ref().map(|meta| meta.sir_version).unwrap_or(1),
            provider: provider_name.to_owned(),
            model: model_name.to_owned(),
            generation_pass: current
                .as_ref()
                .map(|meta| meta.generation_pass.clone())
                .unwrap_or_else(|| "scan".to_owned()),
            updated_at,
            sir_status: "stale".to_owned(),
            last_error: Some(error_message.to_owned()),
            last_attempt_at: updated_at,
        })?;
        Ok(())
    }

    fn refresh_embedding_if_needed(
        &self,
        runtime: &Runtime,
        embedding_provider: Option<&(Arc<dyn EmbeddingProvider>, String, String)>,
        symbol_id: &str,
        sir_hash_value: &str,
        canonical_json: &str,
    ) -> Result<(), AetherMcpError> {
        let Some(vector_store) = self.state.vector_store.as_ref() else {
            return Ok(());
        };
        let Some((provider, provider_name, model_name)) = embedding_provider else {
            return Ok(());
        };
        let existing = runtime.block_on(vector_store.get_embedding_meta(symbol_id))?;
        if existing.as_ref().is_some_and(|meta| {
            meta.sir_hash == sir_hash_value
                && meta.provider == *provider_name
                && meta.model == *model_name
        }) {
            return Ok(());
        }

        let embedding = runtime.block_on(async {
            provider
                .embed_text_with_purpose(canonical_json, EmbeddingPurpose::Document)
                .await
        })?;
        if embedding.is_empty() {
            return Ok(());
        }

        runtime.block_on(vector_store.upsert_embedding(SymbolEmbeddingRecord {
            symbol_id: symbol_id.to_owned(),
            sir_hash: sir_hash_value.to_owned(),
            provider: provider_name.clone(),
            model: model_name.clone(),
            embedding,
            updated_at: current_unix_timestamp(),
        }))?;
        Ok(())
    }
}

#[derive(Debug)]
struct GeneratedSirWithMeta {
    sir: aether_sir::SirAnnotation,
}

fn refactor_scope_from_request(
    request: &AetherRefactorPrepRequest,
) -> Result<RefactorScope, AetherMcpError> {
    if let Some(file) = request
        .file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(RefactorScope::File {
            path: normalize_path(file),
        });
    }
    if let Some(crate_name) = request
        .crate_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(RefactorScope::Crate {
            name: crate_name.to_owned(),
        });
    }
    Err(AetherMcpError::Message(
        "either file or crate_name must be provided".to_owned(),
    ))
}

fn parse_deep_provider(raw: Option<&str>) -> Result<Option<InferenceProviderKind>, AetherMcpError> {
    raw.map(|value| {
        value
            .trim()
            .parse::<InferenceProviderKind>()
            .map_err(|err| {
                AetherMcpError::Message(format!(
                    "invalid sir_quality.deep_provider '{}': {}",
                    value.trim(),
                    err
                ))
            })
    })
    .transpose()
}

fn build_sir_context(
    symbol: &aether_core::Symbol,
    priority_score: f64,
    symbol_text: &str,
) -> SirContext {
    SirContext {
        language: symbol.language.as_str().to_owned(),
        file_path: symbol.file_path.clone(),
        qualified_name: symbol.qualified_name.clone(),
        priority_score: Some(priority_score),
        kind: symbol.kind.as_str().to_owned(),
        is_public: infer_symbol_text_is_public(symbol_text),
        line_count: symbol_text.lines().count(),
    }
}

fn infer_symbol_text_is_public(symbol_text: &str) -> bool {
    let trimmed = symbol_text.trim_start();
    trimmed.starts_with("pub ")
        || trimmed.starts_with("pub(")
        || trimmed.starts_with("export ")
        || trimmed.starts_with("export default ")
}

fn extract_symbol_text(
    workspace: &Path,
    symbol: &aether_core::Symbol,
) -> Result<String, AetherMcpError> {
    let full_path = workspace.join(&symbol.file_path);
    let source = fs::read_to_string(&full_path)?;
    let mut symbol_text = extract_symbol_source_text(&source, symbol.range).ok_or_else(|| {
        AetherMcpError::Message(format!(
            "failed to extract symbol source for {} ({})",
            symbol.qualified_name, symbol.file_path
        ))
    })?;
    if symbol_text.len() > MAX_SYMBOL_TEXT_CHARS {
        let truncated = symbol_text
            .char_indices()
            .take_while(|(index, _)| *index < MAX_SYMBOL_TEXT_CHARS)
            .last()
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(0);
        symbol_text.truncate(truncated);
    }
    Ok(symbol_text)
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

async fn generate_sir_from_prompt_with_retries(
    provider: Arc<dyn InferenceProvider>,
    prompt: String,
    context: SirContext,
    deep_mode: bool,
    timeout_secs: u64,
) -> AnyResult<GeneratedSirWithMeta> {
    let total_attempts = INFERENCE_MAX_RETRIES + 1;
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..total_attempts {
        let timeout_result = timeout(
            Duration::from_secs(timeout_secs.max(1)),
            provider.generate_sir_from_prompt_with_meta(prompt.as_str(), &context, deep_mode),
        )
        .await;

        match timeout_result {
            Ok(Ok(result)) => {
                return Ok(GeneratedSirWithMeta { sir: result.sir });
            }
            Ok(Err(err)) => {
                last_error = Some(anyhow::Error::new(err).context(format!(
                    "attempt {}/{} failed",
                    attempt + 1,
                    total_attempts
                )));
            }
            Err(_) => {
                last_error = Some(anyhow!(
                    "attempt {}/{} timed out after {}s",
                    attempt + 1,
                    total_attempts,
                    timeout_secs.max(1)
                ));
            }
        }

        if attempt + 1 < total_attempts {
            let backoff_ms = (INFERENCE_BACKOFF_BASE_MS << attempt).min(INFERENCE_BACKOFF_MAX_MS);
            sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("inference failed without an error message")))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_core::Language;
    use aether_parse::SymbolExtractor;
    use aether_store::{
        SirMetaRecord, SirStateStore, SnapshotStore, SqliteStore, SymbolCatalogStore, SymbolRecord,
    };
    use tempfile::tempdir;

    use super::{AetherRefactorPrepRequest, AetherVerifyIntentRequest};
    use crate::tools::{AetherMcpServer, current_unix_timestamp};

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"

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

    fn write_demo_workspace(workspace: &Path) -> String {
        let relative = "src/lib.rs";
        fs::create_dir_all(workspace.join("src")).expect("mkdirs");
        fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"mcp-refactor-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write cargo");
        fs::write(
            workspace.join(relative),
            "pub fn alpha() -> i32 { 1 }\n\npub fn beta() -> i32 { alpha() }\n",
        )
        .expect("write source");
        relative.to_owned()
    }

    fn parse_symbols(workspace: &Path, relative: &str) -> Vec<aether_core::Symbol> {
        let source = fs::read_to_string(workspace.join(relative)).expect("read source");
        let mut extractor = SymbolExtractor::new().expect("extractor");
        extractor
            .extract_from_source(Language::Rust, relative, &source)
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
            last_seen_at: current_unix_timestamp(),
        }
    }

    fn seed_deep_sir(store: &SqliteStore, symbol: &aether_core::Symbol) {
        let sir_json = format!(
            "{{\"confidence\":0.95,\"dependencies\":[],\"error_modes\":[],\"inputs\":[],\"intent\":\"{} stable\",\"outputs\":[],\"side_effects\":[]}}",
            symbol.qualified_name
        );
        store
            .write_sir_blob(symbol.id.as_str(), &sir_json)
            .expect("write sir");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol.id.clone(),
                sir_hash: format!("hash-{}", symbol.id),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                generation_pass: "deep".to_owned(),
                updated_at: current_unix_timestamp(),
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: current_unix_timestamp(),
            })
            .expect("upsert meta");
    }

    #[test]
    fn mcp_refactor_prep_creates_snapshot_when_selected_symbols_are_already_fresh() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_workspace(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        let symbols = parse_symbols(temp.path(), &relative);
        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            seed_deep_sir(&store, symbol);
        }

        let server = AetherMcpServer::new(temp.path(), false).expect("server");
        let response = server
            .aether_refactor_prep_logic(AetherRefactorPrepRequest {
                file: Some(relative.clone()),
                crate_name: None,
                top_n: Some(2),
                local: Some(false),
            })
            .expect("refactor prep");

        assert!(response.snapshot_id.starts_with("refactor-prep-"));
        assert_eq!(response.deep_requested, 0);
        assert_eq!(response.deep_completed, 0);
        assert_eq!(response.candidates.len(), 2);
        assert_eq!(response.skipped_fresh, 2);
        assert!(
            server
                .state
                .store
                .get_snapshot(response.snapshot_id.as_str())
                .expect("lookup snapshot")
                .is_some()
        );
    }

    #[test]
    fn mcp_verify_intent_passes_for_unchanged_snapshot() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_workspace(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        let symbols = parse_symbols(temp.path(), &relative);
        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            seed_deep_sir(&store, symbol);
        }

        let server = AetherMcpServer::new(temp.path(), false).expect("server");
        let prep = server
            .aether_refactor_prep_logic(AetherRefactorPrepRequest {
                file: Some(relative),
                crate_name: None,
                top_n: Some(2),
                local: Some(false),
            })
            .expect("refactor prep");
        let verify = server
            .aether_verify_intent_logic(AetherVerifyIntentRequest {
                snapshot: prep.snapshot_id,
                threshold: Some(0.85),
            })
            .expect("verify intent");

        assert!(verify.passed);
        assert_eq!(verify.failed_entries, 0);
    }
}
