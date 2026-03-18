use std::io::{BufRead, BufReader};
use std::path::Path;

use aether_config::{AetherConfig, InferenceProviderKind};
use aether_core::{Language, Position, SourceRange, Symbol, SymbolKind};
use aether_infer::ProviderOverrides;
use aether_sir::SirAnnotation;
use aether_store::{SirFingerprintHistoryRecord, SirMetaRecord, SirStateStore, SqliteStore};
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

use crate::batch::PassConfig;
use crate::batch::hash::diff_prompt_hashes;
use crate::continuous::cosine_distance_from_embeddings;
use crate::sir_pipeline::{SirPipeline, UpsertSirIntentPayload};

#[derive(Debug, Clone, Default)]
pub(crate) struct IngestSummary {
    pub processed: usize,
    pub skipped: usize,
    pub fingerprint_rows: usize,
}

#[derive(Debug, Deserialize)]
struct BatchResponseLine {
    key: String,
    #[serde(default)]
    response: Option<BatchResponse>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct BatchResponse {
    #[serde(default)]
    candidates: Vec<BatchCandidate>,
}

#[derive(Debug, Deserialize)]
struct BatchCandidate {
    content: BatchContent,
}

#[derive(Debug, Deserialize)]
struct BatchContent {
    #[serde(default)]
    parts: Vec<BatchPart>,
}

#[derive(Debug, Deserialize)]
struct BatchPart {
    #[serde(default)]
    text: Option<String>,
}

pub(crate) fn ingest_results(
    workspace: &Path,
    store: &SqliteStore,
    pass_config: &PassConfig,
    results_path: &Path,
    config: &AetherConfig,
) -> Result<IngestSummary> {
    let file = std::fs::File::open(results_path)
        .with_context(|| format!("failed to open batch results {}", results_path.display()))?;
    let reader = BufReader::new(file);
    let pipeline = SirPipeline::new(
        workspace.to_path_buf(),
        1,
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Gemini),
            model: Some(pass_config.model.clone()),
            endpoint: None,
            api_key_env: None,
        },
    )
    .map(|pipeline| pipeline.with_skip_surreal_sync(true))
    .context("failed to initialize batch ingest pipeline")?;

    let mut summary = IngestSummary::default();
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

        match ingest_result_line(&pipeline, store, pass_config, line.as_str(), config) {
            Ok(wrote_fingerprint) => {
                summary.processed += 1;
                if wrote_fingerprint {
                    summary.fingerprint_rows += 1;
                }
            }
            Err(err) => {
                summary.skipped += 1;
                tracing::warn!(
                    line_number = line_number + 1,
                    error = %err,
                    "skipping invalid batch result line"
                );
            }
        }
    }

    Ok(summary)
}

fn ingest_result_line(
    pipeline: &SirPipeline,
    store: &SqliteStore,
    pass_config: &PassConfig,
    raw_line: &str,
    config: &AetherConfig,
) -> Result<bool> {
    let line: BatchResponseLine =
        serde_json::from_str(raw_line).context("failed to parse batch response JSONL line")?;
    if line.error.is_some() {
        return Err(anyhow!("batch response line contains an error envelope"));
    }

    let (symbol_id, prompt_hash) = parse_key(line.key.as_str())?;
    let sir_json = extract_response_text(&line)?;
    let sir = serde_json::from_str::<SirAnnotation>(sir_json.as_str())
        .context("failed to parse SIR JSON from batch response")?;
    let symbol_record = store
        .get_symbol_record(symbol_id)
        .with_context(|| format!("failed to load symbol record for {symbol_id}"))?
        .ok_or_else(|| anyhow!("symbol '{symbol_id}' not found in symbols table"))?;
    let previous_meta = store
        .get_sir_meta(symbol_id)
        .with_context(|| format!("failed to read previous SIR metadata for {symbol_id}"))?;
    let previous_embedding = pipeline
        .load_symbol_embedding(symbol_id)
        .with_context(|| format!("failed to read previous embedding vector for {symbol_id}"))?;

    let payload = UpsertSirIntentPayload {
        symbol: symbol_from_record(&symbol_record)?,
        sir,
        provider_name: InferenceProviderKind::Gemini.as_str().to_owned(),
        model_name: pass_config.model.clone(),
        generation_pass: pass_config.pass.as_str().to_owned(),
        commit_hash: None,
    };
    let (canonical_json, sir_hash_value) = pipeline
        .persist_sir_payload_into_sqlite(store, &payload)
        .with_context(|| format!("failed to persist SIR payload for {symbol_id}"))?;

    let current_meta = store
        .get_sir_meta(symbol_id)
        .with_context(|| format!("failed to reload SIR metadata for {symbol_id}"))?
        .ok_or_else(|| anyhow!("missing persisted SIR metadata for {symbol_id}"))?;
    store
        .upsert_sir_meta(SirMetaRecord {
            prompt_hash: Some(prompt_hash.to_owned()),
            ..current_meta
        })
        .with_context(|| format!("failed to persist prompt_hash for {symbol_id}"))?;

    pipeline
        .refresh_embedding_if_needed(
            symbol_id,
            sir_hash_value.as_str(),
            canonical_json.as_str(),
            false,
            &mut std::io::sink(),
            None,
        )
        .with_context(|| format!("failed to refresh embedding for {symbol_id}"))?;
    let current_embedding = pipeline
        .load_symbol_embedding(symbol_id)
        .with_context(|| format!("failed to read refreshed embedding for {symbol_id}"))?;

    // Contract verification (non-fatal)
    if let Some(ref contracts_config) = config.contracts
        && contracts_config.enabled
        && let Err(err) = crate::contracts::verify_symbol_contracts(
            store,
            symbol_id,
            canonical_json.as_str(),
            current_embedding.as_ref().map(|e| e.embedding.as_slice()),
            config,
            pipeline.workspace_root(),
        )
    {
        tracing::warn!(
            symbol_id = symbol_id,
            error = %err,
            "Contract verification failed during batch ingest"
        );
    }

    write_fingerprint_row(
        store,
        symbol_id,
        prompt_hash,
        previous_meta
            .as_ref()
            .and_then(|record| record.prompt_hash.as_deref()),
        format!("batch_{}", pass_config.pass.as_str()).as_str(),
        pass_config.model.as_str(),
        pass_config.pass.as_str(),
        cosine_distance_from_embeddings(previous_embedding.as_ref(), current_embedding.as_ref()),
    )
    .with_context(|| format!("failed to write fingerprint row for {symbol_id}"))?;

    Ok(true)
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

fn extract_response_text(line: &BatchResponseLine) -> Result<String> {
    let response = line
        .response
        .as_ref()
        .ok_or_else(|| anyhow!("batch response line missing 'response'"))?;
    let text = response
        .candidates
        .first()
        .and_then(|candidate| candidate.content.parts.first())
        .and_then(|part| part.text.as_ref())
        .map(|text| text.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("batch response line missing candidate text"))?;
    Ok(text.to_owned())
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
