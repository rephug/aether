use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_analysis::TestIntentAnalyzer;
use aether_config::{SIR_QUALITY_FLOOR_CONFIDENCE, SIR_QUALITY_FLOOR_WINDOW};
use aether_core::{
    GitContext, Language, Position, SourceRange, Symbol, SymbolChangeEvent, content_hash,
};
use aether_infer::{
    EmbeddingProvider, EmbeddingProviderOverrides, InferenceProvider, ProviderOverrides,
    SirContext, load_embedding_provider_from_config, load_provider_from_env_or_mock,
};
use aether_parse::{SymbolExtractor, TestIntent};
use aether_sir::{
    FileSir, SirAnnotation, canonicalize_file_sir_json, canonicalize_sir_json, file_sir_hash,
    sir_hash, synthetic_file_sir_id, validate_sir,
};
use aether_store::{
    IntentOperation, SirMetaRecord, SqliteStore, Store, SymbolEmbeddingRecord, SymbolRecord,
    TestIntentRecord, VectorStore, WriteIntent, WriteIntentStatus, open_graph_store,
    open_vector_store,
};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};

use crate::quality::SirQualityMonitor;

pub const DEFAULT_SIR_CONCURRENCY: usize = 2;
const SIR_STATUS_FRESH: &str = "fresh";
const SIR_STATUS_STALE: &str = "stale";
const INFERENCE_MAX_RETRIES: usize = 2;
const INFERENCE_ATTEMPT_TIMEOUT_SECS: u64 = 90;
const INFERENCE_BACKOFF_BASE_MS: u64 = 200;
const INFERENCE_BACKOFF_MAX_MS: u64 = 2_000;
const MAX_SYMBOL_TEXT_CHARS: usize = 10_000;

pub struct SirPipeline {
    workspace_root: PathBuf,
    provider: Arc<dyn InferenceProvider>,
    provider_name: String,
    model_name: String,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    embedding_provider_name: Option<String>,
    embedding_model_name: Option<String>,
    vector_store: Arc<dyn VectorStore>,
    runtime: Runtime,
    sir_concurrency: usize,
    quality_monitor: Mutex<SirQualityMonitor>,
}

impl SirPipeline {
    pub fn new(
        workspace_root: PathBuf,
        sir_concurrency: usize,
        provider_overrides: ProviderOverrides,
    ) -> Result<Self> {
        let loaded = load_provider_from_env_or_mock(&workspace_root, provider_overrides)
            .context("failed to load inference provider")?;
        let provider = Arc::<dyn InferenceProvider>::from(loaded.provider);
        let loaded_embedding = load_embedding_provider_from_config(
            &workspace_root,
            EmbeddingProviderOverrides::default(),
        )
        .context("failed to load embedding provider")?;
        let (embedding_provider, embedding_identity) =
            loaded_embedding.map_or((None, None), |loaded| {
                (
                    Some(Arc::<dyn EmbeddingProvider>::from(loaded.provider)),
                    Some((loaded.provider_name, loaded.model_name)),
                )
            });

        Self::new_with_provider_and_embeddings(
            workspace_root,
            sir_concurrency,
            provider,
            loaded.provider_name,
            loaded.model_name,
            embedding_provider,
            embedding_identity,
        )
    }

    pub fn new_with_provider(
        workspace_root: PathBuf,
        sir_concurrency: usize,
        provider: Arc<dyn InferenceProvider>,
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
    ) -> Result<Self> {
        Self::new_with_provider_and_embeddings(
            workspace_root,
            sir_concurrency,
            provider,
            provider_name,
            model_name,
            None,
            None,
        )
    }

    fn new_with_provider_and_embeddings(
        workspace_root: PathBuf,
        sir_concurrency: usize,
        provider: Arc<dyn InferenceProvider>,
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
        embedding_identity: Option<(String, String)>,
    ) -> Result<Self> {
        let concurrency = sir_concurrency.max(1);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(concurrency)
            .enable_all()
            .build()
            .context("failed to build SIR async runtime")?;
        let (embedding_provider_name, embedding_model_name) = embedding_identity
            .map_or((None, None), |identity| {
                (Some(identity.0), Some(identity.1))
            });
        let vector_store = runtime
            .block_on(open_vector_store(&workspace_root))
            .context("failed to initialize vector store")?;

        Ok(Self {
            workspace_root,
            provider,
            provider_name: provider_name.into(),
            model_name: model_name.into(),
            embedding_provider,
            embedding_provider_name,
            embedding_model_name,
            vector_store,
            runtime,
            sir_concurrency: concurrency,
            quality_monitor: Mutex::new(SirQualityMonitor::new(
                SIR_QUALITY_FLOOR_WINDOW,
                SIR_QUALITY_FLOOR_CONFIDENCE,
            )),
        })
    }

    pub fn process_event(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
        force: bool,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        self.process_event_with_priority(store, event, force, print_sir, out, None)
    }

    pub fn process_event_with_priority(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
        force: bool,
        print_sir: bool,
        out: &mut dyn Write,
        priority_score: Option<f64>,
    ) -> Result<()> {
        for symbol in &event.removed {
            store
                .mark_removed(&symbol.id)
                .with_context(|| format!("failed to mark symbol removed: {}", symbol.id))?;
            self.runtime
                .block_on(self.vector_store.delete_embedding(&symbol.id))
                .with_context(|| format!("failed to remove vector embedding for {}", symbol.id))?;
        }

        self.replace_edges_for_file(store, event)?;

        let mut changed_symbols: Vec<(Symbol, bool)> =
            Vec::with_capacity(event.added.len() + event.updated.len());
        changed_symbols.extend(event.added.iter().cloned().map(|symbol| (symbol, true)));
        changed_symbols.extend(event.updated.iter().cloned().map(|symbol| (symbol, false)));

        let commit_hash = resolve_workspace_head_commit(&self.workspace_root);
        tracing::info!(
            file_path = %event.file_path,
            added = event.added.len(),
            updated = event.updated.len(),
            removed = event.removed.len(),
            "processing symbol change event"
        );
        let mut intents_ready_for_graph: Vec<String> = Vec::new();
        if !changed_symbols.is_empty() {
            let now_ts = unix_timestamp_secs();
            for (symbol, _) in &changed_symbols {
                store
                    .upsert_symbol(to_symbol_record(symbol, now_ts))
                    .with_context(|| format!("failed to upsert symbol {}", symbol.id))?;
            }

            let mut jobs = Vec::new();
            let mut skipped_existing = 0usize;
            for (symbol, allow_existing_skip) in changed_symbols {
                if allow_existing_skip
                    && !force
                    && self.should_skip_sir_generation(store, &symbol)?
                {
                    skipped_existing += 1;
                    tracing::debug!(
                        symbol_name = %symbol.name,
                        symbol_id = %symbol.id,
                        "Skipping SIR generation for {}: already exists",
                        symbol.name
                    );
                    if print_sir {
                        writeln!(
                            out,
                            "SIR_SKIPPED symbol_id={} reason=already_exists",
                            symbol.id
                        )
                        .context("failed to write skipped SIR print line")?;
                    }
                    continue;
                }

                match build_job(&self.workspace_root, symbol, priority_score) {
                    Ok(job) => jobs.push(job),
                    Err(err) => {
                        tracing::warn!(
                            file_path = %event.file_path,
                            error = %err,
                            "failed to build SIR job; skipping symbol"
                        );
                    }
                }
            }

            if skipped_existing > 0 {
                tracing::info!(
                    file_path = %event.file_path,
                    skipped_existing,
                    queued_jobs = jobs.len(),
                    "Skipping SIR generation for existing symbols: already exists"
                );
            }

            if jobs.is_empty() {
                tracing::info!(
                    file_path = %event.file_path,
                    "SIR generation processed 0 jobs"
                );
            }

            tracing::info!(
                job_count = jobs.len(),
                provider = %self.provider_name,
                model = %self.model_name,
                force,
                "submitting SIR generation jobs"
            );
            let results = self.runtime.block_on(generate_sir_jobs(
                self.provider.clone(),
                jobs,
                self.sir_concurrency,
            ))?;

            let mut success_count: usize = 0;
            let mut failure_count: usize = 0;
            let mark_intent_failed = |intent_id: &str, message: &str| {
                if let Err(mark_err) = store.mark_intent_failed(intent_id, message) {
                    tracing::error!(
                        intent_id = %intent_id,
                        error = %mark_err,
                        "failed to mark write intent as failed"
                    );
                }
            };

            for result in results {
                match result {
                    SirGenerationOutcome::Success(generated) => {
                        match self.quality_monitor.lock() {
                            Ok(mut monitor) => {
                                monitor.record(generated.sir.confidence);
                            }
                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    "failed to lock SIR quality monitor"
                                );
                            }
                        }

                        let payload = UpsertSirIntentPayload {
                            symbol: generated.symbol.clone(),
                            sir: generated.sir.clone(),
                            provider_name: self.provider_name.clone(),
                            model_name: self.model_name.clone(),
                            commit_hash: commit_hash.clone(),
                        };
                        let payload_json = match payload.to_json_string() {
                            Ok(json) => json,
                            Err(err) => {
                                failure_count += 1;
                                tracing::error!(
                                    symbol_id = %generated.symbol.id,
                                    error = %err,
                                    "failed to serialize write intent payload"
                                );
                                continue;
                            }
                        };

                        let intent = WriteIntent {
                            intent_id: content_hash(
                                format!("{}\n{}", generated.symbol.id, unix_timestamp_millis())
                                    .as_str(),
                            ),
                            symbol_id: generated.symbol.id.clone(),
                            file_path: generated.symbol.file_path.clone(),
                            operation: IntentOperation::UpsertSir,
                            status: WriteIntentStatus::Pending,
                            payload_json: Some(payload_json),
                            created_at: unix_timestamp_secs(),
                            completed_at: None,
                            error_message: None,
                        };
                        if let Err(err) = store.create_write_intent(&intent) {
                            failure_count += 1;
                            tracing::error!(
                                symbol_id = %generated.symbol.id,
                                error = %err,
                                "failed to create write intent; skipping symbol write"
                            );
                            continue;
                        }

                        let canonical_json = canonicalize_sir_json(&generated.sir);
                        let sir_hash_value = sir_hash(&generated.sir);
                        let attempted_at = unix_timestamp_secs();
                        let version_write = match store.record_sir_version_if_changed(
                            &generated.symbol.id,
                            &sir_hash_value,
                            payload.provider_name.as_str(),
                            payload.model_name.as_str(),
                            &canonical_json,
                            attempted_at,
                            payload.commit_hash.as_deref(),
                        ) {
                            Ok(version_write) => version_write,
                            Err(err) => {
                                failure_count += 1;
                                mark_intent_failed(&intent.intent_id, format!("{err:#}").as_str());
                                tracing::error!(
                                    symbol_id = %generated.symbol.id,
                                    error = %err,
                                    "failed to record SIR history"
                                );
                                continue;
                            }
                        };

                        if version_write.changed
                            && let Err(err) =
                                store.write_sir_blob(&generated.symbol.id, &canonical_json)
                        {
                            failure_count += 1;
                            mark_intent_failed(&intent.intent_id, format!("{err:#}").as_str());
                            tracing::error!(
                                symbol_id = %generated.symbol.id,
                                error = %err,
                                "failed to write SIR blob"
                            );
                            continue;
                        }

                        if let Err(err) = store.upsert_sir_meta(SirMetaRecord {
                            id: generated.symbol.id.clone(),
                            sir_hash: sir_hash_value.clone(),
                            sir_version: version_write.version,
                            provider: payload.provider_name.clone(),
                            model: payload.model_name.clone(),
                            updated_at: version_write.updated_at,
                            sir_status: SIR_STATUS_FRESH.to_owned(),
                            last_error: None,
                            last_attempt_at: attempted_at,
                        }) {
                            failure_count += 1;
                            mark_intent_failed(&intent.intent_id, format!("{err:#}").as_str());
                            tracing::error!(
                                symbol_id = %generated.symbol.id,
                                error = %err,
                                "failed to upsert SIR metadata"
                            );
                            continue;
                        }

                        if let Err(err) = store
                            .update_intent_status(&intent.intent_id, WriteIntentStatus::SqliteDone)
                        {
                            failure_count += 1;
                            mark_intent_failed(&intent.intent_id, format!("{err:#}").as_str());
                            tracing::error!(
                                symbol_id = %generated.symbol.id,
                                error = %err,
                                "failed to update write intent status to sqlite_done"
                            );
                            continue;
                        }

                        if let Err(err) = self.refresh_embedding_if_needed(
                            &generated.symbol.id,
                            &sir_hash_value,
                            &canonical_json,
                            print_sir,
                            out,
                        ) {
                            failure_count += 1;
                            mark_intent_failed(&intent.intent_id, format!("{err:#}").as_str());
                            tracing::error!(
                                symbol_id = %generated.symbol.id,
                                error = %err,
                                "embedding refresh error"
                            );
                            continue;
                        }

                        if let Err(err) = store
                            .update_intent_status(&intent.intent_id, WriteIntentStatus::VectorDone)
                        {
                            failure_count += 1;
                            mark_intent_failed(&intent.intent_id, format!("{err:#}").as_str());
                            tracing::error!(
                                symbol_id = %generated.symbol.id,
                                error = %err,
                                "failed to update write intent status to vector_done"
                            );
                            continue;
                        }
                        intents_ready_for_graph.push(intent.intent_id.clone());

                        if print_sir {
                            writeln!(
                                out,
                                "SIR_STORED symbol_id={} sir_hash={} provider={}",
                                generated.symbol.id, sir_hash_value, self.provider_name
                            )
                            .context("failed to write SIR print line")?;
                        }

                        success_count += 1;
                        tracing::debug!(
                            symbol_id = %generated.symbol.id,
                            "SIR generated successfully"
                        );
                    }
                    SirGenerationOutcome::Failure(failed) => {
                        let last_attempt_at = unix_timestamp_secs();
                        let previous_meta =
                            store.get_sir_meta(&failed.symbol.id).with_context(|| {
                                format!("failed to load SIR metadata for {}", failed.symbol.id)
                            })?;

                        let stale_meta = previous_meta.map_or_else(
                            || SirMetaRecord {
                                id: failed.symbol.id.clone(),
                                sir_hash: String::new(),
                                sir_version: 1,
                                provider: self.provider_name.clone(),
                                model: self.model_name.clone(),
                                updated_at: 0,
                                sir_status: SIR_STATUS_STALE.to_owned(),
                                last_error: Some(failed.error_message.clone()),
                                last_attempt_at,
                            },
                            |record| SirMetaRecord {
                                id: failed.symbol.id.clone(),
                                sir_hash: record.sir_hash,
                                sir_version: record.sir_version,
                                provider: if record.provider.trim().is_empty() {
                                    self.provider_name.clone()
                                } else {
                                    record.provider
                                },
                                model: if record.model.trim().is_empty() {
                                    self.model_name.clone()
                                } else {
                                    record.model
                                },
                                updated_at: record.updated_at,
                                sir_status: SIR_STATUS_STALE.to_owned(),
                                last_error: Some(failed.error_message.clone()),
                                last_attempt_at,
                            },
                        );

                        failure_count += 1;
                        tracing::warn!(
                            symbol_id = %failed.symbol.id,
                            qualified_name = %failed.symbol.qualified_name,
                            error = %failed.error_message,
                            "SIR generation failed"
                        );
                        if let Err(err) = store.upsert_sir_meta(stale_meta) {
                            tracing::error!(
                                symbol_id = %failed.symbol.id,
                                error = %err,
                                "failed to store stale SIR metadata"
                            );
                        }

                        if print_sir {
                            writeln!(
                                out,
                                "SIR_STALE symbol_id={} error={}",
                                failed.symbol.id,
                                flatten_error_line(&failed.error_message)
                            )
                            .context("failed to write stale SIR print line")?;
                        }
                    }
                }
            }

            let graph_sync = self.sync_graph_for_file(store, &event.file_path);
            match graph_sync {
                Ok(()) => {
                    for intent_id in intents_ready_for_graph.drain(..) {
                        if let Err(err) =
                            store.update_intent_status(&intent_id, WriteIntentStatus::GraphDone)
                        {
                            tracing::error!(
                                intent_id = %intent_id,
                                error = %err,
                                "failed to update write intent status to graph_done"
                            );
                            let _ = store.mark_intent_failed(
                                &intent_id,
                                format!("graph_done update failed: {err:#}").as_str(),
                            );
                            continue;
                        }
                        if let Err(err) = store.mark_intent_complete(&intent_id) {
                            tracing::error!(
                                intent_id = %intent_id,
                                error = %err,
                                "failed to mark write intent complete"
                            );
                            let _ = store.mark_intent_failed(
                                &intent_id,
                                format!("intent completion failed: {err:#}").as_str(),
                            );
                        }
                    }
                }
                Err(err) => {
                    let error_text = format!("{err:#}");
                    for intent_id in intents_ready_for_graph.drain(..) {
                        let _ = store.mark_intent_failed(&intent_id, error_text.as_str());
                    }
                    tracing::warn!(
                        file_path = %event.file_path,
                        error = %error_text,
                        "graph sync failed after vector stage"
                    );
                }
            }

            if failure_count > 0 {
                tracing::warn!(
                    file_path = %event.file_path,
                    successes = success_count,
                    failures = failure_count,
                    "SIR processing complete with failures"
                );
            } else if success_count > 0 {
                tracing::info!(
                    file_path = %event.file_path,
                    successes = success_count,
                    "SIR processing complete"
                );
            }
        } else if let Err(err) = self.sync_graph_for_file(store, &event.file_path) {
            tracing::warn!(
                file_path = %event.file_path,
                error = %err,
                "graph sync failed for event without changed symbols"
            );
        }

        self.upsert_file_rollup(
            store,
            &event.file_path,
            event.language,
            print_sir,
            out,
            commit_hash.as_deref(),
        )?;

        Ok(())
    }

    pub fn replay_incomplete_intents(
        &self,
        store: &SqliteStore,
        include_failed: bool,
        batch_size: usize,
        verbose: bool,
    ) -> Result<usize> {
        let mut intents = store
            .get_incomplete_intents()
            .context("failed to query incomplete write intents")?;
        if include_failed {
            intents.extend(
                store
                    .get_failed_intents()
                    .context("failed to query failed write intents")?,
            );
        }

        intents.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.intent_id.cmp(&right.intent_id))
        });

        let mut replayed = 0usize;
        for chunk in intents.chunks(batch_size.max(1)) {
            for intent in chunk {
                if let Err(err) = self.replay_write_intent(store, intent, verbose) {
                    tracing::warn!(
                        intent_id = %intent.intent_id,
                        symbol_id = %intent.symbol_id,
                        status = %intent.status,
                        error = %err,
                        "failed to replay write intent"
                    );
                    continue;
                }
                replayed += 1;
            }
        }

        Ok(replayed)
    }

    fn replay_write_intent(
        &self,
        store: &SqliteStore,
        intent: &WriteIntent,
        verbose: bool,
    ) -> Result<()> {
        match intent.operation {
            IntentOperation::UpsertSir => {
                let payload_json = intent.payload_json.as_deref().ok_or_else(|| {
                    anyhow!("missing payload_json for intent {}", intent.intent_id)
                })?;
                let payload =
                    UpsertSirIntentPayload::from_json_str(payload_json).with_context(|| {
                        format!(
                            "failed to deserialize payload_json for intent {}",
                            intent.intent_id
                        )
                    })?;
                if let Err(err) = self.replay_upsert_sir_intent(store, intent, &payload, verbose) {
                    let message = format!("{err:#}");
                    let _ = store.mark_intent_failed(&intent.intent_id, message.as_str());
                    return Err(err);
                }
                Ok(())
            }
            IntentOperation::DeleteSymbol | IntentOperation::UpdateEdges => {
                let message = format!(
                    "unsupported write intent replay operation '{}'",
                    intent.operation
                );
                let _ = store.mark_intent_failed(&intent.intent_id, message.as_str());
                Err(anyhow!(message))
            }
        }
    }

    fn replay_upsert_sir_intent(
        &self,
        store: &SqliteStore,
        intent: &WriteIntent,
        payload: &UpsertSirIntentPayload,
        verbose: bool,
    ) -> Result<()> {
        let mut status = match intent.status {
            WriteIntentStatus::Failed => WriteIntentStatus::Pending,
            ref current => current.clone(),
        };
        let intent_id = intent.intent_id.as_str();

        let (canonical_json, sir_hash_value) = if status == WriteIntentStatus::Pending {
            let (canonical_json, sir_hash_value) = self
                .persist_sir_payload_into_sqlite(store, payload)
                .with_context(|| format!("failed sqlite write stage for intent {intent_id}"))?;
            store
                .update_intent_status(intent_id, WriteIntentStatus::SqliteDone)
                .with_context(|| {
                    format!("failed to update status sqlite_done for intent {intent_id}")
                })?;
            status = WriteIntentStatus::SqliteDone;
            (canonical_json, sir_hash_value)
        } else {
            (
                canonicalize_sir_json(&payload.sir),
                sir_hash(&payload.sir).to_owned(),
            )
        };

        if status == WriteIntentStatus::SqliteDone {
            self.refresh_embedding_if_needed(
                payload.symbol.id.as_str(),
                sir_hash_value.as_str(),
                canonical_json.as_str(),
                false,
                &mut std::io::sink(),
            )
            .with_context(|| format!("failed vector write stage for intent {intent_id}"))?;
            store
                .update_intent_status(intent_id, WriteIntentStatus::VectorDone)
                .with_context(|| {
                    format!("failed to update status vector_done for intent {intent_id}")
                })?;
            status = WriteIntentStatus::VectorDone;
        }

        if status == WriteIntentStatus::VectorDone {
            self.sync_graph_for_file(store, payload.symbol.file_path.as_str())
                .with_context(|| format!("failed graph write stage for intent {intent_id}"))?;
            store
                .update_intent_status(intent_id, WriteIntentStatus::GraphDone)
                .with_context(|| {
                    format!("failed to update status graph_done for intent {intent_id}")
                })?;
            status = WriteIntentStatus::GraphDone;
        }

        if status == WriteIntentStatus::GraphDone {
            store
                .mark_intent_complete(intent_id)
                .with_context(|| format!("failed to mark complete for intent {intent_id}"))?;
        }

        if verbose {
            tracing::info!(
                intent_id = %intent_id,
                symbol_id = %intent.symbol_id,
                "replayed write intent"
            );
        }

        Ok(())
    }

    fn persist_sir_payload_into_sqlite(
        &self,
        store: &SqliteStore,
        payload: &UpsertSirIntentPayload,
    ) -> Result<(String, String)> {
        let canonical_json = canonicalize_sir_json(&payload.sir);
        let sir_hash_value = sir_hash(&payload.sir);
        let attempted_at = unix_timestamp_secs();
        let version_write = store.record_sir_version_if_changed(
            payload.symbol.id.as_str(),
            sir_hash_value.as_str(),
            payload.provider_name.as_str(),
            payload.model_name.as_str(),
            canonical_json.as_str(),
            attempted_at,
            payload.commit_hash.as_deref(),
        )?;

        if version_write.changed {
            store.write_sir_blob(payload.symbol.id.as_str(), canonical_json.as_str())?;
        }

        store.upsert_sir_meta(SirMetaRecord {
            id: payload.symbol.id.clone(),
            sir_hash: sir_hash_value.clone(),
            sir_version: version_write.version,
            provider: payload.provider_name.clone(),
            model: payload.model_name.clone(),
            updated_at: version_write.updated_at,
            sir_status: SIR_STATUS_FRESH.to_owned(),
            last_error: None,
            last_attempt_at: attempted_at,
        })?;

        Ok((canonical_json, sir_hash_value))
    }

    fn should_skip_sir_generation(&self, store: &SqliteStore, symbol: &Symbol) -> Result<bool> {
        let Some(meta) = store
            .get_sir_meta(&symbol.id)
            .with_context(|| format!("failed to read SIR metadata for {}", symbol.id))?
        else {
            return Ok(false);
        };

        let status = meta.sir_status.trim().to_ascii_lowercase();
        if status != SIR_STATUS_FRESH && status != "ready" {
            return Ok(false);
        }

        let Some(existing_blob) = store
            .read_sir_blob(&symbol.id)
            .with_context(|| format!("failed to read SIR blob for {}", symbol.id))?
        else {
            return Ok(false);
        };
        if existing_blob.trim().is_empty() {
            return Ok(false);
        }

        let Some(source_modified_at_ms) =
            source_modified_unix_millis(self.workspace_root.join(&symbol.file_path).as_path())
        else {
            return Ok(false);
        };

        Ok(source_modified_at_ms < meta.updated_at.max(0).saturating_mul(1_000))
    }

    fn replace_edges_for_file(&self, store: &SqliteStore, event: &SymbolChangeEvent) -> Result<()> {
        store
            .delete_edges_for_file(&event.file_path)
            .with_context(|| format!("failed to delete edges for file {}", event.file_path))?;

        let full_path = self.workspace_root.join(&event.file_path);
        let source = match fs::read_to_string(&full_path) {
            Ok(source) => Some(source),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "failed to read source for edge extraction {}",
                        full_path.display()
                    )
                });
            }
        };

        if let Some(source) = source {
            let mut extractor = SymbolExtractor::new().context("failed to initialize parser")?;
            let extracted = extractor
                .extract_with_edges_from_path(Path::new(&event.file_path), &source)
                .with_context(|| format!("failed to extract edges from {}", event.file_path))?;

            store
                .upsert_edges(&extracted.edges)
                .with_context(|| format!("failed to upsert edges for file {}", event.file_path))?;

            let now_ms = unix_timestamp_millis();
            let test_intents = extracted
                .test_intents
                .into_iter()
                .map(|intent| to_test_intent_record(intent, now_ms))
                .collect::<Vec<_>>();
            store
                .replace_test_intents_for_file(event.file_path.as_str(), test_intents.as_slice())
                .with_context(|| {
                    format!("failed to upsert test intents for file {}", event.file_path)
                })?;
        } else {
            store
                .replace_test_intents_for_file(event.file_path.as_str(), &[])
                .with_context(|| {
                    format!("failed to clear test intents for file {}", event.file_path)
                })?;
        }

        let test_intent_analyzer = TestIntentAnalyzer::new(&self.workspace_root)
            .context("failed to initialize test intent analyzer")?;
        let _ = test_intent_analyzer
            .refresh_for_test_file(event.file_path.as_str())
            .with_context(|| {
                format!(
                    "failed to refresh tested_by links for test file {}",
                    event.file_path
                )
            })?;

        Ok(())
    }

    fn sync_graph_for_file(&self, store: &SqliteStore, file_path: &str) -> Result<()> {
        let stats = self.runtime.block_on(async {
            let graph_store = open_graph_store(&self.workspace_root)
                .await
                .context("failed to open configured graph store")?;
            store
                .sync_graph_for_file(graph_store.as_ref(), file_path)
                .await
                .with_context(|| format!("failed to sync graph edges for file {file_path}"))
        })?;

        if stats.unresolved_edges > 0 {
            tracing::debug!(
                file_path = %file_path,
                resolved_edges = stats.resolved_edges,
                unresolved_edges = stats.unresolved_edges,
                "graph sync skipped unresolved call edges"
            );
        }

        Ok(())
    }

    fn refresh_embedding_if_needed(
        &self,
        symbol_id: &str,
        sir_hash_value: &str,
        canonical_json: &str,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        let Some(embedding_provider) = self.embedding_provider.as_ref() else {
            return Ok(());
        };

        let provider_name = self
            .embedding_provider_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("mock");
        let model_name = self
            .embedding_model_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("mock");

        let existing_meta = self
            .runtime
            .block_on(self.vector_store.get_embedding_meta(symbol_id))
            .with_context(|| format!("failed to read embedding metadata for {symbol_id}"))?;
        if let Some(existing_meta) = existing_meta
            && existing_meta.sir_hash == sir_hash_value
            && existing_meta.provider == provider_name
            && existing_meta.model == model_name
        {
            return Ok(());
        }

        let embedding = self
            .runtime
            .block_on(embedding_provider.embed_text(canonical_json))
            .with_context(|| format!("failed to generate embedding for {symbol_id}"))?;

        if embedding.is_empty() {
            return Ok(());
        }

        let updated_at = unix_timestamp_secs();
        self.runtime
            .block_on(self.vector_store.upsert_embedding(SymbolEmbeddingRecord {
                symbol_id: symbol_id.to_owned(),
                sir_hash: sir_hash_value.to_owned(),
                provider: provider_name.to_owned(),
                model: model_name.to_owned(),
                embedding,
                updated_at,
            }))
            .with_context(|| format!("failed to store embedding for {symbol_id}"))?;

        if print_sir {
            writeln!(
                out,
                "EMBEDDING_STORED symbol_id={symbol_id} provider={provider_name} model={model_name}"
            )
            .context("failed to write embedding print line")?;
        }

        Ok(())
    }

    fn upsert_file_rollup(
        &self,
        store: &SqliteStore,
        file_path: &str,
        language: Language,
        print_sir: bool,
        out: &mut dyn Write,
        commit_hash: Option<&str>,
    ) -> Result<()> {
        let rollup_id = synthetic_file_sir_id(language.as_str(), file_path);
        let symbols = store
            .list_symbols_for_file(file_path)
            .with_context(|| format!("failed to list symbols for file {file_path}"))?;

        if symbols.is_empty() {
            store
                .mark_removed(&rollup_id)
                .with_context(|| format!("failed to remove stale file rollup {rollup_id}"))?;
            return Ok(());
        }

        let mut leaf_sirs = Vec::new();
        for symbol in symbols {
            let Some(blob) = store
                .read_sir_blob(&symbol.id)
                .with_context(|| format!("failed to read SIR blob for symbol {}", symbol.id))?
            else {
                continue;
            };

            let parsed = serde_json::from_str::<SirAnnotation>(&blob);
            let Ok(sir) = parsed else {
                tracing::warn!(
                    symbol_id = %symbol.id,
                    file_path = %file_path,
                    "skipping invalid leaf SIR JSON while aggregating file rollup"
                );
                continue;
            };

            if let Err(err) = validate_sir(&sir) {
                tracing::warn!(
                    symbol_id = %symbol.id,
                    file_path = %file_path,
                    error = %err,
                    "skipping invalid leaf SIR annotation while aggregating file rollup"
                );
                continue;
            }

            leaf_sirs.push(FileLeafSir {
                qualified_name: symbol.qualified_name,
                sir,
            });
        }

        if leaf_sirs.is_empty() {
            store
                .mark_removed(&rollup_id)
                .with_context(|| format!("failed to remove stale file rollup {rollup_id}"))?;
            return Ok(());
        }

        let file_sir = self
            .aggregate_file_sir(file_path, language, &leaf_sirs)
            .with_context(|| format!("failed to aggregate file rollup for {file_path}"))?;
        let canonical_json = canonicalize_file_sir_json(&file_sir);
        let sir_hash_value = file_sir_hash(&file_sir);
        let attempted_at = unix_timestamp_secs();
        let version_write = store
            .record_sir_version_if_changed(
                &rollup_id,
                &sir_hash_value,
                &self.provider_name,
                &self.model_name,
                &canonical_json,
                attempted_at,
                commit_hash,
            )
            .with_context(|| format!("failed to record file rollup history for {file_path}"))?;

        if version_write.changed {
            store
                .write_sir_blob(&rollup_id, &canonical_json)
                .with_context(|| format!("failed to write file rollup for {file_path}"))?;
        }

        store
            .upsert_sir_meta(SirMetaRecord {
                id: rollup_id.clone(),
                sir_hash: sir_hash_value.clone(),
                sir_version: version_write.version,
                provider: self.provider_name.clone(),
                model: self.model_name.clone(),
                updated_at: version_write.updated_at,
                sir_status: SIR_STATUS_FRESH.to_owned(),
                last_error: None,
                last_attempt_at: attempted_at,
            })
            .with_context(|| format!("failed to upsert file rollup metadata for {file_path}"))?;

        if print_sir {
            writeln!(
                out,
                "SIR_FILE_STORED symbol_id={} sir_hash={} provider={}",
                rollup_id, sir_hash_value, self.provider_name
            )
            .context("failed to write file rollup print line")?;
        }

        Ok(())
    }

    fn aggregate_file_sir(
        &self,
        file_path: &str,
        language: Language,
        leaf_sirs: &[FileLeafSir],
    ) -> Result<FileSir> {
        let mut sorted = leaf_sirs.to_vec();
        sorted.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));

        let intent = if sorted.len() <= 5 {
            concatenate_leaf_intents(&sorted)
        } else {
            match self.summarize_file_intent(file_path, language, &sorted) {
                Ok(summary) if !summary.trim().is_empty() => summary,
                Ok(_) => concatenate_leaf_intents(&sorted),
                Err(err) => {
                    tracing::warn!(
                        file_path = %file_path,
                        error = %err,
                        "file rollup summarization failed, using deterministic concatenation"
                    );
                    concatenate_leaf_intents(&sorted)
                }
            }
        };

        let mut exports = Vec::new();
        let mut side_effects = Vec::new();
        let mut dependencies = Vec::new();
        let mut error_modes = Vec::new();
        let mut confidence = 1.0f32;

        for entry in &sorted {
            exports.push(entry.qualified_name.clone());
            side_effects.extend(entry.sir.side_effects.clone());
            dependencies.extend(entry.sir.dependencies.clone());
            error_modes.extend(entry.sir.error_modes.clone());
            confidence = confidence.min(entry.sir.confidence);
        }

        sort_and_dedup(&mut exports);
        sort_and_dedup(&mut side_effects);
        sort_and_dedup(&mut dependencies);
        sort_and_dedup(&mut error_modes);

        Ok(FileSir {
            intent,
            exports,
            side_effects,
            dependencies,
            error_modes,
            symbol_count: sorted.len(),
            confidence,
        })
    }

    fn summarize_file_intent(
        &self,
        file_path: &str,
        language: Language,
        leaf_sirs: &[FileLeafSir],
    ) -> Result<String> {
        let mut prompt_sections = Vec::with_capacity(leaf_sirs.len());
        for entry in leaf_sirs {
            prompt_sections.push(format!(
                "symbol: {}\nintent: {}\nside_effects: {}\ndependencies: {}\nerror_modes: {}",
                entry.qualified_name,
                entry.sir.intent,
                join_field(&entry.sir.side_effects),
                join_field(&entry.sir.dependencies),
                join_field(&entry.sir.error_modes),
            ));
        }

        let summary_input = format!(
            "Generate a concise file-level intent summary from the following leaf SIR entries.\n\n{}",
            prompt_sections.join("\n\n")
        );
        let context = SirContext {
            language: language.as_str().to_owned(),
            file_path: file_path.to_owned(),
            qualified_name: format!("file::{file_path}"),
            priority_score: None,
        };

        let summarized = self
            .runtime
            .block_on(generate_sir_with_retries(
                self.provider.clone(),
                summary_input,
                context,
            ))
            .with_context(|| format!("failed to summarize file intent for {file_path}"))?;

        Ok(summarized.intent.trim().to_owned())
    }
}

#[derive(Debug)]
struct SirJob {
    symbol: Symbol,
    symbol_text: String,
    context: SirContext,
}

#[derive(Debug)]
struct GeneratedSir {
    symbol: Symbol,
    sir: SirAnnotation,
}

#[derive(Debug)]
struct FailedSirGeneration {
    symbol: Symbol,
    error_message: String,
}

#[derive(Debug, Clone)]
struct FileLeafSir {
    qualified_name: String,
    sir: SirAnnotation,
}

#[derive(Debug, Clone)]
struct UpsertSirIntentPayload {
    symbol: Symbol,
    sir: SirAnnotation,
    provider_name: String,
    model_name: String,
    commit_hash: Option<String>,
}

impl UpsertSirIntentPayload {
    fn to_json_string(&self) -> Result<String> {
        serde_json::to_string(&json!({
            "symbol": self.symbol,
            "sir": self.sir,
            "provider_name": self.provider_name,
            "model_name": self.model_name,
            "commit_hash": self.commit_hash,
        }))
        .context("failed to serialize upsert intent payload")
    }

    fn from_json_str(raw: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(raw).context("failed to parse payload JSON")?;
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("payload must be a JSON object"))?;
        let symbol_value = object
            .get("symbol")
            .cloned()
            .ok_or_else(|| anyhow!("payload missing field 'symbol'"))?;
        let sir_value = object
            .get("sir")
            .cloned()
            .ok_or_else(|| anyhow!("payload missing field 'sir'"))?;
        let provider_name = payload_required_string(object, "provider_name")?;
        let model_name = payload_required_string(object, "model_name")?;
        let commit_hash = match object.get("commit_hash") {
            Some(Value::String(value)) => Some(value.clone()),
            Some(Value::Null) | None => None,
            Some(_) => {
                return Err(anyhow!(
                    "payload field 'commit_hash' must be a string or null"
                ));
            }
        };

        Ok(Self {
            symbol: serde_json::from_value(symbol_value).context("invalid payload symbol")?,
            sir: serde_json::from_value(sir_value).context("invalid payload sir")?,
            provider_name,
            model_name,
            commit_hash,
        })
    }
}

fn payload_required_string(
    payload: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String> {
    match payload.get(field) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(anyhow!("payload field '{field}' must be a string")),
        None => Err(anyhow!("payload missing field '{field}'")),
    }
}

#[derive(Debug)]
enum SirGenerationOutcome {
    Success(GeneratedSir),
    Failure(FailedSirGeneration),
}

fn to_symbol_record(symbol: &Symbol, now_ts: i64) -> SymbolRecord {
    SymbolRecord {
        id: symbol.id.clone(),
        file_path: symbol.file_path.clone(),
        language: symbol.language.as_str().to_owned(),
        kind: symbol.kind.as_str().to_owned(),
        qualified_name: symbol.qualified_name.clone(),
        signature_fingerprint: symbol.signature_fingerprint.clone(),
        last_seen_at: now_ts,
    }
}

fn build_job(workspace_root: &Path, symbol: Symbol, priority_score: Option<f64>) -> Result<SirJob> {
    let full_path = workspace_root.join(&symbol.file_path);
    let source = fs::read_to_string(&full_path)
        .with_context(|| format!("failed to read symbol source file {}", full_path.display()))?;

    let mut symbol_text = extract_symbol_source_text(&source, symbol.range)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "tree-sitter extraction failed for {} in {} — skipping to avoid whole-file hallucination",
                symbol.qualified_name,
                symbol.file_path,
            )
        })?;
    if symbol_text.len() > MAX_SYMBOL_TEXT_CHARS {
        let truncated = symbol_text
            .char_indices()
            .take_while(|(index, _)| *index < MAX_SYMBOL_TEXT_CHARS)
            .last()
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(0);
        tracing::warn!(
            symbol = %symbol.name,
            original_len = symbol_text.len(),
            truncated_len = truncated,
            "symbol text truncated for inference"
        );
        symbol_text = symbol_text[..truncated].to_owned();
    }

    let context = SirContext {
        language: symbol.language.as_str().to_owned(),
        file_path: symbol.file_path.clone(),
        qualified_name: symbol.qualified_name.clone(),
        priority_score,
    };

    Ok(SirJob {
        symbol,
        symbol_text,
        context,
    })
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

    source.get(start..end).map(|slice| slice.to_owned())
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

async fn generate_sir_jobs(
    provider: Arc<dyn InferenceProvider>,
    jobs: Vec<SirJob>,
    concurrency: usize,
) -> Result<Vec<SirGenerationOutcome>> {
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut join_set = JoinSet::new();

    for job in jobs {
        let provider = provider.clone();
        let semaphore = semaphore.clone();

        join_set.spawn(async move {
            let SirJob {
                symbol,
                symbol_text,
                context,
            } = job;
            let qualified_name = symbol.qualified_name.clone();

            let permit = semaphore.acquire_owned().await;
            let _permit = match permit {
                Ok(permit) => permit,
                Err(_) => {
                    return SirGenerationOutcome::Failure(FailedSirGeneration {
                        symbol,
                        error_message: "inference semaphore closed".to_owned(),
                    });
                }
            };

            let generated = generate_sir_with_retries(provider, symbol_text, context)
                .await
                .with_context(|| format!("failed to generate SIR for symbol {qualified_name}"));

            match generated {
                Ok(sir) => SirGenerationOutcome::Success(GeneratedSir { symbol, sir }),
                Err(err) => SirGenerationOutcome::Failure(FailedSirGeneration {
                    symbol,
                    error_message: format!("{err:#}"),
                }),
            }
        });
    }

    let mut results = Vec::new();
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok(result) => results.push(result),
            Err(err) => return Err(anyhow!("inference task join error: {err}")),
        }
    }

    Ok(results)
}

async fn generate_sir_with_retries(
    provider: Arc<dyn InferenceProvider>,
    symbol_text: String,
    context: SirContext,
) -> Result<SirAnnotation> {
    let total_attempts = INFERENCE_MAX_RETRIES + 1;
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..total_attempts {
        let timeout_result = timeout(
            Duration::from_secs(INFERENCE_ATTEMPT_TIMEOUT_SECS),
            provider.generate_sir(&symbol_text, &context),
        )
        .await;

        match timeout_result {
            Ok(Ok(sir)) => return Ok(sir),
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
                    INFERENCE_ATTEMPT_TIMEOUT_SECS
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

fn flatten_error_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).to_owned()
}

fn concatenate_leaf_intents(leaf_sirs: &[FileLeafSir]) -> String {
    let intents = leaf_sirs
        .iter()
        .map(|entry| entry.sir.intent.trim())
        .filter(|intent| !intent.is_empty())
        .collect::<Vec<_>>();
    if intents.is_empty() {
        "No summarized intent available".to_owned()
    } else {
        intents.join("; ")
    }
}

fn join_field(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_owned()
    } else {
        values.join(", ")
    }
}

fn sort_and_dedup(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn resolve_workspace_head_commit(workspace_root: &Path) -> Option<String> {
    GitContext::open(workspace_root).and_then(|context| context.head_commit_hash())
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn unix_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn source_modified_unix_millis(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
}

fn to_test_intent_record(intent: TestIntent, now_ms: i64) -> TestIntentRecord {
    let material = format!(
        "{}\n{}\n{}",
        intent.file_path.trim(),
        intent.test_name.trim(),
        intent.intent_text.trim(),
    );
    TestIntentRecord {
        intent_id: content_hash(material.as_str()),
        file_path: intent.file_path,
        test_name: intent.test_name,
        intent_text: intent.intent_text,
        group_label: intent.group_label,
        language: intent.language.as_str().to_owned(),
        symbol_id: intent.symbol_id,
        created_at: now_ms.max(0),
        updated_at: now_ms.max(0),
    }
}
