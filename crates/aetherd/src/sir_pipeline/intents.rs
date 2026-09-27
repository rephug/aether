//! Replay of incomplete write intents left by an interrupted run.

use super::*;

impl SirPipeline {
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

    pub(super) fn replay_write_intent(
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

    pub(super) fn replay_upsert_sir_intent(
        &self,
        store: &SqliteStore,
        intent: &WriteIntent,
        payload: &UpsertSirIntentPayload,
        verbose: bool,
    ) -> Result<()> {
        // Only an intent whose write never landed is `failed`: a stage that failed after
        // the SQLite stage committed left the intent at that stage (the store keeps the
        // last completed stage when recording the error), so it resumes below from the
        // stage after it, against its own committed SIR, rather than being re-planned
        // against a store that already holds that write and retired as superseded.
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
            // The intent's write never landed. It is still wanted only while the store
            // holds the SIR the intent was planned against; a newer write (an injection
            // that landed while the daemon was down, say) is not damage to repair but
            // the result that wins, so the intent is retired instead.
            let _inject_guard = acquire_inject_write_lock(&self.workspace_root)?;
            let current = current_sir_identity(store, payload.symbol.id.as_str())?;
            if !payload.prior_sir.still_holds(current.as_ref()) {
                tracing::info!(
                    intent_id = %intent_id,
                    symbol_id = %payload.symbol.id,
                    "retiring write intent: the stored SIR changed since it was planned"
                );
                store
                    .mark_intent_complete(intent_id)
                    .with_context(|| format!("failed to retire superseded intent {intent_id}"))?;
                return Ok(());
            }
            // A payload that says which text its SIR describes is replayed only while
            // the symbol still has that text: after an edit the daemon regenerates the
            // symbol from the new source, and this older description would only
            // pre-empt that job and record itself as fresh.
            if let Some(planned_source) = payload.source_hash.as_deref()
                && current_source_hash(&self.workspace_root, &payload.symbol).as_deref()
                    != Some(planned_source)
            {
                tracing::info!(
                    intent_id = %intent_id,
                    symbol_id = %payload.symbol.id,
                    "retiring write intent: the symbol source changed since it was planned"
                );
                store
                    .mark_intent_complete(intent_id)
                    .with_context(|| format!("failed to retire superseded intent {intent_id}"))?;
                return Ok(());
            }
            let persisted = self
                .persist_sir_payload_into_sqlite(store, payload, Some(intent_id))
                .with_context(|| format!("failed sqlite write stage for intent {intent_id}"))?;
            canonical_json = persisted.0;
            sir_hash_value = persisted.1;
            status = WriteIntentStatus::SqliteDone;
        } else {
            // The intent's SQLite write landed (the status is set in that transaction).
            // A stored SIR that differs now was written afterwards by someone else, so
            // this intent is superseded: leave the newer SIR alone (its own writer
            // embeds it) and retire the intent rather than restore the older payload.
            let stored_blob = store
                .read_sir_blob(payload.symbol.id.as_str())
                .with_context(|| {
                    format!("failed to read sqlite SIR blob for intent {intent_id}")
                })?;
            if stored_blob.as_deref() != Some(canonical_json.as_str()) {
                tracing::info!(
                    intent_id = %intent_id,
                    symbol_id = %payload.symbol.id,
                    "retiring write intent: its SIR was replaced after the sqlite stage"
                );
                store
                    .mark_intent_complete(intent_id)
                    .with_context(|| format!("failed to retire superseded intent {intent_id}"))?;
                return Ok(());
            }
        }

        if status == WriteIntentStatus::SqliteDone {
            self.refresh_embedding_if_needed(
                store,
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
}
