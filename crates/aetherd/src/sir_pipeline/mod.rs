use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use aether_analysis::TestIntentAnalyzer;
use aether_config::{
    ContractsConfig, InferenceProviderKind, SIR_QUALITY_FLOOR_CONFIDENCE, SIR_QUALITY_FLOOR_WINDOW,
    ensure_workspace_config, load_workspace_config,
};
use aether_core::{EdgeKind, GitContext, Language, Symbol, SymbolChangeEvent, content_hash};
use aether_infer::{
    EmbeddingProvider, EmbeddingProviderOverrides, EmbeddingPurpose, InferenceProvider,
    ProviderOverrides, Qwen3LocalProvider, load_embedding_provider_from_config,
    load_provider_from_env_or_mock,
    sir_prompt::{self, SirEnrichmentContext},
};
#[cfg(test)]
use aether_infer::{InferError, SirContext};
use aether_parse::SymbolExtractor;
use aether_sir::{
    FileSir, SirAnnotation, canonicalize_file_sir_json, canonicalize_sir_json, file_sir_hash,
    sir_hash, synthetic_file_sir_id, validate_sir,
};
#[cfg(test)]
use aether_store::SymbolRecord;
use aether_store::{
    BatchCompleteResult, IntentOperation, SirHistoryStore, SirMetaRecord, SirStateStore,
    SqliteStore, SymbolCatalogStore, SymbolEmbeddingRecord, SymbolRelationStore, TestIntentStore,
    VectorEmbeddingMetaRecord, VectorStore, WriteIntent, WriteIntentStatus, open_graph_store,
    open_surreal_graph_store_sync, open_vector_store,
};
use anyhow::{Context, Result, anyhow};
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub(crate) use self::infer::build_job;
use self::infer::{GeneratedSir, SirGenerationOutcome, SirJob, generate_sir_jobs};
pub(crate) use self::persist::UpsertSirIntentPayload;
use self::persist::{flatten_error_line, to_symbol_record, to_test_intent_record};
use self::rollup::{
    FileLeafSir, aggregate_file_sir, concatenate_file_sir, file_sir_from_summary,
    summarize_file_intent_async,
};
use crate::quality::SirQualityMonitor;

mod infer;
mod persist;
mod rollup;

pub const DEFAULT_SIR_CONCURRENCY: usize = 2;
pub(crate) const SIR_STATUS_FRESH: &str = "fresh";
const SIR_STATUS_STALE: &str = "stale";
const INFERENCE_MAX_RETRIES: usize = 2;
const INFERENCE_ATTEMPT_TIMEOUT_SECS: u64 = 90;
const INFERENCE_BACKOFF_BASE_MS: u64 = 200;
const INFERENCE_BACKOFF_MAX_MS: u64 = 2_000;
const EMBED_BATCH_SIZE: usize = 100;
const BULK_SCAN_VECTOR_BATCH_SIZE: usize = 50;
pub(crate) const MAX_SYMBOL_TEXT_CHARS: usize = 10_000;
pub const SIR_GENERATION_PASS_SCAN: &str = "scan";
pub const SIR_GENERATION_PASS_TRIAGE: &str = "triage";
pub const SIR_GENERATION_PASS_DEEP: &str = "deep";
pub const SIR_GENERATION_PASS_PREMIUM: &str = "premium";
pub const SIR_GENERATION_PASS_REGENERATED: &str = "regenerated";

#[derive(Debug, Clone, Default)]
pub struct ProcessEventStats {
    pub success_count: usize,
    pub failure_count: usize,
}

#[derive(Debug, Clone)]
pub struct SirPromptOverride {
    pub prompt: String,
    pub deep_mode: bool,
}

#[derive(Debug, Clone)]
pub struct SirDeepPromptSpec {
    pub enrichment: SirEnrichmentContext,
    pub use_cot: bool,
}

/// A pre-built quality pass candidate ready for batched inference.
#[derive(Debug, Clone)]
pub struct QualityBatchItem {
    pub symbol: Symbol,
    pub priority_score: f64,
    pub enrichment: SirEnrichmentContext,
    pub use_cot: bool,
}

/// Returned by `check_embedding_needed` when a symbol requires a new embedding.
#[derive(Debug, Clone)]
pub(crate) struct EmbeddingNeeded {
    pub provider: String,
    pub model: String,
}

/// Input data for batch embedding record construction.
#[derive(Debug, Clone)]
pub(crate) struct EmbeddingInput {
    pub symbol_id: String,
    pub sir_hash: String,
    pub canonical_json: String,
    pub provider: String,
    pub model: String,
}

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
    inference_timeout_secs: u64,
    quality_monitor: Mutex<SirQualityMonitor>,
    tiered_parse_fallback_provider: Option<Arc<dyn InferenceProvider>>,
    tiered_parse_fallback_model: Option<String>,
    skip_surreal_sync: bool,
    skip_local_edges: bool,
    contracts_config: Option<ContractsConfig>,
}

struct PreparedCandidateJobs {
    jobs: Vec<SirJob>,
    skipped_existing: usize,
}

#[derive(Debug, Clone)]
struct PersistedSuccessfulGeneration {
    intent_id: String,
    symbol_id: String,
    file_path: String,
    sir_hash: String,
    canonical_json: String,
    provider_name: String,
    embedding_needed: Option<EmbeddingNeeded>,
}

#[derive(Debug, Clone)]
struct PendingBulkEmbedding {
    persisted: PersistedSuccessfulGeneration,
    input: EmbeddingInput,
}

#[derive(Debug, Clone)]
struct BufferedEmbeddingWrite {
    record: SymbolEmbeddingRecord,
    persisted: PersistedSuccessfulGeneration,
}

#[derive(Debug, Clone)]
struct RollupJob {
    file_path: String,
    language: Language,
    leaf_sirs: Vec<FileLeafSir>,
}

#[derive(Debug)]
struct CompletedRollup {
    file_path: String,
    language: Language,
    file_sir: FileSir,
}

impl SirPipeline {
    pub(crate) fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn new(
        workspace_root: PathBuf,
        sir_concurrency: usize,
        provider_overrides: ProviderOverrides,
    ) -> Result<Self> {
        let parse_fallback =
            resolve_tiered_parse_fallback_provider(workspace_root.as_path(), &provider_overrides)?;
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

        let (tiered_parse_fallback_provider, tiered_parse_fallback_model) =
            if let Some((provider, model)) = parse_fallback {
                (Some(provider), Some(model))
            } else {
                (None, None)
            };

        Self::new_with_provider_and_embeddings(
            workspace_root,
            sir_concurrency,
            provider,
            loaded.provider_name,
            loaded.model_name,
            embedding_provider,
            embedding_identity,
            tiered_parse_fallback_provider,
            tiered_parse_fallback_model,
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
            None,
            None,
        )
    }

    pub fn new_embeddings_only(workspace_root: PathBuf) -> Result<Self> {
        let loaded_embedding = load_embedding_provider_from_config(
            &workspace_root,
            EmbeddingProviderOverrides::default(),
        )
        .context("failed to load embedding provider")?
        .ok_or_else(|| {
            anyhow!("Embedding provider is not configured. Set [embeddings] in config.")
        })?;
        let embedding_provider = Arc::<dyn EmbeddingProvider>::from(loaded_embedding.provider);
        let embedding_identity =
            Some((loaded_embedding.provider_name, loaded_embedding.model_name));
        let placeholder_provider = Qwen3LocalProvider::new(None, None);
        let placeholder_provider_name = placeholder_provider.provider_name();
        let placeholder_model_name = placeholder_provider.model_name();

        Self::new_with_provider_and_embeddings(
            workspace_root,
            1,
            Arc::new(placeholder_provider),
            placeholder_provider_name,
            placeholder_model_name,
            Some(embedding_provider),
            embedding_identity,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_provider_and_embeddings(
        workspace_root: PathBuf,
        sir_concurrency: usize,
        provider: Arc<dyn InferenceProvider>,
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
        embedding_identity: Option<(String, String)>,
        tiered_parse_fallback_provider: Option<Arc<dyn InferenceProvider>>,
        tiered_parse_fallback_model: Option<String>,
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
        let contracts_config = load_workspace_config(&workspace_root)
            .ok()
            .and_then(|c| c.contracts);

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
            inference_timeout_secs: INFERENCE_ATTEMPT_TIMEOUT_SECS,
            quality_monitor: Mutex::new(SirQualityMonitor::new(
                SIR_QUALITY_FLOOR_WINDOW,
                SIR_QUALITY_FLOOR_CONFIDENCE,
            )),
            tiered_parse_fallback_provider,
            tiered_parse_fallback_model,
            skip_surreal_sync: false,
            skip_local_edges: false,
            contracts_config,
        })
    }

    pub fn with_skip_surreal_sync(mut self, skip_surreal_sync: bool) -> Self {
        self.skip_surreal_sync = skip_surreal_sync;
        self
    }

    pub fn with_skip_local_edges(mut self, skip_local_edges: bool) -> Self {
        self.skip_local_edges = skip_local_edges;
        self
    }

    pub fn with_skip_graph_sync(mut self, skip_graph_sync: bool) -> Self {
        self.skip_surreal_sync = skip_graph_sync;
        self.skip_local_edges = skip_graph_sync;
        self
    }

    pub fn process_event(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
        force: bool,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        let _ = self.process_event_with_priority_and_pass(
            store,
            event,
            force,
            print_sir,
            out,
            None,
            SIR_GENERATION_PASS_SCAN,
        )?;
        Ok(())
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
        let _ = self.process_event_with_priority_and_pass(
            store,
            event,
            force,
            print_sir,
            out,
            priority_score,
            SIR_GENERATION_PASS_SCAN,
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process_event_with_priority_and_pass(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
        force: bool,
        print_sir: bool,
        out: &mut dyn Write,
        priority_score: Option<f64>,
        generation_pass: &str,
    ) -> Result<ProcessEventStats> {
        self.process_event_with_priority_and_pass_and_overrides(
            store,
            event,
            force,
            print_sir,
            out,
            priority_score,
            generation_pass,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process_event_with_priority_and_pass_and_overrides(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
        force: bool,
        print_sir: bool,
        out: &mut dyn Write,
        priority_score: Option<f64>,
        generation_pass: &str,
        prompt_overrides: Option<&HashMap<String, SirPromptOverride>>,
    ) -> Result<ProcessEventStats> {
        self.process_removed_symbols(store, event)?;
        if !self.skip_local_edges {
            self.replace_edges_for_file(store, event)?;
        }

        let changed_symbols = self.collect_changed_symbols(event);
        let commit_hash = resolve_workspace_head_commit(&self.workspace_root);
        tracing::info!(
            file_path = %event.file_path,
            added = event.added.len(),
            updated = event.updated.len(),
            removed = event.removed.len(),
            "processing symbol change event"
        );

        let mut intents_ready_for_graph = Vec::new();
        let mut stats = ProcessEventStats::default();

        if !changed_symbols.is_empty() {
            self.upsert_changed_symbols(store, &changed_symbols)?;

            let prepared = self.prepare_candidate_jobs(
                store,
                event.file_path.as_str(),
                changed_symbols,
                force,
                print_sir,
                out,
                priority_score,
                prompt_overrides,
            )?;
            self.log_prepared_jobs(event.file_path.as_str(), &prepared);

            tracing::info!(
                job_count = prepared.jobs.len(),
                provider = %self.provider_name,
                model = %self.model_name,
                force,
                "submitting SIR generation jobs"
            );
            let results = self.runtime.block_on(generate_sir_jobs(
                self.provider.clone(),
                self.tiered_parse_fallback_provider.clone(),
                self.tiered_parse_fallback_model.clone(),
                prepared.jobs,
                self.sir_concurrency,
                self.inference_timeout_secs,
            ))?;

            for result in results {
                match result {
                    SirGenerationOutcome::Success(generated) => {
                        match self.commit_successful_generation(
                            store,
                            *generated,
                            generation_pass,
                            commit_hash.as_deref(),
                            print_sir,
                            out,
                        )? {
                            Some(intent_id) => {
                                intents_ready_for_graph.push(intent_id);
                                stats.success_count += 1;
                            }
                            None => stats.failure_count += 1,
                        }
                    }
                    SirGenerationOutcome::Failure(failed) => {
                        stats.failure_count += 1;
                        self.handle_failed_generation(
                            store,
                            *failed,
                            generation_pass,
                            print_sir,
                            out,
                        )?;
                    }
                }
            }

            if self.skip_surreal_sync {
                self.complete_graph_stage_without_sync(store, &mut intents_ready_for_graph);
            } else {
                self.finalize_graph_stage(
                    store,
                    event.file_path.as_str(),
                    &mut intents_ready_for_graph,
                );
            }
            self.log_processing_summary(event.file_path.as_str(), &stats);
        } else if !self.skip_surreal_sync
            && let Err(err) = self.sync_graph_for_file(store, &event.file_path)
        {
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
            generation_pass,
        )?;

        Ok(stats)
    }

    pub fn process_quality_batch(
        &self,
        store: &SqliteStore,
        items: Vec<QualityBatchItem>,
        generation_pass: &str,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<ProcessEventStats> {
        let commit_hash = resolve_workspace_head_commit(&self.workspace_root);
        let mut touched_files = BTreeMap::<String, Language>::new();
        let mut intents_by_file = BTreeMap::<String, Vec<String>>::new();
        let mut jobs = Vec::with_capacity(items.len());
        let mut pending_embeddings = Vec::new();
        let mut stats = ProcessEventStats::default();

        for item in items {
            let symbol_id = item.symbol.id.clone();
            let qualified_name = item.symbol.qualified_name.clone();
            let file_path = item.symbol.file_path.clone();
            let language = item.symbol.language;
            touched_files.entry(file_path.clone()).or_insert(language);

            match build_job(
                &self.workspace_root,
                item.symbol,
                Some(item.priority_score),
                None,
            ) {
                Ok(mut job) => {
                    let prompt = if item.use_cot {
                        sir_prompt::build_enriched_sir_prompt_with_cot(
                            &job.symbol_text,
                            &job.context,
                            &item.enrichment,
                        )
                    } else {
                        sir_prompt::build_enriched_sir_prompt(
                            &job.symbol_text,
                            &job.context,
                            &item.enrichment,
                        )
                    };
                    job.custom_prompt = Some(prompt);
                    job.deep_mode = item.use_cot;
                    jobs.push(job);
                }
                Err(err) => {
                    stats.failure_count += 1;
                    tracing::warn!(
                        symbol_id = %symbol_id,
                        qualified_name = %qualified_name,
                        file_path = %file_path,
                        error = %err,
                        "failed to build batched quality SIR job; skipping symbol"
                    );
                }
            }
        }

        tracing::info!(
            job_count = jobs.len(),
            file_count = touched_files.len(),
            provider = %self.provider_name,
            model = %self.model_name,
            "submitting batched quality SIR generation jobs"
        );

        let results = if jobs.is_empty() {
            Vec::new()
        } else {
            self.runtime
                .block_on(generate_sir_jobs(
                    self.provider.clone(),
                    self.tiered_parse_fallback_provider.clone(),
                    self.tiered_parse_fallback_model.clone(),
                    jobs,
                    self.sir_concurrency,
                    self.inference_timeout_secs,
                ))
                .context("failed to submit batched quality SIR generation jobs")?
        };

        for result in results {
            match result {
                SirGenerationOutcome::Success(generated) => {
                    let Some(persisted) = self
                        .persist_successful_generation_sqlite(
                            store,
                            &generated,
                            generation_pass,
                            commit_hash.as_deref(),
                        )
                        .with_context(|| {
                            format!(
                                "failed to persist quality-batch SIR result for {}",
                                generated.symbol.id
                            )
                        })?
                    else {
                        stats.failure_count += 1;
                        continue;
                    };

                    let symbol_id = persisted.symbol_id.clone();
                    self.enqueue_or_finish_persisted_generation(
                        store,
                        persisted,
                        &mut pending_embeddings,
                        &mut intents_by_file,
                        &mut stats,
                        print_sir,
                        out,
                    )
                    .with_context(|| {
                        format!("failed to stage quality-batch vector work for {symbol_id}")
                    })?;
                }
                SirGenerationOutcome::Failure(failed) => {
                    stats.failure_count += 1;
                    self.handle_failed_generation(store, *failed, generation_pass, print_sir, out)?;
                }
            }
        }

        let (embedded_symbols, embedding_calls) = self
            .process_pending_embeddings(
                store,
                &pending_embeddings,
                &mut intents_by_file,
                &mut stats,
                print_sir,
                out,
                "quality batch",
            )
            .context("failed to process quality-batch embedding batches")?;

        tracing::info!(
            embedded = embedded_symbols,
            batch_calls = embedding_calls,
            "Embedded quality batch symbols"
        );

        if self.skip_surreal_sync {
            let batch_result = self.batch_complete_graph_stage_without_sync(store, intents_by_file);
            stats.failure_count += batch_result.failed;
        } else {
            for (file_path, mut intent_ids) in intents_by_file {
                self.finalize_graph_stage(store, file_path.as_str(), &mut intent_ids);
            }
        }

        self.bulk_upsert_file_rollups(
            store,
            touched_files,
            print_sir,
            out,
            commit_hash.as_deref(),
            generation_pass,
        )
        .context("failed to upsert quality-batch file rollups")?;

        Ok(stats)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process_bulk_scan(
        &self,
        store: &SqliteStore,
        symbols: Vec<Symbol>,
        priority_scores: &HashMap<String, f64>,
        force: bool,
        generation_pass: &str,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<ProcessEventStats> {
        let commit_hash = resolve_workspace_head_commit(&self.workspace_root);
        let total_symbols = symbols.len();
        let mut touched_files = BTreeMap::<String, Language>::new();
        let mut intents_by_file = BTreeMap::<String, Vec<String>>::new();
        let mut jobs = Vec::with_capacity(total_symbols);
        let mut pending_embeddings = Vec::new();
        let mut stats = ProcessEventStats::default();
        let mut skipped_existing = 0usize;

        for symbol in symbols {
            let symbol_id = symbol.id.clone();
            let qualified_name = symbol.qualified_name.clone();
            let file_path = symbol.file_path.clone();
            let language = symbol.language;
            touched_files.entry(file_path.clone()).or_insert(language);

            if !force
                && self
                    .should_skip_sir_generation(store, &symbol)
                    .with_context(|| {
                        format!("failed to evaluate bulk-scan skip state for {}", symbol.id)
                    })?
            {
                skipped_existing += 1;
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

            let priority_score = Some(
                priority_scores
                    .get(symbol_id.as_str())
                    .copied()
                    .unwrap_or(0.0),
            );
            match build_job(&self.workspace_root, symbol, priority_score, None) {
                Ok(job) => jobs.push(job),
                Err(err) => {
                    stats.failure_count += 1;
                    tracing::warn!(
                        symbol_id = %symbol_id,
                        qualified_name = %qualified_name,
                        file_path = %file_path,
                        error = %err,
                        "failed to build bulk scan SIR job; skipping symbol"
                    );
                }
            }
        }

        tracing::info!(
            built = jobs.len(),
            total = total_symbols,
            skipped = skipped_existing,
            file_count = touched_files.len(),
            "Building SIR jobs complete"
        );
        tracing::info!(
            job_count = jobs.len(),
            concurrency = self.sir_concurrency,
            provider = %self.provider_name,
            model = %self.model_name,
            "Submitting bulk scan jobs"
        );

        let results = if jobs.is_empty() {
            Vec::new()
        } else {
            self.runtime
                .block_on(generate_sir_jobs(
                    self.provider.clone(),
                    self.tiered_parse_fallback_provider.clone(),
                    self.tiered_parse_fallback_model.clone(),
                    jobs,
                    self.sir_concurrency,
                    self.inference_timeout_secs,
                ))
                .context("failed to submit bulk scan SIR generation jobs")?
        };

        for result in results {
            match result {
                SirGenerationOutcome::Success(generated) => {
                    let Some(persisted) = self
                        .persist_successful_generation_sqlite(
                            store,
                            &generated,
                            generation_pass,
                            commit_hash.as_deref(),
                        )
                        .with_context(|| {
                            format!(
                                "failed to persist bulk-scan SIR result for {}",
                                generated.symbol.id
                            )
                        })?
                    else {
                        stats.failure_count += 1;
                        continue;
                    };

                    let symbol_id = persisted.symbol_id.clone();
                    self.enqueue_or_finish_persisted_generation(
                        store,
                        persisted,
                        &mut pending_embeddings,
                        &mut intents_by_file,
                        &mut stats,
                        print_sir,
                        out,
                    )
                    .with_context(|| {
                        format!("failed to stage bulk-scan vector work for {symbol_id}")
                    })?;
                }
                SirGenerationOutcome::Failure(failed) => {
                    stats.failure_count += 1;
                    let symbol_id = failed.symbol.id.clone();
                    self.handle_failed_generation(store, *failed, generation_pass, print_sir, out)
                        .with_context(|| {
                            format!(
                                "failed to record bulk-scan generation failure for {}",
                                symbol_id
                            )
                        })?;
                }
            }
        }

        let (embedded_symbols, embedding_calls) = self
            .process_pending_embeddings(
                store,
                &pending_embeddings,
                &mut intents_by_file,
                &mut stats,
                print_sir,
                out,
                "bulk scan",
            )
            .context("failed to process bulk-scan embedding batches")?;

        tracing::info!(
            embedded = embedded_symbols,
            batch_calls = embedding_calls,
            "Embedded bulk scan symbols"
        );

        let batch_result = self.batch_complete_graph_stage_without_sync(store, intents_by_file);
        stats.failure_count += batch_result.failed;

        self.bulk_upsert_file_rollups(
            store,
            touched_files,
            print_sir,
            out,
            commit_hash.as_deref(),
            generation_pass,
        )
        .context("failed to upsert bulk-scan file rollups")?;

        Ok(stats)
    }

    fn process_removed_symbols(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
    ) -> Result<()> {
        for symbol in &event.removed {
            store
                .mark_removed(&symbol.id)
                .with_context(|| format!("failed to mark symbol removed: {}", symbol.id))?;
            self.runtime
                .block_on(self.vector_store.delete_embedding(&symbol.id))
                .with_context(|| format!("failed to remove vector embedding for {}", symbol.id))?;
        }

        Ok(())
    }

    fn collect_changed_symbols(&self, event: &SymbolChangeEvent) -> Vec<(Symbol, bool)> {
        let mut changed_symbols: Vec<(Symbol, bool)> =
            Vec::with_capacity(event.added.len() + event.updated.len());
        changed_symbols.extend(event.added.iter().cloned().map(|symbol| (symbol, true)));
        changed_symbols.extend(event.updated.iter().cloned().map(|symbol| (symbol, false)));
        changed_symbols
    }

    fn upsert_changed_symbols(
        &self,
        store: &SqliteStore,
        changed_symbols: &[(Symbol, bool)],
    ) -> Result<()> {
        let now_ts = unix_timestamp_secs();
        for (symbol, _) in changed_symbols {
            store
                .upsert_symbol(to_symbol_record(symbol, now_ts))
                .with_context(|| format!("failed to upsert symbol {}", symbol.id))?;
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_candidate_jobs(
        &self,
        store: &SqliteStore,
        file_path: &str,
        changed_symbols: Vec<(Symbol, bool)>,
        force: bool,
        print_sir: bool,
        out: &mut dyn Write,
        priority_score: Option<f64>,
        prompt_overrides: Option<&HashMap<String, SirPromptOverride>>,
    ) -> Result<PreparedCandidateJobs> {
        let mut jobs = Vec::new();
        let mut skipped_existing = 0usize;

        for (symbol, allow_existing_skip) in changed_symbols {
            if allow_existing_skip && !force && self.should_skip_sir_generation(store, &symbol)? {
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

            match build_job(&self.workspace_root, symbol, priority_score, None) {
                Ok(mut job) => {
                    if let Some(prompt_overrides) = prompt_overrides
                        && let Some(override_spec) = prompt_overrides.get(job.symbol.id.as_str())
                    {
                        job.custom_prompt = Some(override_spec.prompt.clone());
                        job.deep_mode = override_spec.deep_mode;
                    }
                    jobs.push(job);
                }
                Err(err) => {
                    tracing::warn!(
                        file_path = %file_path,
                        error = %err,
                        "failed to build SIR job; skipping symbol"
                    );
                }
            }
        }

        Ok(PreparedCandidateJobs {
            jobs,
            skipped_existing,
        })
    }

    fn log_prepared_jobs(&self, file_path: &str, prepared: &PreparedCandidateJobs) {
        if prepared.skipped_existing > 0 {
            tracing::info!(
                file_path = %file_path,
                skipped_existing = prepared.skipped_existing,
                queued_jobs = prepared.jobs.len(),
                "Skipping SIR generation for existing symbols: already exists"
            );
        }

        if prepared.jobs.is_empty() {
            tracing::info!(file_path = %file_path, "SIR generation processed 0 jobs");
        }
    }

    fn prepare_sir_for_persistence(
        &self,
        store: &SqliteStore,
        symbol: &Symbol,
        sir: &SirAnnotation,
    ) -> Result<(SirAnnotation, String, String)> {
        let mut sir = sir.clone();
        self.inject_method_dependencies(store, symbol, &mut sir)?;
        let canonical_json = canonicalize_sir_json(&sir);
        let sir_hash_value = sir_hash(&sir);
        Ok((sir, canonical_json, sir_hash_value))
    }

    fn inject_method_dependencies(
        &self,
        store: &SqliteStore,
        symbol: &Symbol,
        sir: &mut SirAnnotation,
    ) -> Result<()> {
        if !matches!(
            symbol.kind.as_str(),
            "trait" | "struct" | "enum" | "type_alias"
        ) {
            sir.method_dependencies = None;
            return Ok(());
        }

        let prefix = format!("{}::", symbol.qualified_name);
        let mut method_dependencies = BTreeMap::new();

        for (child, edge) in store.list_method_dependency_edges_for_type(
            symbol.qualified_name.as_str(),
            &[EdgeKind::Calls, EdgeKind::TypeRef],
        )? {
            let Some(method_name) = child.qualified_name.strip_prefix(prefix.as_str()) else {
                continue;
            };

            let dependency = edge
                .target_qualified_name
                .rsplit("::")
                .next()
                .unwrap_or(edge.target_qualified_name.as_str())
                .trim_start_matches("r#")
                .trim()
                .to_owned();

            if !dependency.is_empty() {
                method_dependencies
                    .entry(method_name.to_owned())
                    .or_insert_with(Vec::new)
                    .push(dependency);
            }
        }

        // Trait method declarations are indexed without the trait:: prefix.
        // Fall back to same-file implementor methods when the direct prefix lookup is empty.
        if method_dependencies.is_empty() && symbol.kind.as_str() == "trait" {
            let mut candidates = store
                .list_symbols_for_file(symbol.file_path.as_str())?
                .into_iter()
                .filter(|candidate| {
                    candidate.qualified_name != symbol.qualified_name
                        && matches!(candidate.kind.as_str(), "struct" | "enum")
                })
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| {
                left.qualified_name
                    .cmp(&right.qualified_name)
                    .then(left.id.cmp(&right.id))
            });

            for candidate in candidates {
                let implementor_edges = store.list_method_dependency_edges_for_type(
                    candidate.qualified_name.as_str(),
                    &[EdgeKind::Calls, EdgeKind::TypeRef],
                )?;
                if implementor_edges.is_empty() {
                    continue;
                }

                let implementor_prefix = format!("{}::", candidate.qualified_name);
                for (child, edge) in implementor_edges {
                    let Some(method_name) = child
                        .qualified_name
                        .strip_prefix(implementor_prefix.as_str())
                    else {
                        continue;
                    };

                    let dependency = edge
                        .target_qualified_name
                        .rsplit("::")
                        .next()
                        .unwrap_or(edge.target_qualified_name.as_str())
                        .trim_start_matches("r#")
                        .trim()
                        .to_owned();

                    if !dependency.is_empty() {
                        method_dependencies
                            .entry(method_name.to_owned())
                            .or_insert_with(Vec::new)
                            .push(dependency);
                    }
                }

                if !method_dependencies.is_empty() {
                    tracing::debug!(
                        trait_name = %symbol.qualified_name,
                        implementor = %candidate.qualified_name,
                        method_count = method_dependencies.len(),
                        "used implementor fallback for trait method_dependencies"
                    );
                    break;
                }
            }
        }

        let method_dependencies = method_dependencies
            .into_iter()
            .filter_map(|(method_name, mut dependencies)| {
                dependencies.sort();
                dependencies.dedup();
                if dependencies.is_empty() {
                    None
                } else {
                    Some((method_name, dependencies))
                }
            })
            .collect::<HashMap<_, _>>();

        if method_dependencies.is_empty() {
            sir.method_dependencies = None;
            return Ok(());
        }

        let mut dependencies = sir.dependencies.clone();
        dependencies.extend(
            method_dependencies
                .values()
                .flatten()
                .cloned()
                .collect::<Vec<_>>(),
        );
        dependencies.sort();
        dependencies.dedup();

        sir.dependencies = dependencies;
        sir.method_dependencies = Some(method_dependencies);
        Ok(())
    }

    fn record_generation_quality(&self, confidence: f32) {
        match self.quality_monitor.lock() {
            Ok(mut monitor) => {
                monitor.record(confidence);
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to lock SIR quality monitor");
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_successful_generation(
        &self,
        store: &SqliteStore,
        generated: GeneratedSir,
        generation_pass: &str,
        commit_hash: Option<&str>,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<Option<String>> {
        let Some(persisted) = self.persist_successful_generation_sqlite(
            store,
            &generated,
            generation_pass,
            commit_hash,
        )?
        else {
            return Ok(None);
        };

        if let Err(err) = self.refresh_embedding_if_needed(
            &generated.symbol.id,
            &persisted.sir_hash,
            &persisted.canonical_json,
            print_sir,
            out,
            None,
        ) {
            let message = format!("{err:#}");
            self.mark_intent_failed_safely(store, persisted.intent_id.as_str(), message.as_str());
            tracing::error!(
                symbol_id = %generated.symbol.id,
                error = %err,
                "embedding refresh error"
            );
            return Ok(None);
        }

        if let Err(err) =
            store.update_intent_status(&persisted.intent_id, WriteIntentStatus::VectorDone)
        {
            let message = format!("{err:#}");
            self.mark_intent_failed_safely(store, persisted.intent_id.as_str(), message.as_str());
            tracing::error!(
                symbol_id = %generated.symbol.id,
                error = %err,
                "failed to update write intent status to vector_done"
            );
            return Ok(None);
        }

        if print_sir {
            writeln!(
                out,
                "SIR_STORED symbol_id={} sir_hash={} provider={}",
                generated.symbol.id, persisted.sir_hash, generated.provider_name
            )
            .context("failed to write SIR print line")?;
        }

        tracing::debug!(
            symbol_id = %generated.symbol.id,
            "SIR generated successfully"
        );

        Ok(Some(persisted.intent_id))
    }

    fn persist_successful_generation_sqlite(
        &self,
        store: &SqliteStore,
        generated: &GeneratedSir,
        generation_pass: &str,
        commit_hash: Option<&str>,
    ) -> Result<Option<PersistedSuccessfulGeneration>> {
        self.record_generation_quality(generated.sir.confidence);

        let (sir, canonical_json, sir_hash_value) =
            match self.prepare_sir_for_persistence(store, &generated.symbol, &generated.sir) {
                Ok(prepared) => prepared,
                Err(err) => {
                    tracing::error!(
                        symbol_id = %generated.symbol.id,
                        error = %err,
                        "failed to prepare SIR for persistence"
                    );
                    return Ok(None);
                }
            };

        let payload = UpsertSirIntentPayload {
            symbol: generated.symbol.clone(),
            sir,
            provider_name: generated.provider_name.clone(),
            model_name: generated.model_name.clone(),
            generation_pass: generation_pass.to_owned(),
            reasoning_trace: generated.reasoning_trace.clone(),
            commit_hash: commit_hash.map(str::to_owned),
        };
        let payload_json = match payload.to_json_string() {
            Ok(json) => json,
            Err(err) => {
                tracing::error!(
                    symbol_id = %generated.symbol.id,
                    error = %err,
                    "failed to serialize write intent payload"
                );
                return Ok(None);
            }
        };

        let intent = WriteIntent {
            intent_id: content_hash(
                format!("{}\n{}", generated.symbol.id, unix_timestamp_millis()).as_str(),
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
            tracing::error!(
                symbol_id = %generated.symbol.id,
                error = %err,
                "failed to create write intent; skipping symbol write"
            );
            return Ok(None);
        }

        let attempted_at = unix_timestamp_secs();
        let meta = SirMetaRecord {
            id: generated.symbol.id.clone(),
            sir_hash: sir_hash_value.clone(),
            sir_version: 1,
            provider: generated.provider_name.clone(),
            model: generated.model_name.clone(),
            generation_pass: generation_pass.to_owned(),
            reasoning_trace: generated.reasoning_trace.clone(),
            prompt_hash: None,
            staleness_score: None,
            updated_at: attempted_at,
            sir_status: SIR_STATUS_FRESH.to_owned(),
            last_error: None,
            last_attempt_at: attempted_at,
        };
        if let Err(err) = store.persist_sir_state_atomically(
            meta,
            &canonical_json,
            payload.commit_hash.as_deref(),
            Some(intent.intent_id.as_str()),
        ) {
            let message = format!("{err:#}");
            self.mark_intent_failed_safely(store, intent.intent_id.as_str(), message.as_str());
            tracing::error!(
                symbol_id = %generated.symbol.id,
                error = %err,
                "failed to persist sqlite SIR state"
            );
            return Ok(None);
        }

        let embedding_needed =
            match self.check_embedding_needed(&generated.symbol.id, &sir_hash_value, None) {
                Ok(needed) => needed,
                Err(err) => {
                    let message = format!("{err:#}");
                    self.mark_intent_failed_safely(
                        store,
                        intent.intent_id.as_str(),
                        message.as_str(),
                    );
                    tracing::error!(
                        symbol_id = %generated.symbol.id,
                        error = %err,
                        "failed to determine whether embedding refresh is needed"
                    );
                    return Ok(None);
                }
            };

        Ok(Some(PersistedSuccessfulGeneration {
            intent_id: intent.intent_id,
            symbol_id: generated.symbol.id.clone(),
            file_path: generated.symbol.file_path.clone(),
            sir_hash: sir_hash_value,
            canonical_json,
            provider_name: generated.provider_name.clone(),
            embedding_needed,
        }))
    }

    fn handle_failed_generation(
        &self,
        store: &SqliteStore,
        failed: infer::FailedSirGeneration,
        generation_pass: &str,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        let last_attempt_at = unix_timestamp_secs();
        let previous_meta = store
            .get_sir_meta(&failed.symbol.id)
            .with_context(|| format!("failed to load SIR metadata for {}", failed.symbol.id))?;

        let stale_meta = previous_meta.map_or_else(
            || SirMetaRecord {
                id: failed.symbol.id.clone(),
                sir_hash: String::new(),
                sir_version: 1,
                provider: self.provider_name.clone(),
                model: self.model_name.clone(),
                generation_pass: generation_pass.to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
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
                generation_pass: if record.generation_pass.trim().is_empty() {
                    generation_pass.to_owned()
                } else {
                    record.generation_pass
                },
                reasoning_trace: record.reasoning_trace,
                prompt_hash: record.prompt_hash,
                staleness_score: record.staleness_score,
                updated_at: record.updated_at,
                sir_status: SIR_STATUS_STALE.to_owned(),
                last_error: Some(failed.error_message.clone()),
                last_attempt_at,
            },
        );

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

        Ok(())
    }

    fn finalize_graph_stage(
        &self,
        store: &SqliteStore,
        file_path: &str,
        intents_ready_for_graph: &mut Vec<String>,
    ) {
        match self.sync_graph_for_file(store, file_path) {
            Ok(()) => {
                for intent_id in intents_ready_for_graph.drain(..) {
                    if let Err(err) =
                        store.update_intent_status(&intent_id, WriteIntentStatus::GraphDone)
                    {
                        let message = format!("graph_done update failed: {err:#}");
                        tracing::error!(
                            intent_id = %intent_id,
                            error = %err,
                            "failed to update write intent status to graph_done"
                        );
                        self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
                        continue;
                    }
                    if let Err(err) = store.mark_intent_complete(&intent_id) {
                        let message = format!("intent completion failed: {err:#}");
                        tracing::error!(
                            intent_id = %intent_id,
                            error = %err,
                            "failed to mark write intent complete"
                        );
                        self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
                    }
                }
            }
            Err(err) => {
                let error_text = format!("{err:#}");
                for intent_id in intents_ready_for_graph.drain(..) {
                    self.mark_intent_failed_safely(store, intent_id.as_str(), error_text.as_str());
                }
                tracing::warn!(
                    file_path = %file_path,
                    error = %error_text,
                    "graph sync failed after vector stage"
                );
            }
        }
    }

    fn complete_graph_stage_without_sync(
        &self,
        store: &SqliteStore,
        intents_ready_for_graph: &mut Vec<String>,
    ) {
        for intent_id in intents_ready_for_graph.drain(..) {
            if let Err(err) = store.update_intent_status(&intent_id, WriteIntentStatus::GraphDone) {
                let message = format!("graph_done update failed: {err:#}");
                tracing::error!(
                    intent_id = %intent_id,
                    error = %err,
                    "failed to update write intent status to graph_done"
                );
                self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
                continue;
            }
            if let Err(err) = store.mark_intent_complete(&intent_id) {
                let message = format!("intent completion failed: {err:#}");
                tracing::error!(
                    intent_id = %intent_id,
                    error = %err,
                    "failed to mark write intent complete"
                );
                self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
            }
        }
    }

    fn batch_complete_graph_stage_without_sync(
        &self,
        store: &SqliteStore,
        intents_by_file: BTreeMap<String, Vec<String>>,
    ) -> BatchCompleteResult {
        let intent_ids = intents_by_file.into_values().flatten().collect::<Vec<_>>();

        if intent_ids.is_empty() {
            return BatchCompleteResult::default();
        }

        match store.batch_complete_intents(&intent_ids) {
            Ok(result) => {
                tracing::info!(
                    intent_count = intent_ids.len(),
                    completed = result.completed,
                    failed = result.failed,
                    "completed batched graph stage without sync"
                );
                result
            }
            Err(err) => {
                let message = format!("batched graph completion failed: {err:#}");
                for intent_id in &intent_ids {
                    self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
                }
                tracing::error!(
                    intent_count = intent_ids.len(),
                    error = %err,
                    "failed to complete batched graph stage without sync"
                );
                BatchCompleteResult {
                    completed: 0,
                    failed: intent_ids.len(),
                }
            }
        }
    }

    fn log_processing_summary(&self, file_path: &str, stats: &ProcessEventStats) {
        if stats.failure_count > 0 {
            tracing::warn!(
                file_path = %file_path,
                successes = stats.success_count,
                failures = stats.failure_count,
                "SIR processing complete with failures"
            );
        } else if stats.success_count > 0 {
            tracing::info!(
                file_path = %file_path,
                successes = stats.success_count,
                "SIR processing complete"
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn enqueue_or_finish_persisted_generation(
        &self,
        store: &SqliteStore,
        persisted: PersistedSuccessfulGeneration,
        pending_embeddings: &mut Vec<PendingBulkEmbedding>,
        intents_by_file: &mut BTreeMap<String, Vec<String>>,
        stats: &mut ProcessEventStats,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        if let Some((provider, model)) = persisted
            .embedding_needed
            .as_ref()
            .map(|needed| (needed.provider.clone(), needed.model.clone()))
        {
            pending_embeddings.push(PendingBulkEmbedding {
                input: EmbeddingInput {
                    symbol_id: persisted.symbol_id.clone(),
                    sir_hash: persisted.sir_hash.clone(),
                    canonical_json: persisted.canonical_json.clone(),
                    provider,
                    model,
                },
                persisted,
            });
            return Ok(());
        }

        let symbol_id = persisted.symbol_id.clone();
        let _ = self
            .finish_bulk_scan_success(
                store,
                persisted,
                intents_by_file,
                stats,
                print_sir,
                out,
                None,
            )
            .with_context(|| {
                format!("failed to finalize immediate vector stage for {symbol_id}")
            })?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn process_pending_embeddings(
        &self,
        store: &SqliteStore,
        pending_embeddings: &[PendingBulkEmbedding],
        intents_by_file: &mut BTreeMap<String, Vec<String>>,
        stats: &mut ProcessEventStats,
        print_sir: bool,
        out: &mut dyn Write,
        phase_name: &str,
    ) -> Result<(usize, usize)> {
        let mut embedding_buffer = Vec::with_capacity(BULK_SCAN_VECTOR_BATCH_SIZE);
        let mut embedded_symbols = 0usize;
        let mut embedding_calls = 0usize;

        for chunk in pending_embeddings.chunks(EMBED_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            embedding_calls += 1;
            let texts = chunk
                .iter()
                .map(|item| item.input.canonical_json.as_str())
                .collect::<Vec<_>>();
            let embeddings = match self.batch_embed_texts(&texts, EmbeddingPurpose::Document) {
                Ok(embeddings) => embeddings,
                Err(err) => {
                    let message = format!("{err:#}");
                    tracing::error!(
                        phase = phase_name,
                        error = %err,
                        chunk_size = chunk.len(),
                        "batch embedding failed during SIR pipeline"
                    );
                    for item in chunk {
                        self.mark_intent_failed_safely(
                            store,
                            item.persisted.intent_id.as_str(),
                            message.as_str(),
                        );
                        stats.failure_count += 1;
                    }
                    continue;
                }
            };

            let mut records_by_symbol = SirPipeline::build_embedding_records(
                &chunk
                    .iter()
                    .map(|item| item.input.clone())
                    .collect::<Vec<_>>(),
                embeddings,
            )
            .into_iter()
            .map(|record| (record.symbol_id.clone(), record))
            .collect::<HashMap<_, _>>();

            embedded_symbols += records_by_symbol.len();

            for item in chunk {
                let persisted = PersistedSuccessfulGeneration {
                    embedding_needed: None,
                    ..item.persisted.clone()
                };
                if let Some(record) = records_by_symbol.remove(item.persisted.symbol_id.as_str()) {
                    embedding_buffer.push(BufferedEmbeddingWrite { record, persisted });
                } else {
                    let symbol_id = persisted.symbol_id.clone();
                    self.finish_bulk_scan_success(
                        store,
                        persisted,
                        intents_by_file,
                        stats,
                        print_sir,
                        out,
                        None,
                    )
                    .with_context(|| {
                        format!("failed to finalize {phase_name} vector stage for {symbol_id}")
                    })?;
                }
            }

            if embedding_buffer.len() >= BULK_SCAN_VECTOR_BATCH_SIZE {
                self.flush_bulk_scan_embedding_buffer(
                    store,
                    &mut embedding_buffer,
                    intents_by_file,
                    stats,
                    print_sir,
                    out,
                )
                .with_context(|| format!("failed to flush buffered {phase_name} embeddings"))?;
            }
        }

        self.flush_bulk_scan_embedding_buffer(
            store,
            &mut embedding_buffer,
            intents_by_file,
            stats,
            print_sir,
            out,
        )
        .with_context(|| format!("failed to flush remaining {phase_name} embeddings"))?;

        Ok((embedded_symbols, embedding_calls))
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_bulk_scan_success(
        &self,
        store: &SqliteStore,
        persisted: PersistedSuccessfulGeneration,
        intents_by_file: &mut BTreeMap<String, Vec<String>>,
        stats: &mut ProcessEventStats,
        print_sir: bool,
        out: &mut dyn Write,
        embedding_record: Option<&SymbolEmbeddingRecord>,
    ) -> Result<bool> {
        if let Err(err) =
            store.update_intent_status(&persisted.intent_id, WriteIntentStatus::VectorDone)
        {
            let message = format!("{err:#}");
            self.mark_intent_failed_safely(store, persisted.intent_id.as_str(), message.as_str());
            tracing::error!(
                symbol_id = %persisted.symbol_id,
                error = %err,
                "failed to update write intent status to vector_done"
            );
            stats.failure_count += 1;
            return Ok(false);
        }

        if print_sir && let Some(record) = embedding_record {
            writeln!(
                out,
                "EMBEDDING_STORED symbol_id={} provider={} model={}",
                record.symbol_id, record.provider, record.model
            )
            .context("failed to write embedding print line")?;
        }

        if print_sir {
            writeln!(
                out,
                "SIR_STORED symbol_id={} sir_hash={} provider={}",
                persisted.symbol_id, persisted.sir_hash, persisted.provider_name
            )
            .context("failed to write SIR print line")?;
        }

        intents_by_file
            .entry(persisted.file_path)
            .or_default()
            .push(persisted.intent_id);
        stats.success_count += 1;
        Ok(true)
    }

    fn flush_bulk_scan_embedding_buffer(
        &self,
        store: &SqliteStore,
        buffer: &mut Vec<BufferedEmbeddingWrite>,
        intents_by_file: &mut BTreeMap<String, Vec<String>>,
        stats: &mut ProcessEventStats,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        if buffer.is_empty() {
            return Ok(());
        }

        let pending = std::mem::take(buffer);
        let records = pending
            .iter()
            .map(|item| item.record.clone())
            .collect::<Vec<_>>();
        if let Err(err) = self.flush_embedding_batch(records) {
            let message = format!("{err:#}");
            tracing::error!(
                error = %err,
                record_count = pending.len(),
                "failed to flush embedding batch"
            );
            for item in pending {
                self.mark_intent_failed_safely(
                    store,
                    item.persisted.intent_id.as_str(),
                    message.as_str(),
                );
                stats.failure_count += 1;
            }
            return Ok(());
        }

        for item in pending {
            let _ = self.finish_bulk_scan_success(
                store,
                item.persisted,
                intents_by_file,
                stats,
                print_sir,
                out,
                Some(&item.record),
            )?;
        }

        Ok(())
    }

    fn mark_intent_failed_safely(&self, store: &SqliteStore, intent_id: &str, message: &str) {
        if let Err(mark_err) = store.mark_intent_failed(intent_id, message) {
            tracing::error!(
                intent_id = %intent_id,
                error = %mark_err,
                "failed to mark write intent as failed"
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process_event_with_deep_specs(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
        force: bool,
        print_sir: bool,
        out: &mut dyn Write,
        priority_score: Option<f64>,
        generation_pass: &str,
        deep_specs: &HashMap<String, SirDeepPromptSpec>,
    ) -> Result<ProcessEventStats> {
        let mut prompt_overrides = HashMap::new();

        for symbol in event.added.iter().chain(event.updated.iter()) {
            let Some(spec) = deep_specs.get(symbol.id.as_str()) else {
                continue;
            };
            let job = build_job(&self.workspace_root, symbol.clone(), priority_score, None)
                .with_context(|| {
                    format!("failed to build deep SIR job for {}", symbol.qualified_name)
                })?;
            let prompt = if spec.use_cot {
                sir_prompt::build_enriched_sir_prompt_with_cot(
                    &job.symbol_text,
                    &job.context,
                    &spec.enrichment,
                )
            } else {
                sir_prompt::build_enriched_sir_prompt(
                    &job.symbol_text,
                    &job.context,
                    &spec.enrichment,
                )
            };
            prompt_overrides.insert(
                symbol.id.clone(),
                SirPromptOverride {
                    prompt,
                    deep_mode: spec.use_cot,
                },
            );
        }

        self.process_event_with_priority_and_pass_and_overrides(
            store,
            event,
            force,
            print_sir,
            out,
            priority_score,
            generation_pass,
            Some(&prompt_overrides),
        )
    }

    pub fn provider_name(&self) -> &str {
        self.provider_name.as_str()
    }

    pub fn model_name(&self) -> &str {
        self.model_name.as_str()
    }

    pub(crate) fn embedding_identity(&self) -> Option<(&str, &str)> {
        self.embedding_provider.as_ref()?;

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

        Some((provider_name, model_name))
    }

    pub(crate) fn load_symbol_embedding(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SymbolEmbeddingRecord>> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(None);
        }

        let Some(meta) = self
            .runtime
            .block_on(self.vector_store.get_embedding_meta(symbol_id))
            .with_context(|| format!("failed to read embedding metadata for {symbol_id}"))?
        else {
            return Ok(None);
        };

        let records = self
            .runtime
            .block_on(self.vector_store.list_embeddings_for_symbols(
                meta.provider.as_str(),
                meta.model.as_str(),
                &[symbol_id.to_owned()],
            ))
            .with_context(|| format!("failed to read embedding vector for {symbol_id}"))?;

        Ok(records
            .into_iter()
            .find(|record| record.symbol_id == symbol_id))
    }

    pub fn with_inference_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.inference_timeout_secs = timeout_secs.max(1);
        self
    }

    pub fn run_embeddings_only_pass(
        &self,
        store: &SqliteStore,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        let symbol_ids = store
            .list_all_symbol_ids()
            .context("failed to list symbols for embeddings-only pass")?;
        let processed = symbol_ids.len();
        let mut refreshed = 0usize;
        let mut skipped_no_sir = 0usize;
        let mut skipped_up_to_date = 0usize;
        let mut errors = 0usize;
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

        writeln!(
            out,
            "Re-embedding {processed} symbols with {provider_name}/{model_name}..."
        )
        .context("failed to write embeddings-only start line")?;

        // Pre-fetch embedding metadata to avoid N+1 vector store round trips.
        let existing_metas = self
            .runtime
            .block_on(self.vector_store.get_embedding_metas_batch(&symbol_ids))
            .context("failed to batch-fetch embedding metadata")?;

        for (index, symbol_id) in symbol_ids.iter().enumerate() {
            let current = index + 1;
            if current % 100 == 0 {
                writeln!(out, "Embedding {current}/{processed}...")
                    .context("failed to write embeddings-only progress")?;
            }

            let meta = match store.get_sir_meta(symbol_id) {
                Ok(Some(meta)) => meta,
                Ok(None) => {
                    skipped_no_sir += 1;
                    continue;
                }
                Err(err) => {
                    errors += 1;
                    tracing::warn!(symbol_id = %symbol_id, error = %err, "failed to read SIR metadata");
                    continue;
                }
            };

            let blob = match store.read_sir_blob(symbol_id) {
                Ok(Some(blob)) => blob,
                Ok(None) => {
                    skipped_no_sir += 1;
                    continue;
                }
                Err(err) => {
                    errors += 1;
                    tracing::warn!(symbol_id = %symbol_id, error = %err, "failed to read SIR blob");
                    continue;
                }
            };

            let sir = match serde_json::from_str::<SirAnnotation>(&blob) {
                Ok(sir) => sir,
                Err(err) => {
                    let generation_pass = meta.generation_pass.as_str();
                    skipped_no_sir += 1;
                    tracing::warn!(
                        symbol_id = %symbol_id,
                        generation_pass,
                        error = %err,
                        "skipping symbol with invalid SIR blob during embeddings-only pass"
                    );
                    continue;
                }
            };

            let canonical = canonicalize_sir_json(&sir);
            let hash = sir_hash(&sir);
            match self.refresh_embedding_if_needed(
                symbol_id,
                &hash,
                &canonical,
                print_sir,
                out,
                existing_metas.get(symbol_id),
            ) {
                Ok(true) => refreshed += 1,
                Ok(false) => skipped_up_to_date += 1,
                Err(err) => {
                    errors += 1;
                    tracing::warn!(symbol_id = %symbol_id, error = %err, "failed to refresh embedding");
                }
            }
        }

        writeln!(
            out,
            "Re-embedded {refreshed} of {processed} symbols with {provider_name}/{model_name} ({skipped_no_sir} skipped: no current SIR, {skipped_up_to_date} already up to date, {errors} errors)"
        )
        .context("failed to write embeddings-only summary")?;

        Ok(())
    }

    pub fn delete_embeddings(&self, symbol_ids: &[String]) -> Result<()> {
        self.runtime
            .block_on(self.vector_store.delete_embeddings(symbol_ids))
            .context("failed to delete symbol embeddings")
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

        let (prepared_sir, _, _) = self
            .prepare_sir_for_persistence(store, &payload.symbol, &payload.sir)
            .with_context(|| format!("failed to prepare SIR for intent {intent_id}"))?;
        let mut canonical_json = canonicalize_sir_json(&prepared_sir);
        let mut sir_hash_value = sir_hash(&prepared_sir);

        if status == WriteIntentStatus::Pending {
            let persisted = self
                .persist_sir_payload_into_sqlite(store, payload, Some(intent_id))
                .with_context(|| format!("failed sqlite write stage for intent {intent_id}"))?;
            canonical_json = persisted.0;
            sir_hash_value = persisted.1;
            status = WriteIntentStatus::SqliteDone;
        } else {
            let stored_blob = store
                .read_sir_blob(payload.symbol.id.as_str())
                .with_context(|| {
                    format!("failed to read sqlite SIR blob for intent {intent_id}")
                })?;
            let needs_sqlite_refresh = match stored_blob.as_deref() {
                Some(stored_blob) => stored_blob != canonical_json,
                None => true,
            };
            if needs_sqlite_refresh {
                let persisted = self
                    .persist_sir_payload_into_sqlite(store, payload, Some(intent_id))
                    .with_context(|| {
                        format!("failed sqlite refresh stage for intent {intent_id}")
                    })?;
                canonical_json = persisted.0;
                sir_hash_value = persisted.1;
                status = WriteIntentStatus::SqliteDone;
            }
        }

        if status == WriteIntentStatus::SqliteDone {
            self.refresh_embedding_if_needed(
                payload.symbol.id.as_str(),
                sir_hash_value.as_str(),
                canonical_json.as_str(),
                false,
                &mut std::io::sink(),
                None,
            )
            .with_context(|| format!("failed vector write stage for intent {intent_id}"))?;
            store
                .update_intent_status(intent_id, WriteIntentStatus::VectorDone)
                .with_context(|| {
                    format!("failed to update status vector_done for intent {intent_id}")
                })?;
            status = WriteIntentStatus::VectorDone;
        }

        // Contract verification after embedding refresh (non-fatal)
        if status == WriteIntentStatus::VectorDone
            && let Some(ref contracts_config) = self.contracts_config
            && contracts_config.enabled
        {
            match self.load_symbol_embedding(payload.symbol.id.as_str()) {
                Ok(Some(emb_record)) => {
                    let config = load_workspace_config(&self.workspace_root).unwrap_or_default();
                    if let Err(err) = crate::contracts::verify_symbol_contracts(
                        store,
                        payload.symbol.id.as_str(),
                        canonical_json.as_str(),
                        Some(emb_record.embedding.as_slice()),
                        &config,
                        &self.workspace_root,
                    ) {
                        tracing::warn!(
                            symbol_id = %payload.symbol.id,
                            error = %err,
                            "Contract verification failed during SIR pipeline"
                        );
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(
                        symbol_id = %payload.symbol.id,
                        error = %err,
                        "failed to load symbol embedding for contract verification"
                    );
                }
            }
        }

        if status == WriteIntentStatus::VectorDone {
            if self.skip_surreal_sync {
                store
                    .update_intent_status(intent_id, WriteIntentStatus::GraphDone)
                    .with_context(|| {
                        format!("failed to update status graph_done for intent {intent_id}")
                    })?;
            } else {
                self.sync_graph_for_file(store, payload.symbol.file_path.as_str())
                    .with_context(|| format!("failed graph write stage for intent {intent_id}"))?;
                store
                    .update_intent_status(intent_id, WriteIntentStatus::GraphDone)
                    .with_context(|| {
                        format!("failed to update status graph_done for intent {intent_id}")
                    })?;
            }
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

    pub(crate) fn persist_sir_payload_into_sqlite(
        &self,
        store: &SqliteStore,
        payload: &UpsertSirIntentPayload,
        write_intent_id: Option<&str>,
    ) -> Result<(String, String)> {
        let (_, canonical_json, sir_hash_value) =
            self.prepare_sir_for_persistence(store, &payload.symbol, &payload.sir)?;
        let attempted_at = unix_timestamp_secs();
        // Higher-quality passes still need to promote metadata even when the
        // canonical SIR content is identical to an earlier pass.
        store.persist_sir_state_atomically(
            SirMetaRecord {
                id: payload.symbol.id.clone(),
                sir_hash: sir_hash_value.clone(),
                sir_version: 1,
                provider: payload.provider_name.clone(),
                model: payload.model_name.clone(),
                generation_pass: payload.generation_pass.clone(),
                reasoning_trace: payload.reasoning_trace.clone(),
                prompt_hash: None,
                staleness_score: None,
                updated_at: attempted_at,
                sir_status: SIR_STATUS_FRESH.to_owned(),
                last_error: None,
                last_attempt_at: attempted_at,
            },
            canonical_json.as_str(),
            payload.commit_hash.as_deref(),
            write_intent_id,
        )?;

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
            if let Err(err) = store.populate_symbol_neighbors(event.file_path.as_str()) {
                tracing::warn!(
                    file_path = %event.file_path,
                    error = %err,
                    "failed to populate symbol_neighbors for file"
                );
            }

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

        if let Ok(graph) = open_surreal_graph_store_sync(&self.workspace_root) {
            let test_intent_analyzer = TestIntentAnalyzer::new(&self.workspace_root)
                .context("failed to initialize test intent analyzer")?;
            let _ = test_intent_analyzer
                .refresh_for_test_file_with_graph(&graph, event.file_path.as_str())
                .with_context(|| {
                    format!(
                        "failed to refresh tested_by links for test file {}",
                        event.file_path
                    )
                })?;
        }

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
                "graph sync skipped unresolved structural edges"
            );
        }

        Ok(())
    }

    /// Flush a batch of embedding records to the vector store.
    pub(crate) fn flush_embedding_batch(&self, records: Vec<SymbolEmbeddingRecord>) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        self.runtime
            .block_on(self.vector_store.upsert_embedding_batch(records))
            .context("failed to flush embedding batch to vector store")
    }

    /// Check whether a symbol needs a new embedding without generating one.
    ///
    /// Returns `None` if no embedding provider is configured or the existing
    /// embedding already matches the given sir_hash, provider, and model.
    /// Returns `Some(EmbeddingNeeded)` with the provider/model names when
    /// regeneration is required.
    pub(crate) fn check_embedding_needed(
        &self,
        symbol_id: &str,
        sir_hash_value: &str,
        prefetched_meta: Option<&VectorEmbeddingMetaRecord>,
    ) -> Result<Option<EmbeddingNeeded>> {
        if self.embedding_provider.is_none() {
            return Ok(None);
        }

        let provider_name = self
            .embedding_provider_name
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or("mock");
        let model_name = self
            .embedding_model_name
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or("mock");

        let existing_meta = match prefetched_meta {
            Some(meta) => Some(meta.clone()),
            None => self
                .runtime
                .block_on(self.vector_store.get_embedding_meta(symbol_id))
                .with_context(|| format!("failed to read embedding metadata for {symbol_id}"))?,
        };
        if let Some(existing_meta) = existing_meta
            && existing_meta.sir_hash == sir_hash_value
            && existing_meta.provider == provider_name
            && existing_meta.model == model_name
        {
            return Ok(None);
        }

        Ok(Some(EmbeddingNeeded {
            provider: provider_name.to_owned(),
            model: model_name.to_owned(),
        }))
    }

    /// Batch-embed multiple texts in a single provider call.
    pub(crate) fn batch_embed_texts(
        &self,
        texts: &[&str],
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<Vec<f32>>> {
        let Some(embedding_provider) = self.embedding_provider.as_ref() else {
            return Ok(vec![Vec::new(); texts.len()]);
        };
        self.runtime
            .block_on(embedding_provider.embed_texts_with_purpose(texts, purpose))
            .context("batch embedding request failed")
    }

    /// Build `SymbolEmbeddingRecord`s from pre-computed embedding vectors.
    pub(crate) fn build_embedding_records(
        items: &[EmbeddingInput],
        embeddings: Vec<Vec<f32>>,
    ) -> Vec<SymbolEmbeddingRecord> {
        let updated_at = unix_timestamp_secs();
        items
            .iter()
            .zip(embeddings)
            .filter(|(_, emb)| !emb.is_empty())
            .map(|(item, embedding)| SymbolEmbeddingRecord {
                symbol_id: item.symbol_id.clone(),
                sir_hash: item.sir_hash.clone(),
                provider: item.provider.clone(),
                model: item.model.clone(),
                embedding,
                updated_at,
            })
            .collect()
    }

    pub(crate) fn refresh_embedding_if_needed(
        &self,
        symbol_id: &str,
        sir_hash_value: &str,
        canonical_json: &str,
        print_sir: bool,
        out: &mut dyn Write,
        prefetched_meta: Option<&VectorEmbeddingMetaRecord>,
    ) -> Result<bool> {
        let Some(needed) =
            self.check_embedding_needed(symbol_id, sir_hash_value, prefetched_meta)?
        else {
            return Ok(false);
        };

        let Some(embedding_provider) = self.embedding_provider.as_ref() else {
            return Ok(false);
        };

        let embedding = self
            .runtime
            .block_on(
                embedding_provider
                    .embed_text_with_purpose(canonical_json, EmbeddingPurpose::Document),
            )
            .with_context(|| format!("failed to generate embedding for {symbol_id}"))?;

        if embedding.is_empty() {
            return Ok(false);
        }

        let updated_at = unix_timestamp_secs();
        self.runtime
            .block_on(self.vector_store.upsert_embedding(SymbolEmbeddingRecord {
                symbol_id: symbol_id.to_owned(),
                sir_hash: sir_hash_value.to_owned(),
                provider: needed.provider.clone(),
                model: needed.model.clone(),
                embedding,
                updated_at,
            }))
            .with_context(|| format!("failed to store embedding for {symbol_id}"))?;

        if print_sir {
            writeln!(
                out,
                "EMBEDDING_STORED symbol_id={symbol_id} provider={} model={}",
                needed.provider, needed.model
            )
            .context("failed to write embedding print line")?;
        }

        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn bulk_upsert_file_rollups(
        &self,
        store: &SqliteStore,
        touched_files: BTreeMap<String, Language>,
        print_sir: bool,
        out: &mut dyn Write,
        commit_hash: Option<&str>,
        generation_pass: &str,
    ) -> Result<()> {
        let mut stale_rollups = Vec::new();
        let mut local_only = Vec::new();
        let mut needs_api = Vec::new();

        for (file_path, language) in touched_files {
            let leaf_sirs = self
                .load_file_rollup_leaf_sirs(store, file_path.as_str())
                .with_context(|| format!("failed to prepare file rollup inputs for {file_path}"))?;

            if leaf_sirs.is_empty() {
                stale_rollups.push((file_path, language));
                continue;
            }

            let job = RollupJob {
                file_path,
                language,
                leaf_sirs,
            };

            if job.leaf_sirs.len() <= 5 {
                local_only.push(job);
            } else {
                needs_api.push(job);
            }
        }

        tracing::info!(
            stale = stale_rollups.len(),
            local_only = local_only.len(),
            api_jobs = needs_api.len(),
            "prepared bulk file rollup jobs"
        );

        for (file_path, language) in stale_rollups {
            self.remove_file_rollup(store, file_path.as_str(), language)
                .with_context(|| format!("failed to remove stale file rollup for {file_path}"))?;
        }

        for job in local_only {
            let file_sir = concatenate_file_sir(&job.leaf_sirs);
            self.persist_file_rollup(
                store,
                job.file_path.as_str(),
                job.language,
                &file_sir,
                print_sir,
                out,
                commit_hash,
                generation_pass,
            )
            .with_context(|| {
                format!("failed to persist local file rollup for {}", job.file_path)
            })?;
        }

        for rollup in self
            .generate_api_file_rollups(needs_api)
            .context("failed to generate API-backed file rollups")?
        {
            self.persist_file_rollup(
                store,
                rollup.file_path.as_str(),
                rollup.language,
                &rollup.file_sir,
                print_sir,
                out,
                commit_hash,
                generation_pass,
            )
            .with_context(|| format!("failed to persist file rollup for {}", rollup.file_path))?;
        }

        Ok(())
    }

    fn generate_api_file_rollups(&self, jobs: Vec<RollupJob>) -> Result<Vec<CompletedRollup>> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }

        let provider = self.provider.clone();
        let concurrency = self.sir_concurrency.max(1);
        let timeout_secs = self.inference_timeout_secs;
        let total_jobs = jobs.len();

        let mut completed = self.runtime.block_on(async move {
            let semaphore = Arc::new(Semaphore::new(concurrency));
            let mut join_set = JoinSet::new();

            for job in jobs {
                let provider = provider.clone();
                let semaphore = semaphore.clone();
                join_set.spawn(async move {
                    let file_path = job.file_path;
                    let language = job.language;
                    let leaf_sirs = job.leaf_sirs;

                    let summary = match semaphore.acquire_owned().await {
                        Ok(permit) => {
                            let _permit = permit;
                            summarize_file_intent_async(
                                file_path.as_str(),
                                language,
                                &leaf_sirs,
                                provider,
                                timeout_secs,
                            )
                            .await
                        }
                        Err(_) => Err(anyhow!("file rollup semaphore closed")),
                    };

                    let file_sir = match summary {
                        Ok(summary) if !summary.trim().is_empty() => {
                            file_sir_from_summary(&leaf_sirs, summary)
                        }
                        Ok(_) => concatenate_file_sir(&leaf_sirs),
                        Err(err) => {
                            tracing::debug!(
                                file_path = %file_path,
                                error = %err,
                                "file rollup summarization failed, using deterministic concatenation"
                            );
                            concatenate_file_sir(&leaf_sirs)
                        }
                    };

                    CompletedRollup {
                        file_path,
                        language,
                        file_sir,
                    }
                });
            }

            let mut completed = Vec::with_capacity(total_jobs);
            while let Some(joined) = join_set.join_next().await {
                completed.push(
                    joined.map_err(|err| anyhow!("file rollup task join failed: {err}"))?,
                );
            }

            Ok::<Vec<CompletedRollup>, anyhow::Error>(completed)
        })?;

        completed.sort_by(|left, right| left.file_path.cmp(&right.file_path));

        tracing::info!(
            job_count = total_jobs,
            concurrency,
            "completed bulk API file rollup generation"
        );

        Ok(completed)
    }

    fn load_file_rollup_leaf_sirs(
        &self,
        store: &SqliteStore,
        file_path: &str,
    ) -> Result<Vec<FileLeafSir>> {
        let symbols = store
            .list_symbols_for_file(file_path)
            .with_context(|| format!("failed to list symbols for file {file_path}"))?;
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

        Ok(leaf_sirs)
    }

    fn remove_file_rollup(
        &self,
        store: &SqliteStore,
        file_path: &str,
        language: Language,
    ) -> Result<()> {
        let rollup_id = synthetic_file_sir_id(language.as_str(), file_path);
        store
            .mark_removed(&rollup_id)
            .with_context(|| format!("failed to remove stale file rollup {rollup_id}"))
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_file_rollup(
        &self,
        store: &SqliteStore,
        file_path: &str,
        language: Language,
        file_sir: &FileSir,
        print_sir: bool,
        out: &mut dyn Write,
        commit_hash: Option<&str>,
        generation_pass: &str,
    ) -> Result<()> {
        let rollup_id = synthetic_file_sir_id(language.as_str(), file_path);
        let canonical_json = canonicalize_file_sir_json(file_sir);
        let sir_hash_value = file_sir_hash(file_sir);
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
                generation_pass: generation_pass.to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
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

    #[allow(clippy::too_many_arguments)]
    fn upsert_file_rollup(
        &self,
        store: &SqliteStore,
        file_path: &str,
        language: Language,
        print_sir: bool,
        out: &mut dyn Write,
        commit_hash: Option<&str>,
        generation_pass: &str,
    ) -> Result<()> {
        let leaf_sirs = self
            .load_file_rollup_leaf_sirs(store, file_path)
            .with_context(|| format!("failed to load file rollup inputs for {file_path}"))?;

        if leaf_sirs.is_empty() {
            self.remove_file_rollup(store, file_path, language)
                .with_context(|| format!("failed to remove stale file rollup for {file_path}"))?;
            return Ok(());
        }

        let file_sir = aggregate_file_sir(
            file_path,
            language,
            &leaf_sirs,
            self.provider.clone(),
            &self.runtime,
            self.inference_timeout_secs,
        )
        .with_context(|| format!("failed to aggregate file rollup for {file_path}"))?;

        self.persist_file_rollup(
            store,
            file_path,
            language,
            &file_sir,
            print_sir,
            out,
            commit_hash,
            generation_pass,
        )
    }
}

fn resolve_tiered_parse_fallback_provider(
    workspace_root: &Path,
    overrides: &ProviderOverrides,
) -> Result<Option<(Arc<dyn InferenceProvider>, String)>> {
    let config =
        ensure_workspace_config(workspace_root).context("failed to load workspace config")?;
    let selected_provider = overrides.provider.unwrap_or(config.inference.provider);
    if selected_provider != InferenceProviderKind::Tiered {
        return Ok(None);
    }

    let Some(tiered) = config.inference.tiered.as_ref() else {
        return Ok(None);
    };
    if !tiered.retry_with_fallback {
        return Ok(None);
    }

    let fallback = Qwen3LocalProvider::new(
        tiered.fallback_endpoint.clone(),
        tiered.fallback_model.clone(),
    );
    let model_name = fallback.model_name();
    Ok(Some((Arc::new(fallback), model_name)))
}

fn resolve_workspace_head_commit(workspace_root: &Path) -> Option<String> {
    GitContext::open(workspace_root).and_then(|context| context.head_commit_hash())
}

fn unix_timestamp_secs() -> i64 {
    crate::time::current_unix_timestamp_secs()
}

fn unix_timestamp_millis() -> i64 {
    crate::time::current_unix_timestamp_millis()
}

fn source_modified_unix_millis(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
}

include!("tests.rs");
