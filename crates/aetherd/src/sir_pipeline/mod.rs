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
pub use self::infer::current_source_hash;
use self::infer::{GeneratedSir, SirGenerationOutcome, SirJob, generate_sir_jobs};
pub(crate) use self::persist::{PriorSir, UpsertSirIntentPayload};
pub use self::persist::{SirIdentity, current_sir_identity};
use self::persist::{flatten_error_line, to_symbol_record, to_test_intent_record};
use self::rollup::{
    FileLeafSir, aggregate_file_sir, concatenate_file_sir, file_sir_from_summary,
    summarize_file_intent_async,
};
use crate::quality::SirQualityMonitor;

mod embeddings;
mod events;
mod file_rollups;
mod generation;
mod graph;
mod infer;
mod intents;
mod locks;
mod persist;
mod rollup;

pub use self::file_rollups::refresh_local_file_rollup;

pub use locks::{
    WriteGuard, acquire_embed_write_lock, acquire_embed_write_locks, acquire_inject_write_lock,
};

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
    /// The SIR the override's prompt was built from, when the caller recorded it: the
    /// job then persists only while the store still holds exactly that SIR. Unrecorded,
    /// the job binds to the SIR current when it is queued.
    pub(crate) prior_sir: PriorSir,
}

#[derive(Debug, Clone)]
pub struct SirDeepPromptSpec {
    pub enrichment: SirEnrichmentContext,
    pub use_cot: bool,
    /// Identity of the SIR `enrichment` was built from (`None` when the symbol had no
    /// SIR), read from the same row as that SIR; the deep job is bound to it, so a
    /// replacement written between the enrichment and the queueing supersedes the job
    /// rather than being overwritten by a result reasoned from the old baseline.
    pub baseline_sir_identity: Option<SirIdentity>,
}

/// A pre-built quality pass candidate ready for batched inference.
#[derive(Debug, Clone)]
pub struct QualityBatchItem {
    pub symbol: Symbol,
    pub priority_score: f64,
    pub enrichment: SirEnrichmentContext,
    pub use_cot: bool,
    /// The identity of the SIR the enrichment was built from (`None`: the symbol had no
    /// SIR), read from the same row as that SIR. The generated result is persisted only
    /// while the symbol still holds exactly it; a SIR injected after the enrichment was
    /// built, even before the job is queued, makes the result superseded.
    pub baseline_sir_identity: Option<SirIdentity>,
}

/// Returned by `check_embedding_needed` when a symbol requires a new embedding.
#[derive(Debug, Clone)]
pub(crate) struct EmbeddingNeeded {
    pub provider: String,
    pub model: String,
    /// The vector currently stored for the symbol as observed by the check (`None`: no
    /// vector), so the write can be made conditional on exactly that vector.
    pub existing: Option<VectorEmbeddingMetaRecord>,
}

/// Outcome of `SirPipeline::refresh_embedding_if_current`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingRefresh {
    /// A vector for `sir_hash_value` was stored under this provider and model.
    Refreshed { provider: String, model: String },
    /// The store already held that vector (or no provider is configured).
    Unchanged,
    /// The SIR was replaced while the vector was being generated or stored; nothing
    /// for the old SIR is left in the vector store.
    Superseded,
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

/// What became of one generated SIR at persist time.
enum GenerationPersist {
    Persisted(Box<PersistedSuccessfulGeneration>),
    /// Another writer stored a SIR for the symbol while this one was being generated;
    /// that SIR stands and this result is dropped (not a failure).
    Superseded,
    /// The write failed; the reason was logged and the intent, if any, marked failed.
    Failed,
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
            None,
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
        Self::new_embeddings_only_with(
            workspace_root,
            Arc::<dyn EmbeddingProvider>::from(loaded_embedding.provider),
            loaded_embedding.provider_name,
            loaded_embedding.model_name,
            None,
        )
    }

    /// An embeddings-only pipeline around an embedding provider (and, when given, a
    /// vector store) the caller already holds. Loading a provider is expensive (a local
    /// Candle provider loads its model, a remote one builds a client) and opening a
    /// vector store makes another connection, so a process that embeds many symbols in
    /// one pass, or serves many tool calls, builds this once and shares it rather than
    /// paying that cost per symbol through [`SirPipeline::new_embeddings_only`].
    pub fn new_embeddings_only_with(
        workspace_root: PathBuf,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        embedding_provider_name: impl Into<String>,
        embedding_model_name: impl Into<String>,
        vector_store: Option<Arc<dyn VectorStore>>,
    ) -> Result<Self> {
        let embedding_identity =
            Some((embedding_provider_name.into(), embedding_model_name.into()));
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
            vector_store,
            None,
            None,
        )
    }

    /// The vector store this pipeline writes embeddings to.
    pub fn vector_store(&self) -> &Arc<dyn VectorStore> {
        &self.vector_store
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
        vector_store: Option<Arc<dyn VectorStore>>,
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
        let vector_store = match vector_store {
            Some(vector_store) => vector_store,
            None => runtime
                .block_on(open_vector_store(&workspace_root))
                .context("failed to initialize vector store")?,
        };
        let contracts_config = load_workspace_config(&workspace_root)
            .ok()
            .and_then(|c| c.contracts);
        let provider_name: String = provider_name.into();
        // The mock provider's 0.1 placeholders are intentional: no quality-floor warning.
        let quality_monitor = if provider_name == InferenceProviderKind::Mock.as_str() {
            SirQualityMonitor::disabled()
        } else {
            SirQualityMonitor::new(SIR_QUALITY_FLOOR_WINDOW, SIR_QUALITY_FLOOR_CONFIDENCE)
        };

        Ok(Self {
            workspace_root,
            provider,
            provider_name,
            model_name: model_name.into(),
            embedding_provider,
            embedding_provider_name,
            embedding_model_name,
            vector_store,
            runtime,
            sir_concurrency: concurrency,
            inference_timeout_secs: INFERENCE_ATTEMPT_TIMEOUT_SECS,
            quality_monitor: Mutex::new(quality_monitor),
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

    pub fn provider_name(&self) -> &str {
        self.provider_name.as_str()
    }

    pub fn model_name(&self) -> &str {
        self.model_name.as_str()
    }

    pub fn with_inference_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.inference_timeout_secs = timeout_secs.max(1);
        self
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

#[cfg(test)]
mod tests;
