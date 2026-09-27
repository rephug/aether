//! Embedding refresh and vector-store writes: the currency-checked refresh under the
//! per-symbol embed lock, batch embedding and the embeddings-only pass.

use super::*;

/// How many times a guarded embedding write is retried against the vector another
/// writer stored meanwhile for the same SIR under a different provider or model.
const EMBEDDING_WRITE_ATTEMPTS: usize = 3;

impl SirPipeline {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn process_pending_embeddings(
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

    pub fn load_symbol_embedding(&self, symbol_id: &str) -> Result<Option<SymbolEmbeddingRecord>> {
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
        let mut superseded = 0usize;
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
            // The identity of the write whose blob was just read; the refresh holds the
            // vector to that write, not to any later one with the same hash.
            let committed = match current_sir_identity(store, symbol_id) {
                Ok(Some(identity)) => identity,
                Ok(None) => {
                    skipped_no_sir += 1;
                    continue;
                }
                Err(err) => {
                    errors += 1;
                    tracing::warn!(symbol_id = %symbol_id, error = %err, "failed to read SIR identity");
                    continue;
                }
            };
            match self.refresh_embedding_if_needed(
                store,
                symbol_id,
                &committed,
                &canonical,
                print_sir,
                out,
                existing_metas.get(symbol_id),
            ) {
                Ok(EmbeddingRefresh::Refreshed { .. }) => refreshed += 1,
                Ok(EmbeddingRefresh::Unchanged) => skipped_up_to_date += 1,
                // The SIR read above was replaced before its vector could be stored;
                // the writer that replaced it embeds its own.
                Ok(EmbeddingRefresh::Superseded) => superseded += 1,
                Err(err) => {
                    errors += 1;
                    tracing::warn!(symbol_id = %symbol_id, error = %err, "failed to refresh embedding");
                }
            }
        }

        writeln!(
            out,
            "Re-embedded {refreshed} of {processed} symbols with {provider_name}/{model_name} ({skipped_no_sir} skipped: no current SIR, {skipped_up_to_date} already up to date, {superseded} superseded, {errors} errors)"
        )
        .context("failed to write embeddings-only summary")?;

        Ok(())
    }

    pub fn delete_embeddings(&self, symbol_ids: &[String]) -> Result<()> {
        let _embed_guards =
            acquire_embed_write_locks(&self.workspace_root, symbol_ids.iter().map(String::as_str))?;
        self.runtime
            .block_on(self.vector_store.delete_embeddings(symbol_ids))
            .context("failed to delete symbol embeddings")
    }

    /// Flush a batch of embedding records to the vector store, holding every affected
    /// symbol's embedding lock so the batch never lands over a vector another writer
    /// (an `aether-mcp` injection, say) is placing at the same time. The batch was
    /// embedded while unlocked, so under the locks each record is checked against the
    /// symbol's current SIR: a record whose SIR has been replaced since (the injection
    /// wrote a newer leaf and its own vector) is dropped rather than written over the
    /// newer vector, and a record whose SIR moves on between that check and the write
    /// is taken back afterwards, exactly as the single-symbol refresh does.
    ///
    /// Each record comes with the identity of the leaf write it embeds (hash, history
    /// version, write generation), and currency is that whole identity: a leaf replaced
    /// and then restored with the same content while the batch was embedded is another
    /// write, not this one. The symbols whose vectors were dropped or taken back are
    /// returned, so the caller retires their writes instead of reporting them stored.
    pub(crate) fn flush_embedding_batch(
        &self,
        store: &SqliteStore,
        records: Vec<(SymbolEmbeddingRecord, SirIdentity)>,
    ) -> Result<Vec<String>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let _embed_guards = acquire_embed_write_locks(
            &self.workspace_root,
            records.iter().map(|(record, _)| record.symbol_id.as_str()),
        )?;
        let sir_is_current = |record: &SymbolEmbeddingRecord, identity: &SirIdentity| {
            current_sir_identity(store, record.symbol_id.as_str())
                .map(|current| current.as_ref() == Some(identity))
        };
        let mut superseded = Vec::new();
        let mut current = Vec::with_capacity(records.len());
        for (record, identity) in records {
            if sir_is_current(&record, &identity)? {
                current.push((record, identity));
            } else {
                tracing::debug!(
                    symbol_id = %record.symbol_id,
                    "dropping embedding for a SIR replaced while the batch was being embedded"
                );
                superseded.push(record.symbol_id);
            }
        }
        if current.is_empty() {
            return Ok(superseded);
        }
        self.runtime
            .block_on(
                self.vector_store.upsert_embedding_batch(
                    current.iter().map(|(record, _)| record.clone()).collect(),
                ),
            )
            .context("failed to flush embedding batch to vector store")?;
        for (record, identity) in &current {
            if !sir_is_current(record, identity)? {
                self.runtime
                    .block_on(self.vector_store.delete_embedding_if_matches(
                        record.symbol_id.as_str(),
                        record.sir_hash.as_str(),
                        record.updated_at,
                    ))
                    .with_context(|| {
                        format!(
                            "failed to delete the superseded embedding for {}",
                            record.symbol_id
                        )
                    })?;
                superseded.push(record.symbol_id.clone());
            }
        }
        Ok(superseded)
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
        let existing = existing_meta.clone();
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
            existing,
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

    /// Refresh a symbol's embedding for the SIR it holds right now, under the per-symbol
    /// embedding lock. Callers release the inject lock before embedding, so a newer SIR
    /// can land at any point during the provider call: the refresh re-reads the stored
    /// SIR hash before and after storing the vector (and once when a vector for the hash
    /// already exists) and stores or keeps nothing for a SIR the store no longer holds.
    /// The outcome is returned as is: `Superseded` tells the caller that the SIR it
    /// wrote is no longer the stored one, so its write is not to be recorded as
    /// completed (that SIR's own writer embeds it); it must not be read as "unchanged".
    /// `committed` is the identity of that write (hash, history version and write
    /// generation), read under the inject lock that made it; the vector belongs to the
    /// caller's write only while the store holds exactly that identity, so a leaf
    /// replaced and then restored with the same content meanwhile counts as superseded.
    #[allow(clippy::too_many_arguments)]
    pub fn refresh_embedding_if_needed(
        &self,
        store: &SqliteStore,
        symbol_id: &str,
        committed: &SirIdentity,
        canonical_json: &str,
        print_sir: bool,
        out: &mut dyn Write,
        prefetched_meta: Option<&VectorEmbeddingMetaRecord>,
    ) -> Result<EmbeddingRefresh> {
        let _embed_guard = acquire_embed_write_lock(&self.workspace_root, symbol_id)?;
        let mut still_current = || -> Result<bool> {
            Ok(current_sir_identity(store, symbol_id)?.as_ref() == Some(committed))
        };
        let outcome = self.refresh_embedding_if_current(
            symbol_id,
            committed.sir_hash.as_str(),
            canonical_json,
            prefetched_meta,
            &mut still_current,
        )?;
        if print_sir && let EmbeddingRefresh::Refreshed { provider, model } = &outcome {
            writeln!(
                out,
                "EMBEDDING_STORED symbol_id={symbol_id} provider={provider} model={model}"
            )
            .context("failed to write embedding print line")?;
        }
        Ok(outcome)
    }

    /// Like `refresh_embedding_if_needed`, but for callers that already hold the
    /// symbol's embedding lock (`acquire_embed_write_lock`) and cannot hold the SIR
    /// fixed while the provider runs: `still_current` (typically "the store's SIR hash
    /// for this symbol is still `sir_hash_value`") is consulted before the provider
    /// call, again right before the vector is stored, and once more after it is stored;
    /// when a vector for this hash already exists it is consulted once, and the vector
    /// is removed if the SIR has moved on.
    /// A vector for a SIR that was replaced in the meantime is never left in the store:
    /// the write is skipped, or undone when the replacement landed between the last
    /// check and the write (only this call's own vector is removed, never one a newer
    /// writer stored since), so the symbol carries the newer SIR's vector once that
    /// writer's refresh runs, or none at all rather than a stale one.
    pub fn refresh_embedding_if_current(
        &self,
        symbol_id: &str,
        sir_hash_value: &str,
        canonical_json: &str,
        prefetched_meta: Option<&VectorEmbeddingMetaRecord>,
        still_current: &mut dyn FnMut() -> Result<bool>,
    ) -> Result<EmbeddingRefresh> {
        let mut needed = self.check_embedding_needed(symbol_id, sir_hash_value, prefetched_meta)?;
        if needed.is_none() && prefetched_meta.is_some() {
            // Prefetched metadata was read before the caller took the symbol's lock (the
            // embeddings-only pass batches one read for every symbol up front), so it
            // may describe a vector another writer has since moved to a different
            // provider or model, or removed. It is good enough to say "work is needed",
            // never to say "nothing is": before declaring the vector current, re-read
            // the metadata now, under the lock.
            needed = self.check_embedding_needed(symbol_id, sir_hash_value, None)?;
        }
        let Some(needed) = needed else {
            // A vector for this hash is already stored (or no provider is configured).
            // It is only right while the SIR is still this one: an injection that
            // repeats an earlier hash can reach here after a concurrent injection
            // installed a newer SIR, and if that injector's own refresh then fails the
            // old vector would keep serving the new annotation. Take it back out, but
            // only the exact vector observed (hash and write time), never one another
            // writer stored since.
            if !still_current()? {
                let observed = self
                    .runtime
                    .block_on(self.vector_store.get_embedding_meta(symbol_id))
                    .with_context(|| {
                        format!("failed to read embedding metadata for {symbol_id}")
                    })?;
                if let Some(observed) = observed
                    && observed.sir_hash == sir_hash_value
                {
                    self.runtime
                        .block_on(self.vector_store.delete_embedding_if_matches(
                            symbol_id,
                            sir_hash_value,
                            observed.updated_at,
                        ))
                        .with_context(|| {
                            format!("failed to delete the superseded embedding for {symbol_id}")
                        })?;
                }
                return Ok(EmbeddingRefresh::Superseded);
            }
            return Ok(EmbeddingRefresh::Unchanged);
        };

        let Some(embedding_provider) = self.embedding_provider.as_ref() else {
            return Ok(EmbeddingRefresh::Unchanged);
        };

        if !still_current()? {
            return Ok(EmbeddingRefresh::Superseded);
        }
        let embedding = self
            .runtime
            .block_on(
                embedding_provider
                    .embed_text_with_purpose(canonical_json, EmbeddingPurpose::Document),
            )
            .with_context(|| format!("failed to generate embedding for {symbol_id}"))?;

        if embedding.is_empty() {
            return Ok(EmbeddingRefresh::Unchanged);
        }
        if !still_current()? {
            return Ok(EmbeddingRefresh::Superseded);
        }

        // The write is conditional on the vector the check observed still being the
        // stored one: a writer that takes no embedding lock (the daemon's index or
        // regenerate pass) may have stored the newer SIR's vector during the provider
        // call, and a plain upsert keyed on the symbol would overwrite it.
        let mut expected = needed.existing.clone();
        let mut attempts = 0usize;
        let updated_at = loop {
            let updated_at = unix_timestamp_secs();
            let written = self
                .runtime
                .block_on(self.vector_store.upsert_embedding_if_matches(
                    SymbolEmbeddingRecord {
                        symbol_id: symbol_id.to_owned(),
                        sir_hash: sir_hash_value.to_owned(),
                        provider: needed.provider.clone(),
                        model: needed.model.clone(),
                        embedding: embedding.clone(),
                        updated_at,
                    },
                    expected.as_ref(),
                ))
                .with_context(|| format!("failed to store embedding for {symbol_id}"))?;
            if written {
                break updated_at;
            }
            // Another writer got there first. Its vector stands when it is the one this
            // call needs: this very SIR under the configured provider and model. A
            // vector for another SIR means this call's SIR has been superseded. A vector
            // for this SIR under another provider or model (the embeddings-only pass
            // prefetches its metadata before taking the symbol's lock, so a writer with
            // a different identity may have stored since) leaves the configured identity
            // without a vector, so the write is retried against what is stored now.
            let stored = self
                .runtime
                .block_on(self.vector_store.get_embedding_meta(symbol_id))
                .with_context(|| format!("failed to read embedding metadata for {symbol_id}"))?;
            match stored {
                Some(meta) if meta.sir_hash != sir_hash_value => {
                    return Ok(EmbeddingRefresh::Superseded);
                }
                Some(meta) if meta.provider == needed.provider && meta.model == needed.model => {
                    return Ok(EmbeddingRefresh::Unchanged);
                }
                other => {
                    attempts += 1;
                    if attempts >= EMBEDDING_WRITE_ATTEMPTS {
                        anyhow::bail!(
                            "failed to store embedding for {symbol_id}: the stored vector changed under {EMBEDDING_WRITE_ATTEMPTS} consecutive writes"
                        );
                    }
                    if !still_current()? {
                        return Ok(EmbeddingRefresh::Superseded);
                    }
                    expected = other;
                }
            }
        };

        if !still_current()? {
            // The SIR changed between the last check and the write: the vector just
            // stored describes the old SIR, so take it back out rather than let the new
            // SIR read as semantically identical to the old one. Only this call's own
            // vector is removed (the delete is keyed on the SIR hash and this write's
            // timestamp): a writer that does not take the embedding locks (the daemon's
            // index or regenerate pass) may already have stored the newer SIR's vector,
            // even one for this same hash again (`H1 → H2 → H1`), and that one must stay.
            self.runtime
                .block_on(self.vector_store.delete_embedding_if_matches(
                    symbol_id,
                    sir_hash_value,
                    updated_at,
                ))
                .with_context(|| {
                    format!("failed to delete the superseded embedding for {symbol_id}")
                })?;
            return Ok(EmbeddingRefresh::Superseded);
        }

        Ok(EmbeddingRefresh::Refreshed {
            provider: needed.provider,
            model: needed.model,
        })
    }
}
