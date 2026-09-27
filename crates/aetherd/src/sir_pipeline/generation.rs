//! Persisting generated SIRs: canonicalization, the compare-and-set leaf write under
//! the inject lock, failure markers and quality bookkeeping.

use super::*;

impl SirPipeline {
    pub(crate) fn prepare_sir_for_persistence(
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

    pub(super) fn inject_method_dependencies(
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

    pub(super) fn record_generation_quality(&self, confidence: f32) {
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
    pub(super) fn commit_successful_generation(
        &self,
        store: &SqliteStore,
        generated: GeneratedSir,
        generation_pass: &str,
        commit_hash: Option<&str>,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<Option<String>> {
        let persisted = match self.persist_successful_generation_sqlite(
            store,
            &generated,
            generation_pass,
            commit_hash,
        )? {
            GenerationPersist::Persisted(persisted) => *persisted,
            GenerationPersist::Superseded | GenerationPersist::Failed => return Ok(None),
        };

        if let Err(err) = self.refresh_embedding_if_needed(
            store,
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

    pub(super) fn persist_successful_generation_sqlite(
        &self,
        store: &SqliteStore,
        generated: &GeneratedSir,
        generation_pass: &str,
        commit_hash: Option<&str>,
    ) -> Result<GenerationPersist> {
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
                    return Ok(GenerationPersist::Failed);
                }
            };

        // Generation ran unlocked; under the inject lock every leaf writer shares, persist
        // only while the store still holds the SIR write this job started from (hash and
        // history version: a symbol whose SIR went `H1 → H2 → H1` meanwhile was written
        // twice and the job's H1 is not the current one). Otherwise another writer (an
        // `aether_sir_inject` call with a reviewed, high-confidence SIR, say) landed
        // meanwhile and must not be overwritten by this older result.
        let _inject_guard = acquire_inject_write_lock(&self.workspace_root)?;
        let current_sir = current_sir_identity(store, &generated.symbol.id)?;
        if current_sir != generated.prior_sir {
            // One exception: a write bound to text older than what this job read (an
            // injection that passed its source check just before an edit landed, then
            // committed after it) does not supersede the job. The leaf records the text
            // it was bound to; when that differs from the text this job read, and the
            // source check below confirms the job's text is the current one, the stored
            // SIR describes a body the symbol no longer has and this result replaces it,
            // instead of leaving that SIR `fresh` with no job left to regenerate it.
            let stored_source = store
                .get_sir_source_hash(&generated.symbol.id)
                .with_context(|| {
                    format!("failed to read the source hash for {}", generated.symbol.id)
                })?;
            let stored_describes_older_text =
                stored_source.is_some_and(|stored| stored != generated.source_hash);
            if !stored_describes_older_text {
                tracing::info!(
                    symbol_id = %generated.symbol.id,
                    "skipping generated SIR: the stored SIR changed while it was being generated"
                );
                return Ok(GenerationPersist::Superseded);
            }
            tracing::info!(
                symbol_id = %generated.symbol.id,
                "the stored SIR changed while this job ran, but describes older text than the job read; replacing it"
            );
        }
        // The stored SIR alone does not say the result is current: a symbol edited
        // while this job ran still holds the same SIR until the job that edit queued
        // lands, and both jobs started from it. Persist only while the source still
        // hashes to what this job read; otherwise the newer job's result is the one to
        // keep and this one would only pre-empt it.
        if current_source_hash(&self.workspace_root, &generated.symbol).as_deref()
            != Some(generated.source_hash.as_str())
        {
            tracing::info!(
                symbol_id = %generated.symbol.id,
                "skipping generated SIR: the symbol source changed while it was being generated"
            );
            return Ok(GenerationPersist::Superseded);
        }

        let payload = UpsertSirIntentPayload {
            symbol: generated.symbol.clone(),
            sir,
            provider_name: generated.provider_name.clone(),
            model_name: generated.model_name.clone(),
            generation_pass: generation_pass.to_owned(),
            reasoning_trace: generated.reasoning_trace.clone(),
            commit_hash: commit_hash.map(str::to_owned),
            prior_sir: PriorSir::recorded(generated.prior_sir.clone()),
            prompt_hash: None,
            source_hash: Some(generated.source_hash.clone()),
        };
        let payload_json = match payload.to_json_string() {
            Ok(json) => json,
            Err(err) => {
                tracing::error!(
                    symbol_id = %generated.symbol.id,
                    error = %err,
                    "failed to serialize write intent payload"
                );
                return Ok(GenerationPersist::Failed);
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
            return Ok(GenerationPersist::Failed);
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
        if let Err(err) = store.persist_sir_state_atomically_with_source(
            meta,
            &canonical_json,
            payload.commit_hash.as_deref(),
            Some(intent.intent_id.as_str()),
            Some(generated.source_hash.as_str()),
        ) {
            let message = format!("{err:#}");
            self.mark_intent_failed_safely(store, intent.intent_id.as_str(), message.as_str());
            tracing::error!(
                symbol_id = %generated.symbol.id,
                error = %err,
                "failed to persist sqlite SIR state"
            );
            return Ok(GenerationPersist::Failed);
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
                    return Ok(GenerationPersist::Failed);
                }
            };

        Ok(GenerationPersist::Persisted(Box::new(
            PersistedSuccessfulGeneration {
                intent_id: intent.intent_id,
                symbol_id: generated.symbol.id.clone(),
                file_path: generated.symbol.file_path.clone(),
                sir_hash: sir_hash_value,
                canonical_json,
                provider_name: generated.provider_name.clone(),
                embedding_needed,
            },
        )))
    }

    pub(super) fn handle_failed_generation(
        &self,
        store: &SqliteStore,
        failed: infer::FailedSirGeneration,
        generation_pass: &str,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        let last_attempt_at = unix_timestamp_secs();
        // The failure belongs to the SIR the job started from. Under the inject lock,
        // mark only that SIR stale: a SIR another writer stored meanwhile (an injection
        // from `/scan`, say) is not stale for having outlived an unrelated model error.
        let _inject_guard = acquire_inject_write_lock(&self.workspace_root)?;
        // A symbol removed meanwhile (its row and SIR deleted under this lock) has no
        // SIR to mark: for a job that started with none, "no SIR" would otherwise read
        // as unchanged and the marker would recreate an orphan `sir` row for an id the
        // index no longer holds.
        if store
            .get_symbol_record(&failed.symbol.id)
            .with_context(|| format!("failed to load symbol {}", failed.symbol.id))?
            .is_none()
        {
            tracing::info!(
                symbol_id = %failed.symbol.id,
                error = %failed.error_message,
                "SIR generation failed, but the symbol was removed meanwhile; nothing to mark"
            );
            return Ok(());
        }
        let previous_meta = store
            .get_sir_meta(&failed.symbol.id)
            .with_context(|| format!("failed to load SIR metadata for {}", failed.symbol.id))?;
        if current_sir_identity(store, &failed.symbol.id)? != failed.prior_sir {
            tracing::info!(
                symbol_id = %failed.symbol.id,
                error = %failed.error_message,
                "SIR generation failed, but the stored SIR changed meanwhile; leaving it as written"
            );
            return Ok(());
        }

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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn enqueue_or_finish_persisted_generation(
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

    pub(super) fn mark_intent_failed_safely(
        &self,
        store: &SqliteStore,
        intent_id: &str,
        message: &str,
    ) {
        if let Err(mark_err) = store.mark_intent_failed(intent_id, message) {
            tracing::error!(
                intent_id = %intent_id,
                error = %mark_err,
                "failed to mark write intent as failed"
            );
        }
    }

    /// Persist one leaf SIR (history, JSON and metadata in one transaction). The caller
    /// holds the workspace inject lock (`acquire_inject_write_lock`) around its read of
    /// the prior state and this write.
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
        // canonical SIR content is identical to an earlier pass. The source hash the
        // payload carries (if any) is recorded with the leaf, never cleared to unknown.
        store.persist_sir_state_atomically_with_source(
            SirMetaRecord {
                id: payload.symbol.id.clone(),
                sir_hash: sir_hash_value.clone(),
                sir_version: 1,
                provider: payload.provider_name.clone(),
                model: payload.model_name.clone(),
                generation_pass: payload.generation_pass.clone(),
                reasoning_trace: payload.reasoning_trace.clone(),
                prompt_hash: payload.prompt_hash.clone(),
                staleness_score: None,
                updated_at: attempted_at,
                sir_status: SIR_STATUS_FRESH.to_owned(),
                last_error: None,
                last_attempt_at: attempted_at,
            },
            canonical_json.as_str(),
            payload.commit_hash.as_deref(),
            write_intent_id,
            payload.source_hash.as_deref(),
        )?;

        Ok((canonical_json, sir_hash_value))
    }
}
