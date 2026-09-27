use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use aether_config::{AetherConfig, InferenceProviderKind};
use aether_core::{Language, Position, SourceRange, Symbol, SymbolKind};
use aether_infer::EmbeddingPurpose;
use aether_sir::SirAnnotation;
use aether_store::{
    SirFingerprintHistoryRecord, SirMetaRecord, SirStateStore, SqliteStore, SymbolEmbeddingRecord,
};
use anyhow::{Context, Result, anyhow};

use crate::batch::build::{BatchRequestOrigin, KEYMAP_SIDECAR_KIND, ORIGIN_SIDECAR_KIND};
use crate::batch::hash::diff_prompt_hashes;
use crate::batch::{BatchProvider, BatchResultLine, PassConfig};
use crate::continuous::cosine_distance_from_embeddings;
use crate::sir_pipeline::{
    EmbeddingInput, PriorSir, SirPipeline, UpsertSirIntentPayload, current_sir_identity,
    current_source_hash,
};

/// Number of embedding records to buffer before flushing to the vector store.
/// Keeps memory modest (~600KB for 3072-dim f32 vectors) while reducing LanceDB
/// merge_insert calls from one-per-symbol to one-per-batch.
const INGEST_VECTOR_BATCH_SIZE: usize = 50;

/// Number of symbols to collect before making a single batch embedding API call.
/// Matches the Gemini `batchEmbedContents` limit of 100 texts per request.
const EMBED_BATCH_SIZE: usize = 100;

#[derive(Debug, Clone, Default)]
pub(crate) struct IngestSummary {
    pub processed: usize,
    pub skipped: usize,
    /// Results left unapplied because the symbol's SIR was written (by an injection,
    /// say) or its source edited after the batch request was built; the newer state
    /// stands.
    pub superseded: usize,
    /// Results whose SIR an earlier, downstream-failed ingest attempt had already
    /// persisted (the row still carries that write's batch provenance); counted in
    /// `processed` too, with only the embedding, contract and fingerprint work redone.
    pub resumed: usize,
    pub fingerprint_rows: usize,
}

/// Per-symbol state collected during Phase 1 (parse + persist SIR).
struct PreparedSymbol {
    symbol_id: String,
    prompt_hash: String,
    canonical_json: String,
    sir_hash: String,
    previous_meta: Option<SirMetaRecord>,
    previous_embedding: Option<SymbolEmbeddingRecord>,
    /// Index into the batch embedding input vec, or `None` if embedding is disabled.
    embedding_slot: Option<usize>,
    /// The store still holds this result's own earlier write (an ingest attempt that
    /// failed downstream); the leaf was left as is and only the downstream work is redone.
    resumed: bool,
    /// The `write_generation` of the leaf write this result stands on: the one phase 1
    /// just made, or, for a resumed result, its own earlier write. Downstream rows
    /// carry it, which tells them from rows an older ingest of the same prompt left.
    write_generation: i64,
}

/// Map batch provider name to the closest `InferenceProviderKind`.
fn provider_kind_from_name(name: &str) -> InferenceProviderKind {
    match name {
        "gemini" => InferenceProviderKind::Gemini,
        "openai" => InferenceProviderKind::OpenAiCompat,
        "anthropic" => InferenceProviderKind::OpenAiCompat,
        _ => InferenceProviderKind::Gemini,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ingest_results(
    workspace: &Path,
    store: &SqliteStore,
    pass_config: &PassConfig,
    results_path: &Path,
    config: &AetherConfig,
    provider: &dyn BatchProvider,
    provider_name: &str,
) -> Result<IngestSummary> {
    let keymap = load_keymap(results_path, pass_config.pass.as_str());
    if !keymap.is_empty() {
        tracing::debug!(
            keys = keymap.len(),
            "loaded batch keymap for prompt-hash recovery"
        );
    }
    let origins = load_origins(results_path, pass_config.pass.as_str());

    let file = std::fs::File::open(results_path)
        .with_context(|| format!("failed to open batch results {}", results_path.display()))?;
    let reader = BufReader::new(file);

    let pipeline = SirPipeline::new_embeddings_only(workspace.to_path_buf())
        .map(|pipeline| pipeline.with_skip_surreal_sync(true))
        .context("failed to initialize batch ingest pipeline")?;

    let mut summary = IngestSummary::default();
    let mut embedding_buffer: Vec<SymbolEmbeddingRecord> =
        Vec::with_capacity(INGEST_VECTOR_BATCH_SIZE);

    // Collect raw lines into a buffer so we can process them in chunks.
    let mut line_chunk: Vec<String> = Vec::with_capacity(EMBED_BATCH_SIZE);

    for (line_number, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                summary.skipped += 1;
                tracing::warn!(line_number = line_number + 1, error = %err, "failed to read batch result line");
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        line_chunk.push(line);

        if line_chunk.len() >= EMBED_BATCH_SIZE {
            process_chunk(
                &pipeline,
                store,
                pass_config,
                config,
                provider,
                provider_name,
                &keymap,
                &origins,
                &line_chunk,
                &mut summary,
                &mut embedding_buffer,
            )?;
            line_chunk.clear();
        }
    }

    // Process any remaining lines.
    if !line_chunk.is_empty() {
        process_chunk(
            &pipeline,
            store,
            pass_config,
            config,
            provider,
            provider_name,
            &keymap,
            &origins,
            &line_chunk,
            &mut summary,
            &mut embedding_buffer,
        )?;
    }

    // Flush any remaining buffered embeddings to vector store.
    if !embedding_buffer.is_empty() {
        pipeline
            .flush_embedding_batch(store, embedding_buffer)
            .context("failed to flush final embedding batch during ingest")?;
    }

    Ok(summary)
}

/// Process a chunk of JSONL lines using batched embedding generation.
///
/// Phase 1: Parse + persist SIR for each line, determine which need embeddings.
/// Phase 2: Batch-embed all texts that need new embeddings in a single API call.
/// Phase 3: Finalize each symbol (contract verification, fingerprint, buffer).
#[allow(clippy::too_many_arguments)]
fn process_chunk(
    pipeline: &SirPipeline,
    store: &SqliteStore,
    pass_config: &PassConfig,
    config: &AetherConfig,
    provider: &dyn BatchProvider,
    provider_name: &str,
    keymap: &HashMap<String, String>,
    origins: &HashMap<String, BatchRequestOrigin>,
    lines: &[String],
    summary: &mut IngestSummary,
    embedding_buffer: &mut Vec<SymbolEmbeddingRecord>,
) -> Result<()> {
    let mut prepared: Vec<PreparedSymbol> = Vec::with_capacity(lines.len());
    let mut embed_inputs: Vec<EmbeddingInput> = Vec::new();
    let embedding_identity = pipeline.embedding_identity();

    // Phase 1: Parse, persist SIR, queue embeddings for regeneration.
    for raw_line in lines {
        match prepare_symbol(
            pipeline,
            store,
            pass_config,
            raw_line,
            provider,
            provider_name,
            keymap,
            origins,
        ) {
            Ok(None) => {
                summary.superseded += 1;
            }
            Ok(Some(mut prep)) => {
                if let Some((provider, model)) = embedding_identity {
                    prep.embedding_slot = Some(embed_inputs.len());
                    embed_inputs.push(EmbeddingInput {
                        symbol_id: prep.symbol_id.clone(),
                        sir_hash: prep.sir_hash.clone(),
                        canonical_json: prep.canonical_json.clone(),
                        provider: provider.to_owned(),
                        model: model.to_owned(),
                    });
                }
                prepared.push(prep);
            }
            Err(err) => {
                summary.skipped += 1;
                tracing::warn!(error = %err, "skipping invalid batch result line");
            }
        }
    }

    // Phase 2: Batch-embed all texts that need new embeddings.
    let texts: Vec<&str> = embed_inputs
        .iter()
        .map(|input| input.canonical_json.as_str())
        .collect();
    let embedding_records = if texts.is_empty() {
        Vec::new()
    } else {
        let embeddings = pipeline
            .batch_embed_texts(&texts, EmbeddingPurpose::Document)
            .context("batch embedding failed during ingest")?;
        SirPipeline::build_embedding_records(&embed_inputs, embeddings)
    };

    // Build lookup from symbol_id to the newly generated embedding record.
    let record_map: HashMap<&str, &SymbolEmbeddingRecord> = embedding_records
        .iter()
        .map(|r| (r.symbol_id.as_str(), r))
        .collect();

    // Phase 3: Finalize each symbol.
    for prep in &prepared {
        let generated = record_map.get(prep.symbol_id.as_str()).copied();
        let current_embedding = generated
            .cloned()
            .or_else(|| prep.previous_embedding.clone());

        // Contract verification (non-fatal).
        if let Some(ref contracts_config) = config.contracts
            && contracts_config.enabled
            && let Err(err) = crate::contracts::verify_symbol_contracts(
                store,
                &prep.symbol_id,
                prep.canonical_json.as_str(),
                current_embedding.as_ref().map(|e| e.embedding.as_slice()),
                config,
                pipeline.workspace_root(),
            )
        {
            tracing::warn!(
                symbol_id = %prep.symbol_id,
                error = %err,
                "Contract verification failed during batch ingest"
            );
        }

        let trigger = format!("batch_{}", pass_config.pass.as_str());
        // The row's predecessor is the prompt the symbol was last generated for: the
        // previous metadata's prompt hash when it names another prompt, else (a SIR
        // from an injection or the daemon carries none, and a resumed result's
        // "previous" metadata is its own write) the last prompt the symbol was
        // fingerprinted for. A resumed result may also have written its row before the
        // earlier attempt failed (in the vector flush, say): the row is appended once
        // per result, never per attempt. Only a row carrying this result's own leaf
        // write generation counts as its row; an older row an earlier ingest of the
        // same prompt left behind records that earlier event, not this one.
        let mut previous_prompt_hash = prep
            .previous_meta
            .as_ref()
            .and_then(|record| record.prompt_hash.clone())
            .filter(|previous| *previous != prep.prompt_hash);
        let mut fingerprint_written = false;
        if prep.resumed || previous_prompt_hash.is_none() {
            let history = store
                .list_sir_fingerprint_history(&prep.symbol_id)
                .with_context(|| {
                    format!("failed to read fingerprint history for {}", prep.symbol_id)
                })?;
            if prep.resumed {
                fingerprint_written = history.iter().any(|row| {
                    row.sir_write_generation == Some(prep.write_generation)
                        && row.prompt_hash == prep.prompt_hash
                        && row.trigger == trigger
                });
            }
            if previous_prompt_hash.is_none() {
                previous_prompt_hash = history
                    .iter()
                    .rev()
                    .find(|row| row.prompt_hash != prep.prompt_hash)
                    .map(|row| row.prompt_hash.clone());
            }
        }
        if !fingerprint_written {
            write_fingerprint_row(
                store,
                &prep.symbol_id,
                &prep.prompt_hash,
                previous_prompt_hash.as_deref(),
                trigger.as_str(),
                pass_config.model.as_str(),
                pass_config.pass.as_str(),
                cosine_distance_from_embeddings(
                    prep.previous_embedding.as_ref(),
                    current_embedding.as_ref(),
                ),
                Some(prep.write_generation),
            )
            .with_context(|| format!("failed to write fingerprint row for {}", prep.symbol_id))?;
            summary.fingerprint_rows += 1;
        }

        summary.processed += 1;
        if prep.resumed {
            summary.resumed += 1;
        }
    }

    // Buffer new embedding records for vector store flush.
    for record in embedding_records {
        embedding_buffer.push(record);
    }

    // Flush to vector store if buffer exceeds threshold.
    if embedding_buffer.len() >= INGEST_VECTOR_BATCH_SIZE {
        let batch = std::mem::take(embedding_buffer);
        pipeline
            .flush_embedding_batch(store, batch)
            .context("failed to flush embedding batch during ingest")?;
    }

    Ok(())
}

/// Phase 1: Parse a single batch result line and persist SIR to SQLite.
///
/// Does everything the old `ingest_result_line` did except embedding generation,
/// contract verification, and fingerprint writing. `None`: the result was superseded
/// (see `IngestSummary::superseded`) and nothing was written.
#[allow(clippy::too_many_arguments)]
fn prepare_symbol(
    pipeline: &SirPipeline,
    store: &SqliteStore,
    pass_config: &PassConfig,
    raw_line: &str,
    provider: &dyn BatchProvider,
    provider_name: &str,
    keymap: &HashMap<String, String>,
    origins: &HashMap<String, BatchRequestOrigin>,
) -> Result<Option<PreparedSymbol>> {
    let (symbol_id, prompt_hash, request_key, sir_json, reasoning_trace) =
        match provider.parse_result_line(raw_line)? {
            BatchResultLine::Success {
                key,
                text,
                reasoning_trace,
            } => {
                // If the key lacks a delimiter (e.g. Anthropic truncated custom_id),
                // try to recover the full key from the build-time keymap.
                let resolved_key = if !key.contains('|') {
                    keymap.get(&key).map(String::as_str).unwrap_or(&key)
                } else {
                    &key
                };
                let (sid, phash) = parse_key(resolved_key)?;
                (
                    sid.to_owned(),
                    phash.to_owned(),
                    resolved_key.to_owned(),
                    text,
                    reasoning_trace,
                )
            }
            BatchResultLine::Error { key, message } => {
                return Err(anyhow!("batch response error (key={:?}): {}", key, message));
            }
        };

    // Strip markdown fences and trailing prose from model responses.
    // Some models wrap JSON in ```json...``` fences and/or append explanatory text.
    let sir_json_clean = {
        let s = sir_json.trim();
        let s = s
            .strip_prefix("```json")
            .or_else(|| s.strip_prefix("```"))
            .unwrap_or(s);
        let s = s.trim();
        let s = s.strip_suffix("```").unwrap_or(s);
        let s = s.trim();
        // Find the end of the top-level JSON object by matching braces
        let mut depth = 0i32;
        let mut end = s.len();
        let mut in_string = false;
        let mut escape_next = false;
        for (i, ch) in s.char_indices() {
            if escape_next {
                escape_next = false;
                continue;
            }
            if ch == '\\' && in_string {
                escape_next = true;
                continue;
            }
            if ch == '"' {
                in_string = !in_string;
                continue;
            }
            if in_string {
                continue;
            }
            if ch == '{' {
                depth += 1;
            }
            if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
        }
        &s[..end]
    };
    let sir = serde_json::from_str::<SirAnnotation>(sir_json_clean)
        .context("failed to parse SIR JSON from batch response")?;
    let symbol_record = store
        .get_symbol_record(&symbol_id)
        .with_context(|| format!("failed to load symbol record for {symbol_id}"))?
        .ok_or_else(|| anyhow!("symbol '{symbol_id}' not found in symbols table"))?;
    let previous_meta = store
        .get_sir_meta(&symbol_id)
        .with_context(|| format!("failed to read previous SIR metadata for {symbol_id}"))?;
    // Skip per-symbol LanceDB lookup during batch ingest — previous
    // embedding is only used for delta_sem in fingerprint rows.
    let previous_embedding: Option<SymbolEmbeddingRecord> = None;

    // A key carrying a build id was written with an origin entry; a result whose entry
    // is missing (sidecar moved, deleted or unreadable) is refused rather than ingested
    // unchecked, since nothing else vouches for the SIR and source it was built from.
    // Only a key from before origins existed (no build id) is ingested unchecked.
    let origin = origins.get(&request_key);
    if origin.is_none() && key_has_build_id(&request_key) {
        return Err(anyhow!(
            "batch result '{request_key}' has no entry in its build's origin sidecar; refusing to ingest it unchecked"
        ));
    }
    let provider_kind = provider_kind_from_name(provider_name);
    let payload = UpsertSirIntentPayload {
        symbol: symbol_from_record(&symbol_record)?,
        sir,
        provider_name: provider_kind.as_str().to_owned(),
        model_name: pass_config.model.clone(),
        generation_pass: pass_config.pass.as_str().to_owned(),
        reasoning_trace,
        commit_hash: None,
        prior_sir: origin.map_or(PriorSir::Unrecorded, |origin| {
            PriorSir::recorded(origin.prior_sir.clone())
        }),
        // Written with the leaf, in its transaction: the row then says which batch
        // request produced it (see the resume check below).
        prompt_hash: Some(prompt_hash.clone()),
    };
    // The result was generated from the SIR and the symbol source the request was built
    // from, possibly hours ago. Under the inject lock every leaf writer shares, persist
    // it only while the store still holds exactly that SIR (hash and history version)
    // and the symbol's source still hashes the same; a symbol written since (an
    // `aether_sir_inject` from `/scan`, say) keeps its newer SIR, and a symbol edited
    // since keeps waiting for the daemon's regeneration from the new source rather than
    // taking a SIR of the old one (which would also pre-empt that regeneration).
    //
    // One newer write is this result's own: an earlier ingest attempt that persisted
    // the SIR and then failed downstream (embedding, vector flush, fingerprint). That
    // write carried this request's provenance (prompt hash, pass, provider, model) in
    // the same transaction as the leaf, so the row itself is the recoverable token: a
    // result whose origin identity no longer holds is resumed (the leaf is not
    // rewritten, so its identity is unchanged) only while the store's current SIR has
    // this result's hash and exactly that provenance. An injection or a daemon
    // regeneration never writes a prompt hash, so a same-content SIR written
    // independently since supersedes the result like any other, and a different SIR
    // trivially does.
    let (canonical_json, sir_hash_value, resumed, write_generation) = {
        let _inject_guard =
            crate::sir_pipeline::acquire_inject_write_lock(pipeline.workspace_root())?;
        let current = current_sir_identity(store, &symbol_id)?;
        let (_, canonical_json, sir_hash_value) =
            pipeline.prepare_sir_for_persistence(store, &payload.symbol, &payload.sir)?;
        let resumed_write = if payload.prior_sir.still_holds(current.as_ref()) {
            None
        } else {
            let current_meta = store
                .get_sir_meta(&symbol_id)
                .with_context(|| format!("failed to read current SIR metadata for {symbol_id}"))?;
            let own_write = current
                .as_ref()
                .zip(current_meta.as_ref())
                .filter(|(identity, meta)| {
                    identity.sir_hash == sir_hash_value
                        && meta.prompt_hash.as_deref() == Some(prompt_hash.as_str())
                        && meta.generation_pass == payload.generation_pass
                        && meta.provider == payload.provider_name
                        && meta.model == payload.model_name
                })
                .map(|(identity, _)| identity.write_generation);
            let Some(write_generation) = own_write else {
                tracing::info!(
                    symbol_id = %symbol_id,
                    "skipping batch result: the stored SIR changed since the request was built"
                );
                return Ok(None);
            };
            tracing::info!(
                symbol_id = %symbol_id,
                "resuming batch result: the store holds this result's own earlier write"
            );
            Some(write_generation)
        };
        let resumed = resumed_write.is_some();
        if let Some(origin) = origin
            && !resumed
        {
            // Re-read and re-parse the symbol's file here, under the lock, and find the
            // symbol by id, rather than trusting a snapshot's hash or its recorded range:
            // an edit landing after a snapshot would otherwise slip through, and one
            // elsewhere in the file would move the symbol out of the old range, so an
            // unchanged body would read as changed while a changed one could hide behind
            // whatever text now fills that range. A file that is gone or no longer
            // declares the symbol, or a body that no longer hashes to what the prompt
            // was built from, all count as changed.
            if current_source_hash(pipeline.workspace_root(), &payload.symbol).as_deref()
                != Some(origin.source_hash.as_str())
            {
                tracing::info!(
                    symbol_id = %symbol_id,
                    "skipping batch result: the symbol source changed since the request was built"
                );
                return Ok(None);
            }
        }
        let write_generation = match resumed_write {
            Some(write_generation) => write_generation,
            None => {
                // Leaf, history, metadata and this request's provenance in one transaction.
                pipeline
                    .persist_sir_payload_into_sqlite(store, &payload, None)
                    .with_context(|| format!("failed to persist SIR payload for {symbol_id}"))?;
                current_sir_identity(store, &symbol_id)?
                    .ok_or_else(|| anyhow!("missing persisted SIR identity for {symbol_id}"))?
                    .write_generation
            }
        };
        (canonical_json, sir_hash_value, resumed, write_generation)
    };

    Ok(Some(PreparedSymbol {
        symbol_id,
        prompt_hash,
        canonical_json,
        sir_hash: sir_hash_value,
        previous_meta,
        previous_embedding,
        embedding_slot: None,
        resumed,
        write_generation,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn write_fingerprint_row(
    store: &SqliteStore,
    symbol_id: &str,
    prompt_hash: &str,
    previous_prompt_hash: Option<&str>,
    trigger: &str,
    generation_model: &str,
    generation_pass: &str,
    delta_sem: Option<f64>,
    sir_write_generation: Option<i64>,
) -> Result<()> {
    let (source_changed, neighbor_changed, config_changed) = previous_prompt_hash
        .map_or((false, false, false), |old| {
            diff_prompt_hashes(old, prompt_hash)
        });
    store
        .insert_sir_fingerprint_history(&SirFingerprintHistoryRecord {
            symbol_id: symbol_id.to_owned(),
            timestamp: unix_timestamp_secs(),
            prompt_hash: prompt_hash.to_owned(),
            prompt_hash_previous: previous_prompt_hash.map(str::to_owned),
            trigger: trigger.to_owned(),
            source_changed,
            neighbor_changed,
            config_changed,
            generation_model: Some(generation_model.to_owned()),
            generation_pass: Some(generation_pass.to_owned()),
            delta_sem,
            sir_write_generation,
        })
        .with_context(|| format!("failed to insert fingerprint history row for {symbol_id}"))
}

/// Split a request key `symbol_id|prompt_hash[|build_id]` into its symbol id and prompt
/// hash (the build id only distinguishes sidecar entries, see `BuildSummary::build_id`).
fn key_has_build_id(key: &str) -> bool {
    key_build_id(key).is_some()
}

/// The build id of a three-part key (`symbol_id|prompt_hash|build_id`), if any.
fn key_build_id(key: &str) -> Option<&str> {
    key.splitn(3, '|')
        .nth(2)
        .map(str::trim)
        .filter(|build_id| !build_id.is_empty())
}

fn parse_key(key: &str) -> Result<(&str, &str)> {
    let (symbol_id, rest) = key.split_once('|').unwrap_or((key, "unknown"));
    let prompt_hash = rest
        .split_once('|')
        .map_or(rest, |(prompt_hash, _)| prompt_hash);
    let symbol_id = symbol_id.trim();
    let prompt_hash = prompt_hash.trim();
    if symbol_id.is_empty() || prompt_hash.is_empty() {
        return Err(anyhow!(
            "batch response key is missing symbol_id or prompt_hash"
        ));
    }
    Ok((symbol_id, prompt_hash))
}

/// Load the origin sidecars written during JSONL build (see `BuildSummary::origins`),
/// one per build of the pass still present in the directory. A result whose build has
/// none is refused (see `prepare_symbol`); a legacy result without a build id is
/// ingested unchecked.
fn load_origins(results_path: &Path, pass: &str) -> HashMap<String, BatchRequestOrigin> {
    load_sidecars(results_path, pass, ORIGIN_SIDECAR_KIND)
}

/// Load the keymap sidecars written during JSONL build, one per build of the pass
/// still present in the directory.
///
/// A keymap maps each provider request key to its full `symbol_id|prompt_hash|build_id`
/// key, allowing ingest to recover full batch keys from providers that truncate
/// custom_id (e.g. Anthropic's 64-char limit).
fn load_keymap(results_path: &Path, pass: &str) -> HashMap<String, String> {
    load_sidecars(results_path, pass, KEYMAP_SIDECAR_KIND)
}

/// The union of every `<pass>.<build_id>.<kind>.json` beside the results (each build
/// writes its own, and keys carry the build id, so entries never collide) plus the
/// pre-build-id `<pass>.<kind>.json` when one is present. An unparsable sidecar is
/// skipped with a warning.
fn load_sidecars<V: serde::de::DeserializeOwned>(
    results_path: &Path,
    pass: &str,
    kind: &str,
) -> HashMap<String, V> {
    let Some(batch_dir) = results_path.parent() else {
        return HashMap::new();
    };
    let Ok(entries) = std::fs::read_dir(batch_dir) else {
        return HashMap::new();
    };
    let prefix = format!("{pass}.");
    let suffix = format!(".{kind}.json");
    let legacy = format!("{pass}.{kind}.json");
    let mut merged = HashMap::new();
    let mut paths = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name == legacy || (name.starts_with(&prefix) && name.ends_with(&suffix))
                })
        })
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<HashMap<String, V>>(&content) {
            Ok(entries) => merged.extend(entries),
            Err(err) => tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to parse batch {kind} sidecar, its entries are unavailable"
            ),
        }
    }
    merged
}

fn symbol_from_record(record: &aether_store::SymbolRecord) -> Result<Symbol> {
    Ok(Symbol {
        id: record.id.clone(),
        language: parse_language(record.language.as_str())?,
        file_path: record.file_path.clone(),
        kind: parse_symbol_kind(record.kind.as_str())?,
        name: record
            .qualified_name
            .rsplit("::")
            .next()
            .or_else(|| record.qualified_name.rsplit('.').next())
            .unwrap_or(record.qualified_name.as_str())
            .to_owned(),
        qualified_name: record.qualified_name.clone(),
        signature_fingerprint: record.signature_fingerprint.clone(),
        content_hash: String::new(),
        range: SourceRange {
            start: Position { line: 1, column: 1 },
            end: Position { line: 1, column: 1 },
            start_byte: Some(0),
            end_byte: Some(0),
        },
    })
}

fn parse_language(raw: &str) -> Result<Language> {
    match raw.trim() {
        "rust" => Ok(Language::Rust),
        "typescript" => Ok(Language::TypeScript),
        "tsx" => Ok(Language::Tsx),
        "javascript" => Ok(Language::JavaScript),
        "jsx" => Ok(Language::Jsx),
        "python" => Ok(Language::Python),
        other => Err(anyhow!("unsupported symbol language '{other}'")),
    }
}

fn parse_symbol_kind(raw: &str) -> Result<SymbolKind> {
    match raw.trim() {
        "function" => Ok(SymbolKind::Function),
        "method" => Ok(SymbolKind::Method),
        "class" => Ok(SymbolKind::Class),
        "variable" => Ok(SymbolKind::Variable),
        "struct" => Ok(SymbolKind::Struct),
        "enum" => Ok(SymbolKind::Enum),
        "trait" => Ok(SymbolKind::Trait),
        "interface" => Ok(SymbolKind::Interface),
        "type_alias" => Ok(SymbolKind::TypeAlias),
        other => Err(anyhow!("unsupported symbol kind '{other}'")),
    }
}

fn unix_timestamp_secs() -> i64 {
    crate::time::current_unix_timestamp_secs()
}

#[cfg(test)]
mod tests;
