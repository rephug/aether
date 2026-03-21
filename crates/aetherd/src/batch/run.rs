use std::path::{Path, PathBuf};

use aether_config::AetherConfig;
use aether_store::SqliteStore;
use anyhow::{Context, Result, anyhow};

use crate::batch::build::{build_pass_jsonl, snapshot_workspace_symbols};
use crate::batch::extract::run_extract;
use crate::batch::ingest::ingest_results;
use crate::batch::{
    BatchPollStatus, BatchProvider, BatchRuntimeConfig, create_batch_provider,
    parse_batch_passes_csv, resolve_batch_runtime_config, resolve_build_pass_config,
};
use crate::cli::{BatchArgs, BatchBuildArgs, BatchCommand, BatchIngestArgs, BatchRunArgs};

/// Tracks a submitted batch chunk awaiting completion.
struct PendingBatchJob {
    /// Job IDs returned by the provider's submit call.
    job_ids: Vec<String>,
    /// Path to the JSONL input file for error reporting.
    input_path: PathBuf,
    /// Index in the original chunk list.
    chunk_index: usize,
}

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

fn run_full_batch_command(
    workspace: &Path,
    config: &AetherConfig,
    args: &BatchRunArgs,
) -> Result<()> {
    let batch_config = config.batch.clone().unwrap_or_default();
    let provider = create_batch_provider(&batch_config, args.provider.as_deref())
        .context("failed to create batch provider")?;

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

        let pending = tokio_rt.block_on(submit_all_chunks(
            provider.as_ref(),
            build_summary.files,
            &pass_config.model,
            &runtime,
        ))?;

        let BatchPollOutcome {
            mut completed,
            mut failed,
        } = tokio_rt.block_on(poll_and_download_all(provider.as_ref(), pending, &runtime))?;

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

/// Submit all batch input files without waiting for completion.
async fn submit_all_chunks(
    provider: &dyn BatchProvider,
    input_files: Vec<PathBuf>,
    model: &str,
    runtime: &BatchRuntimeConfig,
) -> Result<Vec<PendingBatchJob>> {
    let mut pending = Vec::with_capacity(input_files.len());

    for (chunk_index, input_path) in input_files.into_iter().enumerate() {
        let job_ids = provider
            .submit(
                &input_path,
                model,
                &runtime.batch_dir,
                runtime.poll_interval_secs,
            )
            .await
            .with_context(|| format!("batch submit failed for chunk {}", input_path.display()))?;

        tracing::info!(
            provider = provider.name(),
            chunk = chunk_index + 1,
            jobs = ?job_ids,
            "submitted batch chunk"
        );

        pending.push(PendingBatchJob {
            job_ids,
            input_path,
            chunk_index,
        });
    }

    tracing::info!(
        total_chunks = pending.len(),
        "all batch chunks submitted, polling for completion"
    );

    Ok(pending)
}

/// Poll all submitted chunks and download results as they complete.
async fn poll_and_download_all(
    provider: &dyn BatchProvider,
    mut pending: Vec<PendingBatchJob>,
    runtime: &BatchRuntimeConfig,
) -> Result<BatchPollOutcome> {
    let mut completed = Vec::new();
    let mut failed = Vec::new();

    while !pending.is_empty() {
        tokio::time::sleep(std::time::Duration::from_secs(runtime.poll_interval_secs)).await;

        let mut still_pending = Vec::with_capacity(pending.len());
        for job in pending {
            match provider.poll(&job.job_ids).await.with_context(|| {
                format!("failed to poll batch chunk {}", job.input_path.display())
            })? {
                BatchPollStatus::Completed => {
                    tracing::info!(
                        chunk = job.chunk_index + 1,
                        "batch chunk completed, downloading results"
                    );
                    match provider
                        .download_results(&job.job_ids, &runtime.batch_dir)
                        .await
                        .with_context(|| {
                            format!(
                                "failed to download results for chunk {}",
                                job.input_path.display()
                            )
                        }) {
                        Ok(result_paths) => {
                            completed.push(CompletedBatchJob {
                                chunk_index: job.chunk_index,
                                result_paths,
                            });
                        }
                        Err(err) => {
                            tracing::error!(
                                chunk = job.chunk_index + 1,
                                error = %err,
                                "batch chunk download failed"
                            );
                            failed.push(FailedBatchJob {
                                chunk_index: job.chunk_index,
                                input_path: job.input_path,
                                message: format!("{err:#}"),
                            });
                        }
                    }
                }
                BatchPollStatus::Failed { message } => {
                    tracing::error!(
                        chunk = job.chunk_index + 1,
                        error = %message,
                        "batch chunk failed"
                    );
                    failed.push(FailedBatchJob {
                        chunk_index: job.chunk_index,
                        input_path: job.input_path,
                        message,
                    });
                }
                BatchPollStatus::InProgress {
                    completed: done,
                    total,
                } => {
                    if let (Some(completed_count), Some(total_count)) = (done, total) {
                        tracing::info!(
                            chunk = job.chunk_index + 1,
                            completed = completed_count,
                            total = total_count,
                            "batch chunk in progress"
                        );
                    } else {
                        tracing::info!(chunk = job.chunk_index + 1, "batch chunk in progress");
                    }
                    still_pending.push(job);
                }
            }
        }

        pending = still_pending;
        if !pending.is_empty() {
            tracing::info!(
                remaining = pending.len(),
                completed = completed.len(),
                failed = failed.len(),
                "polling remaining batch chunks"
            );
        }
    }

    Ok(BatchPollOutcome { completed, failed })
}

fn normalize_batch_dir(workspace: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}
