use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_core::{GitContext, Position, SourceRange, Symbol, SymbolChangeEvent};
use aether_infer::{
    EmbeddingProvider, EmbeddingProviderOverrides, InferenceProvider, ProviderOverrides,
    SirContext, load_embedding_provider_from_config, load_provider_from_env_or_mock,
};
use aether_parse::SymbolExtractor;
use aether_sir::{SirAnnotation, canonicalize_sir_json, sir_hash};
use aether_store::{
    SirMetaRecord, SqliteStore, Store, SymbolEmbeddingRecord, SymbolRecord, VectorStore,
    open_graph_store, open_vector_store,
};
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
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    embedding_provider_name: Option<String>,
    embedding_model_name: Option<String>,
    vector_store: Arc<dyn VectorStore>,
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
        let loaded_embedding = load_embedding_provider_from_config(
            &workspace_root,
            EmbeddingProviderOverrides::default(),
        )
        .context("failed to load embedding provider")?;
        let (embedding_provider, embedding_identity) =
            loaded_embedding.map_or((None, None), |loaded| {
                (
                    Some(Arc::<dyn EmbeddingProvider>::from(loaded.provider)),
                    Some((loaded.provider_name, loaded.model_name)),
                )
            });

        Self::new_with_provider_and_embeddings(
            workspace_root,
            sir_concurrency,
            provider,
            loaded.provider_name,
            loaded.model_name,
            embedding_provider,
            embedding_identity,
        )
    }

    pub fn new_with_provider(
        workspace_root: PathBuf,
        sir_concurrency: usize,
        provider: Arc<dyn InferenceProvider>,
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
    ) -> Result<Self> {
        Self::new_with_provider_and_embeddings(
            workspace_root,
            sir_concurrency,
            provider,
            provider_name,
            model_name,
            None,
            None,
        )
    }

    fn new_with_provider_and_embeddings(
        workspace_root: PathBuf,
        sir_concurrency: usize,
        provider: Arc<dyn InferenceProvider>,
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
        embedding_identity: Option<(String, String)>,
    ) -> Result<Self> {
        let concurrency = sir_concurrency.max(1);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(concurrency)
            .enable_all()
            .build()
            .context("failed to build SIR async runtime")?;
        let (embedding_provider_name, embedding_model_name) = embedding_identity
            .map_or((None, None), |identity| {
                (Some(identity.0), Some(identity.1))
            });
        let vector_store = runtime
            .block_on(open_vector_store(&workspace_root))
            .context("failed to initialize vector store")?;

        Ok(Self {
            workspace_root,
            provider,
            provider_name: provider_name.into(),
            model_name: model_name.into(),
            embedding_provider,
            embedding_provider_name,
            embedding_model_name,
            vector_store,
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
            self.runtime
                .block_on(self.vector_store.delete_embedding(&symbol.id))
                .with_context(|| format!("failed to remove vector embedding for {}", symbol.id))?;
        }

        self.replace_edges_for_file(store, event)?;

        let changed_symbols: Vec<Symbol> = event
            .added
            .iter()
            .chain(event.updated.iter())
            .cloned()
            .collect();

        if changed_symbols.is_empty() {
            return Ok(());
        }

        let commit_hash = resolve_workspace_head_commit(&self.workspace_root);
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
                    let attempted_at = unix_timestamp_secs();
                    let version_write = store
                        .record_sir_version_if_changed(
                            &generated.symbol.id,
                            &sir_hash_value,
                            &self.provider_name,
                            &self.model_name,
                            &canonical_json,
                            attempted_at,
                            commit_hash.as_deref(),
                        )
                        .with_context(|| {
                            format!("failed to record SIR history for {}", generated.symbol.id)
                        })?;

                    if version_write.changed {
                        store
                            .write_sir_blob(&generated.symbol.id, &canonical_json)
                            .with_context(|| {
                                format!("failed to write SIR blob for {}", generated.symbol.id)
                            })?;
                    }

                    store
                        .upsert_sir_meta(SirMetaRecord {
                            id: generated.symbol.id.clone(),
                            sir_hash: sir_hash_value.clone(),
                            sir_version: version_write.version,
                            provider: self.provider_name.clone(),
                            model: self.model_name.clone(),
                            updated_at: version_write.updated_at,
                            sir_status: SIR_STATUS_FRESH.to_owned(),
                            last_error: None,
                            last_attempt_at: attempted_at,
                        })
                        .with_context(|| {
                            format!("failed to upsert SIR metadata for {}", generated.symbol.id)
                        })?;

                    if let Err(err) = self.refresh_embedding_if_needed(
                        &generated.symbol.id,
                        &sir_hash_value,
                        &canonical_json,
                        print_sir,
                        out,
                    ) {
                        tracing::warn!(
                            symbol_id = %generated.symbol.id,
                            error = %err,
                            "embedding refresh error"
                        );
                    }

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

    fn replace_edges_for_file(&self, store: &SqliteStore, event: &SymbolChangeEvent) -> Result<()> {
        store
            .delete_edges_for_file(&event.file_path)
            .with_context(|| format!("failed to delete edges for file {}", event.file_path))?;

        let full_path = self.workspace_root.join(&event.file_path);
        let source = match fs::read_to_string(&full_path) {
            Ok(source) => Some(source),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "failed to read source for edge extraction {}",
                        full_path.display()
                    )
                });
            }
        };

        if let Some(source) = source {
            let mut extractor = SymbolExtractor::new().context("failed to initialize parser")?;
            let extracted = extractor
                .extract_with_edges_from_source(event.language, &event.file_path, &source)
                .with_context(|| format!("failed to extract edges from {}", event.file_path))?;

            store
                .upsert_edges(&extracted.edges)
                .with_context(|| format!("failed to upsert edges for file {}", event.file_path))?;
        }

        self.sync_graph_for_file(store, &event.file_path)?;

        Ok(())
    }

    fn sync_graph_for_file(&self, store: &SqliteStore, file_path: &str) -> Result<()> {
        let graph_store = open_graph_store(&self.workspace_root)
            .context("failed to open configured graph store")?;
        let stats = store
            .sync_graph_for_file(graph_store.as_ref(), file_path)
            .with_context(|| format!("failed to sync graph edges for file {file_path}"))?;

        if stats.unresolved_edges > 0 {
            tracing::debug!(
                file_path = %file_path,
                resolved_edges = stats.resolved_edges,
                unresolved_edges = stats.unresolved_edges,
                "graph sync skipped unresolved call edges"
            );
        }

        Ok(())
    }

    fn refresh_embedding_if_needed(
        &self,
        symbol_id: &str,
        sir_hash_value: &str,
        canonical_json: &str,
        print_sir: bool,
        out: &mut dyn Write,
    ) -> Result<()> {
        let Some(embedding_provider) = self.embedding_provider.as_ref() else {
            return Ok(());
        };

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

        let existing_meta = self
            .runtime
            .block_on(self.vector_store.get_embedding_meta(symbol_id))
            .with_context(|| format!("failed to read embedding metadata for {symbol_id}"))?;
        if let Some(existing_meta) = existing_meta
            && existing_meta.sir_hash == sir_hash_value
            && existing_meta.provider == provider_name
            && existing_meta.model == model_name
        {
            return Ok(());
        }

        let embedding = self
            .runtime
            .block_on(embedding_provider.embed_text(canonical_json))
            .with_context(|| format!("failed to generate embedding for {symbol_id}"))?;

        if embedding.is_empty() {
            return Ok(());
        }

        let updated_at = unix_timestamp_secs();
        self.runtime
            .block_on(self.vector_store.upsert_embedding(SymbolEmbeddingRecord {
                symbol_id: symbol_id.to_owned(),
                sir_hash: sir_hash_value.to_owned(),
                provider: provider_name.to_owned(),
                model: model_name.to_owned(),
                embedding,
                updated_at,
            }))
            .with_context(|| format!("failed to store embedding for {symbol_id}"))?;

        if print_sir {
            writeln!(
                out,
                "EMBEDDING_STORED symbol_id={symbol_id} provider={provider_name} model={model_name}"
            )
            .context("failed to write embedding print line")?;
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

fn resolve_workspace_head_commit(workspace_root: &Path) -> Option<String> {
    GitContext::open(workspace_root).and_then(|context| context.head_commit_hash())
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
