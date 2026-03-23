use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    SirMetaRecord, SirStateStore, SqliteStore, SurrealGraphStore, SymbolCatalogStore,
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
    SirPipeline, build_job,
};

const REQUEST_POLL_BATCH: usize = 128;
const WORKER_IDLE_SLEEP_MS: u64 = 200;
const RECONCILE_DUPLICATE_STALE_REASON: &str = "duplicate stale symbol matched current snapshot";
const RECONCILE_AMBIGUOUS_CURRENT_REASON: &str =
    "multiple current symbols matched reconciliation tuple";
const RECONCILE_MISSING_CURRENT_REASON: &str = "no current symbol matched reconciliation tuple";

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

#[derive(Debug, Default)]
struct GitDebounceState {
    last_git_event_at: Option<Instant>,
    dirty_paths: HashSet<PathBuf>,
}

impl GitDebounceState {
    fn mark_git_event(&mut self, now: Instant) {
        self.last_git_event_at = Some(now);
    }

    fn has_pending(&self) -> bool {
        self.last_git_event_at.is_some()
    }

    fn extend_dirty<I>(&mut self, paths: I)
    where
        I: IntoIterator<Item = PathBuf>,
    {
        self.dirty_paths.extend(paths);
    }

    fn should_fire(&self, now: Instant, debounce_window: Duration) -> bool {
        self.last_git_event_at
            .is_some_and(|last_seen| now.saturating_duration_since(last_seen) >= debounce_window)
    }

    fn take_dirty_paths(&mut self) -> HashSet<PathBuf> {
        self.last_git_event_at = None;
        std::mem::take(&mut self.dirty_paths)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HeadState {
    sha: Option<String>,
    marker: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitTriggerKind {
    BranchSwitch,
    GitPull,
    Merge,
}

#[derive(Debug, Default)]
struct ClassifiedWatchEvent {
    git_event: bool,
    source_paths: Vec<PathBuf>,
    watch_dirs: Vec<PathBuf>,
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

type ReconciliationKey = (String, String, String);

#[derive(Debug, Clone)]
struct PlannedMigration {
    old_symbol: SymbolRecord,
    new_symbol: Symbol,
}

#[derive(Debug, Clone)]
struct PlannedPrune {
    symbol: SymbolRecord,
    reason: &'static str,
}

#[derive(Debug, Clone, Default)]
struct ReconciliationPlan {
    migrations: Vec<PlannedMigration>,
    prunes: Vec<PlannedPrune>,
    cleanup_ids: Vec<String>,
}

impl ReconciliationPlan {
    fn is_empty(&self) -> bool {
        self.migrations.is_empty() && self.prunes.is_empty()
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

fn symbol_reconciliation_key(symbol: &Symbol) -> ReconciliationKey {
    (
        symbol.file_path.clone(),
        symbol.qualified_name.clone(),
        symbol.kind.as_str().to_owned(),
    )
}

fn symbol_record_reconciliation_key(symbol: &SymbolRecord) -> ReconciliationKey {
    (
        symbol.file_path.clone(),
        symbol.qualified_name.clone(),
        symbol.kind.clone(),
    )
}

fn stale_symbol_updated_at(store: &SqliteStore, symbol_id: &str) -> Result<i64> {
    Ok(store
        .get_sir_meta(symbol_id)?
        .map(|meta| meta.updated_at.max(0))
        .unwrap_or(0))
}

fn plan_symbol_reconciliation(
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
) -> Result<ReconciliationPlan> {
    let snapshot_ids = symbols_by_id.keys().cloned().collect::<HashSet<_>>();
    let stale_symbols = store
        .list_stale_symbols(&snapshot_ids)
        .context("failed to list stale symbols for reconciliation")?;
    if stale_symbols.is_empty() {
        return Ok(ReconciliationPlan::default());
    }

    let mut current_by_key = BTreeMap::<ReconciliationKey, Vec<Symbol>>::new();
    for symbol in symbols_by_id.values() {
        current_by_key
            .entry(symbol_reconciliation_key(symbol))
            .or_default()
            .push(symbol.clone());
    }

    let mut stale_by_key = BTreeMap::<ReconciliationKey, Vec<SymbolRecord>>::new();
    let mut updated_at_by_id = HashMap::<String, i64>::new();
    for symbol in stale_symbols {
        updated_at_by_id.insert(
            symbol.id.clone(),
            stale_symbol_updated_at(store, symbol.id.as_str())?,
        );
        stale_by_key
            .entry(symbol_record_reconciliation_key(&symbol))
            .or_default()
            .push(symbol);
    }

    let mut plan = ReconciliationPlan::default();
    for (key, mut stale_matches) in stale_by_key {
        let current_matches = current_by_key.get(&key).cloned().unwrap_or_default();
        match current_matches.len() {
            0 => {
                plan.prunes
                    .extend(stale_matches.into_iter().map(|symbol| PlannedPrune {
                        symbol,
                        reason: RECONCILE_MISSING_CURRENT_REASON,
                    }));
            }
            1 => {
                stale_matches.sort_by(|left, right| {
                    updated_at_by_id
                        .get(right.id.as_str())
                        .copied()
                        .unwrap_or(0)
                        .cmp(&updated_at_by_id.get(left.id.as_str()).copied().unwrap_or(0))
                        .then_with(|| right.last_seen_at.cmp(&left.last_seen_at))
                        .then_with(|| left.id.cmp(&right.id))
                });

                let current_symbol = current_matches[0].clone();
                let winner = stale_matches.remove(0);
                if !stale_matches.is_empty() {
                    tracing::warn!(
                        file_path = %winner.file_path,
                        qualified_name = %winner.qualified_name,
                        current_symbol_id = %current_symbol.id,
                        duplicate_count = stale_matches.len(),
                        "reconciliation found duplicate stale symbols for a single current symbol"
                    );
                }
                plan.migrations.push(PlannedMigration {
                    old_symbol: winner,
                    new_symbol: current_symbol,
                });
                plan.prunes
                    .extend(stale_matches.into_iter().map(|symbol| PlannedPrune {
                        symbol,
                        reason: RECONCILE_DUPLICATE_STALE_REASON,
                    }));
            }
            _ => {
                tracing::warn!(
                    file_path = %key.0,
                    qualified_name = %key.1,
                    kind = %key.2,
                    current_match_count = current_matches.len(),
                    stale_match_count = stale_matches.len(),
                    "reconciliation found ambiguous current snapshot matches; pruning stale symbols"
                );
                plan.prunes
                    .extend(stale_matches.into_iter().map(|symbol| PlannedPrune {
                        symbol,
                        reason: RECONCILE_AMBIGUOUS_CURRENT_REASON,
                    }));
            }
        }
    }

    plan.migrations.sort_by(|left, right| {
        left.old_symbol
            .file_path
            .cmp(&right.old_symbol.file_path)
            .then_with(|| {
                left.old_symbol
                    .qualified_name
                    .cmp(&right.old_symbol.qualified_name)
            })
            .then_with(|| left.old_symbol.id.cmp(&right.old_symbol.id))
    });
    plan.prunes.sort_by(|left, right| {
        left.symbol
            .file_path
            .cmp(&right.symbol.file_path)
            .then_with(|| left.symbol.qualified_name.cmp(&right.symbol.qualified_name))
            .then_with(|| left.symbol.id.cmp(&right.symbol.id))
    });

    let mut cleanup_ids = plan
        .migrations
        .iter()
        .map(|entry| entry.old_symbol.id.clone())
        .chain(plan.prunes.iter().map(|entry| entry.symbol.id.clone()))
        .collect::<Vec<_>>();
    cleanup_ids.sort();
    cleanup_ids.dedup();
    plan.cleanup_ids = cleanup_ids;

    Ok(plan)
}

fn print_reconciliation_dry_run(plan: &ReconciliationPlan, out: &mut dyn Write) -> Result<()> {
    if plan.is_empty() {
        writeln!(out, "DRY_RUN reconciliation: no stale symbols found")
            .context("failed to write dry-run reconciliation summary")?;
        return Ok(());
    }

    for migration in &plan.migrations {
        writeln!(
            out,
            "DRY_RUN migrate file={} symbol={} old_id={} new_id={}",
            migration.old_symbol.file_path,
            migration.old_symbol.qualified_name,
            migration.old_symbol.id,
            migration.new_symbol.id
        )
        .context("failed to write dry-run migration line")?;
    }

    for prune in &plan.prunes {
        writeln!(
            out,
            "DRY_RUN prune file={} symbol={} old_id={} reason={}",
            prune.symbol.file_path, prune.symbol.qualified_name, prune.symbol.id, prune.reason
        )
        .context("failed to write dry-run prune line")?;
    }

    writeln!(
        out,
        "DRY_RUN reconciliation summary: {} migrations, {} prunes",
        plan.migrations.len(),
        plan.prunes.len()
    )
    .context("failed to write dry-run reconciliation summary")?;
    Ok(())
}

fn execute_symbol_reconciliation<GraphCleanup, VectorCleanup>(
    store: &SqliteStore,
    plan: &ReconciliationPlan,
    graph_cleanup: GraphCleanup,
    vector_cleanup: VectorCleanup,
) -> Result<(usize, usize)>
where
    GraphCleanup: FnOnce(&[String]) -> Result<()>,
    VectorCleanup: FnOnce(&[String]) -> Result<()>,
{
    let migrations = plan
        .migrations
        .iter()
        .map(|entry| (entry.old_symbol.id.clone(), entry.new_symbol.id.clone()))
        .collect::<Vec<_>>();
    let prunes = plan
        .prunes
        .iter()
        .map(|entry| entry.symbol.id.clone())
        .collect::<Vec<_>>();

    let (migrated, pruned) = store
        .reconcile_and_prune(&migrations, &prunes)
        .context("failed to reconcile stale symbols")?;

    for migration in &plan.migrations {
        tracing::info!(
            qualified_name = %migration.old_symbol.qualified_name,
            file_path = %migration.old_symbol.file_path,
            old_id = %migration.old_symbol.id,
            new_id = %migration.new_symbol.id,
            "reconciled stale symbol ID"
        );
    }

    if !plan.cleanup_ids.is_empty() {
        graph_cleanup(&plan.cleanup_ids).context("failed to clean stale graph symbols")?;
        if let Err(err) = vector_cleanup(&plan.cleanup_ids) {
            tracing::warn!(
                error = %err,
                cleanup_count = plan.cleanup_ids.len(),
                "vector cleanup failed during reconciliation; embeddings will regenerate on the next pass"
            );
        }
    }

    tracing::info!(
        migrated,
        pruned,
        "Reconciliation complete: {migrated} migrated, {pruned} pruned"
    );
    Ok((migrated, pruned))
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
    contracts_enabled: bool,
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
            thinking: quality.triage_thinking.clone(),
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
        false,
        contracts_enabled,
    )
}

fn run_deep_pass(
    config: &IndexerConfig,
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
    priority_scores: &HashMap<String, f64>,
    quality: &aether_config::SirQualityConfig,
    out: &mut dyn std::io::Write,
    contracts_enabled: bool,
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
            thinking: quality.deep_thinking.clone(),
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
        true,
        contracts_enabled,
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
    use_cot: bool,
    contracts_enabled: bool,
) -> Result<()> {
    let use_cot = use_cot && pipeline.provider_name() == InferenceProviderKind::Qwen3Local.as_str();
    let total = candidates.len();
    let mut batch_items = Vec::with_capacity(total);

    for candidate in candidates {
        let enrichment = build_enrichment_context(
            store,
            &candidate.symbol,
            Some(candidate.baseline_sir),
            priority_scores,
            max_neighbors,
            priority_threshold,
            confidence_threshold,
            candidate.priority_score,
            contracts_enabled,
        )?;
        batch_items.push(QualityBatchItem {
            symbol: candidate.symbol,
            priority_score: candidate.priority_score,
            enrichment,
            use_cot,
        });
    }

    tracing::info!("{pass_label}: submitting {total} symbols for concurrent inference");

    let stats =
        pipeline.process_quality_batch(store, batch_items, generation_pass, print_sir, out)?;

    tracing::info!(
        "{pass_label}: complete - {} improved, {} failed out of {} total",
        stats.success_count,
        stats.failure_count,
        total
    );

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn build_enrichment_context(
    store: &SqliteStore,
    symbol: &Symbol,
    baseline_sir: Option<SirAnnotation>,
    priority_scores: &HashMap<String, f64>,
    max_neighbors: usize,
    priority_threshold: f64,
    confidence_threshold: f64,
    priority_score: f64,
    contracts_enabled: bool,
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
    let baseline_confidence = baseline_sir
        .as_ref()
        .map(|sir| sir.confidence as f64)
        .unwrap_or(0.0);
    let priority_reason = format_priority_reason(
        store,
        symbol.id.as_str(),
        priority_score,
        baseline_confidence,
        priority_threshold,
        confidence_threshold,
    );

    let caller_contract_clauses = if contracts_enabled {
        lookup_caller_contracts(store, symbol.qualified_name.as_str())
    } else {
        Vec::new()
    };

    Ok(SirEnrichmentContext {
        file_intent: Some(file_intent),
        neighbor_intents,
        baseline_sir,
        priority_reason,
        caller_contract_clauses,
    })
}

/// Look up contract clauses from callers of the given symbol.
///
/// For each symbol that calls `qualified_name` and has active contracts,
/// collect (caller_qualified_name, clause_type, clause_text) triples.
fn lookup_caller_contracts(
    store: &SqliteStore,
    qualified_name: &str,
) -> Vec<(String, String, String)> {
    let callers = match store.get_callers(qualified_name) {
        Ok(edges) => edges,
        Err(_) => return Vec::new(),
    };

    let mut result = Vec::new();
    for edge in callers {
        let contracts = match store.list_active_contracts_for_symbol(edge.source_id.as_str()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if contracts.is_empty() {
            continue;
        }
        // Resolve caller qualified name from symbol record
        let caller_name = store
            .get_symbol_record(edge.source_id.as_str())
            .ok()
            .flatten()
            .map(|s| s.qualified_name)
            .unwrap_or_else(|| edge.source_id.clone());
        for contract in contracts {
            result.push((
                caller_name.clone(),
                contract.clause_type,
                contract.clause_text,
            ));
        }
    }
    result
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

fn handle_watch_result(
    workspace: &Path,
    git_watch_dir: Option<&Path>,
    watcher: &mut RecommendedWatcher,
    result: notify::Result<Event>,
    debounce_queue: &mut DebounceQueue,
    git_debounce_state: &mut GitDebounceState,
) -> Result<()> {
    let event = result.context("notify error")?;
    let classified = classify_watch_event(workspace, git_watch_dir, event);
    for path in &classified.watch_dirs {
        let _ = watcher.watch(path, RecursiveMode::NonRecursive);
    }
    enqueue_classified_watch_event(classified, debounce_queue, git_debounce_state);
    Ok(())
}

fn classify_watch_event(
    workspace: &Path,
    git_watch_dir: Option<&Path>,
    event: Event,
) -> ClassifiedWatchEvent {
    let mut classified = ClassifiedWatchEvent::default();
    for path in event.paths {
        if git_watch_dir.is_some_and(|git_dir| path.starts_with(git_dir)) {
            classified.git_event = true;
            continue;
        }

        if path.is_dir() {
            if !is_ignored_path(&path) {
                classified.watch_dirs.push(path);
            }
            continue;
        }

        if is_ignored_path(&path) {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(workspace)
            && is_ignored_path(relative)
        {
            continue;
        }

        classified.source_paths.push(path);
    }

    classified
}

fn enqueue_classified_watch_event(
    classified: ClassifiedWatchEvent,
    debounce_queue: &mut DebounceQueue,
    git_debounce_state: &mut GitDebounceState,
) {
    let now = Instant::now();
    if classified.git_event {
        git_debounce_state.mark_git_event(now);
    }

    if git_debounce_state.has_pending() {
        git_debounce_state.extend_dirty(classified.source_paths);
    } else {
        for path in classified.source_paths {
            debounce_queue.mark(path, now);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_git_trigger(
    config: &IndexerConfig,
    watcher_runtime: &WatcherRuntimeConfig,
    git_watch_dir: Option<&Path>,
    observer: &mut ObserverState,
    structural: &mut StructuralIndexer,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    dirty_paths: HashSet<PathBuf>,
) -> Result<()> {
    let previous_head = read_persisted_head_state(config.workspace.as_path())?;
    let current_head = read_current_head_state(config.workspace.as_path(), git_watch_dir)?;
    let mut paths = dirty_paths.into_iter().collect::<BTreeSet<_>>();

    if previous_head.sha != current_head.sha {
        let trigger_kind =
            classify_git_trigger_kind(config.workspace.as_path(), &previous_head, &current_head);
        if trigger_kind.is_some_and(|kind| git_trigger_enabled(&watcher_runtime.watcher, kind)) {
            let git_paths = if !watcher_runtime.watcher.git_trigger_changed_files_only
                || previous_head.sha.is_none()
            {
                collect_full_reindex_paths(config.workspace.as_path(), observer)
            } else if let (Some(old_sha), Some(new_sha)) =
                (previous_head.sha.as_deref(), current_head.sha.as_deref())
            {
                match changed_paths_between_heads(config.workspace.as_path(), old_sha, new_sha) {
                    Ok(paths) => paths,
                    Err(err) => {
                        tracing::warn!(
                            old_sha,
                            new_sha,
                            error = %err,
                            "failed to diff git heads; falling back to full reindex"
                        );
                        collect_full_reindex_paths(config.workspace.as_path(), observer)
                    }
                }
            } else {
                collect_full_reindex_paths(config.workspace.as_path(), observer)
            };
            paths.extend(git_paths);
        }
    }

    if !paths.is_empty() {
        tracing::info!(path_count = paths.len(), "processing watcher reindex batch");
        process_reindex_paths(
            config,
            observer,
            structural,
            store,
            queue_state,
            paths.into_iter().collect::<Vec<_>>(),
        )?;
    }

    write_persisted_head_state(config.workspace.as_path(), &current_head)?;
    Ok(())
}

fn process_reindex_paths(
    config: &IndexerConfig,
    observer: &mut ObserverState,
    structural: &mut StructuralIndexer,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    paths: Vec<PathBuf>,
) -> Result<()> {
    for path in paths {
        process_reindex_path(config, observer, structural, store, queue_state, &path)?;
    }
    Ok(())
}

fn process_reindex_path(
    config: &IndexerConfig,
    observer: &mut ObserverState,
    structural: &mut StructuralIndexer,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    path: &Path,
) -> Result<()> {
    match observer.process_path(path) {
        Ok(Some(event)) => {
            if config.print_events {
                let line = serde_json::to_string(&event)
                    .context("failed to serialize symbol-change event")?;
                println!("{line}");
            }

            structural.process_event(store, &event)?;
            for removed in &event.removed {
                queue_state.remove_symbol(&removed.id);
            }
            let mut changed = Vec::new();
            collect_changed_symbols(&event, &mut changed);
            for symbol in &changed {
                queue_state.upsert_symbol(symbol.clone());
            }
            if let Err(err) =
                enqueue_changed_symbols(config.workspace.as_path(), store, queue_state, &changed)
            {
                tracing::warn!(
                    file_path = %event.file_path,
                    error = %err,
                    "failed to enqueue changed symbols"
                );
            }
        }
        Ok(None) => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to process {}", path.display()));
        }
    }

    Ok(())
}

fn resolve_git_watch_dir(workspace: &Path) -> Option<PathBuf> {
    let dot_git = workspace.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }

    let raw = fs::read_to_string(&dot_git).ok()?;
    let git_dir = raw.strip_prefix("gitdir:")?.trim();
    let path = PathBuf::from(git_dir);
    Some(if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    })
}

fn read_persisted_head_state(workspace: &Path) -> Result<HeadState> {
    let aether_dir = workspace.join(".aether");
    Ok(HeadState {
        sha: read_optional_trimmed_file(aether_dir.join("last_indexed_head"))?,
        marker: read_optional_trimmed_file(aether_dir.join("last_indexed_head_ref"))?,
    })
}

fn write_persisted_head_state(workspace: &Path, head_state: &HeadState) -> Result<()> {
    let aether_dir = workspace.join(".aether");
    fs::create_dir_all(&aether_dir).with_context(|| {
        format!(
            "failed to create watcher state directory {}",
            aether_dir.display()
        )
    })?;
    write_optional_trimmed_file(
        aether_dir.join("last_indexed_head"),
        head_state.sha.as_deref(),
    )?;
    write_optional_trimmed_file(
        aether_dir.join("last_indexed_head_ref"),
        head_state.marker.as_deref(),
    )?;
    Ok(())
}

fn read_current_head_state(workspace: &Path, git_watch_dir: Option<&Path>) -> Result<HeadState> {
    let marker = git_watch_dir
        .map(|git_dir| git_dir.join("HEAD"))
        .map(read_optional_trimmed_file)
        .transpose()?
        .flatten();
    let sha = GitContext::open(workspace)
        .and_then(|context| context.head_commit_hash())
        .or_else(|| {
            marker
                .as_deref()
                .filter(|value| !value.starts_with("ref:"))
                .map(str::to_owned)
        });
    Ok(HeadState { sha, marker })
}

fn read_optional_trimmed_file(path: PathBuf) -> Result<Option<String>> {
    match fs::read_to_string(&path) {
        Ok(raw) => Ok({
            let trimmed = raw.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err)
            .with_context(|| format!("failed to read watcher state file {}", path.display())),
    }
}

fn write_optional_trimmed_file(path: PathBuf, value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        fs::write(&path, format!("{}\n", value.trim()))
            .with_context(|| format!("failed to write watcher state file {}", path.display()))?;
    } else if let Err(err) = fs::remove_file(&path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        return Err(err)
            .with_context(|| format!("failed to remove watcher state file {}", path.display()));
    }
    Ok(())
}

fn classify_git_trigger_kind(
    workspace: &Path,
    previous_head: &HeadState,
    current_head: &HeadState,
) -> Option<GitTriggerKind> {
    let previous_sha = previous_head.sha.as_deref()?;
    let current_sha = current_head.sha.as_deref()?;
    if previous_sha == current_sha {
        return None;
    }
    if previous_head.marker.as_deref() != current_head.marker.as_deref() {
        return Some(GitTriggerKind::BranchSwitch);
    }
    match current_commit_parent_count(workspace, current_sha) {
        Ok(parent_count) if parent_count > 1 => Some(GitTriggerKind::Merge),
        Ok(_) => Some(GitTriggerKind::GitPull),
        Err(err) => {
            tracing::warn!(
                current_sha,
                error = %err,
                "failed to classify git head advance; treating as git pull"
            );
            Some(GitTriggerKind::GitPull)
        }
    }
}

fn current_commit_parent_count(workspace: &Path, commit_hash: &str) -> Result<usize> {
    let repo = gix::discover(workspace).context("failed to open git repo")?;
    let commit_id = repo
        .rev_parse_single(commit_hash)
        .with_context(|| format!("failed to resolve commit {commit_hash}"))?;
    let commit = repo
        .find_commit(commit_id.detach())
        .with_context(|| format!("failed to load commit {commit_hash}"))?;
    Ok(commit.parent_ids().count())
}

fn git_trigger_enabled(watcher: &WatcherConfig, trigger: GitTriggerKind) -> bool {
    match trigger {
        GitTriggerKind::BranchSwitch => watcher.trigger_on_branch_switch,
        GitTriggerKind::GitPull => watcher.trigger_on_git_pull,
        GitTriggerKind::Merge => watcher.trigger_on_merge,
    }
}

fn changed_paths_between_heads(
    workspace: &Path,
    old_sha: &str,
    new_sha: &str,
) -> Result<Vec<PathBuf>> {
    let repo = gix::discover(workspace).context("failed to open git repo for diff")?;
    let old_id = repo
        .rev_parse_single(old_sha)
        .with_context(|| format!("failed to resolve previous head {old_sha}"))?;
    let new_id = repo
        .rev_parse_single(new_sha)
        .with_context(|| format!("failed to resolve current head {new_sha}"))?;
    let old_commit = repo
        .find_commit(old_id.detach())
        .with_context(|| format!("failed to load previous head commit {old_sha}"))?;
    let new_commit = repo
        .find_commit(new_id.detach())
        .with_context(|| format!("failed to load current head commit {new_sha}"))?;
    let old_tree = old_commit
        .tree()
        .with_context(|| format!("failed to load previous head tree {old_sha}"))?;
    let new_tree = new_commit
        .tree()
        .with_context(|| format!("failed to load current head tree {new_sha}"))?;

    let mut diff_options = gix::diff::Options::default();
    diff_options.track_rewrites(None);
    let changes = repo
        .diff_tree_to_tree(Some(&old_tree), Some(&new_tree), Some(diff_options))
        .with_context(|| format!("failed to diff git heads {old_sha}..{new_sha}"))?;

    let mut paths = BTreeSet::new();
    for change in changes {
        let raw_path = change.location().to_str_lossy();
        let normalized = normalize_path(normalize_git_rename_path(raw_path.as_ref()).as_str());
        if normalized.is_empty() || is_ignored_path(Path::new(&normalized)) {
            continue;
        }
        paths.insert(workspace.join(normalized));
    }

    Ok(paths.into_iter().collect())
}

fn normalize_git_rename_path(path: &str) -> String {
    let value = path.trim();
    if let (Some(brace_start), Some(brace_end)) = (value.find('{'), value.find('}'))
        && brace_start < brace_end
    {
        let prefix = &value[..brace_start];
        let inner = &value[brace_start + 1..brace_end];
        let suffix = &value[brace_end + 1..];
        if let Some((_, new_part)) = inner.split_once("=>") {
            return format!("{}{}{}", prefix, new_part.trim(), suffix);
        }
    }
    if let Some((_, right)) = value.rsplit_once("=>") {
        return right.trim().to_owned();
    }
    value.to_owned()
}

fn collect_full_reindex_paths(workspace: &Path, observer: &ObserverState) -> Vec<PathBuf> {
    let mut paths = observer
        .tracked_paths()
        .into_iter()
        .collect::<BTreeSet<_>>();
    for entry in WalkBuilder::new(workspace).standard_filters(true).build() {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        if is_ignored_path(entry.path()) || language_for_path(entry.path()).is_none() {
            continue;
        }
        paths.insert(entry.path().to_path_buf());
    }
    paths.into_iter().collect()
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

fn spawn_semantic_worker(
    worker_id: usize,
    config: &IndexerConfig,
    watcher_runtime: &WatcherRuntimeConfig,
    store: Arc<SqliteStore>,
    queue_state: SharedQueueState,
) -> Result<()> {
    let pipeline = SirPipeline::new(
        config.workspace.clone(),
        1,
        watcher_runtime.provider_overrides(config),
    )
    .with_context(|| format!("failed to initialize SIR pipeline for worker {worker_id}"))?;
    let generation_pass = watcher_runtime.generation_pass;
    let prompt_config_fingerprint = watcher_runtime.prompt_config_fingerprint(&pipeline);
    let workspace_root = config.workspace.clone();

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

                let previous_meta = store.get_sir_meta(symbol_id.as_str()).ok().flatten();
                let previous_embedding = pipeline
                    .load_symbol_embedding(symbol_id.as_str())
                    .ok()
                    .flatten();
                let event = SymbolChangeEvent {
                    file_path: symbol.file_path.clone(),
                    language: symbol.language,
                    added: vec![symbol.clone()],
                    removed: Vec::new(),
                    updated: Vec::new(),
                };
                let mut sink = std::io::sink();
                let result = pipeline.process_event_with_priority_and_pass(
                    store.as_ref(),
                    &event,
                    false,
                    false,
                    &mut sink,
                    Some(score),
                    generation_pass,
                );
                match result {
                    Ok(stats) => {
                        if stats.success_count > 0
                            && let Err(err) = finalize_watcher_generation(
                                &pipeline,
                                workspace_root.as_path(),
                                store.as_ref(),
                                &symbol,
                                previous_meta.as_ref(),
                                previous_embedding.as_ref(),
                                generation_pass,
                                &prompt_config_fingerprint,
                            )
                        {
                            tracing::warn!(
                                symbol_id = %symbol_id,
                                error = %err,
                                "failed to record watcher prompt hash and fingerprint history"
                            );
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            symbol_id = %symbol_id,
                            error = %err,
                            "semantic indexing failed for queued symbol"
                        );
                        let _ = queue_state.queue.lock().map(|mut queue| {
                            queue.push(symbol_id.clone(), score.clamp(0.0, 1.0));
                        });
                    }
                }

                queue_state.complete_task(symbol_id.as_str());
            }
        })
        .context("failed to spawn semantic worker thread")?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finalize_watcher_generation(
    pipeline: &SirPipeline,
    workspace_root: &Path,
    store: &SqliteStore,
    symbol: &Symbol,
    previous_meta: Option<&SirMetaRecord>,
    previous_embedding: Option<&SymbolEmbeddingRecord>,
    generation_pass: &str,
    prompt_config_fingerprint: &str,
) -> Result<()> {
    let job = build_job(workspace_root, symbol.clone(), None, None)
        .with_context(|| format!("failed to rebuild prompt hash input for {}", symbol.id))?;
    let prompt_hash = compute_prompt_hash(job.symbol_text.as_str(), &[], prompt_config_fingerprint);

    let current_meta = store
        .get_sir_meta(symbol.id.as_str())
        .with_context(|| format!("failed to reload SIR metadata for {}", symbol.id))?
        .ok_or_else(|| anyhow::anyhow!("missing SIR metadata for {}", symbol.id))?;
    store
        .upsert_sir_meta(SirMetaRecord {
            prompt_hash: Some(prompt_hash.clone()),
            ..current_meta
        })
        .with_context(|| format!("failed to persist watcher prompt hash for {}", symbol.id))?;
    let current_embedding = pipeline
        .load_symbol_embedding(symbol.id.as_str())
        .with_context(|| format!("failed to reload embedding for {}", symbol.id))?;
    write_fingerprint_row(
        store,
        symbol.id.as_str(),
        prompt_hash.as_str(),
        previous_meta.and_then(|meta| meta.prompt_hash.as_deref()),
        "watcher",
        pipeline.model_name(),
        generation_pass,
        cosine_distance_from_embeddings(previous_embedding, current_embedding.as_ref()),
    )
    .with_context(|| format!("failed to write watcher fingerprint row for {}", symbol.id))
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
    surreal_graph_store: Option<SurrealGraphStore>,
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
        let surreal_graph_store = open_surreal_graph_store_sync(&workspace_root).ok();

        Ok(Self {
            workspace_root,
            extractor,
            test_intent_analyzer,
            graph_runtime,
            graph_store,
            surreal_graph_store,
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

        if let Some(graph) = self.surreal_graph_store.as_ref() {
            let _ = self
                .test_intent_analyzer
                .refresh_for_test_file_with_graph(graph, event.file_path.as_str())
                .with_context(|| {
                    format!(
                        "failed to refresh tested_by links for test file {}",
                        event.file_path
                    )
                })?;
        }

        let stats = self
            .graph_runtime
            .block_on(store.sync_graph_for_file(self.graph_store.as_ref(), &event.file_path))
            .with_context(|| format!("failed to sync graph edges for {}", event.file_path))?;
        if stats.unresolved_edges > 0 {
            tracing::debug!(
                file_path = %event.file_path,
                resolved_edges = stats.resolved_edges,
                unresolved_edges = stats.unresolved_edges,
                "graph sync skipped unresolved structural edges"
            );
        }

        Ok(())
    }

    fn delete_symbols_batch(&self, symbol_ids: &[String]) -> Result<()> {
        self.graph_runtime
            .block_on(self.graph_store.delete_symbols_batch(symbol_ids))
            .context("failed to delete stale graph symbols")
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

#[cfg(test)]
mod tests {
    use std::fs;

    use aether_config::WatcherConfig;
    use aether_core::{Language, Position, SourceRange, SymbolKind};
    use tempfile::tempdir;

    use super::*;

    fn test_symbol(symbol_id: &str, signature: &str) -> Symbol {
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

    fn write_default_config(workspace: &Path) {
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

    fn upsert_symbol_snapshot(store: &SqliteStore, symbol: &Symbol, last_seen_at: i64) {
        store
            .upsert_symbol(to_symbol_record(symbol, last_seen_at))
            .expect("upsert symbol snapshot");
    }

    fn upsert_sir_state(
        store: &SqliteStore,
        symbol_id: &str,
        sir_hash: &str,
        sir_json: &str,
        updated_at: i64,
    ) {
        let version = store
            .record_sir_version_if_changed(
                symbol_id,
                sir_hash,
                "mock",
                "mock-model",
                sir_json,
                updated_at,
                None,
            )
            .expect("record SIR history");
        store
            .write_sir_blob(symbol_id, sir_json)
            .expect("write SIR blob");
        store
            .upsert_sir_meta(aether_store::SirMetaRecord {
                id: symbol_id.to_owned(),
                sir_hash: sir_hash.to_owned(),
                sir_version: version.version,
                provider: "mock".to_owned(),
                model: "mock-model".to_owned(),
                generation_pass: "scan".to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at: version.updated_at,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: version.updated_at,
            })
            .expect("upsert SIR metadata");
    }

    #[test]
    fn reconcile_picks_most_recent_on_ambiguity() {
        let temp = tempdir().expect("tempdir");
        write_default_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");

        let old_a = test_symbol("sym-old-a", "sig-old-a");
        let old_b = test_symbol("sym-old-b", "sig-old-b");
        let new_symbol = test_symbol("sym-new", "sig-new");

        upsert_symbol_snapshot(&store, &old_a, 10);
        upsert_symbol_snapshot(&store, &old_b, 20);
        upsert_symbol_snapshot(&store, &new_symbol, 30);
        upsert_sir_state(
            &store,
            old_a.id.as_str(),
            "hash-old-a",
            r#"{"intent":"old-a"}"#,
            100,
        );
        upsert_sir_state(
            &store,
            old_b.id.as_str(),
            "hash-old-b",
            r#"{"intent":"old-b"}"#,
            200,
        );

        let symbols_by_id = HashMap::from_iter([(new_symbol.id.clone(), new_symbol.clone())]);
        let plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("plan reconcile");

        assert_eq!(plan.migrations.len(), 1);
        assert_eq!(plan.migrations[0].old_symbol.id, old_b.id);
        assert_eq!(plan.migrations[0].new_symbol.id, new_symbol.id);
        assert_eq!(plan.prunes.len(), 1);
        assert_eq!(plan.prunes[0].symbol.id, old_a.id);
        assert_eq!(plan.prunes[0].reason, RECONCILE_DUPLICATE_STALE_REASON);
    }

    #[test]
    fn dry_run_reports_without_mutating() {
        let temp = tempdir().expect("tempdir");
        write_default_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");

        let old_symbol = test_symbol("sym-old", "sig-old");
        let new_symbol = test_symbol("sym-new", "sig-new");
        upsert_symbol_snapshot(&store, &old_symbol, 10);
        upsert_sir_state(
            &store,
            old_symbol.id.as_str(),
            "hash-old",
            r#"{"intent":"old"}"#,
            100,
        );

        let before = store
            .count_symbols_with_sir()
            .expect("count symbols before dry run");
        let symbols_by_id = HashMap::from_iter([(new_symbol.id.clone(), new_symbol.clone())]);
        let plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("plan reconcile");

        let mut output = Vec::<u8>::new();
        print_reconciliation_dry_run(&plan, &mut output).expect("print dry run report");
        let rendered = String::from_utf8(output).expect("utf8 output");
        assert!(rendered.contains("DRY_RUN migrate"));

        let after = store
            .count_symbols_with_sir()
            .expect("count symbols after dry run");
        assert_eq!(before, after);
        assert!(
            store
                .get_symbol_record(old_symbol.id.as_str())
                .expect("query old symbol")
                .is_some()
        );
        assert!(
            store
                .get_symbol_record(new_symbol.id.as_str())
                .expect("query new symbol")
                .is_none()
        );
    }

    #[test]
    fn coverage_reaches_100_after_reconcile() {
        let temp = tempdir().expect("tempdir");
        write_default_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");

        let old_symbol = test_symbol("sym-old", "sig-old");
        let new_symbol = test_symbol("sym-new", "sig-new");
        upsert_symbol_snapshot(&store, &old_symbol, 10);
        upsert_symbol_snapshot(&store, &new_symbol, 20);
        upsert_sir_state(
            &store,
            old_symbol.id.as_str(),
            "hash-old",
            r#"{"intent":"old"}"#,
            100,
        );

        let symbols_by_id = HashMap::from_iter([(new_symbol.id.clone(), new_symbol.clone())]);
        let plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("plan reconcile");
        let (migrated, pruned) =
            execute_symbol_reconciliation(&store, &plan, |_| Ok(()), |_| Ok(()))
                .expect("execute reconcile");
        assert_eq!((migrated, pruned), (1, 0));

        let (total_symbols, symbols_with_sir) = store
            .count_symbols_with_sir()
            .expect("count coverage after reconcile");
        assert_eq!(total_symbols, 1);
        assert_eq!(symbols_with_sir, 1);
    }

    #[test]
    fn full_reconcile_is_idempotent() {
        let temp = tempdir().expect("tempdir");
        write_default_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");

        let old_symbol = test_symbol("sym-old", "sig-old");
        let new_symbol = test_symbol("sym-new", "sig-new");
        upsert_symbol_snapshot(&store, &old_symbol, 10);
        upsert_symbol_snapshot(&store, &new_symbol, 20);
        upsert_sir_state(
            &store,
            old_symbol.id.as_str(),
            "hash-old",
            r#"{"intent":"old"}"#,
            100,
        );

        let symbols_by_id = HashMap::from_iter([(new_symbol.id.clone(), new_symbol.clone())]);
        let first_plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("first plan");
        execute_symbol_reconciliation(&store, &first_plan, |_| Ok(()), |_| Ok(()))
            .expect("first reconcile");

        let second_plan = plan_symbol_reconciliation(&store, &symbols_by_id).expect("second plan");
        assert!(second_plan.is_empty());
        let (migrated, pruned) =
            execute_symbol_reconciliation(&store, &second_plan, |_| Ok(()), |_| Ok(()))
                .expect("second reconcile");
        assert_eq!((migrated, pruned), (0, 0));

        let (total_symbols, symbols_with_sir) = store
            .count_symbols_with_sir()
            .expect("count coverage after second reconcile");
        assert_eq!((total_symbols, symbols_with_sir), (1, 1));
    }

    #[test]
    fn resolve_git_watch_dir_follows_worktree_gitdir_file() {
        let temp = tempdir().expect("tempdir");
        let git_admin = temp.path().join("git-admin/worktrees/demo");
        fs::create_dir_all(&git_admin).expect("create git admin dir");
        fs::write(
            temp.path().join(".git"),
            format!("gitdir: {}\n", git_admin.display()),
        )
        .expect("write gitdir pointer");

        let resolved = resolve_git_watch_dir(temp.path()).expect("resolve git watch dir");
        assert_eq!(resolved, git_admin);
    }

    #[test]
    fn classify_watch_event_recognizes_git_paths_outside_workspace() {
        let temp = tempdir().expect("tempdir");
        let git_admin = temp.path().join("git-admin/worktrees/demo");
        let source_file = temp.path().join("src/lib.rs");
        fs::create_dir_all(source_file.parent().expect("source parent")).expect("create src dir");
        let event = Event {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![git_admin.join("HEAD"), source_file.clone()],
            attrs: Default::default(),
        };

        let classified = classify_watch_event(temp.path(), Some(&git_admin), event);
        assert!(classified.git_event);
        assert_eq!(classified.source_paths, vec![source_file]);
    }

    #[test]
    fn git_events_suppress_normal_file_debounce_until_settled() {
        let mut debounce_queue = DebounceQueue::default();
        let mut git_state = GitDebounceState::default();
        let source_a = PathBuf::from("src/lib.rs");
        let source_b = PathBuf::from("src/main.rs");

        enqueue_classified_watch_event(
            ClassifiedWatchEvent {
                git_event: true,
                source_paths: vec![source_a.clone()],
                watch_dirs: Vec::new(),
            },
            &mut debounce_queue,
            &mut git_state,
        );
        enqueue_classified_watch_event(
            ClassifiedWatchEvent {
                git_event: false,
                source_paths: vec![source_b.clone()],
                watch_dirs: Vec::new(),
            },
            &mut debounce_queue,
            &mut git_state,
        );

        assert!(git_state.has_pending());
        assert!(
            debounce_queue
                .drain_due(Instant::now(), Duration::ZERO)
                .is_empty()
        );

        let mut dirty = git_state.take_dirty_paths().into_iter().collect::<Vec<_>>();
        dirty.sort();
        assert_eq!(dirty, vec![source_a, source_b]);
    }
}
