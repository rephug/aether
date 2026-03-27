mod aftershock;
mod community;
mod epicenter;
mod velocity;

use aether_config::{AetherConfig, SeismographConfig};
use aether_graph_algo::{GraphAlgorithmEdge, page_rank_sync};
use aether_store::{
    CascadeRecord, CommunityStabilityRecord as StoreCommunityStabilityRecord, DriftStore,
    SeismographMetricRecord, SqliteStore,
};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

pub use aftershock::{AftershockModel, AftershockPrediction, TrainingSample};
pub use community::CommunityStabilityResult;
pub use epicenter::{CascadeStep, build_history_map, trace_epicenter};
pub use velocity::VelocityResult;

use crate::cli::{SeismographArgs, SeismographCommand};

/// Summary of a complete seismograph analysis run.
#[derive(Debug, Clone)]
pub struct SeismographReport {
    pub batch_timestamp: i64,
    pub codebase_shift: f64,
    pub semantic_velocity: f64,
    pub symbols_regenerated: usize,
    pub symbols_above_noise: usize,
    pub community_results: Vec<CommunityStabilityResult>,
    pub cascade_count: usize,
    pub aftershock_predictions: Vec<AftershockPrediction>,
}

pub fn run_seismograph_command(
    workspace: &Path,
    config: &AetherConfig,
    args: SeismographArgs,
) -> Result<()> {
    match args.command {
        SeismographCommand::Status(_) => run_status(workspace),
        SeismographCommand::Trace(trace_args) => {
            run_trace(workspace, config, &trace_args.symbol_id)
        }
        SeismographCommand::RunOnce(_) => {
            let report = run_seismograph_analysis(workspace, config)?;
            print_report(&report);
            Ok(())
        }
        SeismographCommand::Train(_) => train_aftershock_model(workspace, config),
    }
}

/// Run the full seismograph analysis pipeline.
pub fn run_seismograph_analysis(
    workspace: &Path,
    config: &AetherConfig,
) -> Result<SeismographReport> {
    let seismo_config = resolve_seismograph_config(config);
    let store =
        SqliteStore::open(workspace).context("failed to open store for seismograph analysis")?;

    let now = unix_now();

    // 1. Determine the latest batch timestamp from fingerprint history
    let recent = store
        .list_recent_fingerprint_changes(1)
        .context("failed to load recent fingerprint changes")?;
    let batch_timestamp = recent.first().map(|r| r.timestamp).unwrap_or(now);

    // 2. Load fingerprint data for the batch
    let batch_records = store
        .list_fingerprint_history_for_batch(batch_timestamp)
        .context("failed to load fingerprint history for batch")?;

    // 3. Load graph edges and compute PageRank
    let graph_edges = store
        .list_graph_dependency_edges()
        .context("failed to load dependency edges")?;

    let pagerank_edges: Vec<GraphAlgorithmEdge> = graph_edges
        .iter()
        .map(|edge| GraphAlgorithmEdge {
            source_id: edge.source_symbol_id.clone(),
            target_id: edge.target_symbol_id.clone(),
            edge_kind: edge.edge_kind.clone(),
        })
        .collect();

    let pagerank_map: HashMap<String, f64> = page_rank_sync(&pagerank_edges, 0.85, 25)
        .into_iter()
        .collect();

    // 4. Compute semantic velocity
    let prev_velocity = store
        .latest_seismograph_metric()
        .ok()
        .flatten()
        .map(|m| m.semantic_velocity);

    let vel = velocity::compute_semantic_velocity(
        &batch_records,
        &pagerank_map,
        seismo_config.noise_floor,
        seismo_config.ema_alpha,
        prev_velocity,
    );

    // 5. Compute community stability over rolling window
    let window_seconds = i64::from(seismo_config.community_window_days) * 24 * 60 * 60;
    let window_start = now - window_seconds;

    let window_records = store
        .list_fingerprint_history_window(window_start, now)
        .context("failed to load fingerprint history window")?;

    let community_snapshot = store
        .list_latest_community_snapshot()
        .context("failed to load community snapshot")?;

    let community_map: HashMap<String, String> = community_snapshot
        .iter()
        .map(|cs| (cs.symbol_id.clone(), cs.community_id.to_string()))
        .collect();

    let community_results = community::compute_community_stability(
        &window_records,
        &community_map,
        &pagerank_map,
        seismo_config.noise_floor,
    );

    // 6. Epicenter tracing for propagated changes
    let history_map = build_history_map(&window_records);

    // Build dependency lookup from graph edges: source_id → Vec<target_id>
    let mut dep_map: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &graph_edges {
        dep_map
            .entry(edge.source_symbol_id.clone())
            .or_default()
            .push(edge.target_symbol_id.clone());
    }
    let edge_lookup = |id: &str| dep_map.get(id).cloned().unwrap_or_default();

    let mut cascade_count = 0;
    for record in &batch_records {
        let delta = record.delta_sem.unwrap_or(0.0);
        if delta > seismo_config.noise_floor && !record.source_changed {
            let chain = trace_epicenter(
                &record.symbol_id,
                &history_map,
                &edge_lookup,
                seismo_config.noise_floor,
                seismo_config.cascade_max_depth,
            );
            if chain.len() > 1 {
                let epicenter_id = chain[0].symbol_id.clone();
                let max_delta = chain.iter().map(|s| s.delta_sem).fold(0.0_f64, f64::max);
                let chain_json = serde_json::to_string(&chain).unwrap_or_else(|_| "[]".to_owned());

                store.insert_cascade(&CascadeRecord {
                    epicenter_symbol_id: epicenter_id,
                    chain_json,
                    total_hops: chain.len() as i64,
                    max_delta_sem: max_delta,
                    detected_at: now,
                })?;
                cascade_count += 1;
            }
        }
    }

    // 7. Aftershock prediction (optional)
    let aftershock_predictions = if seismo_config.aftershock_enabled {
        predict_aftershocks(
            &store,
            &batch_records,
            &pagerank_map,
            &dep_map,
            &seismo_config,
        )?
    } else {
        Vec::new()
    };

    // 8. Persist velocity and community metrics
    store.insert_seismograph_metric(&SeismographMetricRecord {
        batch_timestamp,
        codebase_shift: vel.codebase_shift,
        semantic_velocity: vel.semantic_velocity,
        symbols_regenerated: vel.symbols_regenerated as i64,
        symbols_above_noise: vel.symbols_above_noise as i64,
    })?;

    for cr in &community_results {
        store.insert_community_stability(&StoreCommunityStabilityRecord {
            community_id: cr.community_id.clone(),
            computed_at: now,
            stability: cr.stability,
            symbol_count: cr.symbol_count as i64,
            breach_count: cr.breach_count as i64,
        })?;
    }

    Ok(SeismographReport {
        batch_timestamp,
        codebase_shift: vel.codebase_shift,
        semantic_velocity: vel.semantic_velocity,
        symbols_regenerated: vel.symbols_regenerated,
        symbols_above_noise: vel.symbols_above_noise,
        community_results,
        cascade_count,
        aftershock_predictions,
    })
}

/// Run aftershock predictions for high-Δ_sem symbols in the batch.
fn predict_aftershocks(
    store: &SqliteStore,
    batch_records: &[aether_store::SirFingerprintHistoryRecord],
    pagerank_map: &HashMap<String, f64>,
    dep_map: &HashMap<String, Vec<String>>,
    config: &SeismographConfig,
) -> Result<Vec<AftershockPrediction>> {
    let model_record = store.latest_aftershock_model()?;
    let Some(record) = model_record else {
        return Ok(Vec::new());
    };

    let weights: [f64; 5] = serde_json::from_str(&record.weights_json)
        .context("failed to parse aftershock model weights")?;
    let model = AftershockModel {
        weights,
        trained_at: record.trained_at,
    };

    let mut predictions = Vec::new();

    for br in batch_records {
        let delta = br.delta_sem.unwrap_or(0.0);
        if delta <= config.noise_floor {
            continue;
        }

        // For each downstream neighbor of this high-Δ_sem symbol,
        // predict if it will breach. We look at symbols that depend on this one
        // by checking reverse edges. Since our dep_map is source→targets,
        // we need to find entries where this symbol is a target.
        // For efficiency, predict for direct dependents only.
        for (source_id, targets) in dep_map {
            if targets.contains(&br.symbol_id) {
                let pr_target = pagerank_map.get(source_id).copied().unwrap_or(0.0);
                let prob = model.predict(delta, 1.0, 1.0, pr_target);
                if prob > 0.5 {
                    predictions.push(AftershockPrediction {
                        target_symbol_id: source_id.clone(),
                        probability: prob,
                    });
                }
            }
        }
    }

    predictions.sort_by(|a, b| {
        b.probability
            .partial_cmp(&a.probability)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    predictions.truncate(50); // Cap output

    Ok(predictions)
}

/// Train the aftershock model from fingerprint history.
pub fn train_aftershock_model(workspace: &Path, config: &AetherConfig) -> Result<()> {
    let seismo_config = resolve_seismograph_config(config);
    let store = SqliteStore::open(workspace)?;

    let now = unix_now();
    let window_seconds = i64::from(seismo_config.community_window_days) * 24 * 60 * 60;
    let window_start = now - window_seconds;

    let records = store
        .list_fingerprint_history_window(window_start, now)
        .context("failed to load training data")?;

    // Build dependency map
    let graph_edges = store
        .list_graph_dependency_edges()
        .context("failed to load graph edges")?;

    let mut dep_map: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &graph_edges {
        dep_map
            .entry(edge.source_symbol_id.clone())
            .or_default()
            .push(edge.target_symbol_id.clone());
    }

    // Build PageRank
    let pagerank_edges: Vec<GraphAlgorithmEdge> = graph_edges
        .iter()
        .map(|edge| GraphAlgorithmEdge {
            source_id: edge.source_symbol_id.clone(),
            target_id: edge.target_symbol_id.clone(),
            edge_kind: edge.edge_kind.clone(),
        })
        .collect();
    let pagerank_map: HashMap<String, f64> = page_rank_sync(&pagerank_edges, 0.85, 25)
        .into_iter()
        .collect();

    // Build max delta_sem per symbol in window
    let mut max_delta: HashMap<&str, f64> = HashMap::new();
    for r in &records {
        let d = r.delta_sem.unwrap_or(0.0);
        let entry = max_delta.entry(&r.symbol_id).or_insert(0.0);
        if d > *entry {
            *entry = d;
        }
    }

    // Build training samples: for each (source, target) edge pair,
    // check if source had a high Δ_sem and whether target subsequently breached
    let mut samples = Vec::new();
    for (source_id, targets) in &dep_map {
        let source_delta = max_delta.get(source_id.as_str()).copied().unwrap_or(0.0);
        if source_delta <= seismo_config.noise_floor {
            continue;
        }

        for target_id in targets {
            let target_delta = max_delta.get(target_id.as_str()).copied().unwrap_or(0.0);
            let pr_target = pagerank_map.get(target_id).copied().unwrap_or(0.0);

            samples.push(TrainingSample {
                delta_sem_source: source_delta,
                coupling: 1.0, // Simplified: no coupling data from SQLite
                graph_distance: 1.0,
                pagerank_target: pr_target,
                target_breached: target_delta > seismo_config.noise_floor,
            });
        }
    }

    if samples.is_empty() {
        println!("No training samples available. Need more fingerprint history.");
        return Ok(());
    }

    let model = aftershock::train(&samples, 0.01, 1000);
    let auc = aftershock::compute_auc_roc(&model, &samples);

    let weights_json = serde_json::to_string(&model.weights)?;
    store.insert_aftershock_model(&aether_store::AftershockModelRecord {
        trained_at: now,
        training_samples: samples.len() as i64,
        weights_json,
        auc_roc: auc,
    })?;

    println!(
        "Aftershock model trained: {} samples, AUC-ROC: {:.4}",
        samples.len(),
        auc.unwrap_or(0.0)
    );
    println!("Weights: {:?}", model.weights);

    Ok(())
}

fn run_status(workspace: &Path) -> Result<()> {
    let store = SqliteStore::open_readonly(workspace).context("failed to open store")?;

    // Latest velocity
    match store.latest_seismograph_metric()? {
        Some(m) => {
            println!("Seismograph status");
            println!("Batch timestamp:         {}", m.batch_timestamp);
            println!("Codebase shift:          {:.4}", m.codebase_shift);
            println!("Semantic velocity:       {:.4}", m.semantic_velocity);
            println!("Symbols regenerated:     {}", m.symbols_regenerated);
            println!("Symbols above noise:     {}", m.symbols_above_noise);
        }
        None => {
            println!("Seismograph status");
            println!("No seismograph data recorded yet. Run 'aetherd seismograph run-once' first.");
            return Ok(());
        }
    }

    // Top 5 unstable communities
    let communities = store.latest_community_stability()?;
    if !communities.is_empty() {
        println!();
        println!("Top unstable communities:");
        for (i, c) in communities.iter().take(5).enumerate() {
            println!(
                "  {}. Community {} — stability {:.4} ({} symbols, {} breaches)",
                i + 1,
                c.community_id,
                c.stability,
                c.symbol_count,
                c.breach_count,
            );
        }
    }

    // Recent cascades
    let cascades = store.list_cascades(5)?;
    if !cascades.is_empty() {
        println!();
        println!("Recent cascades:");
        for c in &cascades {
            println!(
                "  Epicenter {} — {} hops, max Δ_sem {:.4}, at {}",
                c.epicenter_symbol_id, c.total_hops, c.max_delta_sem, c.detected_at,
            );
        }
    }

    Ok(())
}

fn run_trace(workspace: &Path, config: &AetherConfig, symbol_id: &str) -> Result<()> {
    let seismo_config = resolve_seismograph_config(config);
    let store = SqliteStore::open_readonly(workspace).context("failed to open store")?;

    let now = unix_now();
    let window_seconds = i64::from(seismo_config.community_window_days) * 24 * 60 * 60;
    let window_start = now - window_seconds;

    let window_records = store
        .list_fingerprint_history_window(window_start, now)
        .context("failed to load fingerprint history")?;

    let history_map = build_history_map(&window_records);

    // Build dependency lookup
    let graph_edges = store
        .list_graph_dependency_edges()
        .context("failed to load graph edges")?;

    let mut dep_map: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &graph_edges {
        dep_map
            .entry(edge.source_symbol_id.clone())
            .or_default()
            .push(edge.target_symbol_id.clone());
    }
    let edge_lookup = |id: &str| dep_map.get(id).cloned().unwrap_or_default();

    let chain = trace_epicenter(
        symbol_id,
        &history_map,
        &edge_lookup,
        seismo_config.noise_floor,
        seismo_config.cascade_max_depth,
    );

    if chain.is_empty() {
        println!("No cascade chain found for symbol '{symbol_id}'.");
        println!("The symbol may not have fingerprint history in the current window.");
        return Ok(());
    }

    println!("Cascade chain for '{symbol_id}' ({} hops):", chain.len());
    for step in &chain {
        let marker = if step.source_changed {
            "[EPICENTER]"
        } else {
            "           "
        };
        println!(
            "  {marker} {} — Δ_sem {:.4}, t={}, hop {}",
            step.symbol_id, step.delta_sem, step.timestamp, step.hop,
        );
    }

    Ok(())
}

fn print_report(report: &SeismographReport) {
    println!("Seismograph analysis complete");
    println!("Batch timestamp:         {}", report.batch_timestamp);
    println!("Codebase shift:          {:.4}", report.codebase_shift);
    println!("Semantic velocity:       {:.4}", report.semantic_velocity);
    println!("Symbols regenerated:     {}", report.symbols_regenerated);
    println!("Symbols above noise:     {}", report.symbols_above_noise);
    println!("Cascades detected:       {}", report.cascade_count);

    if !report.community_results.is_empty() {
        println!();
        println!(
            "Community stability ({} communities):",
            report.community_results.len()
        );
        for (i, cr) in report.community_results.iter().take(5).enumerate() {
            println!(
                "  {}. Community {} — stability {:.4} ({} symbols, {} breaches)",
                i + 1,
                cr.community_id,
                cr.stability,
                cr.symbol_count,
                cr.breach_count,
            );
        }
    }

    if !report.aftershock_predictions.is_empty() {
        println!();
        println!(
            "Aftershock predictions ({}):",
            report.aftershock_predictions.len()
        );
        for p in report.aftershock_predictions.iter().take(10) {
            println!(
                "  {} — P(breach) = {:.4}",
                p.target_symbol_id, p.probability
            );
        }
    }
}

pub(crate) fn resolve_seismograph_config(config: &AetherConfig) -> SeismographConfig {
    config.seismograph.clone().unwrap_or_default()
}

fn unix_now() -> i64 {
    crate::time::current_unix_timestamp_secs()
}
