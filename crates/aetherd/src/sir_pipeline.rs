use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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

pub const DEFAULT_SIR_CONCURRENCY: usize = 2;

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

            let sir_blob_path = store.sir_dir().join(format!("{}.json", symbol.id));
            match fs::remove_file(&sir_blob_path) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("failed to remove SIR blob {}", sir_blob_path.display())
                    });
                }
            }
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
        ));

        for result in results {
            let generated = result?;
            let canonical_json = canonicalize_sir_json(&generated.sir);
            let sir_hash_value = sir_hash(&generated.sir);

            store
                .write_sir_blob(&generated.symbol.id, &canonical_json)
                .with_context(|| format!("failed to write SIR blob for {}", generated.symbol.id))?;

            store
                .upsert_sir_meta(SirMetaRecord {
                    id: generated.symbol.id.clone(),
                    sir_hash: sir_hash_value.clone(),
                    sir_version: 1,
                    provider: self.provider_name.clone(),
                    model: self.model_name.clone(),
                    updated_at: unix_timestamp_secs(),
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
) -> Vec<Result<GeneratedSir>> {
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut join_set = JoinSet::new();

    for job in jobs {
        let provider = provider.clone();
        let semaphore = semaphore.clone();

        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| anyhow!("inference semaphore closed"))?;

            let sir = provider
                .generate_sir(&job.symbol_text, &job.context)
                .await
                .with_context(|| {
                    format!(
                        "failed to generate SIR for symbol {}",
                        job.symbol.qualified_name
                    )
                })?;

            Ok::<GeneratedSir, anyhow::Error>(GeneratedSir {
                symbol: job.symbol,
                sir,
            })
        });
    }

    let mut results = Vec::new();
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok(result) => results.push(result),
            Err(err) => results.push(Err(anyhow!("inference task join error: {err}"))),
        }
    }

    results
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
