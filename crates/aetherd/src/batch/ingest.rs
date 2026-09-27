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

use crate::batch::build::{
    BatchRequestOrigin, KEYMAP_SIDECAR_KIND, ORIGIN_SIDECAR_KIND, snapshot_workspace_symbols,
};
use crate::batch::hash::diff_prompt_hashes;
use crate::batch::{BatchProvider, BatchResultLine, PassConfig};
use crate::continuous::cosine_distance_from_embeddings;
use crate::sir_pipeline::{
    EmbeddingInput, PriorSir, SirPipeline, UpsertSirIntentPayload, current_sir_identity,
    extract_symbol_source_text,
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
    symbols_by_id: Option<&HashMap<String, Symbol>>,
) -> Result<IngestSummary> {
    let keymap = load_keymap(results_path, pass_config.pass.as_str());
    if !keymap.is_empty() {
        tracing::debug!(
            keys = keymap.len(),
            "loaded batch keymap for prompt-hash recovery"
        );
    }
    let origins = load_origins(results_path, pass_config.pass.as_str());
    // Results carrying an origin are checked against the symbol source as it is now,
    // so the workspace is snapshotted once here unless the caller already has one.
    let snapshot;
    let current_symbols = if origins.is_empty() {
        symbols_by_id
    } else {
        match symbols_by_id {
            Some(symbols) => Some(symbols),
            None => {
                snapshot = snapshot_workspace_symbols(workspace)
                    .context("failed to snapshot workspace symbols for batch ingest")?;
                Some(&snapshot)
            }
        }
    };

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
                current_symbols,
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
            current_symbols,
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
    current_symbols: Option<&HashMap<String, Symbol>>,
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
            current_symbols,
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
        // per result, never per attempt.
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
                fingerprint_written = history
                    .iter()
                    .any(|row| row.prompt_hash == prep.prompt_hash && row.trigger == trigger);
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
    current_symbols: Option<&HashMap<String, Symbol>>,
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
    let (canonical_json, sir_hash_value, resumed) = {
        let _inject_guard =
            crate::sir_pipeline::acquire_inject_write_lock(pipeline.workspace_root())?;
        let current = current_sir_identity(store, &symbol_id)?;
        let (_, canonical_json, sir_hash_value) =
            pipeline.prepare_sir_for_persistence(store, &payload.symbol, &payload.sir)?;
        let resumed = if payload.prior_sir.still_holds(current.as_ref()) {
            false
        } else {
            let current_meta = store
                .get_sir_meta(&symbol_id)
                .with_context(|| format!("failed to read current SIR metadata for {symbol_id}"))?;
            let own_write =
                current
                    .as_ref()
                    .zip(current_meta.as_ref())
                    .is_some_and(|(identity, meta)| {
                        identity.sir_hash == sir_hash_value
                            && meta.prompt_hash.as_deref() == Some(prompt_hash.as_str())
                            && meta.generation_pass == payload.generation_pass
                            && meta.provider == payload.provider_name
                            && meta.model == payload.model_name
                    });
            if !own_write {
                tracing::info!(
                    symbol_id = %symbol_id,
                    "skipping batch result: the stored SIR changed since the request was built"
                );
                return Ok(None);
            }
            tracing::info!(
                symbol_id = %symbol_id,
                "resuming batch result: the store holds this result's own earlier write"
            );
            true
        };
        if let Some(origin) = origin
            && !resumed
        {
            // Re-read the symbol's source here, under the lock, rather than trusting
            // the snapshot's hash: an edit landing after the snapshot would otherwise
            // slip through. The snapshot only says where the symbol is; a symbol it
            // does not know, a file that is gone, or text at that range that no longer
            // hashes to what the prompt was built from all count as changed.
            let current_source_hash = current_symbols
                .and_then(|symbols| symbols.get(&symbol_id))
                .and_then(|symbol| {
                    let source =
                        std::fs::read_to_string(pipeline.workspace_root().join(&symbol.file_path))
                            .ok()?;
                    extract_symbol_source_text(&source, symbol.range)
                })
                .map(|text| aether_core::content_hash(&text));
            if current_source_hash.as_deref() != Some(origin.source_hash.as_str()) {
                tracing::info!(
                    symbol_id = %symbol_id,
                    "skipping batch result: the symbol source changed since the request was built"
                );
                return Ok(None);
            }
        }
        if !resumed {
            // Leaf, history, metadata and this request's provenance in one transaction.
            pipeline
                .persist_sir_payload_into_sqlite(store, &payload, None)
                .with_context(|| format!("failed to persist SIR payload for {symbol_id}"))?;
        }
        (canonical_json, sir_hash_value, resumed)
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
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use aether_store::{SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord};
    use async_trait::async_trait;
    use tempfile::tempdir;

    use super::*;
    use crate::batch::{BatchPollStatus, BatchProvider, BatchResultLine};
    use crate::cli::BatchPass;

    struct StubBatchProvider {
        key: String,
        text: String,
        reasoning_trace: Option<String>,
    }

    #[async_trait]
    impl BatchProvider for StubBatchProvider {
        fn format_request(
            &self,
            _key: &str,
            _system_prompt: &str,
            _user_prompt: &str,
            _model: &str,
            _thinking: &str,
        ) -> Result<String> {
            unreachable!("format_request is not used in ingest tests")
        }

        async fn submit(
            &self,
            _input_path: &Path,
            _model: &str,
            _batch_dir: &Path,
            _poll_interval_secs: u64,
        ) -> Result<Vec<String>> {
            unreachable!("submit is not used in ingest tests")
        }

        async fn poll(&self, _job_ids: &[String]) -> Result<BatchPollStatus> {
            unreachable!("poll is not used in ingest tests")
        }

        async fn download_results(
            &self,
            _job_ids: &[String],
            _output_dir: &Path,
        ) -> Result<Vec<PathBuf>> {
            unreachable!("download_results is not used in ingest tests")
        }

        fn parse_result_line(&self, _line: &str) -> Result<BatchResultLine> {
            Ok(BatchResultLine::Success {
                key: self.key.clone(),
                text: self.text.clone(),
                reasoning_trace: self.reasoning_trace.clone(),
            })
        }

        fn name(&self) -> &str {
            "stub"
        }
    }

    fn write_embeddings_only_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn demo_symbol_record(symbol_id: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: symbol_id.to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{symbol_id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    fn triage_pass_config() -> PassConfig {
        PassConfig {
            pass: BatchPass::Triage,
            model: "triage-model".to_owned(),
            thinking: "low".to_owned(),
            neighbor_depth: 1,
            max_chars: 8_000,
            prompt_tier: "standard".to_owned(),
        }
    }

    fn demo_sir() -> SirAnnotation {
        SirAnnotation {
            intent: "Demo intent".to_owned(),
            behavior: None,
            inputs: vec!["input".to_owned()],
            outputs: vec!["output".to_owned()],
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.9,
            edge_cases: None,
            complexity: None,
            method_dependencies: None,
        }
    }

    #[test]
    fn parse_key_reads_two_and_three_part_keys() {
        assert_eq!(parse_key("sym|hash").expect("two parts"), ("sym", "hash"));
        assert_eq!(
            parse_key("sym|hash|0123456789ab").expect("three parts"),
            ("sym", "hash")
        );
        assert!(parse_key("|hash").is_err());
        assert!(parse_key("sym|").is_err());
        assert!(key_has_build_id("sym|hash|0123456789ab"));
        assert!(!key_has_build_id("sym|hash"));
        assert!(!key_has_build_id("sym|hash|"));
        assert!(!key_has_build_id("sym"));
    }

    #[test]
    fn prepare_symbol_skips_a_result_whose_sir_moved_on_since_the_build() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let record = demo_symbol_record("sym-late", "demo::late");
        store.upsert_symbol(record.clone()).expect("upsert symbol");
        let pipeline = SirPipeline::new_embeddings_only(workspace.to_path_buf())
            .map(|pipeline| pipeline.with_skip_surreal_sync(true))
            .expect("build embeddings-only pipeline");
        // The symbol's source on disk, as the build snapshot saw it.
        let source = "fn late() {}\n";
        fs::create_dir_all(workspace.join("src")).expect("create src");
        fs::write(workspace.join("src/lib.rs"), source).expect("write source");
        let symbol = Symbol {
            content_hash: aether_core::content_hash(source),
            range: SourceRange {
                start: Position { line: 1, column: 1 },
                end: Position {
                    line: 1,
                    column: source.trim_end().len() + 1,
                },
                start_byte: Some(0),
                end_byte: Some(source.len()),
            },
            ..symbol_from_record(&record).expect("build symbol")
        };
        let persist = |sir: &SirAnnotation, pass: &str| {
            pipeline
                .persist_sir_payload_into_sqlite(
                    &store,
                    &UpsertSirIntentPayload {
                        symbol: symbol.clone(),
                        sir: sir.clone(),
                        provider_name: "gemini".to_owned(),
                        model_name: "scan-model".to_owned(),
                        generation_pass: pass.to_owned(),
                        reasoning_trace: None,
                        commit_hash: None,
                        prompt_hash: None,
                        prior_sir: PriorSir::Unrecorded,
                    },
                    None,
                )
                .expect("persist payload");
        };

        // The batch was built while the store held the scan SIR...
        persist(&demo_sir(), "scan");
        let built_against = current_sir_identity(&store, "sym-late").expect("identity");
        let key = "sym-late|prompt-late|build-1".to_owned();
        let source_hash = symbol.content_hash.clone();
        let origins = HashMap::from([(
            key.clone(),
            BatchRequestOrigin {
                prior_sir: built_against,
                source_hash: source_hash.clone(),
            },
        )]);
        let current_symbols = HashMap::from([("sym-late".to_owned(), symbol.clone())]);

        // ...and an injection replaced it before the result came back.
        let reviewed = SirAnnotation {
            intent: "Reviewed by hand".to_owned(),
            confidence: 0.97,
            ..demo_sir()
        };
        persist(&reviewed, "injected");
        let reviewed_identity = current_sir_identity(&store, "sym-late").expect("identity");

        let batch_sir = SirAnnotation {
            intent: "Triage result from the older state".to_owned(),
            ..demo_sir()
        };
        let provider = StubBatchProvider {
            key: key.clone(),
            text: serde_json::to_string(&batch_sir).expect("serialize sir"),
            reasoning_trace: None,
        };
        let outcome = prepare_symbol(
            &pipeline,
            &store,
            &triage_pass_config(),
            "ignored",
            &provider,
            "gemini",
            &HashMap::new(),
            &origins,
            Some(&current_symbols),
        )
        .expect("prepare symbol");
        assert!(
            outcome.is_none(),
            "a result for a replaced SIR is not applied"
        );
        let meta = store
            .get_sir_meta("sym-late")
            .expect("load sir meta")
            .expect("sir meta exists");
        assert_eq!(
            current_sir_identity(&store, "sym-late").expect("identity"),
            reviewed_identity,
            "the injected SIR must stand"
        );
        assert_eq!(meta.generation_pass, "injected");
        assert_eq!(
            meta.prompt_hash, None,
            "no provenance from the skipped result"
        );

        // Built against the SIR the store still holds but a source since edited on
        // disk (same id, different body; the snapshot still says the old hash), the
        // result is not applied either: the daemon's regeneration from the new source
        // must not be pre-empted by a SIR of the old.
        let origins = HashMap::from([(
            key.clone(),
            BatchRequestOrigin {
                prior_sir: reviewed_identity.clone(),
                source_hash: source_hash.clone(),
            },
        )]);
        fs::write(workspace.join("src/lib.rs"), "fn late() { edited }\n").expect("edit source");
        let outcome = prepare_symbol(
            &pipeline,
            &store,
            &triage_pass_config(),
            "ignored",
            &provider,
            "gemini",
            &HashMap::new(),
            &origins,
            Some(&current_symbols),
        )
        .expect("prepare symbol");
        assert!(
            outcome.is_none(),
            "a result for an edited source is not applied"
        );
        assert_eq!(
            store
                .get_sir_meta("sym-late")
                .expect("load sir meta")
                .expect("sir meta exists")
                .generation_pass,
            "injected"
        );

        fs::write(workspace.join("src/lib.rs"), source).expect("restore source");

        // A result carrying a build id but no origin entry is refused, not ingested
        // unchecked.
        let err = match prepare_symbol(
            &pipeline,
            &store,
            &triage_pass_config(),
            "ignored",
            &provider,
            "gemini",
            &HashMap::new(),
            &HashMap::new(),
            Some(&current_symbols),
        ) {
            Err(err) => err,
            Ok(_) => panic!("a modern key without an origin must be refused"),
        };
        assert!(
            err.to_string()
                .contains("has no entry in its build's origin sidecar"),
            "unexpected error: {err:#}"
        );
        assert_eq!(
            store
                .get_sir_meta("sym-late")
                .expect("load sir meta")
                .expect("sir meta exists")
                .generation_pass,
            "injected"
        );

        // Built against the SIR and source the workspace still holds, the result is
        // applied.
        let outcome = prepare_symbol(
            &pipeline,
            &store,
            &triage_pass_config(),
            "ignored",
            &provider,
            "gemini",
            &HashMap::new(),
            &origins,
            Some(&current_symbols),
        )
        .expect("prepare symbol");
        assert!(outcome.is_some());
        let meta = store
            .get_sir_meta("sym-late")
            .expect("load sir meta")
            .expect("sir meta exists");
        assert_eq!(meta.sir_hash, aether_sir::sir_hash(&batch_sir));
        assert_eq!(meta.prompt_hash.as_deref(), Some("prompt-late"));
    }

    #[test]
    fn prepare_symbol_resumes_a_result_an_earlier_attempt_already_persisted() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let record = demo_symbol_record("sym-retry", "demo::retry");
        store.upsert_symbol(record.clone()).expect("upsert symbol");
        let pipeline = SirPipeline::new_embeddings_only(workspace.to_path_buf())
            .map(|pipeline| pipeline.with_skip_surreal_sync(true))
            .expect("build embeddings-only pipeline");
        let source = "fn retry() {}\n";
        fs::create_dir_all(workspace.join("src")).expect("create src");
        fs::write(workspace.join("src/lib.rs"), source).expect("write source");
        let symbol = Symbol {
            content_hash: aether_core::content_hash(source),
            range: SourceRange {
                start: Position { line: 1, column: 1 },
                end: Position {
                    line: 1,
                    column: source.trim_end().len() + 1,
                },
                start_byte: Some(0),
                end_byte: Some(source.len()),
            },
            ..symbol_from_record(&record).expect("build symbol")
        };
        let persist = |sir: &SirAnnotation, pass: &str| {
            pipeline
                .persist_sir_payload_into_sqlite(
                    &store,
                    &UpsertSirIntentPayload {
                        symbol: symbol.clone(),
                        sir: sir.clone(),
                        provider_name: "gemini".to_owned(),
                        model_name: "scan-model".to_owned(),
                        generation_pass: pass.to_owned(),
                        reasoning_trace: None,
                        commit_hash: None,
                        prompt_hash: None,
                        prior_sir: PriorSir::Unrecorded,
                    },
                    None,
                )
                .expect("persist payload");
        };
        persist(&demo_sir(), "scan");
        let built_against = current_sir_identity(&store, "sym-retry").expect("identity");
        let key = "sym-retry|prompt-retry|build-1".to_owned();
        let origins = HashMap::from([(
            key.clone(),
            BatchRequestOrigin {
                prior_sir: built_against,
                source_hash: symbol.content_hash.clone(),
            },
        )]);
        let current_symbols = HashMap::from([("sym-retry".to_owned(), symbol.clone())]);
        let batch_sir = SirAnnotation {
            intent: "Triage result".to_owned(),
            ..demo_sir()
        };
        let provider = StubBatchProvider {
            key: key.clone(),
            text: serde_json::to_string(&batch_sir).expect("serialize sir"),
            reasoning_trace: Some("triage reasoning".to_owned()),
        };
        let prepare = |origins: &HashMap<String, BatchRequestOrigin>| {
            prepare_symbol(
                &pipeline,
                &store,
                &triage_pass_config(),
                "ignored",
                &provider,
                "gemini",
                &HashMap::new(),
                origins,
                Some(&current_symbols),
            )
            .expect("prepare symbol")
        };

        // The first attempt persists the SIR (and, in the real flow, then fails in the
        // embedding or fingerprint phase, so the sidecars are kept for a retry).
        let first = prepare(&origins).expect("first attempt applies the result");
        assert!(!first.resumed);
        let written = current_sir_identity(&store, "sym-retry")
            .expect("identity")
            .expect("sir written");
        assert_eq!(written.sir_hash, aether_sir::sir_hash(&batch_sir));

        // The retry finds the store holding exactly this result's SIR: it is resumed
        // for the downstream work, not dropped as superseded, and the leaf is left as
        // it is (same history version and write generation).
        let retry = prepare(&origins).expect("retry resumes the result");
        assert!(retry.resumed, "the retry must resume, not rewrite");
        assert_eq!(retry.sir_hash, written.sir_hash);
        assert_eq!(retry.canonical_json, first.canonical_json);
        assert_eq!(
            current_sir_identity(&store, "sym-retry").expect("identity"),
            Some(written.clone()),
            "resuming must not rewrite the leaf"
        );
        let meta = store
            .get_sir_meta("sym-retry")
            .expect("load sir meta")
            .expect("sir meta exists");
        assert_eq!(meta.prompt_hash.as_deref(), Some("prompt-retry"));
        assert_eq!(meta.generation_pass, "triage");

        // The row itself records which request wrote it, in the leaf's transaction.
        let meta = store
            .get_sir_meta("sym-retry")
            .expect("load sir meta")
            .expect("sir meta exists");
        assert_eq!(meta.prompt_hash.as_deref(), Some("prompt-retry"));
        assert_eq!(meta.generation_pass, "triage");

        // An independent write of the very same content (an injection, say) is not
        // this result's own: it carries no batch provenance, so the result is superseded
        // and the injection keeps its provenance.
        persist(&batch_sir, "injected");
        let injected = current_sir_identity(&store, "sym-retry").expect("identity");
        assert_ne!(injected, Some(written.clone()));
        assert!(
            prepare(&origins).is_none(),
            "equal content does not identify the writer"
        );
        let meta = store
            .get_sir_meta("sym-retry")
            .expect("load sir meta")
            .expect("sir meta exists");
        assert_eq!(meta.generation_pass, "injected");
        assert_eq!(meta.prompt_hash, None);
        assert_eq!(
            current_sir_identity(&store, "sym-retry").expect("identity"),
            injected
        );

        // A different SIR written since is superseded as before.
        persist(
            &SirAnnotation {
                intent: "Reviewed by hand".to_owned(),
                ..demo_sir()
            },
            "injected",
        );
        let reviewed = current_sir_identity(&store, "sym-retry").expect("identity");
        assert!(
            prepare(&origins).is_none(),
            "a replaced SIR supersedes the result"
        );
        assert_eq!(
            current_sir_identity(&store, "sym-retry").expect("identity"),
            reviewed
        );
    }

    struct FixedEmbeddingProvider;

    #[async_trait]
    impl aether_infer::EmbeddingProvider for FixedEmbeddingProvider {
        async fn embed_text(&self, _text: &str) -> Result<Vec<f32>, aether_infer::InferError> {
            Ok(vec![1.0, 0.0])
        }

        async fn embed_text_with_purpose(
            &self,
            _text: &str,
            _purpose: EmbeddingPurpose,
        ) -> Result<Vec<f32>, aether_infer::InferError> {
            Ok(vec![1.0, 0.0])
        }

        async fn embed_texts_with_purpose(
            &self,
            texts: &[&str],
            _purpose: EmbeddingPurpose,
        ) -> Result<Vec<Vec<f32>>, aether_infer::InferError> {
            Ok(vec![vec![1.0, 0.0]; texts.len()])
        }
    }

    #[test]
    fn a_resumed_result_writes_its_fingerprint_row_once_against_its_true_predecessor() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);
        let config = aether_config::load_workspace_config(workspace).expect("load config");

        let store = SqliteStore::open(workspace).expect("open store");
        let record = demo_symbol_record("sym-fp", "demo::fp");
        store.upsert_symbol(record.clone()).expect("upsert symbol");
        let pipeline = SirPipeline::new_embeddings_only_with(
            workspace.to_path_buf(),
            std::sync::Arc::new(FixedEmbeddingProvider),
            "test_embedding".to_owned(),
            "test-model".to_owned(),
            None,
        )
        .map(|pipeline| pipeline.with_skip_surreal_sync(true))
        .expect("build embeddings-only pipeline");
        let source = "fn fp() {}\n";
        fs::create_dir_all(workspace.join("src")).expect("create src");
        fs::write(workspace.join("src/lib.rs"), source).expect("write source");
        let symbol = Symbol {
            content_hash: aether_core::content_hash(source),
            range: SourceRange {
                start: Position { line: 1, column: 1 },
                end: Position {
                    line: 1,
                    column: source.trim_end().len() + 1,
                },
                start_byte: Some(0),
                end_byte: Some(source.len()),
            },
            ..symbol_from_record(&record).expect("build symbol")
        };

        // The scan SIR the batch was built against, and the fingerprint row of the
        // prompt that produced it.
        pipeline
            .persist_sir_payload_into_sqlite(
                &store,
                &UpsertSirIntentPayload {
                    symbol: symbol.clone(),
                    sir: demo_sir(),
                    provider_name: "gemini".to_owned(),
                    model_name: "scan-model".to_owned(),
                    generation_pass: "scan".to_owned(),
                    reasoning_trace: None,
                    commit_hash: None,
                    prompt_hash: None,
                    prior_sir: PriorSir::Unrecorded,
                },
                None,
            )
            .expect("persist scan sir");
        write_fingerprint_row(
            &store,
            "sym-fp",
            "prompt-scan",
            None,
            "batch_scan",
            "scan-model",
            "scan",
            None,
        )
        .expect("scan fingerprint");
        let built_against = current_sir_identity(&store, "sym-fp").expect("identity");
        let key = "sym-fp|prompt-fp|build-1".to_owned();
        let origins = HashMap::from([(
            key.clone(),
            BatchRequestOrigin {
                prior_sir: built_against,
                source_hash: symbol.content_hash.clone(),
            },
        )]);
        let current_symbols = HashMap::from([("sym-fp".to_owned(), symbol.clone())]);
        let batch_sir = SirAnnotation {
            intent: "Triage result".to_owned(),
            ..demo_sir()
        };
        let provider = StubBatchProvider {
            key,
            text: serde_json::to_string(&batch_sir).expect("serialize sir"),
            reasoning_trace: None,
        };
        let chunk = |summary: &mut IngestSummary| {
            let mut buffer = Vec::new();
            process_chunk(
                &pipeline,
                &store,
                &triage_pass_config(),
                &config,
                &provider,
                "gemini",
                &HashMap::new(),
                &origins,
                Some(&current_symbols),
                &["ignored".to_owned()],
                summary,
                &mut buffer,
            )
            .expect("process chunk");
        };

        // The first attempt persists the SIR and its fingerprint row, then (say) fails
        // in the vector flush. The retry resumes: no second row.
        let mut summary = IngestSummary::default();
        chunk(&mut summary);
        assert_eq!(
            (summary.processed, summary.resumed, summary.fingerprint_rows),
            (1, 0, 1)
        );
        let mut summary = IngestSummary::default();
        chunk(&mut summary);
        assert_eq!(
            (summary.processed, summary.resumed, summary.fingerprint_rows),
            (1, 1, 0)
        );
        let history = store
            .list_sir_fingerprint_history("sym-fp")
            .expect("history");
        assert_eq!(history.len(), 2, "scan row plus one batch row: {history:?}");
        let batch_row = &history[1];
        assert_eq!(batch_row.prompt_hash, "prompt-fp");
        assert_eq!(
            batch_row.prompt_hash_previous.as_deref(),
            Some("prompt-scan")
        );

        // An attempt that persisted the SIR but failed before its fingerprint row leaves
        // no row; the resumed retry writes it against the last prompt the symbol was
        // fingerprinted for, not against this result's own write.
        let record2 = demo_symbol_record("sym-fp2", "demo::fp2");
        store.upsert_symbol(record2.clone()).expect("upsert symbol");
        let symbol2 = Symbol {
            content_hash: symbol.content_hash.clone(),
            range: symbol.range,
            ..symbol_from_record(&record2).expect("build symbol")
        };
        pipeline
            .persist_sir_payload_into_sqlite(
                &store,
                &UpsertSirIntentPayload {
                    symbol: symbol2.clone(),
                    sir: demo_sir(),
                    provider_name: "gemini".to_owned(),
                    model_name: "scan-model".to_owned(),
                    generation_pass: "scan".to_owned(),
                    reasoning_trace: None,
                    commit_hash: None,
                    prompt_hash: None,
                    prior_sir: PriorSir::Unrecorded,
                },
                None,
            )
            .expect("persist scan sir");
        write_fingerprint_row(
            &store,
            "sym-fp2",
            "prompt-scan-2",
            None,
            "batch_scan",
            "scan-model",
            "scan",
            None,
        )
        .expect("scan fingerprint");
        let key2 = "sym-fp2|prompt-fp2|build-1".to_owned();
        let origins2 = HashMap::from([(
            key2.clone(),
            BatchRequestOrigin {
                prior_sir: current_sir_identity(&store, "sym-fp2").expect("identity"),
                source_hash: symbol2.content_hash.clone(),
            },
        )]);
        let current_symbols2 = HashMap::from([("sym-fp2".to_owned(), symbol2.clone())]);
        let provider2 = StubBatchProvider {
            key: key2,
            text: serde_json::to_string(&batch_sir).expect("serialize sir"),
            reasoning_trace: None,
        };
        // Phase 1 alone: the SIR lands, no fingerprint row yet.
        prepare_symbol(
            &pipeline,
            &store,
            &triage_pass_config(),
            "ignored",
            &provider2,
            "gemini",
            &HashMap::new(),
            &origins2,
            Some(&current_symbols2),
        )
        .expect("prepare symbol")
        .expect("applied");
        let mut summary = IngestSummary::default();
        let mut buffer = Vec::new();
        process_chunk(
            &pipeline,
            &store,
            &triage_pass_config(),
            &config,
            &provider2,
            "gemini",
            &HashMap::new(),
            &origins2,
            Some(&current_symbols2),
            &["ignored".to_owned()],
            &mut summary,
            &mut buffer,
        )
        .expect("process chunk");
        assert_eq!((summary.resumed, summary.fingerprint_rows), (1, 1));
        let history = store
            .list_sir_fingerprint_history("sym-fp2")
            .expect("history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].prompt_hash, "prompt-fp2");
        assert_eq!(
            history[1].prompt_hash_previous.as_deref(),
            Some("prompt-scan-2"),
            "the predecessor is the last fingerprinted prompt, not this result's own"
        );
    }

    #[test]
    fn prepare_symbol_promotes_metadata_when_sir_hash_is_unchanged() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let record = demo_symbol_record("sym-batch", "demo::run");
        store.upsert_symbol(record.clone()).expect("upsert symbol");

        let pipeline = SirPipeline::new_embeddings_only(workspace.to_path_buf())
            .map(|pipeline| pipeline.with_skip_surreal_sync(true))
            .expect("build embeddings-only pipeline");
        let sir = demo_sir();
        let symbol = symbol_from_record(&record).expect("build symbol");

        pipeline
            .persist_sir_payload_into_sqlite(
                &store,
                &UpsertSirIntentPayload {
                    symbol: symbol.clone(),
                    sir: sir.clone(),
                    provider_name: "gemini".to_owned(),
                    model_name: "scan-model".to_owned(),
                    generation_pass: "scan".to_owned(),
                    reasoning_trace: None,
                    commit_hash: None,
                    prompt_hash: None,
                    prior_sir: PriorSir::Unrecorded,
                },
                None,
            )
            .expect("persist scan payload");

        let provider = StubBatchProvider {
            key: "sym-batch|prompt-123".to_owned(),
            text: serde_json::to_string(&sir).expect("serialize sir"),
            reasoning_trace: Some("triage reasoning".to_owned()),
        };
        prepare_symbol(
            &pipeline,
            &store,
            &triage_pass_config(),
            "ignored",
            &provider,
            "gemini",
            &HashMap::new(),
            &HashMap::new(),
            None,
        )
        .expect("prepare symbol")
        .expect("result applied");

        let meta = store
            .get_sir_meta("sym-batch")
            .expect("load sir meta")
            .expect("sir meta exists");
        assert_eq!(meta.sir_version, 1);
        assert_eq!(meta.generation_pass, "triage");
        assert_eq!(meta.prompt_hash.as_deref(), Some("prompt-123"));
        assert_eq!(meta.reasoning_trace.as_deref(), Some("triage reasoning"));
    }
}
