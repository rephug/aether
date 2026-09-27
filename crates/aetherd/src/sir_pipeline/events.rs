//! Event-driven processing: symbol change events, quality batches, bulk scans and
//! symbol removal, from candidate selection through the persisted generation.

use super::*;

impl SirPipeline {
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
                    // Not re-read here: the enrichment in the prompt describes the SIR
                    // the item was built from, so that is the write the guard compares.
                    job.prior_sir = item.baseline_sir_identity;
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
                    let persisted = match self
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
                        })? {
                        GenerationPersist::Persisted(persisted) => *persisted,
                        GenerationPersist::Superseded => continue,
                        GenerationPersist::Failed => {
                            stats.failure_count += 1;
                            continue;
                        }
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
                Ok(mut job) => {
                    job.prior_sir = current_sir_identity(store, &job.symbol.id)?;
                    jobs.push(job)
                }
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
                    let persisted = match self
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
                        })? {
                        GenerationPersist::Persisted(persisted) => *persisted,
                        GenerationPersist::Superseded => continue,
                        GenerationPersist::Failed => {
                            stats.failure_count += 1;
                            continue;
                        }
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

    pub(super) fn process_removed_symbols(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
    ) -> Result<()> {
        for symbol in &event.removed {
            // Removal deletes the symbol row and its SIR, so it takes the inject lock the
            // leaf writers hold: an `aether_sir_inject` call that resolved this symbol
            // re-checks it exists under that lock and cannot persist a leaf for a symbol
            // removed underneath it (the `sir` table has no foreign key to catch that).
            {
                let _inject_guard = acquire_inject_write_lock(&self.workspace_root)?;
                store
                    .mark_removed(&symbol.id)
                    .with_context(|| format!("failed to mark symbol removed: {}", symbol.id))?;
            }
            let _embed_guard = acquire_embed_write_lock(&self.workspace_root, &symbol.id)?;
            self.runtime
                .block_on(self.vector_store.delete_embedding(&symbol.id))
                .with_context(|| format!("failed to remove vector embedding for {}", symbol.id))?;
        }

        Ok(())
    }

    pub(super) fn collect_changed_symbols(&self, event: &SymbolChangeEvent) -> Vec<(Symbol, bool)> {
        let mut changed_symbols: Vec<(Symbol, bool)> =
            Vec::with_capacity(event.added.len() + event.updated.len());
        changed_symbols.extend(event.added.iter().cloned().map(|symbol| (symbol, true)));
        changed_symbols.extend(event.updated.iter().cloned().map(|symbol| (symbol, false)));
        changed_symbols
    }

    pub(super) fn upsert_changed_symbols(
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
    pub(super) fn prepare_candidate_jobs(
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
                    job.prior_sir = current_sir_identity(store, &job.symbol.id)?;
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

    pub(super) fn log_prepared_jobs(&self, file_path: &str, prepared: &PreparedCandidateJobs) {
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

    pub(super) fn log_processing_summary(&self, file_path: &str, stats: &ProcessEventStats) {
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
    pub(super) fn finish_bulk_scan_success(
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

    pub(super) fn flush_bulk_scan_embedding_buffer(
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
        if let Err(err) = self.flush_embedding_batch(store, records) {
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

    pub(super) fn should_skip_sir_generation(
        &self,
        store: &SqliteStore,
        symbol: &Symbol,
    ) -> Result<bool> {
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
}
