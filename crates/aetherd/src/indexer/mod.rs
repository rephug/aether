use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use aether_analysis::TestIntentAnalyzer;
use aether_config::{
    InferenceProviderKind, WatcherConfig, ensure_workspace_config, gemini_thinking_fingerprint,
};
use aether_core::{GitContext, Symbol, SymbolChangeEvent, content_hash, normalize_path};
use aether_graph_algo::{GraphAlgorithmEdge, page_rank_sync};
use aether_infer::ProviderOverrides;
use aether_infer::sir_prompt::SirEnrichmentContext;
use aether_parse::{SymbolExtractor, TestIntent, language_for_path};
use aether_sir::{FileSir, SirAnnotation, synthetic_file_sir_id};
#[cfg(test)]
use aether_store::SirHistoryStore;
use aether_store::{
    SirIdentity, SirMetaRecord, SirStateStore, SqliteStore, SurrealGraphStore, SymbolCatalogStore,
    SymbolEmbeddingRecord, SymbolRecord, SymbolRelationStore, TestIntentRecord, TestIntentStore,
    open_graph_store, open_surreal_graph_store_sync,
};
use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use gix::traverse::commit::simple::CommitTimeOrder;
use ignore::WalkBuilder;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::batch::hash::compute_prompt_hash;
use crate::batch::write_fingerprint_row;
use crate::continuous::cosine_distance_from_embeddings;
use crate::observer::{DebounceQueue, ObserverState, is_ignored_path};
use crate::priority_queue::{
    SirPriorityQueue, compute_priority_score, kind_priority_score, size_inverse_score,
};
use crate::sir_pipeline::{
    MAX_SYMBOL_TEXT_CHARS, QualityBatchItem, SIR_GENERATION_PASS_DEEP, SIR_GENERATION_PASS_PREMIUM,
    SIR_GENERATION_PASS_REGENERATED, SIR_GENERATION_PASS_SCAN, SIR_GENERATION_PASS_TRIAGE,
    SirPipeline, acquire_inject_write_lock, build_job,
};

mod enrichment;
mod git_watch;
mod priority;
mod quality;
mod reconciliation;
mod structural;
mod worker;

pub use self::enrichment::build_enrichment_context;
use self::git_watch::{
    GitDebounceState, handle_watch_result, process_git_trigger, process_reindex_path,
    resolve_git_watch_dir,
};
pub use self::priority::compute_symbol_priority_scores;
use self::structural::StructuralIndexer;
use self::worker::{SharedQueueState, enqueue_symbols_missing_sir, spawn_semantic_worker};

use self::quality::{run_deep_pass, run_triage_pass};
use self::reconciliation::{
    execute_symbol_reconciliation, plan_symbol_reconciliation, print_reconciliation_dry_run,
};

const REQUEST_POLL_BATCH: usize = 128;
const WORKER_IDLE_SLEEP_MS: u64 = 200;
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub workspace: PathBuf,
    pub debounce_ms: u64,
    pub print_events: bool,
    pub print_sir: bool,
    pub embeddings_only: bool,
    pub force: bool,
    pub full: bool,
    pub deep: bool,
    pub turbo_concurrency: Option<usize>,
    pub dry_run: bool,
    pub sir_concurrency: usize,
    pub lifecycle_logs: bool,
    pub inference_provider: Option<InferenceProviderKind>,
    pub inference_model: Option<String>,
    pub inference_endpoint: Option<String>,
    pub inference_api_key_env: Option<String>,
    /// When set, the indexer skips processing debounced events while the flag is true.
    /// Events are still accumulated so they fire once the indexer is resumed.
    pub pause_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Debug, Clone)]
struct WatcherRuntimeConfig {
    watcher: WatcherConfig,
    premium_provider: Option<InferenceProviderKind>,
    premium_model: Option<String>,
    inference_thinking: Option<String>,
    tiered_primary_uses_gemini_thinking: bool,
    generation_pass: &'static str,
}

impl WatcherRuntimeConfig {
    fn from_workspace(config: &IndexerConfig) -> Result<Self> {
        let workspace_config = ensure_workspace_config(&config.workspace)
            .context("failed to load workspace config for watcher")?;
        let watcher = workspace_config.watcher.unwrap_or_default();
        let premium_provider = if watcher.realtime_provider.trim().is_empty() {
            None
        } else {
            Some(
                watcher
                    .realtime_provider
                    .parse()
                    .map_err(anyhow::Error::msg)
                    .context("invalid [watcher].realtime_provider")?,
            )
        };
        let premium_model = {
            let value = watcher.realtime_model.trim();
            (!value.is_empty()).then(|| value.to_owned())
        };
        let generation_pass = if premium_model.is_some() {
            SIR_GENERATION_PASS_PREMIUM
        } else {
            SIR_GENERATION_PASS_SCAN
        };
        let tiered_primary_uses_gemini_thinking = workspace_config
            .inference
            .tiered
            .as_ref()
            .is_some_and(|tiered| {
                tiered
                    .primary
                    .trim()
                    .eq_ignore_ascii_case(InferenceProviderKind::Gemini.as_str())
            });
        Ok(Self {
            watcher,
            premium_provider,
            premium_model,
            inference_thinking: workspace_config.inference.thinking,
            tiered_primary_uses_gemini_thinking,
            generation_pass,
        })
    }

    fn provider_overrides(&self, config: &IndexerConfig) -> ProviderOverrides {
        if let Some(model) = self.premium_model.as_ref() {
            ProviderOverrides {
                provider: self.premium_provider.or(config.inference_provider),
                model: Some(model.clone()),
                endpoint: config.inference_endpoint.clone(),
                api_key_env: config.inference_api_key_env.clone(),
                thinking: None,
            }
        } else {
            ProviderOverrides {
                provider: config.inference_provider,
                model: config.inference_model.clone(),
                endpoint: config.inference_endpoint.clone(),
                api_key_env: config.inference_api_key_env.clone(),
                thinking: None,
            }
        }
    }

    fn git_debounce_window(&self) -> Duration {
        Duration::from_secs_f64(self.watcher.git_debounce_secs.max(0.1))
    }

    fn prompt_config_fingerprint(&self, pipeline: &SirPipeline) -> String {
        format!(
            "{}:{}:{}",
            pipeline.model_name(),
            self.prompt_thinking_fingerprint(pipeline.provider_name()),
            MAX_SYMBOL_TEXT_CHARS
        )
    }

    fn prompt_thinking_fingerprint(&self, provider_name: &str) -> &'static str {
        let uses_gemini_thinking = match provider_name {
            name if name == InferenceProviderKind::Gemini.as_str() => true,
            name if name == InferenceProviderKind::Tiered.as_str() => {
                self.tiered_primary_uses_gemini_thinking
            }
            _ => false,
        };

        if uses_gemini_thinking {
            gemini_thinking_fingerprint(self.inference_thinking.as_deref())
        } else {
            "none"
        }
    }
}

fn collect_initial_snapshot(
    observer: &ObserverState,
) -> (Vec<SymbolChangeEvent>, HashMap<String, Symbol>, usize) {
    let events = observer.initial_symbol_events();
    let mut symbol_count = 0usize;
    let mut symbols_by_id = HashMap::<String, Symbol>::new();
    for event in &events {
        symbol_count += event.added.len() + event.updated.len();
        for symbol in event.added.iter().chain(event.updated.iter()) {
            symbols_by_id.insert(symbol.id.clone(), symbol.clone());
        }
    }
    (events, symbols_by_id, symbol_count)
}

fn run_full_index_once_inner(config: &IndexerConfig, skip_teardown: bool) -> Result<()> {
    if config.dry_run {
        if config.lifecycle_logs {
            println!("INDEX: starting");
        }

        let mut observer = ObserverState::new(config.workspace.clone())?;
        observer.seed_from_disk()?;
        let store = SqliteStore::open_readonly(&config.workspace)
            .context("failed to open local store for dry-run reconciliation")?;
        let (_, symbols_by_id, symbol_count) = collect_initial_snapshot(&observer);
        tracing::info!(
            symbol_count,
            "Structural snapshot complete: {} symbols parsed for dry-run reconciliation",
            symbol_count
        );

        let plan = plan_symbol_reconciliation(&store, &symbols_by_id)?;
        let mut stdout = std::io::stdout();
        print_reconciliation_dry_run(&plan, &mut stdout)?;
        return Ok(());
    }

    let (observer, store, sir_pipeline) = initialize_full_indexer(config)?;
    let mut structural = StructuralIndexer::new(config.workspace.clone())?;
    let mut stdout = std::io::stdout();
    let (initial_events, symbols_by_id, symbol_count) = collect_initial_snapshot(&observer);

    for event in &initial_events {
        structural.process_event(&store, event)?;
    }

    let reconciliation_plan = plan_symbol_reconciliation(&store, &symbols_by_id)?;
    let _ = execute_symbol_reconciliation(
        &config.workspace,
        &store,
        &reconciliation_plan,
        |symbol_ids| structural.delete_symbols_batch(symbol_ids),
        |symbol_ids| sir_pipeline.delete_embeddings(symbol_ids),
    )?;

    let all_symbols = symbols_by_id.values().cloned().collect::<Vec<_>>();
    let priority_scores = compute_symbol_priority_scores(&config.workspace, &store, &all_symbols);
    tracing::info!(
        symbol_count,
        "Structural index complete: {} symbols indexed, lexical search + graph queries available",
        symbol_count
    );

    let candidate_symbol_ids = if config.force {
        store.list_all_symbol_ids()?
    } else {
        store.list_symbol_ids_without_sir()?
    };

    let mut symbols_by_file = BTreeMap::<String, Vec<Symbol>>::new();
    let mut unresolved = 0usize;
    for symbol_id in candidate_symbol_ids {
        if let Some(symbol) = symbols_by_id.get(symbol_id.as_str()) {
            symbols_by_file
                .entry(symbol.file_path.clone())
                .or_default()
                .push(symbol.clone());
        } else {
            unresolved += 1;
            tracing::warn!(
                symbol_id = %symbol_id,
                "Scan pass symbol missing from initial snapshot; skipping"
            );
        }
    }

    let scan_symbol_count: usize = symbols_by_file.values().map(Vec::len).sum();
    tracing::info!(
        symbol_count = scan_symbol_count,
        file_count = symbols_by_file.len(),
        force = config.force,
        "Scan pass: generating SIR for {} symbols",
        scan_symbol_count
    );
    if unresolved > 0 {
        tracing::warn!(
            unresolved,
            "Scan pass skipped symbols missing from initial snapshot"
        );
    }

    let mut all_scan_symbols = Vec::with_capacity(scan_symbol_count);
    for symbols in symbols_by_file.into_values() {
        all_scan_symbols.extend(symbols);
    }

    let scan_stats = sir_pipeline.process_bulk_scan(
        &store,
        all_scan_symbols,
        &priority_scores,
        config.force,
        SIR_GENERATION_PASS_SCAN,
        config.print_sir,
        &mut stdout,
    )?;
    tracing::info!(
        successes = scan_stats.success_count,
        failures = scan_stats.failure_count,
        "Bulk scan complete"
    );

    let workspace_config = ensure_workspace_config(&config.workspace)
        .context("failed to load workspace config for quality passes")?;
    let contracts_enabled = workspace_config
        .contracts
        .as_ref()
        .is_some_and(|c| c.enabled);
    let quality = workspace_config.sir_quality;
    let run_triage = quality.triage_pass || config.deep;
    let run_deep = quality.deep_pass || config.deep;

    if run_triage {
        run_triage_pass(
            config,
            &store,
            &symbols_by_id,
            &priority_scores,
            &quality,
            &mut stdout,
            contracts_enabled,
        )?;
    }
    if run_deep {
        run_deep_pass(
            config,
            &store,
            &symbols_by_id,
            &priority_scores,
            &quality,
            &mut stdout,
            contracts_enabled,
        )?;
    }

    let (total_symbols, symbols_with_sir) = store
        .count_symbols_with_sir()
        .context("failed to compute SIR coverage after quality pipeline")?;
    let coverage_pct = if total_symbols > 0 {
        (symbols_with_sir as f64 / total_symbols as f64) * 100.0
    } else {
        0.0
    };
    tracing::info!(
        symbols_with_sir,
        total_symbols,
        coverage_pct = coverage_pct,
        "Quality pipeline complete: SIR coverage"
    );

    if config.lifecycle_logs {
        println!("INDEX: full scan complete");
    }

    if skip_teardown {
        // In one-shot CLI mode we exit immediately from main; skipping teardown avoids
        // backend shutdown hangs on certain graph runtimes.
        std::mem::forget(structural);
        std::mem::forget(sir_pipeline);
        std::mem::forget(store);
    }
    Ok(())
}

pub fn run_full_index_once(config: &IndexerConfig) -> Result<()> {
    run_full_index_once_inner(config, false)
}

pub fn run_full_index_once_for_cli(config: &IndexerConfig) -> Result<()> {
    run_full_index_once_inner(config, true)
}

fn run_embeddings_only_once(config: &IndexerConfig) -> Result<()> {
    ensure_workspace_config(&config.workspace)
        .context("failed to load workspace config for embeddings-only command")?;
    let pipeline = SirPipeline::new_embeddings_only(config.workspace.clone())
        .context("failed to initialize embeddings-only pipeline")?;
    let store = SqliteStore::open(&config.workspace).context("failed to open local store")?;
    let mut stdout = std::io::stdout();
    pipeline
        .run_embeddings_only_pass(&store, config.print_sir, &mut stdout)
        .context("failed to run embeddings-only reindex")
}

fn run_initial_index_once_inner(config: &IndexerConfig, skip_teardown: bool) -> Result<()> {
    if config.embeddings_only {
        return run_embeddings_only_once(config);
    }

    if config.full {
        return if skip_teardown {
            run_full_index_once_for_cli(config)
        } else {
            run_full_index_once(config)
        };
    }

    let (observer, store) = initialize_observer_and_store(config)?;
    let mut structural = StructuralIndexer::new(config.workspace.clone())?;
    let mut symbol_count = 0usize;

    for event in observer.initial_symbol_events() {
        symbol_count += event.added.len() + event.updated.len();
        structural.process_event(store.as_ref(), &event)?;
    }

    tracing::info!(
        symbol_count,
        "Structural index complete: {} symbols indexed, lexical search + graph queries available",
        symbol_count
    );
    if config.lifecycle_logs {
        println!("INDEX: structural scan complete");
    }

    if skip_teardown {
        // In one-shot CLI mode we exit immediately from main; skipping teardown avoids
        // backend shutdown hangs on certain graph runtimes.
        std::mem::forget(structural);
        std::mem::forget(store);
    }
    Ok(())
}

pub(crate) fn run_structural_index_once(
    workspace: &Path,
) -> Result<(SqliteStore, HashMap<String, Symbol>, usize)> {
    let mut observer = ObserverState::new(workspace.to_path_buf())?;
    observer.seed_from_disk()?;
    let store = SqliteStore::open(workspace).context("failed to initialize local store")?;
    let mut structural = StructuralIndexer::new(workspace.to_path_buf())?;
    let (initial_events, symbols_by_id, symbol_count) = collect_initial_snapshot(&observer);
    for event in &initial_events {
        structural.process_event(&store, event)?;
    }
    Ok((store, symbols_by_id, symbol_count))
}

pub fn run_initial_index_once(config: &IndexerConfig) -> Result<()> {
    run_initial_index_once_inner(config, false)
}

pub fn run_initial_index_once_for_cli(config: &IndexerConfig) -> Result<()> {
    run_initial_index_once_inner(config, true)
}

pub fn run_indexing_loop(config: IndexerConfig) -> Result<()> {
    let (mut observer, store) = initialize_observer_and_store(&config)?;
    let watcher_runtime = WatcherRuntimeConfig::from_workspace(&config)?;
    let mut structural = StructuralIndexer::new(config.workspace.clone())?;

    let mut initial_symbol_index = HashMap::<String, Symbol>::new();
    let mut initial_symbols = Vec::<Symbol>::new();
    let mut symbol_count = 0usize;

    for event in observer.initial_symbol_events() {
        symbol_count += event.added.len() + event.updated.len();
        collect_changed_symbols(&event, &mut initial_symbols);
        for symbol in event.added.iter().chain(event.updated.iter()) {
            initial_symbol_index.insert(symbol.id.clone(), symbol.clone());
        }
        structural.process_event(store.as_ref(), &event)?;
    }
    tracing::info!(
        symbol_count,
        "Structural index complete: {} symbols indexed, lexical search + graph queries available",
        symbol_count
    );

    let queue_state = SharedQueueState::new(initial_symbol_index);
    let queued = enqueue_symbols_missing_sir(
        config.workspace.as_path(),
        store.as_ref(),
        &queue_state,
        &initial_symbols,
    )?;
    tracing::info!(
        queued,
        "Scan pass queued: {} symbols for SIR generation",
        queued
    );

    let mut worker_started = 0usize;
    for worker_id in 0..config.sir_concurrency.max(1) {
        match spawn_semantic_worker(
            worker_id,
            &config,
            &watcher_runtime,
            store.clone(),
            queue_state.clone(),
        ) {
            Ok(()) => {
                worker_started += 1;
            }
            Err(err) => {
                tracing::warn!(
                    worker_id,
                    error = %err,
                    "failed to start semantic worker; structural indexing will continue without scan pass"
                );
                break;
            }
        }
    }
    tracing::info!(worker_started, "started semantic workers");

    if config.lifecycle_logs {
        println!("INDEX: watching");
    }

    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            let _ = tx.send(result);
        },
        Config::default(),
    )
    .context("failed to initialize file watcher")?;
    let git_watch_dir = resolve_git_watch_dir(&config.workspace);

    for entry in WalkBuilder::new(&config.workspace)
        .hidden(true)
        .git_ignore(true)
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter(|entry| !is_ignored_path(entry.path()))
    {
        watcher
            .watch(entry.path(), RecursiveMode::NonRecursive)
            .with_context(|| format!("failed to watch directory {}", entry.path().display()))?;
    }
    if let Some(git_watch_dir) = git_watch_dir.as_ref()
        && git_watch_dir.exists()
    {
        watcher
            .watch(git_watch_dir, RecursiveMode::Recursive)
            .with_context(|| {
                format!(
                    "failed to watch git metadata directory {}",
                    git_watch_dir.display()
                )
            })?;
    }

    let debounce_window = Duration::from_millis(config.debounce_ms);
    let git_debounce_window = watcher_runtime.git_debounce_window();
    let poll_interval = Duration::from_millis(50);
    let mut debounce_queue = DebounceQueue::default();
    let mut git_debounce_state = GitDebounceState::default();

    loop {
        match rx.recv_timeout(poll_interval) {
            Ok(result) => {
                if let Err(err) = handle_watch_result(
                    &config.workspace,
                    git_watch_dir.as_deref(),
                    &mut watcher,
                    result,
                    &mut debounce_queue,
                    &mut git_debounce_state,
                ) {
                    tracing::warn!(error = ?err, "watch event error");
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow::anyhow!("watcher channel disconnected"));
            }
        }

        while let Ok(result) = rx.try_recv() {
            if let Err(err) = handle_watch_result(
                &config.workspace,
                git_watch_dir.as_deref(),
                &mut watcher,
                result,
                &mut debounce_queue,
                &mut git_debounce_state,
            ) {
                tracing::warn!(error = ?err, "watch event error");
            }
        }

        // Skip processing while paused — events stay queued and fire on resume.
        if config
            .pause_flag
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
        {
            continue;
        }

        let now = Instant::now();
        if git_debounce_state.should_fire(now, git_debounce_window) {
            git_debounce_state.extend_dirty(debounce_queue.drain_due(now, Duration::ZERO));
            let dirty_paths = git_debounce_state.take_dirty_paths();
            if let Err(err) = process_git_trigger(
                &config,
                &watcher_runtime,
                git_watch_dir.as_deref(),
                &mut observer,
                &mut structural,
                store.as_ref(),
                &queue_state,
                dirty_paths,
            ) {
                tracing::warn!(error = %err, "git-triggered reindex failed");
            }
            continue;
        }

        if git_debounce_state.has_pending() {
            continue;
        }

        for path in debounce_queue.drain_due(now, debounce_window) {
            if let Err(err) = process_reindex_path(
                &config,
                &mut observer,
                &mut structural,
                store.as_ref(),
                &queue_state,
                &path,
            ) {
                tracing::error!(path = %path.display(), error = %err, "process error");
            }
        }
    }
}

fn initialize_observer_and_store(
    config: &IndexerConfig,
) -> Result<(ObserverState, Arc<SqliteStore>)> {
    if config.lifecycle_logs {
        println!("INDEX: starting");
    }

    let mut observer = ObserverState::new(config.workspace.clone())?;
    observer.seed_from_disk()?;
    let store =
        Arc::new(SqliteStore::open(&config.workspace).context("failed to initialize local store")?);

    Ok((observer, store))
}

fn initialize_full_indexer(
    config: &IndexerConfig,
) -> Result<(ObserverState, SqliteStore, SirPipeline)> {
    if config.lifecycle_logs {
        println!("INDEX: starting");
    }

    let mut observer = ObserverState::new(config.workspace.clone())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(&config.workspace).context("failed to initialize local store")?;
    let scan_concurrency = config
        .turbo_concurrency
        .unwrap_or(config.sir_concurrency)
        .max(1);
    let sir_pipeline = SirPipeline::new(
        config.workspace.clone(),
        scan_concurrency,
        ProviderOverrides {
            provider: config.inference_provider,
            model: config.inference_model.clone(),
            endpoint: config.inference_endpoint.clone(),
            api_key_env: config.inference_api_key_env.clone(),
            thinking: None,
        },
    )
    .context("failed to initialize SIR pipeline")?;

    match sir_pipeline.replay_incomplete_intents(&store, false, 100, false) {
        Ok(replayed) => {
            tracing::info!(
                replayed,
                "Replayed {} incomplete write intents from previous session",
                replayed
            );
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to replay incomplete write intents");
        }
    }
    match store.prune_completed_intents(604_800) {
        Ok(pruned) => {
            if pruned > 0 {
                tracing::info!(pruned, "pruned completed write intents older than 7 days");
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to prune completed write intents");
        }
    }

    Ok((observer, store, sir_pipeline))
}

fn collect_changed_symbols(event: &SymbolChangeEvent, symbols: &mut Vec<Symbol>) {
    symbols.extend(event.added.iter().cloned());
    symbols.extend(event.updated.iter().cloned());
}

fn source_line_count(workspace: &Path, file_path: &str) -> usize {
    let full_path = workspace.join(file_path);
    fs::read_to_string(full_path)
        .map(|source| source.lines().count())
        .unwrap_or(0)
}

fn read_source_file(workspace: &Path, file_path: &str) -> String {
    fs::read_to_string(workspace.join(file_path)).unwrap_or_default()
}

fn infer_symbol_is_public(source: &str, symbol: &Symbol) -> bool {
    let name = symbol.name.trim();
    if name.is_empty() {
        return false;
    }
    let rust_pub = format!("pub fn {name}");
    let rust_pub_struct = format!("pub struct {name}");
    let rust_pub_trait = format!("pub trait {name}");
    let ts_export_fn = format!("export function {name}");
    let ts_export_const = format!("export const {name}");
    let ts_export_class = format!("export class {name}");

    source.lines().any(|line| {
        let line = line.trim();
        line.contains(rust_pub.as_str())
            || line.contains(rust_pub_struct.as_str())
            || line.contains(rust_pub_trait.as_str())
            || line.contains(ts_export_fn.as_str())
            || line.contains(ts_export_const.as_str())
            || line.contains(ts_export_class.as_str())
    })
}

fn unix_timestamp_secs() -> i64 {
    crate::time::current_unix_timestamp_secs()
}

fn unix_timestamp_millis() -> i64 {
    crate::time::current_unix_timestamp_millis()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use aether_config::WatcherConfig;
    use aether_core::{Language, Position, SourceRange, SymbolKind};
    use tracing::dispatcher::{self, Dispatch};
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter(self.0.clone())
        }
    }

    impl Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub(super) fn capture_logs<T>(run: impl FnOnce() -> T) -> (T, String) {
        let buffer = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_writer(buffer.clone())
            .finish();
        let result = dispatcher::with_default(&Dispatch::new(subscriber), run);
        let logs = String::from_utf8(buffer.0.lock().expect("log buffer lock").clone())
            .expect("utf8 logs");
        (result, logs)
    }

    pub(super) fn test_symbol(symbol_id: &str, signature: &str) -> Symbol {
        Symbol {
            id: symbol_id.to_owned(),
            language: Language::Rust,
            file_path: "src/lib.rs".to_owned(),
            kind: SymbolKind::Function,
            name: "run".to_owned(),
            qualified_name: "demo::run".to_owned(),
            signature_fingerprint: signature.to_owned(),
            content_hash: content_hash(signature),
            range: SourceRange {
                start: Position { line: 1, column: 0 },
                end: Position {
                    line: 1,
                    column: 10,
                },
                start_byte: Some(0),
                end_byte: Some(10),
            },
        }
    }

    pub(super) fn write_default_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether dir");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
graph_backend = "sqlite"

[embeddings]
enabled = false
vector_backend = "sqlite"
"#,
        )
        .expect("write test config");
    }

    #[test]
    fn watcher_prompt_thinking_fingerprint_tracks_effective_provider_behavior() {
        let runtime = WatcherRuntimeConfig {
            watcher: WatcherConfig::default(),
            premium_provider: None,
            premium_model: None,
            inference_thinking: Some("high".to_owned()),
            tiered_primary_uses_gemini_thinking: true,
            generation_pass: SIR_GENERATION_PASS_SCAN,
        };

        assert_eq!(
            runtime.prompt_thinking_fingerprint(InferenceProviderKind::Gemini.as_str()),
            "high"
        );
        assert_eq!(
            runtime.prompt_thinking_fingerprint(InferenceProviderKind::Tiered.as_str()),
            "high"
        );
        assert_eq!(
            runtime.prompt_thinking_fingerprint(InferenceProviderKind::Qwen3Local.as_str()),
            "none"
        );
    }

    #[test]
    fn watcher_prompt_thinking_fingerprint_uses_dynamic_for_omitted_gemini_thinking() {
        let runtime = WatcherRuntimeConfig {
            watcher: WatcherConfig::default(),
            premium_provider: None,
            premium_model: None,
            inference_thinking: Some("dynamic".to_owned()),
            tiered_primary_uses_gemini_thinking: true,
            generation_pass: SIR_GENERATION_PASS_SCAN,
        };

        assert_eq!(
            runtime.prompt_thinking_fingerprint(InferenceProviderKind::Gemini.as_str()),
            "dynamic"
        );
    }
}
