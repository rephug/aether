mod math;
mod monitor;
mod priority;
mod staleness;

use std::path::Path;

use aether_config::{AetherConfig, ContinuousConfig};
use aether_store::SqliteStore;
use anyhow::{Result, anyhow};

use crate::cli::{BatchPass, ContinuousArgs, ContinuousCommand};

pub(crate) use math::cosine_distance_from_embeddings;
pub(crate) use monitor::{ContinuousStatusSnapshot, load_status_snapshot, run_monitor_once};

pub fn run_continuous_command(
    workspace: &Path,
    config: &AetherConfig,
    args: ContinuousArgs,
) -> Result<()> {
    match args.command {
        ContinuousCommand::RunOnce(_) => {
            let status = run_monitor_once(workspace, config)?;
            print_status_snapshot(&status);
            Ok(())
        }
        ContinuousCommand::Status(_) => run_status_command(workspace),
    }
}

pub(crate) fn resolve_continuous_config(config: &AetherConfig) -> ContinuousConfig {
    config.continuous.clone().unwrap_or_default()
}

pub(crate) fn ensure_supported_schedule(schedule: &str) -> Result<()> {
    match schedule.trim().to_ascii_lowercase().as_str() {
        "hourly" | "nightly" => Ok(()),
        other => Err(anyhow!(
            "unsupported [continuous].schedule '{other}', expected 'hourly' or 'nightly'"
        )),
    }
}

pub(crate) fn parse_requeue_pass(raw: &str) -> Result<BatchPass> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "scan" => Ok(BatchPass::Scan),
        "triage" => Ok(BatchPass::Triage),
        "deep" => Ok(BatchPass::Deep),
        other => Err(anyhow!(
            "unsupported [continuous].requeue_pass '{other}', expected one of: scan, triage, deep"
        )),
    }
}

fn run_status_command(workspace: &Path) -> Result<()> {
    let store = SqliteStore::open(workspace)?;
    let (total_symbols, symbols_with_sir) = store.count_symbols_with_sir()?;

    match load_status_snapshot(workspace)? {
        Some(status) => {
            print_status_snapshot(&status);
            println!("Symbols currently indexed: {total_symbols}");
            println!("Symbols with SIR:        {symbols_with_sir}");
        }
        None => {
            println!("Continuous intelligence status");
            println!("No continuous run recorded yet.");
            println!("Symbols currently indexed: {total_symbols}");
            println!("Symbols with SIR:        {symbols_with_sir}");
        }
    }

    Ok(())
}

fn print_status_snapshot(status: &ContinuousStatusSnapshot) {
    println!("Continuous intelligence status");
    if let Some(timestamp) = status.last_started_at {
        println!("Last run started:        {timestamp}");
    }
    if let Some(timestamp) = status.last_completed_at {
        println!("Last run completed:      {timestamp}");
    }
    if let Some(timestamp) = status.last_successful_completed_at {
        println!("Last successful run:     {timestamp}");
    }
    println!("Total symbols:           {}", status.total_symbols);
    println!("Symbols with SIR:        {}", status.symbols_with_sir);
    println!("Scored symbols:          {}", status.scored_symbols);
    println!("Stale >= 0.8:            {}", status.score_bands.critical);
    println!("Stale >= 0.5:            {}", status.score_bands.high);
    println!("Stale >= 0.2:            {}", status.score_bands.medium);
    println!("Stale < 0.2:             {}", status.score_bands.low);
    if let Some(symbol) = status.most_stale_symbol.as_ref() {
        println!(
            "Most stale symbol:       {} ({}) score {:.3}",
            symbol.qualified_name, symbol.symbol_id, symbol.staleness_score
        );
    }
    println!("Selected for requeue:    {}", status.selected_symbols);
    println!("JSONL requests written:  {}", status.written_requests);
    println!("Prompt-hash skips:       {}", status.skipped_requests);
    println!("Unresolved symbols:      {}", status.unresolved_symbols);
    println!("Batch chunks:            {}", status.chunk_count);
    println!("Auto-submit enabled:     {}", status.auto_submit);
    println!("Submitted chunks:        {}", status.submitted_chunks);
    println!("Ingested results:        {}", status.ingested_results);
    println!("Fingerprint rows:        {}", status.fingerprint_rows);
    println!("Requeue pass:            {}", status.requeue_pass);
    if let Some(error) = status.last_error.as_deref() {
        println!("Last error:              {error}");
    }
}
