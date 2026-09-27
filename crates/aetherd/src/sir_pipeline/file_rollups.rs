//! File-level rollup SIRs: computed from the leaf SIRs under the inject lock and
//! persisted only while those leaves are unchanged.

use super::*;

impl SirPipeline {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bulk_upsert_file_rollups(
        &self,
        store: &SqliteStore,
        touched_files: BTreeMap<String, Language>,
        print_sir: bool,
        out: &mut dyn Write,
        commit_hash: Option<&str>,
        generation_pass: &str,
    ) -> Result<()> {
        let mut stale_rollups = Vec::new();
        let mut local_only = Vec::new();
        let mut needs_api = Vec::new();
        // What each rollup was computed from; `persist_file_rollup` refuses to store a
        // rollup whose leaves have changed since (an injection may have rebuilt it).
        let mut fingerprints: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();

        for (file_path, language) in touched_files {
            let leaf_sirs = self
                .load_file_rollup_leaf_sirs(store, file_path.as_str())
                .with_context(|| format!("failed to prepare file rollup inputs for {file_path}"))?;

            if leaf_sirs.is_empty() {
                stale_rollups.push((file_path, language));
                continue;
            }
            fingerprints.insert(file_path.clone(), leaf_fingerprint(&leaf_sirs));

            let job = RollupJob {
                file_path,
                language,
                leaf_sirs,
            };

            if job.leaf_sirs.len() <= 5 {
                local_only.push(job);
            } else {
                needs_api.push(job);
            }
        }

        tracing::info!(
            stale = stale_rollups.len(),
            local_only = local_only.len(),
            api_jobs = needs_api.len(),
            "prepared bulk file rollup jobs"
        );

        for (file_path, language) in stale_rollups {
            self.remove_file_rollup(store, file_path.as_str(), language)
                .with_context(|| format!("failed to remove stale file rollup for {file_path}"))?;
        }

        for job in local_only {
            let file_sir = concatenate_file_sir(&job.leaf_sirs);
            self.persist_file_rollup(
                store,
                job.file_path.as_str(),
                job.language,
                &file_sir,
                fingerprints.get(job.file_path.as_str()).map(Vec::as_slice),
                print_sir,
                out,
                commit_hash,
                generation_pass,
            )
            .with_context(|| {
                format!("failed to persist local file rollup for {}", job.file_path)
            })?;
        }

        for rollup in self
            .generate_api_file_rollups(needs_api)
            .context("failed to generate API-backed file rollups")?
        {
            self.persist_file_rollup(
                store,
                rollup.file_path.as_str(),
                rollup.language,
                &rollup.file_sir,
                fingerprints
                    .get(rollup.file_path.as_str())
                    .map(Vec::as_slice),
                print_sir,
                out,
                commit_hash,
                generation_pass,
            )
            .with_context(|| format!("failed to persist file rollup for {}", rollup.file_path))?;
        }

        Ok(())
    }

    pub(super) fn generate_api_file_rollups(
        &self,
        jobs: Vec<RollupJob>,
    ) -> Result<Vec<CompletedRollup>> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }

        let provider = self.provider.clone();
        let concurrency = self.sir_concurrency.max(1);
        let timeout_secs = self.inference_timeout_secs;
        let total_jobs = jobs.len();

        let mut completed = self.runtime.block_on(async move {
            let semaphore = Arc::new(Semaphore::new(concurrency));
            let mut join_set = JoinSet::new();

            for job in jobs {
                let provider = provider.clone();
                let semaphore = semaphore.clone();
                join_set.spawn(async move {
                    let file_path = job.file_path;
                    let language = job.language;
                    let leaf_sirs = job.leaf_sirs;

                    let summary = match semaphore.acquire_owned().await {
                        Ok(permit) => {
                            let _permit = permit;
                            summarize_file_intent_async(
                                file_path.as_str(),
                                language,
                                &leaf_sirs,
                                provider,
                                timeout_secs,
                            )
                            .await
                        }
                        Err(_) => Err(anyhow!("file rollup semaphore closed")),
                    };

                    let file_sir = match summary {
                        Ok(summary) if !summary.trim().is_empty() => {
                            file_sir_from_summary(&leaf_sirs, summary)
                        }
                        Ok(_) => concatenate_file_sir(&leaf_sirs),
                        Err(err) => {
                            tracing::debug!(
                                file_path = %file_path,
                                error = %err,
                                "file rollup summarization failed, using deterministic concatenation"
                            );
                            concatenate_file_sir(&leaf_sirs)
                        }
                    };

                    CompletedRollup {
                        file_path,
                        language,
                        file_sir,
                    }
                });
            }

            let mut completed = Vec::with_capacity(total_jobs);
            while let Some(joined) = join_set.join_next().await {
                completed.push(
                    joined.map_err(|err| anyhow!("file rollup task join failed: {err}"))?,
                );
            }

            Ok::<Vec<CompletedRollup>, anyhow::Error>(completed)
        })?;

        completed.sort_by(|left, right| left.file_path.cmp(&right.file_path));

        tracing::info!(
            job_count = total_jobs,
            concurrency,
            "completed bulk API file rollup generation"
        );

        Ok(completed)
    }

    pub(super) fn load_file_rollup_leaf_sirs(
        &self,
        store: &SqliteStore,
        file_path: &str,
    ) -> Result<Vec<FileLeafSir>> {
        load_file_leaf_sirs(store, file_path)
    }

    /// Retire a file's rollup because the file has no leaf SIRs left. Runs under the
    /// workspace inject lock and re-checks the leaves there: an `aether_sir_inject` call
    /// may have written a leaf and rebuilt the rollup since the file was snapshotted as
    /// empty, and that rollup must stay.
    pub(super) fn remove_file_rollup(
        &self,
        store: &SqliteStore,
        file_path: &str,
        language: Language,
    ) -> Result<()> {
        let _inject_guard = acquire_inject_write_lock(&self.workspace_root)?;
        if !load_file_leaf_sirs(store, file_path)?.is_empty() {
            tracing::info!(
                file_path = %file_path,
                "keeping file rollup: leaves were written since the file was seen empty"
            );
            return Ok(());
        }
        let rollup_id = synthetic_file_sir_id(language.as_str(), file_path);
        store
            .mark_removed(&rollup_id)
            .with_context(|| format!("failed to remove stale file rollup {rollup_id}"))
    }

    /// Persist a file rollup under the workspace inject lock. With `computed_from`, the
    /// leaves the rollup was built from, the write is skipped when the file's leaves have
    /// changed since (another writer, typically an `aether_sir_inject` call, rebuilt the
    /// rollup from newer leaves while this one was being generated), so a rollup from an
    /// older snapshot never overwrites a newer one.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn persist_file_rollup(
        &self,
        store: &SqliteStore,
        file_path: &str,
        language: Language,
        file_sir: &FileSir,
        computed_from: Option<&[(String, String)]>,
        print_sir: bool,
        out: &mut dyn Write,
        commit_hash: Option<&str>,
        generation_pass: &str,
    ) -> Result<()> {
        let _inject_guard = acquire_inject_write_lock(&self.workspace_root)?;
        if let Some(expected) = computed_from {
            let current = leaf_fingerprint(&load_file_leaf_sirs(store, file_path)?);
            if current != expected {
                tracing::info!(
                    file_path = %file_path,
                    "skipping file rollup computed from leaves that have since changed"
                );
                return Ok(());
            }
        }
        let rollup_id = synthetic_file_sir_id(language.as_str(), file_path);
        let canonical_json = canonicalize_file_sir_json(file_sir);
        let sir_hash_value = file_sir_hash(file_sir);
        let attempted_at = unix_timestamp_secs();
        let version_write = store
            .record_sir_version_if_changed(
                &rollup_id,
                &sir_hash_value,
                &self.provider_name,
                &self.model_name,
                &canonical_json,
                attempted_at,
                commit_hash,
            )
            .with_context(|| format!("failed to record file rollup history for {file_path}"))?;

        if version_write.changed {
            store
                .write_sir_blob(&rollup_id, &canonical_json)
                .with_context(|| format!("failed to write file rollup for {file_path}"))?;
        }

        store
            .upsert_sir_meta(SirMetaRecord {
                id: rollup_id.clone(),
                sir_hash: sir_hash_value.clone(),
                sir_version: version_write.version,
                provider: self.provider_name.clone(),
                model: self.model_name.clone(),
                generation_pass: generation_pass.to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at: version_write.updated_at,
                sir_status: SIR_STATUS_FRESH.to_owned(),
                last_error: None,
                last_attempt_at: attempted_at,
            })
            .with_context(|| format!("failed to upsert file rollup metadata for {file_path}"))?;

        if print_sir {
            writeln!(
                out,
                "SIR_FILE_STORED symbol_id={} sir_hash={} provider={}",
                rollup_id, sir_hash_value, self.provider_name
            )
            .context("failed to write file rollup print line")?;
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn upsert_file_rollup(
        &self,
        store: &SqliteStore,
        file_path: &str,
        language: Language,
        print_sir: bool,
        out: &mut dyn Write,
        commit_hash: Option<&str>,
        generation_pass: &str,
    ) -> Result<()> {
        let leaf_sirs = self
            .load_file_rollup_leaf_sirs(store, file_path)
            .with_context(|| format!("failed to load file rollup inputs for {file_path}"))?;

        if leaf_sirs.is_empty() {
            self.remove_file_rollup(store, file_path, language)
                .with_context(|| format!("failed to remove stale file rollup for {file_path}"))?;
            return Ok(());
        }

        let fingerprint = leaf_fingerprint(&leaf_sirs);
        let file_sir = aggregate_file_sir(
            file_path,
            language,
            &leaf_sirs,
            self.provider.clone(),
            &self.runtime,
            self.inference_timeout_secs,
        )
        .with_context(|| format!("failed to aggregate file rollup for {file_path}"))?;

        self.persist_file_rollup(
            store,
            file_path,
            language,
            &file_sir,
            Some(&fingerprint),
            print_sir,
            out,
            commit_hash,
            generation_pass,
        )
    }
}

/// Leaf SIRs of every symbol in `file_path` that carries a valid annotation.
/// The identity of a file's leaves as a rollup input: which symbols, with which SIR.
pub(super) fn leaf_fingerprint(leaves: &[FileLeafSir]) -> Vec<(String, String)> {
    let mut fingerprint: Vec<(String, String)> = leaves
        .iter()
        .map(|leaf| (leaf.qualified_name.clone(), aether_sir::sir_hash(&leaf.sir)))
        .collect();
    fingerprint.sort();
    fingerprint
}

pub(super) fn load_file_leaf_sirs(
    store: &SqliteStore,
    file_path: &str,
) -> Result<Vec<FileLeafSir>> {
    let symbols = store
        .list_symbols_for_file(file_path)
        .with_context(|| format!("failed to list symbols for file {file_path}"))?;
    let mut leaf_sirs = Vec::new();

    for symbol in symbols {
        let Some(blob) = store
            .read_sir_blob(&symbol.id)
            .with_context(|| format!("failed to read SIR blob for symbol {}", symbol.id))?
        else {
            continue;
        };

        let parsed = serde_json::from_str::<SirAnnotation>(&blob);
        let Ok(sir) = parsed else {
            tracing::warn!(
                symbol_id = %symbol.id,
                file_path = %file_path,
                "skipping invalid leaf SIR JSON while aggregating file rollup"
            );
            continue;
        };

        if let Err(err) = validate_sir(&sir) {
            tracing::warn!(
                symbol_id = %symbol.id,
                file_path = %file_path,
                error = %err,
                "skipping invalid leaf SIR annotation while aggregating file rollup"
            );
            continue;
        }

        leaf_sirs.push(FileLeafSir {
            qualified_name: symbol.qualified_name,
            sir,
        });
    }

    Ok(leaf_sirs)
}

/// Rebuild the deterministic (concatenated, no model call) file rollup for `file_path`
/// from its current leaf SIRs and persist it under the synthetic file SIR id, so file
/// and module level reads reflect leaf injections immediately. Removes the rollup when
/// the file has no valid leaf SIR left. Returns `true` when a rollup was written.
/// Rebuild a file rollup from its leaves by deterministic concatenation. The caller must
/// hold the workspace inject lock (`acquire_inject_write_lock`), as `aether_sir_inject`
/// does around the leaf write and this rebuild.
pub fn refresh_local_file_rollup(
    store: &SqliteStore,
    file_path: &str,
    language: Language,
    provider: &str,
    model: &str,
    generation_pass: &str,
) -> Result<bool> {
    let leaf_sirs = load_file_leaf_sirs(store, file_path)?;
    let rollup_id = synthetic_file_sir_id(language.as_str(), file_path);
    if leaf_sirs.is_empty() {
        store
            .mark_removed(&rollup_id)
            .with_context(|| format!("failed to remove stale file rollup {rollup_id}"))?;
        return Ok(false);
    }
    let file_sir = concatenate_file_sir(&leaf_sirs);
    let canonical_json = canonicalize_file_sir_json(&file_sir);
    let sir_hash_value = file_sir_hash(&file_sir);
    let attempted_at = unix_timestamp_secs();
    let version_write = store
        .record_sir_version_if_changed(
            &rollup_id,
            &sir_hash_value,
            provider,
            model,
            &canonical_json,
            attempted_at,
            None,
        )
        .with_context(|| format!("failed to record file rollup history for {file_path}"))?;
    // Always rewrite the (cheap, deterministic) blob: a retry after a failed write must
    // repair the live rollup even though the history already carries this hash.
    store
        .write_sir_blob(&rollup_id, &canonical_json)
        .with_context(|| format!("failed to write file rollup for {file_path}"))?;
    store
        .upsert_sir_meta(SirMetaRecord {
            id: rollup_id,
            sir_hash: sir_hash_value,
            sir_version: version_write.version,
            provider: provider.to_owned(),
            model: model.to_owned(),
            generation_pass: generation_pass.to_owned(),
            reasoning_trace: None,
            prompt_hash: None,
            staleness_score: None,
            updated_at: version_write.updated_at,
            sir_status: SIR_STATUS_FRESH.to_owned(),
            last_error: None,
            last_attempt_at: attempted_at,
        })
        .with_context(|| format!("failed to upsert file rollup metadata for {file_path}"))?;
    Ok(true)
}
