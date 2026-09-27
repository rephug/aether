//! The semantic worker of the indexing loop: the shared priority queue, the worker
//! thread that drains it through the SIR pipeline, and the enqueueing of changed and
//! SIR-less symbols.

use super::*;

#[derive(Clone)]
pub(super) struct SharedQueueState {
    pub(super) queue: Arc<Mutex<SirPriorityQueue>>,
    pub(super) symbol_index: Arc<Mutex<HashMap<String, Symbol>>>,
    pub(super) in_progress: Arc<Mutex<HashSet<String>>>,
}

impl SharedQueueState {
    pub(super) fn new(symbol_index: HashMap<String, Symbol>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(SirPriorityQueue::default())),
            symbol_index: Arc::new(Mutex::new(symbol_index)),
            in_progress: Arc::new(Mutex::new(HashSet::new())),
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

    pub(super) fn lock_queue(&self) -> MutexGuard<'_, SirPriorityQueue> {
        Self::lock_or_recover(&self.queue, "queue")
    }

    pub(super) fn lock_symbol_index(&self) -> MutexGuard<'_, HashMap<String, Symbol>> {
        Self::lock_or_recover(&self.symbol_index, "symbol_index")
    }

    pub(super) fn lock_in_progress(&self) -> MutexGuard<'_, HashSet<String>> {
        Self::lock_or_recover(&self.in_progress, "in_progress")
    }

    pub(super) fn requeue(&self, symbol_id: String, score: f64) {
        let _ = self.lock_queue().push(symbol_id, score.clamp(0.0, 1.0));
    }

    pub(super) fn remove_symbol(&self, symbol_id: &str) {
        self.lock_symbol_index().remove(symbol_id);
        self.lock_queue().remove(symbol_id);
        self.lock_in_progress().remove(symbol_id);
    }

    pub(super) fn upsert_symbol(&self, symbol: Symbol) {
        self.lock_symbol_index().insert(symbol.id.clone(), symbol);
    }

    pub(super) fn bump_to_front(&self, symbol_id: &str) -> bool {
        if self.lock_in_progress().contains(symbol_id) {
            return false;
        }
        if self.lock_queue().bump_to_front(symbol_id) {
            return true;
        }

        let maybe_symbol = self.lock_symbol_index().get(symbol_id).cloned();
        let Some(symbol) = maybe_symbol else {
            return false;
        };
        let mut queue = self.lock_queue();
        let _ = queue.push(symbol.id.clone(), 1.0);
        queue.bump_to_front(symbol_id)
    }

    pub(super) fn pop_task(&self) -> Option<(f64, Symbol)> {
        let (score, symbol_id) = {
            let mut queue = self.lock_queue();
            queue.pop()?
        };
        {
            let mut in_progress = self.lock_in_progress();
            if !in_progress.insert(symbol_id.clone()) {
                return None;
            }
        }
        let symbol = self.lock_symbol_index().get(&symbol_id).cloned();
        if symbol.is_none() {
            let mut in_progress = self.lock_in_progress();
            in_progress.remove(&symbol_id);
        }
        symbol.map(|symbol| (score, symbol))
    }

    pub(super) fn complete_task(&self, symbol_id: &str) {
        self.lock_in_progress().remove(symbol_id);
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
    .with_context(|| format!("failed to initialize SIR pipeline for worker {worker_id}"))?;
    let generation_pass = watcher_runtime.generation_pass;
    let prompt_config_fingerprint = watcher_runtime.prompt_config_fingerprint(&pipeline);
    let workspace_root = config.workspace.clone();

    std::thread::Builder::new()
        .name(format!("aether-sir-{worker_id}"))
        .spawn(move || {
            loop {
                match store.consume_sir_requests(REQUEST_POLL_BATCH) {
                    Ok(requested) => {
                        for symbol_id in requested {
                            let _ = queue_state.bump_to_front(symbol_id.as_str());
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "failed to consume SIR requests for watcher worker, will retry"
                        );
                    }
                }

                let Some((score, symbol)) = queue_state.pop_task() else {
                    std::thread::sleep(Duration::from_millis(WORKER_IDLE_SLEEP_MS));
                    continue;
                };

                let symbol_id = symbol.id.clone();
                let exists = match store.get_symbol_record(symbol_id.as_str()) {
                    Ok(record) => record.is_some(),
                    Err(err) => {
                        tracing::warn!(
                            symbol_id = %symbol_id,
                            error = %err,
                            "failed to confirm queued symbol before watcher generation"
                        );
                        queue_state.complete_task(symbol_id.as_str());
                        continue;
                    }
                };
                if !exists {
                    queue_state.complete_task(symbol_id.as_str());
                    continue;
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
                        queue_state.requeue(symbol_id.clone(), score);
                    }
                }

                queue_state.complete_task(symbol_id.as_str());
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
        None,
    )
    .with_context(|| format!("failed to write watcher fingerprint row for {}", symbol.id))
}

pub(super) fn enqueue_symbols_missing_sir(
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
    let in_progress = queue_state.lock_in_progress();
    let mut queued = 0usize;

    for symbol in changed_symbols {
        if in_progress.contains(symbol.id.as_str()) {
            continue;
        }
        let score = scores.get(symbol.id.as_str()).copied().unwrap_or(0.0);
        let mut queue = queue_state.lock_queue();
        if queue.push(symbol.id.clone(), score) {
            queued += 1;
            drop(queue);
            queue_state.upsert_symbol(symbol.clone());
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
        let poisoned_queue = queue_state.queue.clone();

        let _ = std::panic::catch_unwind(move || {
            let _guard = poisoned_queue.lock().expect("lock queue");
            panic!("poison queue");
        });

        let (bumped, logs) = capture_logs(|| queue_state.bump_to_front(symbol.id.as_str()));

        assert!(bumped);
        assert!(logs.contains("SharedQueueState mutex poisoned; recovering"));
        assert!(logs.contains("queue"));
    }
}
