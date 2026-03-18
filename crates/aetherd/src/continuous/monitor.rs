use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{AetherConfig, GraphBackend, aether_dir};
use aether_core::{GitContext, Symbol};
use aether_graph_algo::{GraphAlgorithmEdge, page_rank_sync};
use aether_health::git_signals::compute_file_git_stats;
use aether_store::{
    SirStateStore, SqliteStore, SurrealGraphStore, block_on_store_future,
    open_surreal_graph_store_readonly,
};
use anyhow::{Context, Result};
use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};

use super::priority::compute_priority;
use super::staleness::{compute_staleness, effective_age, time_staleness};
use super::{ensure_supported_schedule, parse_requeue_pass, resolve_continuous_config};
use crate::batch::hash::{compute_source_hash_segment, decompose_prompt_hash};
use crate::batch::{
    build_pass_jsonl_for_ids, ingest_results, resolve_batch_runtime_config, submit_batch_chunk,
};
use crate::indexer::run_structural_index_once;
use crate::sir_pipeline::build_job;

const DEPRECATED_MODELS: &[&str] = &["qwen2.5-coder:7b"];
const VOLATILITY_WINDOW_DAYS: i64 = 30;
const VOLATILITY_DELTA_THRESHOLD: f64 = 0.2;
const VOLATILITY_EVENT_THRESHOLD: usize = 3;
const VOLATILITY_TIME_MULTIPLIER: f64 = 1.5;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScoreBands {
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ContinuousStatusSymbol {
    pub symbol_id: String,
    pub qualified_name: String,
    pub staleness_score: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ContinuousStatusSnapshot {
    pub last_started_at: Option<i64>,
    pub last_completed_at: Option<i64>,
    pub last_successful_completed_at: Option<i64>,
    pub total_symbols: usize,
    pub symbols_with_sir: usize,
    pub scored_symbols: usize,
    pub score_bands: ScoreBands,
    pub most_stale_symbol: Option<ContinuousStatusSymbol>,
    pub selected_symbols: usize,
    pub written_requests: usize,
    pub skipped_requests: usize,
    pub unresolved_symbols: usize,
    pub chunk_count: usize,
    pub auto_submit: bool,
    pub submitted_chunks: usize,
    pub ingested_results: usize,
    pub fingerprint_rows: usize,
    pub requeue_pass: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct SymbolScoreRow {
    symbol: Symbol,
    final_score: f64,
    priority: f64,
}

pub(crate) fn run_monitor_once(
    workspace: &Path,
    config: &AetherConfig,
) -> Result<ContinuousStatusSnapshot> {
    let started_at = unix_timestamp_secs();
    match run_monitor_once_inner(workspace, config, started_at) {
        Ok(snapshot) => Ok(snapshot),
        Err(err) => {
            record_failure_snapshot(workspace, started_at, err.to_string())?;
            Err(err)
        }
    }
}

pub(crate) fn load_status_snapshot(workspace: &Path) -> Result<Option<ContinuousStatusSnapshot>> {
    let path = status_path(workspace);
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let snapshot = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(snapshot))
}

fn run_monitor_once_inner(
    workspace: &Path,
    config: &AetherConfig,
    started_at: i64,
) -> Result<ContinuousStatusSnapshot> {
    let continuous = resolve_continuous_config(config);
    ensure_supported_schedule(continuous.schedule.as_str())?;
    let requeue_pass = parse_requeue_pass(continuous.requeue_pass.as_str())?;

    let previous_status = load_status_snapshot(workspace)?;
    let last_successful_at = previous_status
        .as_ref()
        .and_then(|status| status.last_successful_completed_at);
    let runtime = resolve_batch_runtime_config(workspace, config, None);

    let (store, symbols_by_id, total_symbols) = run_structural_index_once(workspace)
        .context("continuous run-once failed to refresh structural index")?;

    let current_symbols = symbols_by_id;
    let symbol_ids_with_sir = store
        .list_symbol_ids_with_sir()
        .context("failed to list symbols with SIR for continuous monitor")?
        .into_iter()
        .filter(|symbol_id| current_symbols.contains_key(symbol_id))
        .collect::<HashSet<_>>();
    let symbols_with_sir = symbol_ids_with_sir.len();
    let mut candidate_ids = current_symbols.keys().cloned().collect::<Vec<_>>();
    candidate_ids.sort();

    let scored_rows = score_symbols(
        workspace,
        &store,
        &current_symbols,
        &candidate_ids,
        config,
        &continuous,
        &runtime,
        last_successful_at,
    )?;

    let mut prioritized = scored_rows
        .iter()
        .map(|row| (row.symbol.id.clone(), row.priority))
        .collect::<Vec<_>>();
    prioritized.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    let selected_ids = prioritized
        .into_iter()
        .filter(|(symbol_id, _)| {
            matches!(requeue_pass, crate::cli::BatchPass::Scan)
                || symbol_ids_with_sir.contains(symbol_id)
        })
        .take(continuous.max_requeue_per_run)
        .map(|(symbol_id, _)| symbol_id)
        .collect::<Vec<_>>();

    let contracts_enabled = config.contracts.as_ref().is_some_and(|c| c.enabled);
    let pass_config = runtime.for_pass(requeue_pass).clone();
    let build_summary = build_pass_jsonl_for_ids(
        workspace,
        &store,
        &runtime,
        &pass_config,
        &current_symbols,
        Some(selected_ids.as_slice()),
        contracts_enabled,
    )?;

    persist_staleness_scores(&store, &scored_rows)?;

    let mut submitted_chunks = 0usize;
    let mut ingested_results = 0usize;
    let mut fingerprint_rows = 0usize;
    if continuous.auto_submit && !build_summary.files.is_empty() {
        let script = workspace.join("scripts/gemini_batch_submit.sh");
        if !script.exists() {
            anyhow::bail!("missing batch submit script at {}", script.display());
        }
        for input_jsonl in &build_summary.files {
            let results_jsonl =
                submit_batch_chunk(workspace, &script, &runtime, &pass_config, input_jsonl)?;
            submitted_chunks += 1;
            let ingest_summary =
                ingest_results(workspace, &store, &pass_config, &results_jsonl, config)?;
            ingested_results += ingest_summary.processed;
            fingerprint_rows += ingest_summary.fingerprint_rows;
        }
    }

    let completed_at = unix_timestamp_secs();
    let final_scores = scored_rows
        .iter()
        .map(|row| row.final_score)
        .collect::<Vec<_>>();
    let score_bands = bands_from_scores(&final_scores);
    let most_stale_symbol = scored_rows
        .iter()
        .max_by(|left, right| {
            left.final_score
                .partial_cmp(&right.final_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| right.symbol.id.cmp(&left.symbol.id))
        })
        .map(|row| ContinuousStatusSymbol {
            symbol_id: row.symbol.id.clone(),
            qualified_name: row.symbol.qualified_name.clone(),
            staleness_score: row.final_score,
        });

    let snapshot = ContinuousStatusSnapshot {
        last_started_at: Some(started_at),
        last_completed_at: Some(completed_at),
        last_successful_completed_at: Some(completed_at),
        total_symbols,
        symbols_with_sir,
        scored_symbols: scored_rows.len(),
        score_bands,
        most_stale_symbol,
        selected_symbols: selected_ids.len(),
        written_requests: build_summary.written,
        skipped_requests: build_summary.skipped,
        unresolved_symbols: build_summary.unresolved_symbols,
        chunk_count: build_summary.files.len(),
        auto_submit: continuous.auto_submit,
        submitted_chunks,
        ingested_results,
        fingerprint_rows,
        requeue_pass: continuous.requeue_pass.clone(),
        last_error: None,
    };
    write_status_snapshot(workspace, &snapshot)?;
    Ok(snapshot)
}

#[allow(clippy::too_many_arguments)]
fn score_symbols(
    workspace: &Path,
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
    candidate_ids: &[String],
    config: &AetherConfig,
    continuous: &aether_config::ContinuousConfig,
    runtime: &crate::batch::BatchRuntimeConfig,
    last_successful_at: Option<i64>,
) -> Result<Vec<SymbolScoreRow>> {
    let graph_edges = store
        .list_graph_dependency_edges()
        .context("failed to load dependency edges for continuous monitor")?;
    let (graph, node_map) = load_dependency_graph(&graph_edges);

    let pagerank_edges = graph_edges
        .iter()
        .map(|edge| GraphAlgorithmEdge {
            source_id: edge.source_symbol_id.clone(),
            target_id: edge.target_symbol_id.clone(),
            edge_kind: edge.edge_kind.clone(),
        })
        .collect::<Vec<_>>();
    let pagerank_by_symbol = page_rank_sync(&pagerank_edges, 0.85, 25)
        .into_iter()
        .collect::<HashMap<_, _>>();
    let pr_max = pagerank_by_symbol.values().copied().fold(0.0_f64, f64::max);

    let churn_by_file = compute_git_churn_by_file(workspace, symbols_by_id);
    let history_cutoff = unix_timestamp_secs() - VOLATILITY_WINDOW_DAYS * 24 * 60 * 60;

    let mut base_score_by_symbol = HashMap::new();
    let mut delta_sem_by_symbol = HashMap::new();
    let mut scored = Vec::new();
    for symbol_id in candidate_ids {
        let Some(symbol) = symbols_by_id.get(symbol_id.as_str()).cloned() else {
            continue;
        };
        let meta = store
            .get_sir_meta(symbol_id)
            .with_context(|| format!("failed to load SIR metadata for {symbol_id}"))?;
        let missing_meta = meta.is_none();
        let max_chars = if let Some(record) = meta.as_ref() {
            source_hash_limit_for_pass(runtime, record.generation_pass.as_str())
        } else {
            None
        };
        let source_changed = missing_meta
            || source_segment_changed(
                workspace,
                &symbol,
                meta.as_ref()
                    .and_then(|record| record.prompt_hash.as_deref()),
                max_chars,
            );
        let model_deprecated = meta
            .as_ref()
            .is_some_and(|record| model_is_deprecated(record.model.as_str()));
        let churn_30d = churn_by_file
            .get(symbol.file_path.as_str())
            .copied()
            .unwrap_or(0.0);

        let (time_score, latest_delta) = if let Some(meta) = meta.as_ref() {
            let days_since =
                ((unix_timestamp_secs() - meta.updated_at.max(0)) as f64 / 86_400.0).max(0.0);
            let age = effective_age(days_since, churn_30d);
            let mut time_score = time_staleness(
                age,
                continuous.staleness_half_life_days,
                continuous.staleness_sigmoid_k,
            );

            let history = store
                .list_sir_fingerprint_history(symbol_id)
                .with_context(|| format!("failed to load fingerprint history for {symbol_id}"))?;
            let latest_delta = history.iter().rev().find_map(|row| row.delta_sem);
            let volatility_events = history
                .iter()
                .filter(|row| row.timestamp >= history_cutoff)
                .filter_map(|row| row.delta_sem)
                .filter(|delta| *delta > VOLATILITY_DELTA_THRESHOLD)
                .count();
            if volatility_events >= VOLATILITY_EVENT_THRESHOLD {
                time_score = (time_score * VOLATILITY_TIME_MULTIPLIER).clamp(0.0, 1.0);
            }

            (time_score, latest_delta)
        } else {
            (0.0, None)
        };

        let base_score = compute_staleness(source_changed, model_deprecated, time_score, 0.0);
        if let Some(delta) = latest_delta {
            delta_sem_by_symbol.insert(symbol.id.clone(), delta);
        }
        base_score_by_symbol.insert(symbol.id.clone(), base_score);
        scored.push((symbol, base_score));
    }

    let neighbor_scores = propagate_neighbor_staleness(
        &graph,
        &node_map,
        &base_score_by_symbol,
        &delta_sem_by_symbol,
        continuous.neighbor_decay,
        continuous.neighbor_cutoff,
    );
    let surreal_graph = if matches!(
        config.storage.graph_backend,
        GraphBackend::Surreal | GraphBackend::Cozo
    ) {
        open_surreal_graph_store_readonly(workspace).ok()
    } else {
        None
    };
    let coupling_scores = coupling_predict(
        workspace,
        surreal_graph.as_ref(),
        config,
        last_successful_at,
        symbols_by_id,
        candidate_ids,
        continuous.coupling_predict_threshold,
    );

    let mut rows = Vec::with_capacity(scored.len());
    for (symbol, base_score) in scored {
        let neighbor_score = neighbor_scores
            .get(symbol.id.as_str())
            .copied()
            .unwrap_or(0.0);
        let coupling_score = coupling_scores
            .get(symbol.id.as_str())
            .copied()
            .unwrap_or(0.0);
        let final_score = base_score.max(coupling_score).max(compute_staleness(
            false,
            false,
            base_score,
            neighbor_score,
        ));
        let pagerank = pagerank_by_symbol
            .get(symbol.id.as_str())
            .copied()
            .unwrap_or(0.0);
        let priority = compute_priority(
            final_score,
            pagerank,
            pr_max,
            continuous.priority_pagerank_alpha,
        );
        rows.push(SymbolScoreRow {
            symbol,
            final_score,
            priority,
        });
    }

    Ok(rows)
}

fn compute_git_churn_by_file(
    workspace: &Path,
    symbols_by_id: &HashMap<String, Symbol>,
) -> HashMap<String, f64> {
    let Some(git) = GitContext::open(workspace) else {
        tracing::warn!(
            "continuous monitor could not open git repository; git churn defaults to zero"
        );
        return HashMap::new();
    };

    let unique_files = symbols_by_id
        .values()
        .map(|symbol| symbol.file_path.clone())
        .collect::<HashSet<_>>();
    let mut churn = HashMap::new();
    for file_path in unique_files {
        let stats = compute_file_git_stats(&git, Path::new(file_path.as_str()));
        churn.insert(file_path, stats.commits_30d as f64);
    }
    churn
}

fn source_segment_changed(
    workspace: &Path,
    symbol: &Symbol,
    stored_prompt_hash: Option<&str>,
    max_chars: Option<usize>,
) -> bool {
    let Some(stored_source) = stored_prompt_hash.and_then(|hash| decompose_prompt_hash(hash).0)
    else {
        return true;
    };
    let current_source = build_job(workspace, symbol.clone(), None, max_chars)
        .map(|job| compute_source_hash_segment(job.symbol_text.as_str()))
        .ok();
    match current_source.as_deref() {
        Some(segment) => segment != stored_source,
        None => true,
    }
}

fn source_hash_limit_for_pass(
    runtime: &crate::batch::BatchRuntimeConfig,
    generation_pass: &str,
) -> Option<usize> {
    match generation_pass.trim().to_ascii_lowercase().as_str() {
        "scan" => Some(runtime.scan.max_chars),
        "triage" => Some(runtime.triage.max_chars),
        "deep" => Some(runtime.deep.max_chars),
        _ => None,
    }
}

fn model_is_deprecated(model: &str) -> bool {
    DEPRECATED_MODELS
        .iter()
        .any(|deprecated| model.trim().eq_ignore_ascii_case(deprecated))
}

fn load_dependency_graph(
    edges: &[aether_store::GraphDependencyEdgeRecord],
) -> (DiGraph<String, ()>, HashMap<String, NodeIndex>) {
    let mut graph = DiGraph::new();
    let mut node_map = HashMap::new();
    for edge in edges {
        let source_idx = *node_map
            .entry(edge.source_symbol_id.clone())
            .or_insert_with(|| graph.add_node(edge.source_symbol_id.clone()));
        let target_idx = *node_map
            .entry(edge.target_symbol_id.clone())
            .or_insert_with(|| graph.add_node(edge.target_symbol_id.clone()));
        graph.add_edge(source_idx, target_idx, ());
    }
    (graph, node_map)
}

fn propagate_neighbor_staleness(
    graph: &DiGraph<String, ()>,
    node_map: &HashMap<String, NodeIndex>,
    seed_staleness: &HashMap<String, f64>,
    delta_sem: &HashMap<String, f64>,
    gamma: f64,
    cutoff: f64,
) -> HashMap<String, f64> {
    let mut induced = HashMap::<String, f64>::new();
    for (symbol_id, seed_score) in seed_staleness {
        let Some(seed_delta) = delta_sem.get(symbol_id.as_str()).copied() else {
            continue;
        };
        let Some(&seed_idx) = node_map.get(symbol_id.as_str()) else {
            continue;
        };

        let first_hop = seed_score * gamma * seed_delta;
        if first_hop < cutoff {
            continue;
        }

        let mut queue = VecDeque::from([(seed_idx, first_hop)]);
        let mut seen = HashSet::from([seed_idx]);
        while let Some((current_idx, propagated_score)) = queue.pop_front() {
            for dependent_idx in graph.neighbors_directed(current_idx, Direction::Incoming) {
                if !seen.insert(dependent_idx) {
                    continue;
                }

                let Some(symbol_id) = graph.node_weight(dependent_idx).cloned() else {
                    continue;
                };
                induced
                    .entry(symbol_id.clone())
                    .and_modify(|score| *score = score.max(propagated_score))
                    .or_insert(propagated_score);

                let next_score = propagated_score * gamma;
                if next_score >= cutoff {
                    queue.push_back((dependent_idx, next_score));
                }
            }
        }
    }
    induced
}

fn coupling_predict(
    workspace: &Path,
    graph_store: Option<&SurrealGraphStore>,
    config: &AetherConfig,
    last_successful_at: Option<i64>,
    symbols_by_id: &HashMap<String, Symbol>,
    candidate_ids: &[String],
    threshold: f64,
) -> HashMap<String, f64> {
    let Some(last_successful_at) = last_successful_at else {
        return HashMap::new();
    };
    if !matches!(
        config.storage.graph_backend,
        GraphBackend::Surreal | GraphBackend::Cozo
    ) {
        tracing::warn!(
            "continuous monitor skipped coupling prediction because graph backend is not surreal/cozo"
        );
        return HashMap::new();
    }

    let Some(graph_store) = graph_store else {
        tracing::warn!("continuous monitor skipped coupling prediction");
        return HashMap::new();
    };

    let candidate_set = candidate_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut symbols_by_file = HashMap::<String, Vec<String>>::new();
    for symbol in symbols_by_id.values() {
        if !candidate_set.contains(symbol.id.as_str()) {
            continue;
        }
        symbols_by_file
            .entry(symbol.file_path.clone())
            .or_default()
            .push(symbol.id.clone());
    }

    let recently_edited = symbols_by_file
        .keys()
        .filter(|file_path| {
            file_was_modified_after(workspace, file_path.as_str(), last_successful_at)
        })
        .cloned()
        .collect::<Vec<_>>();

    let mut bumps = HashMap::<String, f64>::new();
    for file_path in recently_edited {
        let edges = match block_on_store_future(
            graph_store.list_co_change_edges_for_file(file_path.as_str(), threshold as f32),
        ) {
            Ok(Ok(edges)) => edges,
            _ => continue,
        };
        if edges.is_empty() {
            continue;
        }
        for edge in edges {
            if f64::from(edge.fused_score) < threshold {
                continue;
            }
            let other_file = if edge.file_a == file_path {
                edge.file_b
            } else {
                edge.file_a
            };
            let bump = (f64::from(edge.fused_score) * 0.5).clamp(0.0, 1.0);
            for symbol_id in symbols_by_file
                .get(other_file.as_str())
                .into_iter()
                .flatten()
            {
                bumps
                    .entry(symbol_id.clone())
                    .and_modify(|score| *score = score.max(bump))
                    .or_insert(bump);
            }
        }
    }

    bumps
}

fn persist_staleness_scores(store: &SqliteStore, scored_rows: &[SymbolScoreRow]) -> Result<()> {
    for row in scored_rows {
        let Some(meta) = store.get_sir_meta(row.symbol.id.as_str())? else {
            continue;
        };
        store.upsert_sir_meta(aether_store::SirMetaRecord {
            staleness_score: Some(row.final_score),
            ..meta
        })?;
    }
    Ok(())
}

fn bands_from_scores(scores: &[f64]) -> ScoreBands {
    let mut bands = ScoreBands::default();
    for score in scores {
        if *score >= 0.8 {
            bands.critical += 1;
        } else if *score >= 0.5 {
            bands.high += 1;
        } else if *score >= 0.2 {
            bands.medium += 1;
        } else {
            bands.low += 1;
        }
    }
    bands
}

fn status_path(workspace: &Path) -> PathBuf {
    aether_dir(workspace).join("continuous").join("status.json")
}

fn write_status_snapshot(workspace: &Path, snapshot: &ContinuousStatusSnapshot) -> Result<()> {
    let path = status_path(workspace);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(snapshot)?;
    fs::write(&path, raw).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn record_failure_snapshot(workspace: &Path, started_at: i64, error: String) -> Result<()> {
    let mut snapshot = load_status_snapshot(workspace)?.unwrap_or_default();
    snapshot.last_started_at = Some(started_at);
    snapshot.last_completed_at = Some(unix_timestamp_secs());
    snapshot.last_error = Some(error);
    write_status_snapshot(workspace, &snapshot)
}

fn file_was_modified_after(workspace: &Path, file_path: &str, cutoff_secs: i64) -> bool {
    let full_path = workspace.join(file_path);
    fs::metadata(full_path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64 > cutoff_secs)
        .unwrap_or(false)
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use tempfile::tempdir;

    use super::{
        ContinuousStatusSnapshot, bands_from_scores, load_dependency_graph, load_status_snapshot,
        propagate_neighbor_staleness, write_status_snapshot,
    };

    #[test]
    fn propagation_walks_reverse_edges_with_decay_and_cutoff() {
        let edges = vec![
            aether_store::GraphDependencyEdgeRecord {
                source_symbol_id: "a".to_owned(),
                target_symbol_id: "b".to_owned(),
                edge_kind: "calls".to_owned(),
            },
            aether_store::GraphDependencyEdgeRecord {
                source_symbol_id: "c".to_owned(),
                target_symbol_id: "a".to_owned(),
                edge_kind: "calls".to_owned(),
            },
        ];
        let (graph, node_map) = load_dependency_graph(&edges);
        let induced = propagate_neighbor_staleness(
            &graph,
            &node_map,
            &HashMap::from([("b".to_owned(), 1.0)]),
            &HashMap::from([("b".to_owned(), 1.0)]),
            0.5,
            0.1,
        );

        assert_eq!(induced.get("a").copied(), Some(0.5));
        assert_eq!(induced.get("c").copied(), Some(0.25));
    }

    #[test]
    fn status_snapshot_round_trips() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join(".aether")).expect("create .aether");
        let snapshot = ContinuousStatusSnapshot {
            total_symbols: 10,
            symbols_with_sir: 8,
            scored_symbols: 8,
            ..ContinuousStatusSnapshot::default()
        };

        write_status_snapshot(temp.path(), &snapshot).expect("write status snapshot");
        let loaded = load_status_snapshot(temp.path())
            .expect("load status snapshot")
            .expect("snapshot present");
        assert_eq!(loaded, snapshot);
    }

    #[test]
    fn score_bands_bucket_scores() {
        let bands = bands_from_scores(&[0.95, 0.7, 0.3, 0.05]);
        assert_eq!(bands.critical, 1);
        assert_eq!(bands.high, 1);
        assert_eq!(bands.medium, 1);
        assert_eq!(bands.low, 1);
    }
}
