use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aether_analysis::TestIntentAnalyzer;
use aether_config::{InferenceProviderKind, ensure_workspace_config};
use aether_core::{GitContext, Symbol, SymbolChangeEvent, content_hash, normalize_path};
use aether_graph_algo::{GraphAlgorithmEdge, page_rank_sync};
use aether_infer::ProviderOverrides;
use aether_infer::sir_prompt::SirEnrichmentContext;
use aether_parse::{SymbolExtractor, TestIntent};
use aether_sir::{FileSir, SirAnnotation, synthetic_file_sir_id};
use aether_store::{SqliteStore, Store, SymbolRecord, TestIntentRecord, open_graph_store};
use anyhow::{Context, Result};
use gix::traverse::commit::simple::CommitTimeOrder;
use ignore::WalkBuilder;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::observer::{DebounceQueue, ObserverState, is_ignored_path};
use crate::priority_queue::{
    SirPriorityQueue, compute_priority_score, kind_priority_score, size_inverse_score,
};
use crate::sir_pipeline::{
    SIR_GENERATION_PASS_DEEP, SIR_GENERATION_PASS_REGENERATED, SIR_GENERATION_PASS_SCAN,
    SIR_GENERATION_PASS_TRIAGE, SirDeepPromptSpec, SirPipeline,
};

const REQUEST_POLL_BATCH: usize = 128;
const WORKER_IDLE_SLEEP_MS: u64 = 200;

#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub workspace: PathBuf,
    pub debounce_ms: u64,
    pub print_events: bool,
    pub print_sir: bool,
    pub force: bool,
    pub full: bool,
    pub deep: bool,
    pub sir_concurrency: usize,
    pub lifecycle_logs: bool,
    pub inference_provider: Option<InferenceProviderKind>,
    pub inference_model: Option<String>,
    pub inference_endpoint: Option<String>,
    pub inference_api_key_env: Option<String>,
}

#[derive(Clone)]
struct SharedQueueState {
    queue: Arc<Mutex<SirPriorityQueue>>,
    symbol_index: Arc<Mutex<HashMap<String, Symbol>>>,
    in_progress: Arc<Mutex<HashSet<String>>>,
}

impl SharedQueueState {
    fn new(symbol_index: HashMap<String, Symbol>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(SirPriorityQueue::default())),
            symbol_index: Arc::new(Mutex::new(symbol_index)),
            in_progress: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn remove_symbol(&self, symbol_id: &str) {
        if let Ok(mut symbols) = self.symbol_index.lock() {
            symbols.remove(symbol_id);
        }
        if let Ok(mut queue) = self.queue.lock() {
            queue.remove(symbol_id);
        }
        if let Ok(mut in_progress) = self.in_progress.lock() {
            in_progress.remove(symbol_id);
        }
    }

    fn upsert_symbol(&self, symbol: Symbol) {
        if let Ok(mut symbols) = self.symbol_index.lock() {
            symbols.insert(symbol.id.clone(), symbol);
        }
    }

    fn bump_to_front(&self, symbol_id: &str) -> bool {
        if let Ok(in_progress) = self.in_progress.lock()
            && in_progress.contains(symbol_id)
        {
            return false;
        }
        if let Ok(mut queue) = self.queue.lock()
            && queue.bump_to_front(symbol_id)
        {
            return true;
        }

        let maybe_symbol = self
            .symbol_index
            .lock()
            .ok()
            .and_then(|symbols| symbols.get(symbol_id).cloned());
        let Some(symbol) = maybe_symbol else {
            return false;
        };
        if let Ok(mut queue) = self.queue.lock() {
            let _ = queue.push(symbol.id.clone(), 1.0);
            queue.bump_to_front(symbol_id)
        } else {
            false
        }
    }

    fn pop_task(&self) -> Option<(f64, Symbol)> {
        let (score, symbol_id) = {
            let mut queue = self.queue.lock().ok()?;
            queue.pop()?
        };
        {
            let mut in_progress = self.in_progress.lock().ok()?;
            if !in_progress.insert(symbol_id.clone()) {
                return None;
            }
        }
        let symbol = self
            .symbol_index
            .lock()
            .ok()
            .and_then(|symbols| symbols.get(&symbol_id).cloned());
        if symbol.is_none()
            && let Ok(mut in_progress) = self.in_progress.lock()
        {
            in_progress.remove(&symbol_id);
        }
        symbol.map(|symbol| (score, symbol))
    }

    fn complete_task(&self, symbol_id: &str) {
        if let Ok(mut in_progress) = self.in_progress.lock() {
            in_progress.remove(symbol_id);
        }
    }
}

fn run_full_index_once_inner(config: &IndexerConfig, skip_teardown: bool) -> Result<()> {
    let (observer, store, sir_pipeline) = initialize_full_indexer(config)?;
    let mut structural = StructuralIndexer::new(config.workspace.clone())?;
    let mut stdout = std::io::stdout();
    let mut symbol_count = 0usize;
    let mut symbols_by_id = HashMap::<String, Symbol>::new();

    for event in observer.initial_symbol_events() {
        symbol_count += event.added.len() + event.updated.len();
        for symbol in event.added.iter().chain(event.updated.iter()) {
            symbols_by_id.insert(symbol.id.clone(), symbol.clone());
        }
        structural.process_event(&store, &event)?;
    }
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

    for (file_path, symbols) in symbols_by_file {
        if symbols.is_empty() {
            continue;
        }
        let max_priority = symbols
            .iter()
            .filter_map(|symbol| priority_scores.get(symbol.id.as_str()).copied())
            .fold(0.0_f64, f64::max);
        let event = SymbolChangeEvent {
            file_path,
            language: symbols[0].language,
            added: Vec::new(),
            removed: Vec::new(),
            updated: symbols,
        };
        if let Err(err) = sir_pipeline.process_event_with_priority_and_pass(
            &store,
            &event,
            config.force,
            config.print_sir,
            &mut stdout,
            Some(max_priority),
            SIR_GENERATION_PASS_SCAN,
        ) {
            tracing::error!(
                file_path = %event.file_path,
                error = %err,
                "Scan pass SIR processing error"
            );
        }
    }

    let workspace_config = ensure_workspace_config(&config.workspace)
        .context("failed to load workspace config for quality passes")?;
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

#[derive(Debug, Clone)]
struct QualityPassCandidate {
    symbol: Symbol,
    priority_score: f64,
    baseline_sir: SirAnnotation,
}

fn run_triage_pass(
    config: &IndexerConfig,
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
    priority_scores: &HashMap<String, f64>,
    quality: &aether_config::SirQualityConfig,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let eligible_candidates =
        collect_quality_pass_candidates(store, symbols_by_id, priority_scores, |pass| {
            pass == SIR_GENERATION_PASS_TRIAGE
                || pass == SIR_GENERATION_PASS_DEEP
                || pass == SIR_GENERATION_PASS_REGENERATED
        })?;
    let candidates = select_quality_pass_candidates(
        "Triage pass",
        eligible_candidates,
        quality.triage_priority_threshold,
        quality.triage_confidence_threshold,
        quality.triage_max_symbols,
    );
    if candidates.is_empty() {
        tracing::info!("Triage pass: 0 symbols selected");
        return Ok(());
    }

    let triage_provider = parse_quality_provider(
        quality.triage_provider.clone(),
        "sir_quality.triage_provider",
    )?
    .or(config.inference_provider);
    let triage_pipeline = SirPipeline::new(
        config.workspace.clone(),
        quality.triage_concurrency.max(1),
        ProviderOverrides {
            provider: triage_provider,
            model: quality
                .triage_model
                .clone()
                .or_else(|| config.inference_model.clone()),
            endpoint: quality
                .triage_endpoint
                .clone()
                .or_else(|| config.inference_endpoint.clone()),
            api_key_env: quality
                .triage_api_key_env
                .clone()
                .or_else(|| config.inference_api_key_env.clone()),
        },
    )
    .map(|pipeline| pipeline.with_inference_timeout_secs(quality.triage_timeout_secs))
    .context("failed to initialize triage-pass provider pipeline")?;
    run_quality_pass(
        "Triage pass",
        store,
        &triage_pipeline,
        candidates,
        priority_scores,
        quality.deep_max_neighbors,
        quality.triage_priority_threshold,
        quality.triage_confidence_threshold,
        config.print_sir,
        out,
        SIR_GENERATION_PASS_TRIAGE,
    )
}

fn run_deep_pass(
    config: &IndexerConfig,
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
    priority_scores: &HashMap<String, f64>,
    quality: &aether_config::SirQualityConfig,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let eligible_candidates =
        collect_quality_pass_candidates(store, symbols_by_id, priority_scores, |pass| {
            pass == SIR_GENERATION_PASS_DEEP || pass == SIR_GENERATION_PASS_REGENERATED
        })?;
    let candidates = select_quality_pass_candidates(
        "Deep pass",
        eligible_candidates,
        quality.deep_priority_threshold,
        quality.deep_confidence_threshold,
        quality.deep_max_symbols,
    );
    if candidates.is_empty() {
        tracing::info!("Deep pass: 0 symbols selected");
        return Ok(());
    }

    let deep_provider =
        parse_quality_provider(quality.deep_provider.clone(), "sir_quality.deep_provider")?
            .or(config.inference_provider);
    let deep_pipeline = SirPipeline::new(
        config.workspace.clone(),
        quality.deep_concurrency.max(1),
        ProviderOverrides {
            provider: deep_provider,
            model: quality
                .deep_model
                .clone()
                .or_else(|| config.inference_model.clone()),
            endpoint: quality
                .deep_endpoint
                .clone()
                .or_else(|| config.inference_endpoint.clone()),
            api_key_env: quality
                .deep_api_key_env
                .clone()
                .or_else(|| config.inference_api_key_env.clone()),
        },
    )
    .map(|pipeline| pipeline.with_inference_timeout_secs(quality.deep_timeout_secs))
    .context("failed to initialize deep-pass provider pipeline")?;
    run_quality_pass(
        "Deep pass",
        store,
        &deep_pipeline,
        candidates,
        priority_scores,
        quality.deep_max_neighbors,
        quality.deep_priority_threshold,
        quality.deep_confidence_threshold,
        config.print_sir,
        out,
        SIR_GENERATION_PASS_DEEP,
    )
}

fn collect_quality_pass_candidates<F>(
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
    priority_scores: &HashMap<String, f64>,
    should_skip_pass: F,
) -> Result<Vec<QualityPassCandidate>>
where
    F: Fn(&str) -> bool,
{
    let mut candidates = Vec::new();
    for symbol_id in store.list_all_symbol_ids()? {
        let Some(symbol) = symbols_by_id.get(symbol_id.as_str()) else {
            tracing::warn!(
                symbol_id = %symbol_id,
                "Quality pass symbol missing from initial snapshot; skipping"
            );
            continue;
        };
        let Some(meta) = store.get_sir_meta(symbol.id.as_str())? else {
            continue;
        };
        let pass = meta.generation_pass.to_ascii_lowercase();
        if should_skip_pass(pass.as_str()) {
            continue;
        }

        let Some(blob) = store.read_sir_blob(symbol.id.as_str())? else {
            continue;
        };
        let Ok(baseline_sir) = serde_json::from_str::<SirAnnotation>(&blob) else {
            continue;
        };
        candidates.push(QualityPassCandidate {
            symbol: symbol.clone(),
            priority_score: priority_scores
                .get(symbol.id.as_str())
                .copied()
                .unwrap_or(0.0),
            baseline_sir,
        });
    }

    Ok(candidates)
}

fn select_quality_pass_candidates(
    pass_label: &str,
    mut eligible_candidates: Vec<QualityPassCandidate>,
    priority_threshold: f64,
    confidence_threshold: f64,
    max_symbols: usize,
) -> Vec<QualityPassCandidate> {
    let mut candidates = eligible_candidates
        .iter()
        .filter(|candidate| {
            let low_confidence = (candidate.baseline_sir.confidence as f64) <= confidence_threshold;
            let high_priority = candidate.priority_score >= priority_threshold;
            high_priority || low_confidence
        })
        .cloned()
        .collect::<Vec<_>>();

    sort_quality_pass_candidates(&mut candidates);
    if candidates.is_empty() && max_symbols > 0 {
        sort_quality_pass_candidates(&mut eligible_candidates);
        candidates = eligible_candidates.into_iter().take(max_symbols).collect();
        tracing::info!(
            "{pass_label}: threshold selected 0, using top-{max_symbols} by priority as floor"
        );
    } else if max_symbols > 0 && candidates.len() > max_symbols {
        candidates.truncate(max_symbols);
    }

    candidates
}

fn sort_quality_pass_candidates(candidates: &mut [QualityPassCandidate]) {
    candidates.sort_by(|left, right| {
        right
            .priority_score
            .total_cmp(&left.priority_score)
            .then_with(|| left.symbol.id.cmp(&right.symbol.id))
    });
}

fn parse_quality_provider(
    provider_raw: Option<String>,
    field_name: &str,
) -> Result<Option<InferenceProviderKind>> {
    provider_raw
        .map(|provider_raw| {
            provider_raw
                .parse::<InferenceProviderKind>()
                .map_err(|error| anyhow::anyhow!("invalid {field_name} '{provider_raw}': {error}"))
        })
        .transpose()
}

#[allow(clippy::too_many_arguments)]
fn run_quality_pass(
    pass_label: &str,
    store: &SqliteStore,
    pipeline: &SirPipeline,
    candidates: Vec<QualityPassCandidate>,
    priority_scores: &HashMap<String, f64>,
    max_neighbors: usize,
    priority_threshold: f64,
    confidence_threshold: f64,
    print_sir: bool,
    out: &mut dyn std::io::Write,
    generation_pass: &str,
) -> Result<()> {
    let use_cot = pipeline.provider_name() == InferenceProviderKind::Qwen3Local.as_str();
    let total = candidates.len();
    let mut successes = 0usize;
    let mut failures = 0usize;

    for (index, candidate) in candidates.into_iter().enumerate() {
        let enrichment = build_enrichment_context(
            store,
            &candidate.symbol,
            candidate.baseline_sir,
            priority_scores,
            max_neighbors,
            priority_threshold,
            confidence_threshold,
            candidate.priority_score,
        )?;
        let event = SymbolChangeEvent {
            file_path: candidate.symbol.file_path.clone(),
            language: candidate.symbol.language,
            added: Vec::new(),
            removed: Vec::new(),
            updated: vec![candidate.symbol.clone()],
        };
        let mut deep_specs = HashMap::new();
        deep_specs.insert(
            candidate.symbol.id.clone(),
            SirDeepPromptSpec {
                enrichment,
                use_cot,
            },
        );

        match pipeline.process_event_with_deep_specs(
            store,
            &event,
            true,
            print_sir,
            out,
            Some(candidate.priority_score),
            generation_pass,
            &deep_specs,
        ) {
            Ok(stats) => {
                successes += stats.success_count;
                failures += stats.failure_count;
            }
            Err(err) => {
                failures += 1;
                tracing::warn!(
                    symbol_id = %candidate.symbol.id,
                    qualified_name = %candidate.symbol.qualified_name,
                    error = %err,
                    "{pass_label} symbol processing failed"
                );
            }
        }

        tracing::info!(
            "{pass_label}: {}/{} symbols, {} improved, {} failed",
            index + 1,
            total,
            successes,
            failures
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_enrichment_context(
    store: &SqliteStore,
    symbol: &Symbol,
    baseline_sir: SirAnnotation,
    priority_scores: &HashMap<String, f64>,
    max_neighbors: usize,
    priority_threshold: f64,
    confidence_threshold: f64,
    priority_score: f64,
) -> Result<SirEnrichmentContext> {
    let file_rollup_id = synthetic_file_sir_id(symbol.language.as_str(), symbol.file_path.as_str());
    let file_intent = store
        .read_sir_blob(file_rollup_id.as_str())?
        .and_then(|blob| serde_json::from_str::<FileSir>(&blob).ok())
        .map(|sir| sir.intent.trim().to_owned())
        .unwrap_or_default();

    let mut neighbors = Vec::<(f64, String, String)>::new();
    for peer in store.list_symbols_for_file(symbol.file_path.as_str())? {
        if peer.id == symbol.id {
            continue;
        }
        let Some(blob) = store.read_sir_blob(peer.id.as_str())? else {
            continue;
        };
        let Ok(peer_sir) = serde_json::from_str::<SirAnnotation>(&blob) else {
            continue;
        };
        neighbors.push((
            priority_scores
                .get(peer.id.as_str())
                .copied()
                .unwrap_or(0.0),
            peer.qualified_name,
            peer_sir.intent,
        ));
    }
    neighbors.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    if max_neighbors > 0 && neighbors.len() > max_neighbors {
        neighbors.truncate(max_neighbors);
    }

    let neighbor_intents = neighbors
        .into_iter()
        .map(|(_, name, intent)| (name, intent))
        .collect::<Vec<_>>();
    let priority_reason = format_priority_reason(
        store,
        symbol.id.as_str(),
        priority_score,
        baseline_sir.confidence as f64,
        priority_threshold,
        confidence_threshold,
    );

    Ok(SirEnrichmentContext {
        file_intent: Some(file_intent),
        neighbor_intents,
        baseline_sir: Some(baseline_sir),
        priority_reason,
    })
}

fn format_priority_reason(
    store: &SqliteStore,
    symbol_id: &str,
    priority_score: f64,
    confidence: f64,
    priority_threshold: f64,
    confidence_threshold: f64,
) -> String {
    let mut reasons = Vec::<String>::new();
    if priority_score >= priority_threshold {
        reasons.push(format!(
            "priority {:.2} at or above threshold {:.2}",
            priority_score, priority_threshold
        ));
    }
    if confidence <= confidence_threshold {
        reasons.push(format!(
            "baseline confidence {:.2} at or below threshold {:.2}",
            confidence, confidence_threshold
        ));
    }

    if let Ok(Some(metadata)) = store.get_symbol_metadata(symbol_id) {
        if metadata.is_public {
            reasons.push("public API symbol".to_owned());
        }
        let kind = metadata.kind.to_ascii_lowercase();
        if kind == "function" || kind == "method" {
            reasons.push("function/method".to_owned());
        }
    }

    if reasons.is_empty() {
        "selected for deeper analysis".to_owned()
    } else {
        reasons.join(" + ")
    }
}

fn run_initial_index_once_inner(config: &IndexerConfig, skip_teardown: bool) -> Result<()> {
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

pub fn run_initial_index_once(config: &IndexerConfig) -> Result<()> {
    run_initial_index_once_inner(config, false)
}

pub fn run_initial_index_once_for_cli(config: &IndexerConfig) -> Result<()> {
    run_initial_index_once_inner(config, true)
}

pub fn run_indexing_loop(config: IndexerConfig) -> Result<()> {
    let (mut observer, store) = initialize_observer_and_store(&config)?;
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
        match spawn_semantic_worker(worker_id, &config, store.clone(), queue_state.clone()) {
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

    let debounce_window = Duration::from_millis(config.debounce_ms);
    let poll_interval = Duration::from_millis(50);
    let mut debounce_queue = DebounceQueue::default();

    loop {
        match rx.recv_timeout(poll_interval) {
            Ok(result) => {
                if let Ok(ref event) = result {
                    for path in &event.paths {
                        if path.is_dir() && !crate::observer::is_ignored_path(path) {
                            let _ = watcher.watch(path, notify::RecursiveMode::NonRecursive);
                        }
                    }
                }
                if let Err(err) =
                    enqueue_event_paths(&config.workspace, result, &mut debounce_queue)
                {
                    tracing::warn!(error = ?err, "watch event error");
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow::anyhow!("watcher channel disconnected"));
            }
        }

        while let Ok(result) = rx.try_recv() {
            if let Ok(ref event) = result {
                for path in &event.paths {
                    if path.is_dir() && !crate::observer::is_ignored_path(path) {
                        let _ = watcher.watch(path, notify::RecursiveMode::NonRecursive);
                    }
                }
            }
            if let Err(err) = enqueue_event_paths(&config.workspace, result, &mut debounce_queue) {
                tracing::warn!(error = ?err, "watch event error");
            }
        }

        for path in debounce_queue.drain_due(Instant::now(), debounce_window) {
            match observer.process_path(&path) {
                Ok(Some(event)) => {
                    if config.print_events {
                        let line = serde_json::to_string(&event)
                            .context("failed to serialize symbol-change event")?;
                        println!("{line}");
                    }

                    structural.process_event(store.as_ref(), &event)?;
                    for removed in &event.removed {
                        queue_state.remove_symbol(&removed.id);
                    }
                    let mut changed = Vec::new();
                    collect_changed_symbols(&event, &mut changed);
                    for symbol in &changed {
                        queue_state.upsert_symbol(symbol.clone());
                    }
                    if let Err(err) = enqueue_changed_symbols(
                        config.workspace.as_path(),
                        store.as_ref(),
                        &queue_state,
                        &changed,
                    ) {
                        tracing::warn!(
                            file_path = %event.file_path,
                            error = %err,
                            "failed to enqueue changed symbols"
                        );
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::error!(
                    path = %path.display(),
                    error = %err,
                    "process error"
                ),
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
    let sir_pipeline = SirPipeline::new(
        config.workspace.clone(),
        config.sir_concurrency,
        ProviderOverrides {
            provider: config.inference_provider,
            model: config.inference_model.clone(),
            endpoint: config.inference_endpoint.clone(),
            api_key_env: config.inference_api_key_env.clone(),
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

fn spawn_semantic_worker(
    worker_id: usize,
    config: &IndexerConfig,
    store: Arc<SqliteStore>,
    queue_state: SharedQueueState,
) -> Result<()> {
    let pipeline = SirPipeline::new(
        config.workspace.clone(),
        1,
        ProviderOverrides {
            provider: config.inference_provider,
            model: config.inference_model.clone(),
            endpoint: config.inference_endpoint.clone(),
            api_key_env: config.inference_api_key_env.clone(),
        },
    )
    .with_context(|| format!("failed to initialize SIR pipeline for worker {worker_id}"))?;

    std::thread::Builder::new()
        .name(format!("aether-sir-{worker_id}"))
        .spawn(move || {
            loop {
                if let Ok(requested) = store.consume_sir_requests(REQUEST_POLL_BATCH) {
                    for symbol_id in requested {
                        let _ = queue_state.bump_to_front(symbol_id.as_str());
                    }
                }

                let Some((score, symbol)) = queue_state.pop_task() else {
                    std::thread::sleep(Duration::from_millis(WORKER_IDLE_SLEEP_MS));
                    continue;
                };

                let symbol_id = symbol.id.clone();
                let exists = store
                    .get_symbol_record(symbol_id.as_str())
                    .map(|record| record.is_some())
                    .unwrap_or(false);
                if !exists {
                    queue_state.complete_task(symbol_id.as_str());
                    continue;
                }

                let event = SymbolChangeEvent {
                    file_path: symbol.file_path.clone(),
                    language: symbol.language,
                    added: vec![symbol],
                    removed: Vec::new(),
                    updated: Vec::new(),
                };
                let mut sink = std::io::sink();
                let result = pipeline.process_event_with_priority(
                    store.as_ref(),
                    &event,
                    false,
                    false,
                    &mut sink,
                    Some(score),
                );
                if let Err(err) = result {
                    tracing::warn!(
                        symbol_id = %symbol_id,
                        error = %err,
                        "semantic indexing failed for queued symbol"
                    );
                    let _ = queue_state.queue.lock().map(|mut queue| {
                        queue.push(symbol_id.clone(), score.clamp(0.0, 1.0));
                    });
                }

                queue_state.complete_task(symbol_id.as_str());
            }
        })
        .context("failed to spawn semantic worker thread")?;

    Ok(())
}

fn enqueue_symbols_missing_sir(
    workspace: &Path,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    symbols: &[Symbol],
) -> Result<usize> {
    let missing = store.list_symbol_ids_without_sir()?;
    if missing.is_empty() {
        return Ok(0);
    }

    let by_id = symbols
        .iter()
        .cloned()
        .map(|symbol| (symbol.id.clone(), symbol))
        .collect::<HashMap<_, _>>();
    let mut missing_symbols = Vec::new();
    for symbol_id in missing {
        if let Some(symbol) = by_id.get(&symbol_id) {
            missing_symbols.push(symbol.clone());
        }
    }
    enqueue_changed_symbols(workspace, store, queue_state, &missing_symbols)
}

fn enqueue_changed_symbols(
    workspace: &Path,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    changed_symbols: &[Symbol],
) -> Result<usize> {
    if changed_symbols.is_empty() {
        return Ok(0);
    }

    let scores = compute_symbol_priority_scores(workspace, store, changed_symbols);
    let in_progress = queue_state
        .in_progress
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let mut queued = 0usize;

    for symbol in changed_symbols {
        if in_progress.contains(symbol.id.as_str()) {
            continue;
        }
        let score = scores.get(symbol.id.as_str()).copied().unwrap_or(0.0);
        let mut queue = queue_state
            .queue
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if queue.push(symbol.id.clone(), score) {
            queued += 1;
            drop(queue);
            queue_state.upsert_symbol(symbol.clone());
        }
    }

    Ok(queued)
}

pub fn compute_symbol_priority_scores(
    workspace: &Path,
    store: &SqliteStore,
    symbols: &[Symbol],
) -> HashMap<String, f64> {
    let file_paths = symbols
        .iter()
        .map(|symbol| symbol.file_path.clone())
        .collect::<HashSet<_>>();
    let git_scores = collect_git_recency_scores(workspace, &file_paths);
    let page_rank_scores = collect_page_rank_scores(store);

    let mut source_cache = HashMap::<String, String>::new();
    let mut line_count_cache = HashMap::<String, usize>::new();
    let mut scores = HashMap::new();
    for symbol in symbols {
        let git_recency = git_scores
            .get(symbol.file_path.as_str())
            .copied()
            .unwrap_or(0.0);
        let page_rank = page_rank_scores
            .get(symbol.id.as_str())
            .copied()
            .unwrap_or(0.0);
        let line_count = *line_count_cache
            .entry(symbol.file_path.clone())
            .or_insert_with(|| source_line_count(workspace, symbol.file_path.as_str()));
        let source = source_cache
            .entry(symbol.file_path.clone())
            .or_insert_with(|| read_source_file(workspace, symbol.file_path.as_str()));
        let is_public = infer_symbol_is_public(source.as_str(), symbol);
        let kind_score = kind_priority_score(symbol.kind.as_str(), is_public);
        let size_score = size_inverse_score(line_count);
        let score = compute_priority_score(git_recency, page_rank, kind_score, size_score);
        scores.insert(symbol.id.clone(), score);
    }

    scores
}

fn collect_page_rank_scores(store: &SqliteStore) -> HashMap<String, f64> {
    let Ok(edges) = store.list_graph_dependency_edges() else {
        return HashMap::new();
    };
    if edges.is_empty() {
        return HashMap::new();
    }

    let algo_edges = edges
        .into_iter()
        .map(|edge| GraphAlgorithmEdge {
            source_id: edge.source_symbol_id,
            target_id: edge.target_symbol_id,
            edge_kind: edge.edge_kind,
        })
        .collect::<Vec<_>>();

    let ranked = page_rank_sync(&algo_edges, 0.85, 20);
    let max_score = ranked
        .iter()
        .map(|(_, score)| *score)
        .fold(0.0_f64, f64::max);
    if max_score <= f64::EPSILON {
        return ranked
            .into_iter()
            .map(|(symbol_id, _)| (symbol_id, 0.0))
            .collect();
    }

    ranked
        .into_iter()
        .map(|(symbol_id, score)| (symbol_id, (score / max_score).clamp(0.0, 1.0)))
        .collect()
}

fn collect_git_recency_scores(
    workspace: &Path,
    file_paths: &HashSet<String>,
) -> HashMap<String, f64> {
    let Some(context) = GitContext::open(workspace) else {
        return HashMap::new();
    };
    let recent_commit_positions = recent_commit_positions(workspace, 10);
    if recent_commit_positions.is_empty() {
        return HashMap::new();
    }

    let mut scores = HashMap::new();
    for file_path in file_paths {
        let history = context.file_log(Path::new(file_path), 128);
        let mut score = 0.0_f64;
        for commit in history {
            if let Some(position) = recent_commit_positions.get(commit.hash.as_str()) {
                score = (1.0 - (*position as f64 / 10.0)).clamp(0.0, 1.0);
                break;
            }
        }
        scores.insert(file_path.clone(), score);
    }

    scores
}

fn recent_commit_positions(workspace: &Path, limit: usize) -> HashMap<String, usize> {
    let mut positions = HashMap::new();
    if limit == 0 {
        return positions;
    }

    let Ok(repo) = gix::discover(workspace) else {
        return positions;
    };
    let Some(head_id) = repo.head_id().ok().map(|id| id.detach()) else {
        return positions;
    };
    let Ok(walk) = repo
        .rev_walk([head_id])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            CommitTimeOrder::NewestFirst,
        ))
        .all()
    else {
        return positions;
    };

    for (position, entry) in walk.flatten().take(limit).enumerate() {
        positions.insert(entry.id.to_string().to_ascii_lowercase(), position);
    }
    positions
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

struct StructuralIndexer {
    workspace_root: PathBuf,
    extractor: SymbolExtractor,
    test_intent_analyzer: TestIntentAnalyzer,
    graph_runtime: tokio::runtime::Runtime,
    graph_store: Box<dyn aether_store::GraphStore>,
}

impl StructuralIndexer {
    fn new(workspace_root: PathBuf) -> Result<Self> {
        let extractor = SymbolExtractor::new().context("failed to initialize parser")?;
        let test_intent_analyzer = TestIntentAnalyzer::new(&workspace_root)
            .context("failed to initialize test intent analyzer")?;
        let graph_runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("failed to build graph sync runtime")?;
        let graph_store = graph_runtime
            .block_on(open_graph_store(&workspace_root))
            .context("failed to open configured graph store")?;

        Ok(Self {
            workspace_root,
            extractor,
            test_intent_analyzer,
            graph_runtime,
            graph_store,
        })
    }

    fn process_event(&mut self, store: &SqliteStore, event: &SymbolChangeEvent) -> Result<()> {
        for symbol in &event.removed {
            store
                .mark_removed(&symbol.id)
                .with_context(|| format!("failed to mark symbol removed: {}", symbol.id))?;
        }

        let now_ts = unix_timestamp_secs();
        for symbol in event.added.iter().chain(event.updated.iter()) {
            store
                .upsert_symbol(to_symbol_record(symbol, now_ts))
                .with_context(|| format!("failed to upsert symbol {}", symbol.id))?;
        }

        store
            .delete_edges_for_file(&event.file_path)
            .with_context(|| format!("failed to delete edges for file {}", event.file_path))?;

        let full_path = self.workspace_root.join(&event.file_path);
        let source = fs::read_to_string(&full_path);
        match source {
            Ok(source) => {
                let extracted = self
                    .extractor
                    .extract_with_edges_from_path(Path::new(&event.file_path), &source)
                    .with_context(|| format!("failed to extract edges from {}", event.file_path))?;
                store.upsert_edges(&extracted.edges).with_context(|| {
                    format!("failed to upsert edges for file {}", event.file_path)
                })?;
                let now_ms = unix_timestamp_millis();
                let test_intents = extracted
                    .test_intents
                    .into_iter()
                    .map(|intent| to_test_intent_record(intent, now_ms))
                    .collect::<Vec<_>>();
                store
                    .replace_test_intents_for_file(event.file_path.as_str(), &test_intents)
                    .with_context(|| {
                        format!("failed to upsert test intents for file {}", event.file_path)
                    })?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                store
                    .replace_test_intents_for_file(event.file_path.as_str(), &[])
                    .with_context(|| {
                        format!("failed to clear test intents for file {}", event.file_path)
                    })?;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "failed to read source for edge extraction {}",
                        full_path.display()
                    )
                });
            }
        }

        let _ = self
            .test_intent_analyzer
            .refresh_for_test_file(event.file_path.as_str())
            .with_context(|| {
                format!(
                    "failed to refresh tested_by links for test file {}",
                    event.file_path
                )
            })?;

        let stats = self
            .graph_runtime
            .block_on(store.sync_graph_for_file(self.graph_store.as_ref(), &event.file_path))
            .with_context(|| format!("failed to sync graph edges for {}", event.file_path))?;
        if stats.unresolved_edges > 0 {
            tracing::debug!(
                file_path = %event.file_path,
                resolved_edges = stats.resolved_edges,
                unresolved_edges = stats.unresolved_edges,
                "graph sync skipped unresolved call edges"
            );
        }

        Ok(())
    }
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

fn to_test_intent_record(intent: TestIntent, now_ms: i64) -> TestIntentRecord {
    let material = format!(
        "{}\n{}\n{}",
        intent.file_path.trim(),
        intent.test_name.trim(),
        intent.intent_text.trim(),
    );
    TestIntentRecord {
        intent_id: content_hash(material.as_str()),
        file_path: normalize_path(intent.file_path.as_str()),
        test_name: intent.test_name,
        intent_text: intent.intent_text,
        group_label: intent.group_label,
        language: intent.language.as_str().to_owned(),
        symbol_id: intent.symbol_id,
        created_at: now_ms.max(0),
        updated_at: now_ms.max(0),
    }
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

fn enqueue_event_paths(
    workspace: &PathBuf,
    event: notify::Result<Event>,
    queue: &mut DebounceQueue,
) -> Result<()> {
    let event = event.context("notify error")?;
    let now = Instant::now();

    for path in event.paths {
        if is_ignored_path(&path) {
            continue;
        }

        if let Ok(relative) = path.strip_prefix(workspace)
            && is_ignored_path(relative)
        {
            continue;
        }

        if path.is_dir() {
            continue;
        }

        queue.mark(path, now);
    }

    Ok(())
}
