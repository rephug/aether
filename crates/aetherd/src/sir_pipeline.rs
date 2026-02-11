use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_core::{Position, SourceRange, Symbol, SymbolChangeEvent};
use aether_infer::{
    InferenceProvider, ProviderOverrides, SirContext, load_provider_from_env_or_mock,
};
use aether_sir::{SirAnnotation, canonicalize_sir_json, sir_hash};
use aether_store::{SirMetaRecord, SqliteStore, Store, SymbolRecord};
use anyhow::{Context, Result, anyhow};
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};

pub const DEFAULT_SIR_CONCURRENCY: usize = 2;
const SIR_STATUS_FRESH: &str = "fresh";
const SIR_STATUS_STALE: &str = "stale";
const INFERENCE_MAX_RETRIES: usize = 2;
const INFERENCE_ATTEMPT_TIMEOUT_SECS: u64 = 30;
const INFERENCE_BACKOFF_BASE_MS: u64 = 200;
const INFERENCE_BACKOFF_MAX_MS: u64 = 2_000;

pub struct SirPipeline {
    workspace_root: PathBuf,
    provider: Arc<dyn InferenceProvider>,
    provider_name: String,
    model_name: String,
    runtime: Runtime,
    sir_concurrency: usize,
}

impl SirPipeline {
    pub fn new(
        workspace_root: PathBuf,
        sir_concurrency: usize,
        provider_overrides: ProviderOverrides,
    ) -> Result<Self> {
        let loaded = load_provider_from_env_or_mock(&workspace_root, provider_overrides)
            .context("failed to load inference provider")?;
        let provider = Arc::<dyn InferenceProvider>::from(loaded.provider);

        Self::new_with_provider(
            workspace_root,
            sir_concurrency,
            provider,
            loaded.provider_name,
            loaded.model_name,
        )
    }

    pub fn new_with_provider(
        workspace_root: PathBuf,
        sir_concurrency: usize,
        provider: Arc<dyn InferenceProvider>,
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
    ) -> Result<Self> {
        let concurrency = sir_concurrency.max(1);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(concurrency)
            .enable_all()
            .build()
            .context("failed to build SIR async runtime")?;

        Ok(Self {
            workspace_root,
            provider,
            provider_name: provider_name.into(),
            model_name: model_name.into(),
            runtime,
            sir_concurrency: concurrency,
        })
    }

    pub fn process_event(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        for symbol in &event.removed {
            store
                .mark_removed(&symbol.id)
                .with_context(|| format!("failed to mark symbol removed: {}", symbol.id))?;
        }

        let changed_symbols: Vec<Symbol> = event
            .added
            .iter()
            .chain(event.updated.iter())
            .cloned()
            .collect();

        if changed_symbols.is_empty() {
            return Ok(());
        }

        let now_ts = unix_timestamp_secs();

        for symbol in &changed_symbols {
            store
                .upsert_symbol(to_symbol_record(symbol, now_ts))
                .with_context(|| format!("failed to upsert symbol {}", symbol.id))?;
        }

        let jobs = changed_symbols
            .into_iter()
            .map(|symbol| build_job(&self.workspace_root, symbol))
            .collect::<Result<Vec<_>>>()?;

        let results = self.runtime.block_on(generate_sir_jobs(
            self.provider.clone(),
            jobs,
            self.sir_concurrency,
        ))?;

        for result in results {
            match result {
                SirGenerationOutcome::Success(generated) => {
                    let canonical_json = canonicalize_sir_json(&generated.sir);
                    let sir_hash_value = sir_hash(&generated.sir);
                    let updated_at = unix_timestamp_secs();

                    store
                        .write_sir_blob(&generated.symbol.id, &canonical_json)
                        .with_context(|| {
                            format!("failed to write SIR blob for {}", generated.symbol.id)
                        })?;

                    store
                        .upsert_sir_meta(SirMetaRecord {
                            id: generated.symbol.id.clone(),
                            sir_hash: sir_hash_value.clone(),
                            sir_version: 1,
                            provider: self.provider_name.clone(),
                            model: self.model_name.clone(),
                            updated_at,
                            sir_status: SIR_STATUS_FRESH.to_owned(),
                            last_error: None,
                            last_attempt_at: updated_at,
                        })
                        .with_context(|| {
                            format!("failed to upsert SIR metadata for {}", generated.symbol.id)
                        })?;

                    if print_sir {
                        writeln!(
                            out,
                            "SIR_STORED symbol_id={} sir_hash={} provider={}",
                            generated.symbol.id, sir_hash_value, self.provider_name
                        )
                        .context("failed to write SIR print line")?;
                    }
                }
                SirGenerationOutcome::Failure(failed) => {
                    let last_attempt_at = unix_timestamp_secs();
                    let previous_meta =
                        store.get_sir_meta(&failed.symbol.id).with_context(|| {
                            format!("failed to load SIR metadata for {}", failed.symbol.id)
                        })?;

                    let stale_meta = previous_meta.map_or_else(
                        || SirMetaRecord {
                            id: failed.symbol.id.clone(),
                            sir_hash: String::new(),
                            sir_version: 1,
                            provider: self.provider_name.clone(),
                            model: self.model_name.clone(),
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
                            updated_at: record.updated_at,
                            sir_status: SIR_STATUS_STALE.to_owned(),
                            last_error: Some(failed.error_message.clone()),
                            last_attempt_at,
                        },
                    );

                    store.upsert_sir_meta(stale_meta).with_context(|| {
                        format!(
                            "failed to store stale SIR metadata for {}",
                            failed.symbol.id
                        )
                    })?;

                    if print_sir {
                        writeln!(
                            out,
                            "SIR_STALE symbol_id={} error={}",
                            failed.symbol.id,
                            flatten_error_line(&failed.error_message)
                        )
                        .context("failed to write stale SIR print line")?;
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
struct SirJob {
    symbol: Symbol,
    symbol_text: String,
    context: SirContext,
}

#[derive(Debug)]
struct GeneratedSir {
    symbol: Symbol,
    sir: SirAnnotation,
}

#[derive(Debug)]
struct FailedSirGeneration {
    symbol: Symbol,
    error_message: String,
}

#[derive(Debug)]
enum SirGenerationOutcome {
    Success(GeneratedSir),
    Failure(FailedSirGeneration),
}

fn to_symbol_record(symbol: &Symbol, now_ts: i64) -> SymbolRecord {
    SymbolRecord {
        id: symbol.id.clone(),
        file_path: symbol.file_path.clone(),
        language: symbol.language.as_str().to_owned(),
        kind: symbol.kind.as_str().to_owned(),
        qualified_name: symbol.qualified_name.clone(),
        signature_fingerprint: symbol.signature_fingerprint.clone(),
        last_seen_at: now_ts,
    }
}

fn build_job(workspace_root: &Path, symbol: Symbol) -> Result<SirJob> {
    let full_path = workspace_root.join(&symbol.file_path);
    let source = fs::read_to_string(&full_path)
        .with_context(|| format!("failed to read symbol source file {}", full_path.display()))?;

    let symbol_text = extract_symbol_source_text(&source, symbol.range)
        .filter(|text| !text.trim().is_empty())
        .unwrap_or(source);

    let context = SirContext {
        language: symbol.language.as_str().to_owned(),
        file_path: symbol.file_path.clone(),
        qualified_name: symbol.qualified_name.clone(),
    };

    Ok(SirJob {
        symbol,
        symbol_text,
        context,
    })
}

fn extract_symbol_source_text(source: &str, range: SourceRange) -> Option<String> {
    let start = byte_offset_for_position(source, range.start)?;
    let end = byte_offset_for_position(source, range.end)?;

    if start > end || end > source.len() {
        return None;
    }

    source.get(start..end).map(|slice| slice.to_owned())
}

fn byte_offset_for_position(source: &str, position: Position) -> Option<usize> {
    let mut line = 1usize;
    let mut column = 1usize;

    if position.line == 1 && position.column == 1 {
        return Some(0);
    }

    for (index, ch) in source.char_indices() {
        if line == position.line && column == position.column {
            return Some(index);
        }

        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += ch.len_utf8();
        }
    }

    if line == position.line && column == position.column {
        Some(source.len())
    } else {
        None
    }
}

async fn generate_sir_jobs(
    provider: Arc<dyn InferenceProvider>,
    jobs: Vec<SirJob>,
    concurrency: usize,
) -> Result<Vec<SirGenerationOutcome>> {
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut join_set = JoinSet::new();

    for job in jobs {
        let provider = provider.clone();
        let semaphore = semaphore.clone();

        join_set.spawn(async move {
            let SirJob {
                symbol,
                symbol_text,
                context,
            } = job;
            let qualified_name = symbol.qualified_name.clone();

            let permit = semaphore.acquire_owned().await;
            let _permit = match permit {
                Ok(permit) => permit,
                Err(_) => {
                    return SirGenerationOutcome::Failure(FailedSirGeneration {
                        symbol,
                        error_message: "inference semaphore closed".to_owned(),
                    });
                }
            };

            let generated = generate_sir_with_retries(provider, symbol_text, context)
                .await
                .with_context(|| format!("failed to generate SIR for symbol {qualified_name}"));

            match generated {
                Ok(sir) => SirGenerationOutcome::Success(GeneratedSir { symbol, sir }),
                Err(err) => SirGenerationOutcome::Failure(FailedSirGeneration {
                    symbol,
                    error_message: format!("{err:#}"),
                }),
            }
        });
    }

    let mut results = Vec::new();
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok(result) => results.push(result),
            Err(err) => return Err(anyhow!("inference task join error: {err}")),
        }
    }

    Ok(results)
}

async fn generate_sir_with_retries(
    provider: Arc<dyn InferenceProvider>,
    symbol_text: String,
    context: SirContext,
) -> Result<SirAnnotation> {
    let total_attempts = INFERENCE_MAX_RETRIES + 1;
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..total_attempts {
        let timeout_result = timeout(
            Duration::from_secs(INFERENCE_ATTEMPT_TIMEOUT_SECS),
            provider.generate_sir(&symbol_text, &context),
        )
        .await;

        match timeout_result {
            Ok(Ok(sir)) => return Ok(sir),
            Ok(Err(err)) => {
                last_error = Some(anyhow::Error::new(err).context(format!(
                    "attempt {}/{} failed",
                    attempt + 1,
                    total_attempts
                )));
            }
            Err(_) => {
                last_error = Some(anyhow!(
                    "attempt {}/{} timed out after {}s",
                    attempt + 1,
                    total_attempts,
                    INFERENCE_ATTEMPT_TIMEOUT_SECS
                ));
            }
        }

        if attempt + 1 < total_attempts {
            let backoff_ms = (INFERENCE_BACKOFF_BASE_MS << attempt).min(INFERENCE_BACKOFF_MAX_MS);
            sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("inference failed without an error message")))
}

fn flatten_error_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).to_owned()
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
