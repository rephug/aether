use std::path::{Path, PathBuf};
use std::process::Command;

use aether_config::AetherConfig;
use aether_store::SqliteStore;
use anyhow::{Context, Result, anyhow};

use crate::batch::build::{build_pass_jsonl, snapshot_workspace_symbols};
use crate::batch::extract::run_extract;
use crate::batch::ingest::ingest_results;
use crate::batch::{
    BatchRuntimeConfig, PassConfig, parse_batch_passes_csv, resolve_batch_runtime_config,
    resolve_build_pass_config,
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
    let store = SqliteStore::open(workspace).context("failed to open store for batch build")?;
    let symbols_by_id = snapshot_workspace_symbols(workspace)?;
    let mut runtime = resolve_batch_runtime_config(workspace, config, None);
    if let Some(batch_dir) = args.batch_dir.as_deref() {
        runtime.batch_dir = normalize_batch_dir(workspace, batch_dir);
    }
    let pass_config = resolve_build_pass_config(&runtime, args);
    let summary = build_pass_jsonl(workspace, &store, &runtime, &pass_config, &symbols_by_id)?;
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
    let runtime = resolve_batch_runtime_config(workspace, config, Some(args));
    let passes = parse_batch_passes_csv(args.passes.as_str())?;
    let script = workspace.join("scripts/gemini_batch_submit.sh");
    if !script.exists() {
        return Err(anyhow!(
            "missing batch submit script at {}",
            script.display()
        ));
    }

    let extract_summary = run_extract(workspace)?;
    println!("Extracted {} symbols", extract_summary.symbol_count);
    for pass in passes {
        let pass_config = runtime.for_pass(pass).clone();
        let build_summary = build_pass_jsonl(
            workspace,
            &extract_summary.store,
            &runtime,
            &pass_config,
            &extract_summary.symbols_by_id,
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
            let results_jsonl = submit_batch_chunk(
                workspace,
                &script,
                &runtime,
                &pass_config,
                input_jsonl.as_path(),
            )?;
            let ingest_summary = ingest_results(
                workspace,
                &extract_summary.store,
                &pass_config,
                results_jsonl.as_path(),
                config,
            )?;
            println!(
                "Ingested {} chunk {}: processed {}, skipped {}, fingerprint rows {}",
                pass.as_str(),
                results_jsonl.display(),
                ingest_summary.processed,
                ingest_summary.skipped,
                ingest_summary.fingerprint_rows
            );
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

pub(crate) fn submit_batch_chunk(
    workspace: &Path,
    script: &Path,
    runtime: &BatchRuntimeConfig,
    pass_config: &PassConfig,
    input_jsonl: &Path,
) -> Result<PathBuf> {
    let output = Command::new("bash")
        .arg(script)
        .arg(input_jsonl)
        .arg(pass_config.model.as_str())
        .arg(&runtime.batch_dir)
        .arg(runtime.poll_interval_secs.to_string())
        .current_dir(workspace)
        .output()
        .with_context(|| format!("failed to launch batch submit script {}", script.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "batch submit script failed for {}: {}",
            input_jsonl.display(),
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result_path = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .ok_or_else(|| anyhow!("batch submit script did not print a result path"))?;
    let result_path = normalize_batch_dir(workspace, result_path);
    if !result_path.exists() {
        return Err(anyhow!(
            "batch submit script returned missing result path {}",
            result_path.display()
        ));
    }
    Ok(result_path)
}

fn normalize_batch_dir(workspace: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}
