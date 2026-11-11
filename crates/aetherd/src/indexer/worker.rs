//! The semantic worker of the indexing loop: the shared scheduler state, the worker
//! thread that drains it through the SIR pipeline, and the enqueueing of changed,
//! SIR-less and stale symbols. The scheduling rules themselves are in `scheduler`.

use std::time::Instant;

use aether_core::Language;

use super::scheduler::{
    Completion, MAX_TASK_RETRIES, Scheduled, SemanticScheduler, SymbolTask, TaskOutcome,
};
use super::snapshot::{outdated_rollups, symbols_with_stale_source};
use super::*;
use crate::sir_pipeline::{ProviderGenerationFailure, SIR_STATUS_FRESH};

#[derive(Clone)]
pub(super) struct SharedQueueState {
    /// Every scheduling decision goes through this one lock (see `scheduler`).
    pub(super) scheduler: Arc<Mutex<SemanticScheduler>>,
    /// Files whose rollup was outdated when the daemon started and none of whose
    /// symbols is queued (processing a queued leaf rebuilds its file's rollup). The
    /// workers rebuild them a few at a time between symbol tasks.
    pub(super) pending_rollups: Arc<Mutex<BTreeMap<String, Language>>>,
}

impl SharedQueueState {
    pub(super) fn new(symbol_index: HashMap<String, Symbol>) -> Self {
        Self {
            scheduler: Arc::new(Mutex::new(SemanticScheduler::new(symbol_index))),
            pending_rollups: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(super) fn lock_or_recover<'a, T>(
        mutex: &'a Mutex<T>,
        mutex_name: &'static str,
    ) -> MutexGuard<'a, T> {
        mutex.lock().unwrap_or_else(|err| {
            tracing::error!(
                mutex = mutex_name,
                error = %err,
                "SharedQueueState mutex poisoned; recovering"
            );
            err.into_inner()
        })
    }

    pub(super) fn lock_scheduler(&self) -> MutexGuard<'_, SemanticScheduler> {
        Self::lock_or_recover(&self.scheduler, "scheduler")
    }

    pub(super) fn lock_pending_rollups(&self) -> MutexGuard<'_, BTreeMap<String, Language>> {
        Self::lock_or_recover(&self.pending_rollups, "pending_rollups")
    }

    pub(super) fn add_pending_rollups(&self, files: BTreeMap<String, Language>) {
        self.lock_pending_rollups().extend(files);
    }

    /// Up to `max` pending rollups, taken off the set (`None` once it is empty). Taken a
    /// few at a time, so a worker serves symbol tasks and SIR requests between chunks
    /// and a chunk that fails costs only its own files.
    pub(super) fn take_pending_rollup_chunk(
        &self,
        max: usize,
    ) -> Option<BTreeMap<String, Language>> {
        let mut pending = self.lock_pending_rollups();
        if pending.is_empty() {
            return None;
        }
        let keys = pending.keys().take(max.max(1)).cloned().collect::<Vec<_>>();
        Some(
            keys.into_iter()
                .filter_map(|file_path| {
                    let language = pending.remove(&file_path)?;
                    Some((file_path, language))
                })
                .collect(),
        )
    }

    /// Record `symbol` as its id's latest version and make sure a task runs for it: now
    /// if the id is idle, after its flight if a worker is processing an earlier version.
    #[cfg(test)]
    pub(super) fn schedule(&self, symbol: Symbol, score: f64) -> Scheduled {
        self.lock_scheduler().schedule(symbol, score)
    }

    pub(super) fn remove_symbol(&self, symbol_id: &str) {
        self.lock_scheduler().remove_symbol(symbol_id);
    }

    pub(super) fn upsert_symbol(&self, symbol: Symbol) {
        self.lock_scheduler().upsert_symbol(symbol);
    }

    pub(super) fn bump_to_front(&self, symbol_id: &str) -> bool {
        self.lock_scheduler().bump_to_front(symbol_id)
    }

    #[cfg(test)]
    pub(super) fn is_queued(&self, symbol_id: &str) -> bool {
        self.lock_scheduler().is_queued(symbol_id)
    }

    #[cfg(test)]
    pub(super) fn pop_task(&self) -> Option<SymbolTask> {
        self.lock_scheduler().start_next()
    }

    fn pop_task_at(&self, now: Instant) -> Option<SymbolTask> {
        self.lock_scheduler().start_next_at(now)
    }

    /// End `task`'s flight and queue what it leaves owed: the symbol's latest version if
    /// it was edited meanwhile, or a bounded retry if the task errored. `is_current`
    /// runs under the scheduler lock, so no edit can be scheduled between its answer
    /// and the decision taken on it.
    #[cfg(test)]
    pub(super) fn complete_task(
        &self,
        task: &SymbolTask,
        outcome: TaskOutcome,
        is_current: impl FnOnce(&Symbol) -> bool,
    ) -> Completion {
        self.lock_scheduler().complete(task, outcome, is_current)
    }

    fn complete_task_at(
        &self,
        task: &SymbolTask,
        outcome: TaskOutcome,
        is_current: impl FnOnce(&Symbol) -> bool,
        now: Instant,
    ) -> Completion {
        self.lock_scheduler()
            .complete_at(task, outcome, is_current, now)
    }

    #[cfg(test)]
    pub(super) fn delayed_work(&self) -> usize {
        self.lock_scheduler().delayed_len()
    }

    /// How many symbols are queued and how many are being processed.
    #[cfg(test)]
    pub(super) fn pending_work(&self) -> (usize, usize) {
        let scheduler = self.lock_scheduler();
        (scheduler.queued_len(), scheduler.in_flight_len())
    }
}

/// Whether the store holds a fresh SIR generated from exactly this version of the
/// symbol. A read error counts as "no": the version is then processed again, which is
/// the safe side.
fn stored_sir_is_current(store: &SqliteStore, symbol: &Symbol) -> bool {
    let fresh = match store.get_sir_meta(symbol.id.as_str()) {
        Ok(Some(meta)) => {
            let status = meta.sir_status.trim().to_ascii_lowercase();
            status == SIR_STATUS_FRESH || status == "ready"
        }
        Ok(None) | Err(_) => false,
    };
    fresh
        && stored_source_hash(store, symbol).as_deref() == Some(symbol.content_hash.as_str())
        && store
            .read_sir_blob(symbol.id.as_str())
            .ok()
            .flatten()
            .is_some_and(|blob| !blob.trim().is_empty())
}

/// The hash of the source the symbol's stored SIR was generated from, if it has a SIR
/// that records one.
fn stored_source_hash(store: &SqliteStore, symbol: &Symbol) -> Option<String> {
    store.get_sir_source_hash(symbol.id.as_str()).ok().flatten()
}

/// What one semantic worker runs its symbol tasks with.
pub(super) struct WorkerContext {
    pub(super) pipeline: SirPipeline,
    pub(super) store: Arc<SqliteStore>,
    pub(super) queue_state: SharedQueueState,
    pub(super) workspace_root: PathBuf,
    pub(super) generation_pass: &'static str,
    pub(super) prompt_config_fingerprint: String,
}

impl WorkerContext {
    /// Take the next queued symbol, if any, run it through the SIR pipeline and settle
    /// what the symbol is owed next. `false`: the queue had no task to start.
    pub(super) fn run_next_symbol_task(&self) -> bool {
        self.run_next_symbol_task_with_clock(Instant::now, || {})
    }

    #[cfg(test)]
    pub(super) fn run_next_symbol_task_at(&self, now: Instant) -> bool {
        self.run_next_symbol_task_with_clock(|| now, || {})
    }

    #[cfg(test)]
    pub(super) fn run_next_symbol_task_after_preflight(
        &self,
        now: Instant,
        before_run: impl FnOnce(),
    ) -> bool {
        self.run_next_symbol_task_with_clock(|| now, before_run)
    }

    fn run_next_symbol_task_with_clock(
        &self,
        clock: impl Fn() -> Instant,
        before_run: impl FnOnce(),
    ) -> bool {
        let Some(task) = self.queue_state.pop_task_at(clock()) else {
            return false;
        };
        let symbol_id = task.symbol.id.as_str();
        // A manual injection or another writer may have satisfied the obligation
        // during cooldown. Do not infer again over a current leaf just because its
        // original provider call failed.
        let outcome =
            if task.is_provider_retry && stored_sir_is_current(self.store.as_ref(), &task.symbol) {
                TaskOutcome::Finished
            } else {
                before_run();
                self.run_symbol_task(&task)
            };
        // Preserve an existing provider obligation even if this recovery turn
        // cannot get far enough to report pipeline statistics (for example a
        // temporary catalog read error).
        let outcome = if task.is_provider_retry && outcome == TaskOutcome::Errored {
            TaskOutcome::ProviderFailed
        } else {
            outcome
        };
        let completion = self.queue_state.complete_task_at(
            &task,
            outcome,
            |latest| stored_sir_is_current(self.store.as_ref(), latest),
            clock(),
        );
        match completion {
            Completion::Done => {}
            Completion::FollowUpQueued => tracing::debug!(
                symbol_id = %symbol_id,
                "symbol changed while it was being processed; queued its latest version"
            ),
            Completion::RetryQueued { attempt } => tracing::warn!(
                symbol_id = %symbol_id,
                attempt,
                max_retries = MAX_TASK_RETRIES,
                "semantic indexing errored for queued symbol; queued a retry behind the other symbols"
            ),
            Completion::ProviderRetryScheduled { attempt, delay } => tracing::warn!(
                symbol_id = %symbol_id,
                attempt,
                retry_after_seconds = delay.as_secs(),
                "provider generation failed; scheduled a delayed recovery attempt"
            ),
            Completion::GaveUp => tracing::warn!(
                symbol_id = %symbol_id,
                max_retries = MAX_TASK_RETRIES,
                "semantic indexing kept erroring for queued symbol; giving up until its next edit, SIR request or daemon start"
            ),
            Completion::StaleTicket => tracing::error!(
                symbol_id = %symbol_id,
                "completed a symbol task that was not in flight; ignored"
            ),
        }
        true
    }

    /// Run one started task through the SIR pipeline.
    fn run_symbol_task(&self, task: &SymbolTask) -> TaskOutcome {
        let Self {
            pipeline,
            store,
            workspace_root,
            generation_pass,
            prompt_config_fingerprint,
            ..
        } = self;
        let generation_pass = *generation_pass;
        let symbol = &task.symbol;

        let symbol_id = symbol.id.clone();
        let exists = match store.get_symbol_record(symbol_id.as_str()) {
            Ok(record) => record.is_some(),
            Err(err) => {
                tracing::warn!(
                    symbol_id = %symbol_id,
                    error = %err,
                    "failed to confirm queued symbol before watcher generation"
                );
                return TaskOutcome::Errored;
            }
        };
        if !exists {
            return TaskOutcome::Finished;
        }

        let previous_meta = match store.get_sir_meta(symbol_id.as_str()) {
            Ok(meta) => meta,
            Err(err) => {
                tracing::warn!(
                    symbol_id = %symbol_id,
                    error = %err,
                    "failed to read prior SIR metadata for queued watcher symbol"
                );
                None
            }
        };
        let previous_embedding = match pipeline.load_symbol_embedding(symbol_id.as_str()) {
            Ok(embedding) => embedding,
            Err(err) => {
                tracing::warn!(
                    symbol_id = %symbol_id,
                    error = %err,
                    "failed to read prior embedding for queued watcher symbol"
                );
                None
            }
        };
        let event = SymbolChangeEvent {
            file_path: symbol.file_path.clone(),
            language: symbol.language,
            added: vec![symbol.clone()],
            removed: Vec::new(),
            updated: Vec::new(),
        };
        // The pipeline checks source hashes at its skip decision. Do not compute
        // force from an earlier read: a manual injection landing between that read
        // and job preparation must still be allowed to satisfy the obligation.
        let mut sink = std::io::sink();
        let result = pipeline.process_event_with_priority_and_pass(
            store.as_ref(),
            &event,
            false,
            false,
            &mut sink,
            Some(task.score),
            generation_pass,
        );
        match result {
            Ok(stats) => {
                if stats.success_count > 0
                    && let Err(err) = finalize_watcher_generation(
                        pipeline,
                        workspace_root.as_path(),
                        store.as_ref(),
                        symbol,
                        previous_meta.as_ref(),
                        previous_embedding.as_ref(),
                        generation_pass,
                        prompt_config_fingerprint,
                    )
                {
                    tracing::warn!(
                        symbol_id = %symbol_id,
                        error = %err,
                        "failed to record watcher prompt hash and fingerprint history"
                    );
                }
                if stats.generation_failure_count > 0
                    || (task.is_provider_retry
                        && stats.unresolved_leaf_count > 0
                        && !stored_sir_is_current(store.as_ref(), symbol))
                {
                    TaskOutcome::ProviderFailed
                } else {
                    TaskOutcome::Finished
                }
            }
            Err(err) => {
                tracing::warn!(
                    symbol_id = %symbol_id,
                    error = %err,
                    "semantic indexing failed for queued symbol"
                );
                if err.is::<ProviderGenerationFailure>() {
                    TaskOutcome::ProviderFailed
                } else {
                    TaskOutcome::Errored
                }
            }
        }
    }
}

pub(super) fn spawn_semantic_worker(
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
    .with_context(|| format!("failed to initialize SIR pipeline for worker {worker_id}"))?
    .with_structural_updates(false);
    let prompt_config_fingerprint = watcher_runtime.prompt_config_fingerprint(&pipeline);
    let worker = WorkerContext {
        pipeline,
        store,
        queue_state,
        workspace_root: config.workspace.clone(),
        generation_pass: watcher_runtime.generation_pass,
        prompt_config_fingerprint,
    };

    std::thread::Builder::new()
        .name(format!("aether-sir-{worker_id}"))
        .spawn(move || {
            loop {
                match worker.store.consume_sir_requests(REQUEST_POLL_BATCH) {
                    Ok(requested) => {
                        for symbol_id in requested {
                            let _ = worker.queue_state.bump_to_front(symbol_id.as_str());
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "failed to consume SIR requests for watcher worker, will retry"
                        );
                    }
                }

                // One chunk of the rollups left outdated at startup per turn, between
                // symbol tasks.
                let rebuilt_rollups = match worker
                    .queue_state
                    .take_pending_rollup_chunk(PENDING_ROLLUP_CHUNK)
                {
                    Some(files) => {
                        let file_count = files.len();
                        if let Err(err) = refresh_pending_rollups(
                            &worker.pipeline,
                            worker.store.as_ref(),
                            files,
                            worker.generation_pass,
                        ) {
                            tracing::warn!(
                                file_count,
                                error = %err,
                                "failed to rebuild file rollups left outdated at startup; they stay stale until a full index run"
                            );
                        }
                        true
                    }
                    None => false,
                };

                if !worker.run_next_symbol_task() && !rebuilt_rollups {
                    std::thread::sleep(Duration::from_millis(WORKER_IDLE_SLEEP_MS));
                }
            }
        })
        .context("failed to spawn semantic worker thread")?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_watcher_generation(
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

    // Keep the read and metadata update together with respect to structural
    // retirement; a late finalizer must not recreate a leaf removed between them.
    let inject_guard = acquire_inject_write_lock(workspace_root)?;
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
    drop(inject_guard);
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
        None,
    )
    .with_context(|| format!("failed to write watcher fingerprint row for {}", symbol.id))
}

/// Rebuild those of `files` whose rollup is still outdated (stale, or missing while the
/// file has leaf SIRs); a leaf processed since the files were set aside, after a live
/// edit say, has rebuilt its file's rollup already. Returns how many were rebuilt.
pub(super) fn refresh_pending_rollups(
    pipeline: &SirPipeline,
    store: &SqliteStore,
    files: BTreeMap<String, Language>,
    generation_pass: &str,
) -> Result<usize> {
    let outdated = outdated_rollups(store, files)?;
    let count = outdated.len();
    if count > 0 {
        tracing::info!(
            file_count = count,
            "Rebuilding {count} file rollups left outdated at startup"
        );
        let mut sink = std::io::sink();
        pipeline.refresh_file_rollups(store, outdated, generation_pass, false, &mut sink)?;
    }
    Ok(count)
}

/// Queue the symbols of `symbols` that need a SIR: those without one, and those whose
/// SIR is stale because it was written for a body they no longer have (see
/// `snapshot::symbols_with_stale_source`; a leaf left stale by a failed attempt on an
/// unchanged body keeps its SIR).
pub(super) fn enqueue_symbols_needing_sir(
    workspace: &Path,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    symbols_by_id: &HashMap<String, Symbol>,
) -> Result<usize> {
    let mut missing = store.list_symbol_ids_without_sir()?;
    missing.extend(symbols_with_stale_source(store, symbols_by_id)?);
    missing.sort();
    missing.dedup();
    if missing.is_empty() {
        return Ok(0);
    }

    let missing_symbols = missing
        .iter()
        .filter_map(|symbol_id| symbols_by_id.get(symbol_id))
        .cloned()
        .collect::<Vec<_>>();
    enqueue_changed_symbols(workspace, store, queue_state, &missing_symbols)
}

pub(super) fn enqueue_changed_symbols(
    workspace: &Path,
    store: &SqliteStore,
    queue_state: &SharedQueueState,
    changed_symbols: &[Symbol],
) -> Result<usize> {
    if changed_symbols.is_empty() {
        return Ok(0);
    }

    let scores = compute_symbol_priority_scores(workspace, store, changed_symbols);
    let mut scheduler = queue_state.lock_scheduler();
    let mut queued = 0usize;

    for symbol in changed_symbols {
        let score = scores.get(symbol.id.as_str()).copied().unwrap_or(0.0);
        // A symbol a worker is processing is not skipped: its latest version is
        // recorded and runs as a follow-up once that flight completes.
        if scheduler.schedule(symbol.clone(), score) == Scheduled::Queued {
            queued += 1;
        }
    }

    Ok(queued)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::super::tests::{capture_logs, test_symbol};
    use super::*;

    #[test]
    fn shared_queue_state_recovers_from_poisoned_queue_lock() {
        let symbol = test_symbol("sym-poison", "sig-poison");
        let queue_state =
            SharedQueueState::new(HashMap::from_iter([(symbol.id.clone(), symbol.clone())]));
        let poisoned_scheduler = queue_state.scheduler.clone();

        let _ = std::panic::catch_unwind(move || {
            let _guard = poisoned_scheduler.lock().expect("lock scheduler");
            panic!("poison scheduler");
        });

        let (bumped, logs) = capture_logs(|| queue_state.bump_to_front(symbol.id.as_str()));

        assert!(bumped);
        assert!(logs.contains("SharedQueueState mutex poisoned; recovering"));
        assert!(logs.contains("scheduler"));
    }

    #[test]
    fn pending_rollups_are_taken_a_chunk_at_a_time_whatever_the_queue_holds() {
        let symbol = test_symbol("sym-queued", "sig-queued");
        let queue_state =
            SharedQueueState::new(HashMap::from_iter([(symbol.id.clone(), symbol.clone())]));
        // A symbol that keeps failing and being requeued must not hold the rollups back.
        let _ = queue_state.schedule(symbol.clone(), 0.5);
        queue_state.add_pending_rollups(BTreeMap::from([
            ("src/a.rs".to_owned(), Language::Rust),
            ("src/b.rs".to_owned(), Language::Rust),
            ("src/c.rs".to_owned(), Language::Rust),
        ]));

        let first = queue_state
            .take_pending_rollup_chunk(2)
            .expect("first chunk");
        assert_eq!(
            first.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["src/a.rs", "src/b.rs"]
        );
        let second = queue_state
            .take_pending_rollup_chunk(2)
            .expect("second chunk");
        assert_eq!(
            second.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["src/c.rs"]
        );
        assert!(queue_state.take_pending_rollup_chunk(2).is_none());
        assert!(queue_state.is_queued(&symbol.id));
    }
}
