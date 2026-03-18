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

        for input_jsonl in build_summary.files {
            let results_paths = tokio_rt.block_on(submit_and_wait(
                provider.as_ref(),
                &input_jsonl,
                &pass_config.model,
                &runtime,
            ))?;

            for result_path in results_paths {
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
                    result_path.display(),
                    ingest_summary.processed,
                    ingest_summary.skipped,
                    ingest_summary.fingerprint_rows
                );
            }
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

/// Submit a batch input file, poll until completion, and download results.
async fn submit_and_wait(
    provider: &dyn BatchProvider,
    input_path: &Path,
    model: &str,
    runtime: &BatchRuntimeConfig,
) -> Result<Vec<PathBuf>> {
    let job_ids = provider
        .submit(
            input_path,
            model,
            &runtime.batch_dir,
            runtime.poll_interval_secs,
        )
        .await
        .with_context(|| format!("batch submit failed for {}", input_path.display()))?;

    tracing::info!(
        provider = provider.name(),
        jobs = ?job_ids,
        "batch job(s) submitted, polling for completion"
    );

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(runtime.poll_interval_secs)).await;

        match provider.poll(&job_ids).await? {
            BatchPollStatus::Completed => {
                tracing::info!("batch job completed, downloading results");
                break;
            }
            BatchPollStatus::Failed { message } => {
                return Err(anyhow!("batch job failed: {}", message));
            }
            BatchPollStatus::InProgress { completed, total } => {
                if let (Some(c), Some(t)) = (completed, total) {
                    tracing::info!(completed = c, total = t, "batch job in progress");
                } else {
                    tracing::info!("batch job in progress");
                }
            }
        }
    }

    provider
        .download_results(&job_ids, &runtime.batch_dir)
        .await
        .context("failed to download batch results")
}

fn normalize_batch_dir(workspace: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}
