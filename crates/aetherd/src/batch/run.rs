use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aether_config::AetherConfig;
use aether_store::SqliteStore;
use anyhow::{Context, Result, anyhow};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::batch::build::{build_pass_jsonl, snapshot_workspace_symbols};
use crate::batch::extract::run_extract;
use crate::batch::ingest::ingest_results;
use crate::batch::{
    BatchPollStatus, BatchProvider, BatchRuntimeConfig, create_batch_provider,
    parse_batch_passes_csv, resolve_batch_runtime_config, resolve_build_pass_config,
};
use crate::cli::{BatchArgs, BatchBuildArgs, BatchCommand, BatchIngestArgs, BatchRunArgs};

// ---------------------------------------------------------------------------
// Retry constants for 429 / rate-limit errors during batch submission
// ---------------------------------------------------------------------------

const SUBMIT_MAX_RETRIES: usize = 5;
const SUBMIT_BACKOFF_BASE_SECS: u64 = 30;
const SUBMIT_BACKOFF_CAP_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

struct CompletedBatchJob {
    chunk_index: usize,
    result_paths: Vec<PathBuf>,
}

struct FailedBatchJob {
    chunk_index: usize,
    input_path: PathBuf,
    message: String,
}

struct BatchPollOutcome {
    completed: Vec<CompletedBatchJob>,
    failed: Vec<FailedBatchJob>,
}

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

pub fn run_batch_command(workspace: &Path, config: &AetherConfig, args: BatchArgs) -> Result<()> {
    match args.command {
        BatchCommand::Extract => run_extract_command(workspace),
        BatchCommand::Build(args) => run_build_command(workspace, config, &args),
        BatchCommand::Ingest(args) => run_ingest_command(workspace, config, &args),
        BatchCommand::Run(args) => run_full_batch_command(workspace, config, &args),
    }
}

fn run_extract_command(workspace: &Path) -> Result<()> {
    let summary = run_extract(workspace)?;
    println!("Extracted {} symbols", summary.symbol_count);
    Ok(())
}

fn run_build_command(workspace: &Path, config: &AetherConfig, args: &BatchBuildArgs) -> Result<()> {
    let batch_config = config.batch.clone().unwrap_or_default();
    let provider = create_batch_provider(&batch_config, args.provider.as_deref())
        .context("failed to create batch provider for build")?;
    let store = SqliteStore::open(workspace).context("failed to open store for batch build")?;
    let symbols_by_id = snapshot_workspace_symbols(workspace)?;
    let mut runtime = resolve_batch_runtime_config(workspace, config, None);
    if let Some(batch_dir) = args.batch_dir.as_deref() {
        runtime.batch_dir = normalize_batch_dir(workspace, batch_dir);
    }
    let pass_config = resolve_build_pass_config(&runtime, args);
    let contracts_enabled = config.contracts.as_ref().is_some_and(|c| c.enabled);
    let summary = build_pass_jsonl(
        workspace,
        &store,
        &runtime,
        &pass_config,
        &symbols_by_id,
        contracts_enabled,
        provider.as_ref(),
    )?;
    println!(
        "Built {} chunk(s), wrote {} request(s), skipped {}, unresolved {}",
        summary.files.len(),
        summary.written,
        summary.skipped,
        summary.unresolved_symbols
    );
    Ok(())
}

fn run_ingest_command(
    workspace: &Path,
    config: &AetherConfig,
    args: &BatchIngestArgs,
) -> Result<()> {
    let batch_config = config.batch.clone().unwrap_or_default();
    let provider = create_batch_provider(&batch_config, args.provider.as_deref())
        .context("failed to create batch provider for ingest")?;
    let store = SqliteStore::open(workspace).context("failed to open store for batch ingest")?;
    let runtime = resolve_batch_runtime_config(workspace, config, None);
    let mut pass_config = runtime.for_pass(args.pass).clone();
    if let Some(model) = args.model.as_ref() {
        pass_config.model = model.clone();
    }
    let summary = ingest_results(
        workspace,
        &store,
        &pass_config,
        args.results_jsonl.as_path(),
        config,
        provider.as_ref(),
        provider.name(),
    )?;
    println!(
        "Ingested {} result(s), skipped {}, wrote {} fingerprint row(s)",
        summary.processed, summary.skipped, summary.fingerprint_rows
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Full batch pipeline: extract -> build -> submit -> poll -> download -> ingest
// ---------------------------------------------------------------------------

fn run_full_batch_command(
    workspace: &Path,
    config: &AetherConfig,
    args: &BatchRunArgs,
) -> Result<()> {
    let batch_config = config.batch.clone().unwrap_or_default();
    let provider: Arc<dyn BatchProvider> = Arc::from(
        create_batch_provider(&batch_config, args.provider.as_deref())
            .context("failed to create batch provider")?,
    );

    let runtime = resolve_batch_runtime_config(workspace, config, Some(args));
    let passes = parse_batch_passes_csv(args.passes.as_str())?;

    let contracts_enabled = config.contracts.as_ref().is_some_and(|c| c.enabled);
    let extract_summary = run_extract(workspace)?;
    println!("Extracted {} symbols", extract_summary.symbol_count);

    // Create a tokio runtime for async provider calls (submit/poll/download).
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for batch submission")?;

    for pass in passes {
        let pass_config = runtime.for_pass(pass).clone();
        let build_summary = build_pass_jsonl(
            workspace,
            &extract_summary.store,
            &runtime,
            &pass_config,
            &extract_summary.symbols_by_id,
            contracts_enabled,
            provider.as_ref(),
        )?;
        println!(
            "Built {} chunk(s) for {}: wrote {}, skipped {}, unresolved {}",
            build_summary.files.len(),
            pass.as_str(),
            build_summary.written,
            build_summary.skipped,
            build_summary.unresolved_symbols
        );

        tracing::info!(
            max_concurrent_jobs = runtime.max_concurrent_jobs,
            total_chunks = build_summary.files.len(),
            pass = pass.as_str(),
            "starting batch submission"
        );

        let BatchPollOutcome {
            mut completed,
            mut failed,
        } = tokio_rt.block_on(process_all_chunks(
            provider.clone(),
            build_summary.files,
            pass_config.model.clone(),
            &runtime,
        ))?;

        completed.sort_by_key(|job| job.chunk_index);
        for job in completed {
            for result_path in job.result_paths {
                let ingest_summary = ingest_results(
                    workspace,
                    &extract_summary.store,
                    &pass_config,
                    result_path.as_path(),
                    config,
                    provider.as_ref(),
                    provider.name(),
                )?;
                println!(
                    "Ingested {} chunk {}: processed {}, skipped {}, fingerprint rows {}",
                    pass.as_str(),
                    job.chunk_index + 1,
                    ingest_summary.processed,
                    ingest_summary.skipped,
                    ingest_summary.fingerprint_rows
                );
            }
        }

        if !failed.is_empty() {
            failed.sort_by_key(|job| job.chunk_index);
            let failed_count = failed.len();
            let failure_summary = failed
                .into_iter()
                .map(|job| {
                    format!(
                        "chunk {} ({}): {}",
                        job.chunk_index + 1,
                        job.input_path.display(),
                        job.message
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(anyhow!(
                "{} batch chunk(s) failed during {}: {}",
                failed_count,
                pass.as_str(),
                failure_summary
            ));
        }
    }

    // Post-batch hook: run Seismograph analysis if enabled
    if let Some(ref seismo_config) = config.seismograph
        && seismo_config.enabled
    {
        tracing::info!("Running post-batch Seismograph analysis");
        match crate::seismograph::run_seismograph_analysis(workspace, config) {
            Ok(report) => {
                tracing::info!(
                    velocity = report.semantic_velocity,
                    shift = report.codebase_shift,
                    cascades = report.cascade_count,
                    "Seismograph analysis complete"
                );
            }
            Err(err) => {
                tracing::warn!("Seismograph analysis failed: {err:#}");
                // Non-fatal — batch still succeeded
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Concurrent chunk processing: submit -> poll -> download per chunk,
// gated by a semaphore to limit active provider-side jobs.
// ---------------------------------------------------------------------------

async fn process_all_chunks(
    provider: Arc<dyn BatchProvider>,
    input_files: Vec<PathBuf>,
    model: String,
    runtime: &BatchRuntimeConfig,
) -> Result<BatchPollOutcome> {
    let total_chunks = input_files.len();
    let semaphore = Arc::new(Semaphore::new(runtime.max_concurrent_jobs.max(1)));
    let poll_interval = Duration::from_secs(runtime.poll_interval_secs);
    let batch_dir = runtime.batch_dir.clone();
    let poll_secs = runtime.poll_interval_secs;
    let mut join_set = JoinSet::new();

    for (chunk_index, input_path) in input_files.into_iter().enumerate() {
        let provider = provider.clone();
        let semaphore = semaphore.clone();
        let model = model.clone();
        let batch_dir = batch_dir.clone();

        join_set.spawn(async move {
            // Acquire permit — blocks until a slot opens
            let permit = match semaphore.acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    return Err(FailedBatchJob {
                        chunk_index,
                        input_path,
                        message: "batch semaphore closed".to_owned(),
                    });
                }
            };

            tracing::info!(
                chunk = chunk_index + 1,
                total = total_chunks,
                "acquired concurrency permit, submitting chunk"
            );

            // Submit with retry on 429
            let job_ids = match submit_with_retry(
                provider.as_ref(),
                &input_path,
                &model,
                &batch_dir,
                poll_secs,
            )
            .await
            {
                Ok(ids) => ids,
                Err(err) => {
                    drop(permit);
                    return Err(FailedBatchJob {
                        chunk_index,
                        input_path,
                        message: format!("{err:#}"),
                    });
                }
            };

            tracing::info!(
                provider = provider.name(),
                chunk = chunk_index + 1,
                jobs = ?job_ids,
                "submitted batch chunk"
            );

            // Poll until complete or failed
            if let Err(msg) =
                poll_until_done(provider.as_ref(), &job_ids, chunk_index, poll_interval).await
            {
                drop(permit);
                return Err(FailedBatchJob {
                    chunk_index,
                    input_path,
                    message: msg,
                });
            }

            // Download results
            let result = match provider
                .download_results(&job_ids, &batch_dir)
                .await
                .with_context(|| format!("download failed for chunk {}", chunk_index + 1))
            {
                Ok(result_paths) => Ok(CompletedBatchJob {
                    chunk_index,
                    result_paths,
                }),
                Err(err) => Err(FailedBatchJob {
                    chunk_index,
                    input_path,
                    message: format!("{err:#}"),
                }),
            };

            // Release permit — next chunk can now start
            drop(permit);
            result
        });
    }

    // Collect all results
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    let mut panicked = 0usize;
    while let Some(join_result) = join_set.join_next().await {
        match join_result {
            Ok(Ok(job)) => completed.push(job),
            Ok(Err(job)) => failed.push(job),
            Err(join_err) => {
                tracing::error!(error = %join_err, "batch chunk task panicked");
                panicked += 1;
            }
        }
    }

    // Treat panics as hard failures — a panic indicates a programming bug
    // and must not silently reduce the processed chunk count.
    if panicked > 0 {
        return Err(anyhow!(
            "{panicked} batch chunk task(s) panicked — this is a bug, please report it"
        ));
    }

    tracing::info!(
        completed = completed.len(),
        failed = failed.len(),
        "all batch chunks processed"
    );

    Ok(BatchPollOutcome { completed, failed })
}

// ---------------------------------------------------------------------------
// Submit a single chunk with exponential backoff retry on 429 / rate-limit.
// ---------------------------------------------------------------------------

async fn submit_with_retry(
    provider: &dyn BatchProvider,
    input_path: &Path,
    model: &str,
    batch_dir: &Path,
    poll_interval_secs: u64,
) -> Result<Vec<String>> {
    for attempt in 0..=SUBMIT_MAX_RETRIES {
        match provider
            .submit(input_path, model, batch_dir, poll_interval_secs)
            .await
        {
            Ok(job_ids) => return Ok(job_ids),
            Err(err) => {
                let err_str = format!("{err:#}");
                let err_lower = err_str.to_ascii_lowercase();
                let is_rate_limit = err_str.contains("429")
                    || err_lower.contains("rate limit")
                    || err_lower.contains("resource_exhausted");

                if is_rate_limit && attempt < SUBMIT_MAX_RETRIES {
                    let backoff_secs =
                        (SUBMIT_BACKOFF_BASE_SECS << attempt).min(SUBMIT_BACKOFF_CAP_SECS);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries = SUBMIT_MAX_RETRIES,
                        backoff_secs,
                        error = %err,
                        "batch submit rate limited, retrying"
                    );
                    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                } else {
                    return Err(err).with_context(|| {
                        format!("batch submit failed for {}", input_path.display())
                    });
                }
            }
        }
    }
    unreachable!()
}

// ---------------------------------------------------------------------------
// Poll a single batch job until it completes or fails.
// ---------------------------------------------------------------------------

async fn poll_until_done(
    provider: &dyn BatchProvider,
    job_ids: &[String],
    chunk_index: usize,
    poll_interval: Duration,
) -> std::result::Result<(), String> {
    loop {
        tokio::time::sleep(poll_interval).await;

        let poll_status = match provider.poll(job_ids).await {
            Ok(status) => status,
            Err(err) => {
                return Err(format!("poll failed: {err:#}"));
            }
        };

        match poll_status {
            BatchPollStatus::Completed => {
                tracing::info!(
                    chunk = chunk_index + 1,
                    "batch chunk completed, downloading results"
                );
                return Ok(());
            }
            BatchPollStatus::Failed { message } => {
                tracing::error!(
                    chunk = chunk_index + 1,
                    error = %message,
                    "batch chunk failed"
                );
                return Err(message);
            }
            BatchPollStatus::InProgress { completed, total } => {
                if let (Some(completed_count), Some(total_count)) = (completed, total) {
                    tracing::info!(
                        chunk = chunk_index + 1,
                        completed = completed_count,
                        total = total_count,
                        "batch chunk in progress"
                    );
                } else {
                    tracing::info!(chunk = chunk_index + 1, "batch chunk in progress");
                }
            }
        }
    }
}

fn normalize_batch_dir(workspace: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}
