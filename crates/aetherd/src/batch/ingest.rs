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

use crate::batch::hash::diff_prompt_hashes;
use crate::batch::{BatchProvider, BatchResultLine, PassConfig};
use crate::continuous::cosine_distance_from_embeddings;
use crate::sir_pipeline::{EmbeddingInput, SirPipeline, UpsertSirIntentPayload};

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
            &line_chunk,
            &mut summary,
            &mut embedding_buffer,
        )?;
    }

    // Flush any remaining buffered embeddings to vector store.
    if !embedding_buffer.is_empty() {
        pipeline
            .flush_embedding_batch(embedding_buffer)
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
        ) {
            Ok(mut prep) => {
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

        write_fingerprint_row(
            store,
            &prep.symbol_id,
            &prep.prompt_hash,
            prep.previous_meta
                .as_ref()
                .and_then(|record| record.prompt_hash.as_deref()),
            format!("batch_{}", pass_config.pass.as_str()).as_str(),
            pass_config.model.as_str(),
            pass_config.pass.as_str(),
            cosine_distance_from_embeddings(
                prep.previous_embedding.as_ref(),
                current_embedding.as_ref(),
            ),
        )
        .with_context(|| format!("failed to write fingerprint row for {}", prep.symbol_id))?;

        summary.processed += 1;
        summary.fingerprint_rows += 1;
    }

    // Buffer new embedding records for vector store flush.
    for record in embedding_records {
        embedding_buffer.push(record);
    }

    // Flush to vector store if buffer exceeds threshold.
    if embedding_buffer.len() >= INGEST_VECTOR_BATCH_SIZE {
        let batch = std::mem::take(embedding_buffer);
        pipeline
            .flush_embedding_batch(batch)
            .context("failed to flush embedding batch during ingest")?;
    }

    Ok(())
}

/// Phase 1: Parse a single batch result line and persist SIR to SQLite.
///
/// Does everything the old `ingest_result_line` did except embedding generation,
/// contract verification, and fingerprint writing.
fn prepare_symbol(
    pipeline: &SirPipeline,
    store: &SqliteStore,
    pass_config: &PassConfig,
    raw_line: &str,
    provider: &dyn BatchProvider,
    provider_name: &str,
) -> Result<PreparedSymbol> {
    let (symbol_id, prompt_hash, sir_json) = match provider.parse_result_line(raw_line)? {
        BatchResultLine::Success { key, text } => {
            let (sid, phash) = parse_key(&key)?;
            (sid.to_owned(), phash.to_owned(), text)
        }
        BatchResultLine::Error { key, message } => {
            return Err(anyhow!("batch response error (key={:?}): {}", key, message));
        }
    };

    let sir = serde_json::from_str::<SirAnnotation>(sir_json.as_str())
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

    let provider_kind = provider_kind_from_name(provider_name);
    let payload = UpsertSirIntentPayload {
        symbol: symbol_from_record(&symbol_record)?,
        sir,
        provider_name: provider_kind.as_str().to_owned(),
        model_name: pass_config.model.clone(),
        generation_pass: pass_config.pass.as_str().to_owned(),
        commit_hash: None,
    };
    let (canonical_json, sir_hash_value) = pipeline
        .persist_sir_payload_into_sqlite(store, &payload)
        .with_context(|| format!("failed to persist SIR payload for {symbol_id}"))?;

    let current_meta = store
        .get_sir_meta(&symbol_id)
        .with_context(|| format!("failed to reload SIR metadata for {symbol_id}"))?
        .ok_or_else(|| anyhow!("missing persisted SIR metadata for {symbol_id}"))?;
    store
        .upsert_sir_meta(SirMetaRecord {
            prompt_hash: Some(prompt_hash.clone()),
            ..current_meta
        })
        .with_context(|| format!("failed to persist prompt_hash for {symbol_id}"))?;

    Ok(PreparedSymbol {
        symbol_id,
        prompt_hash,
        canonical_json,
        sir_hash: sir_hash_value,
        previous_meta,
        previous_embedding,
        embedding_slot: None,
    })
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

fn parse_key(key: &str) -> Result<(&str, &str)> {
    let (symbol_id, prompt_hash) = key
        .split_once('|')
        .ok_or_else(|| anyhow!("batch response key missing prompt hash delimiter"))?;
    let symbol_id = symbol_id.trim();
    let prompt_hash = prompt_hash.trim();
    if symbol_id.is_empty() || prompt_hash.is_empty() {
        return Err(anyhow!(
            "batch response key is missing symbol_id or prompt_hash"
        ));
    }
    Ok((symbol_id, prompt_hash))
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
