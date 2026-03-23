use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use aether_core::Symbol;
use aether_infer::sir_prompt::{
    SirEnrichmentContext, resolve_prompt_tier, sir_enriched_system_prompt,
    sir_enriched_user_prompt, sir_scan_system_prompt, sir_scan_user_prompt,
};
use aether_sir::{FileSir, SirAnnotation, synthetic_file_sir_id};
use aether_store::{GraphDependencyEdgeRecord, SirStateStore, SqliteStore};
use anyhow::{Context, Result, anyhow};

use crate::batch::hash::compute_prompt_hash;
use crate::batch::{BatchProvider, BatchRuntimeConfig, PassConfig};
use crate::cli::BatchPass;
use crate::observer::ObserverState;
use crate::sir_pipeline::build_job;

#[derive(Debug, Clone)]
pub(crate) struct BuildSummary {
    pub files: Vec<PathBuf>,
    pub written: usize,
    pub skipped: usize,
    pub unresolved_symbols: usize,
    /// Maps symbol_id → full batch key (`symbol_id|prompt_hash`) for providers
    /// that truncate the key in their custom_id field (e.g. Anthropic's 64-char limit).
    pub keymap: HashMap<String, String>,
}

pub(crate) fn snapshot_workspace_symbols(workspace: &Path) -> Result<HashMap<String, Symbol>> {
    let mut observer = ObserverState::new(workspace.to_path_buf())
        .context("failed to initialize batch symbol observer")?;
    observer
        .seed_from_disk()
        .context("failed to snapshot workspace symbols for batch build")?;

    let mut symbols_by_id = HashMap::new();
    for event in observer.initial_symbol_events() {
        for symbol in event.added.into_iter().chain(event.updated.into_iter()) {
            symbols_by_id.insert(symbol.id.clone(), symbol);
        }
    }
    Ok(symbols_by_id)
}

pub(crate) fn build_pass_jsonl(
    workspace: &Path,
    store: &SqliteStore,
    runtime: &BatchRuntimeConfig,
    pass_config: &PassConfig,
    symbols_by_id: &HashMap<String, Symbol>,
    contracts_enabled: bool,
    provider: &dyn BatchProvider,
) -> Result<BuildSummary> {
    build_pass_jsonl_for_ids(
        workspace,
        store,
        runtime,
        pass_config,
        symbols_by_id,
        None,
        contracts_enabled,
        provider,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_pass_jsonl_for_ids(
    workspace: &Path,
    store: &SqliteStore,
    runtime: &BatchRuntimeConfig,
    pass_config: &PassConfig,
    symbols_by_id: &HashMap<String, Symbol>,
    candidate_ids: Option<&[String]>,
    contracts_enabled: bool,
    provider: &dyn BatchProvider,
) -> Result<BuildSummary> {
    if pass_config.model.trim().is_empty() {
        return Err(anyhow!(
            "batch {} requires a model (set [batch].{}_model or pass --model)",
            pass_config.pass.as_str(),
            pass_config.pass.as_str()
        ));
    }

    fs::create_dir_all(&runtime.batch_dir).with_context(|| {
        format!(
            "failed to create batch output directory {}",
            runtime.batch_dir.display()
        )
    })?;

    // Resolve prompt tier and compute the static system prompt once for all symbols.
    let tier = resolve_prompt_tier(&pass_config.prompt_tier, provider.name());
    let system_prompt = match pass_config.pass {
        BatchPass::Scan => sir_scan_system_prompt(tier),
        BatchPass::Triage | BatchPass::Deep => sir_enriched_system_prompt(tier),
    };

    let provider_name = provider.name();

    let mut summary = BuildSummary {
        files: Vec::new(),
        written: 0,
        skipped: 0,
        unresolved_symbols: 0,
        keymap: HashMap::new(),
    };
    let symbol_ids = match candidate_ids {
        Some(ids) => ids.to_vec(),
        None => store
            .list_all_symbol_ids()
            .context("failed to list symbols for batch build")?,
    };
    let raw_edges = if matches!(pass_config.pass, BatchPass::Scan) {
        Vec::new()
    } else {
        store
            .list_graph_dependency_edges()
            .context("failed to list graph dependency edges for batch build")?
    };
    let graph = build_neighbor_graph(&raw_edges);
    let caller_contracts_map = if contracts_enabled && !matches!(pass_config.pass, BatchPass::Scan)
    {
        build_caller_contracts_map(store, &raw_edges, symbols_by_id)
    } else {
        HashMap::new()
    };

    let mut candidate_ids = Vec::new();
    for symbol_id in symbol_ids {
        if symbols_by_id.contains_key(symbol_id.as_str()) {
            candidate_ids.push(symbol_id);
        } else {
            summary.unresolved_symbols += 1;
            tracing::warn!(
                symbol_id = %symbol_id,
                "batch build skipped symbol missing from current workspace snapshot"
            );
        }
    }
    candidate_ids.sort();
    if runtime.max_symbols > 0 && candidate_ids.len() > runtime.max_symbols {
        let original_total = candidate_ids.len();
        candidate_ids.truncate(runtime.max_symbols);
        tracing::info!(
            pass = pass_config.pass.as_str(),
            max_symbols = runtime.max_symbols,
            original_total,
            truncated_total = candidate_ids.len(),
            "truncated batch build candidate set to symbol limit"
        );
    }

    let baseline_sirs = if matches!(pass_config.pass, BatchPass::Scan) {
        HashMap::new()
    } else {
        parse_sir_map(
            &store
                .list_sir_blobs_for_ids(&candidate_ids)
                .context("failed to prefetch baseline SIR blobs for batch build")?,
        )
    };

    let mut file_rollup_ids = HashSet::new();
    let mut neighbor_ids = HashSet::new();
    if !matches!(pass_config.pass, BatchPass::Scan) {
        for symbol_id in &candidate_ids {
            let Some(symbol) = symbols_by_id.get(symbol_id.as_str()) else {
                continue;
            };
            file_rollup_ids.insert(synthetic_file_sir_id(
                symbol.language.as_str(),
                symbol.file_path.as_str(),
            ));
            for neighbor_id in collect_neighbor_ids(&graph, symbol_id, pass_config.neighbor_depth) {
                neighbor_ids.insert(neighbor_id);
            }
        }
    }

    let file_intents = parse_file_intents(
        &store
            .list_sir_blobs_for_ids(&file_rollup_ids.into_iter().collect::<Vec<_>>())
            .context("failed to prefetch file rollup SIR blobs for batch build")?,
    );
    let neighbor_sirs = parse_sir_map(
        &store
            .list_sir_blobs_for_ids(&neighbor_ids.into_iter().collect::<Vec<_>>())
            .context("failed to prefetch neighbor SIR blobs for batch build")?,
    );

    let mut chunk_index = 0usize;
    let mut current_lines = 0usize;
    let mut writer = None::<BufWriter<File>>;
    for symbol_id in candidate_ids {
        let Some(symbol) = symbols_by_id.get(symbol_id.as_str()) else {
            continue;
        };
        let job = match build_job(workspace, symbol.clone(), None, Some(pass_config.max_chars)) {
            Ok(job) => job,
            Err(err) => {
                tracing::warn!(symbol_id = %symbol.id, error = %err, "failed to build batch SIR job");
                continue;
            }
        };

        // Build per-symbol user prompt and collect neighbor entries for hash.
        let (neighbor_entries, user_prompt) = match pass_config.pass {
            BatchPass::Scan => (
                Vec::new(),
                sir_scan_user_prompt(&job.symbol_text, &job.context),
            ),
            BatchPass::Triage | BatchPass::Deep => {
                let Some(baseline_sir) = baseline_sirs.get(symbol_id.as_str()).cloned() else {
                    tracing::warn!(
                        symbol_id = %symbol_id,
                        pass = pass_config.pass.as_str(),
                        "batch build skipped symbol without baseline SIR"
                    );
                    continue;
                };
                let enrichment = build_enrichment_context(
                    symbol,
                    pass_config,
                    &graph,
                    &file_intents,
                    &neighbor_sirs,
                    symbols_by_id,
                    baseline_sir,
                    &caller_contracts_map,
                );
                let include_cot = matches!(pass_config.pass, BatchPass::Deep);
                let user = sir_enriched_user_prompt(
                    &job.symbol_text,
                    &job.context,
                    &enrichment,
                    include_cot,
                );
                (enrichment.neighbor_intents, user)
            }
        };

        let neighbor_texts = neighbor_entries
            .iter()
            .map(|(_, intent)| intent.as_str())
            .collect::<Vec<_>>();
        let prompt_hash = compute_prompt_hash(
            job.symbol_text.as_str(),
            &neighbor_texts,
            pass_config.config_fingerprint(provider_name).as_str(),
        );
        let existing_hash = store
            .get_sir_meta(symbol_id.as_str())
            .with_context(|| format!("failed to read SIR metadata for {symbol_id}"))?
            .and_then(|record| record.prompt_hash);
        if existing_hash.as_deref() == Some(prompt_hash.as_str()) {
            summary.skipped += 1;
            continue;
        }

        if writer.is_none() || current_lines >= runtime.jsonl_chunk_size {
            chunk_index += 1;
            current_lines = 0;
            let file_path = runtime.batch_dir.join(format!(
                "{}-{:04}.jsonl",
                pass_config.pass.as_str(),
                chunk_index
            ));
            let file = File::create(&file_path).with_context(|| {
                format!("failed to create batch JSONL file {}", file_path.display())
            })?;
            writer = Some(BufWriter::new(file));
            summary.files.push(file_path);
        }

        let key_str = format!("{}|{}", symbol_id, prompt_hash);
        summary.keymap.insert(symbol_id.clone(), key_str.clone());
        let line = provider.format_request(
            &key_str,
            &system_prompt,
            &user_prompt,
            &pass_config.model,
            &pass_config.thinking,
        )?;
        let writer_ref = writer.as_mut().expect("writer initialized");
        writer_ref
            .write_all(line.as_bytes())
            .context("failed to write batch JSONL line")?;
        writer_ref
            .write_all(b"\n")
            .context("failed to terminate batch JSONL line")?;
        current_lines += 1;
        summary.written += 1;
    }

    if let Some(writer) = writer.as_mut() {
        writer
            .flush()
            .context("failed to flush batch JSONL output")?;
    }

    // Write keymap sidecar so ingest can recover full keys from providers that
    // truncate custom_id (e.g. Anthropic's 64-char limit).
    if !summary.keymap.is_empty() {
        let keymap_path = runtime
            .batch_dir
            .join(format!("{}.keymap.json", pass_config.pass.as_str()));
        let keymap_json =
            serde_json::to_string(&summary.keymap).context("failed to serialize batch keymap")?;
        fs::write(&keymap_path, keymap_json)
            .with_context(|| format!("failed to write batch keymap {}", keymap_path.display()))?;
    }

    Ok(summary)
}

fn build_neighbor_graph(edges: &[GraphDependencyEdgeRecord]) -> HashMap<String, BTreeSet<String>> {
    let mut graph = HashMap::<String, BTreeSet<String>>::new();
    for edge in edges {
        graph
            .entry(edge.source_symbol_id.clone())
            .or_default()
            .insert(edge.target_symbol_id.clone());
        graph
            .entry(edge.target_symbol_id.clone())
            .or_default()
            .insert(edge.source_symbol_id.clone());
    }
    graph
}

fn collect_neighbor_ids(
    graph: &HashMap<String, BTreeSet<String>>,
    symbol_id: &str,
    max_depth: u32,
) -> Vec<String> {
    if max_depth == 0 {
        return Vec::new();
    }

    let mut seen = HashSet::<String>::from([symbol_id.to_owned()]);
    let mut queue = VecDeque::<(String, u32)>::from([(symbol_id.to_owned(), 0)]);
    let mut ordered = Vec::new();
    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let Some(neighbors) = graph.get(current.as_str()) else {
            continue;
        };
        for neighbor in neighbors {
            if seen.insert(neighbor.clone()) {
                ordered.push(neighbor.clone());
                queue.push_back((neighbor.clone(), depth + 1));
            }
        }
    }

    ordered
}

fn parse_sir_map(blobs: &HashMap<String, String>) -> HashMap<String, SirAnnotation> {
    let mut parsed = HashMap::new();
    for (symbol_id, blob) in blobs {
        match serde_json::from_str::<SirAnnotation>(blob) {
            Ok(sir) => {
                parsed.insert(symbol_id.clone(), sir);
            }
            Err(err) => {
                tracing::warn!(symbol_id = %symbol_id, error = %err, "skipping invalid SIR blob during batch build");
            }
        }
    }
    parsed
}

fn parse_file_intents(blobs: &HashMap<String, String>) -> HashMap<String, String> {
    let mut parsed = HashMap::new();
    for (symbol_id, blob) in blobs {
        match serde_json::from_str::<FileSir>(blob) {
            Ok(file_sir) => {
                parsed.insert(symbol_id.clone(), file_sir.intent);
            }
            Err(err) => {
                tracing::warn!(symbol_id = %symbol_id, error = %err, "skipping invalid file rollup SIR during batch build");
            }
        }
    }
    parsed
}

#[allow(clippy::too_many_arguments)]
fn build_enrichment_context(
    symbol: &Symbol,
    pass_config: &PassConfig,
    graph: &HashMap<String, BTreeSet<String>>,
    file_intents: &HashMap<String, String>,
    neighbor_sirs: &HashMap<String, SirAnnotation>,
    symbols_by_id: &HashMap<String, Symbol>,
    baseline_sir: SirAnnotation,
    caller_contracts: &HashMap<String, Vec<(String, String, String)>>,
) -> SirEnrichmentContext {
    let file_rollup_id = synthetic_file_sir_id(symbol.language.as_str(), symbol.file_path.as_str());
    let mut neighbor_intents =
        collect_neighbor_ids(graph, symbol.id.as_str(), pass_config.neighbor_depth)
            .into_iter()
            .filter_map(|neighbor_id| {
                let neighbor_symbol = symbols_by_id.get(neighbor_id.as_str())?;
                let neighbor_sir = neighbor_sirs.get(neighbor_id.as_str())?;
                Some((
                    neighbor_symbol.qualified_name.clone(),
                    neighbor_sir.intent.clone(),
                ))
            })
            .collect::<Vec<_>>();
    neighbor_intents.sort_by(|left, right| left.0.cmp(&right.0));

    let caller_contract_clauses = caller_contracts
        .get(symbol.id.as_str())
        .cloned()
        .unwrap_or_default();

    SirEnrichmentContext {
        file_intent: file_intents.get(file_rollup_id.as_str()).cloned(),
        neighbor_intents,
        baseline_sir: Some(baseline_sir),
        priority_reason: format!(
            "Selected for batch {} regeneration",
            pass_config.pass.as_str()
        ),
        caller_contract_clauses,
    }
}

/// Build a map from target symbol ID to contract clauses imposed by its callers.
///
/// For each "calls" edge A→B, if A has active contracts, those clauses are
/// collected under B's symbol ID so B's enrichment prompt can reference them.
fn build_caller_contracts_map(
    store: &SqliteStore,
    edges: &[GraphDependencyEdgeRecord],
    symbols_by_id: &HashMap<String, Symbol>,
) -> HashMap<String, Vec<(String, String, String)>> {
    // Build inverted index: target_symbol_id → deduplicated set of caller symbol IDs
    let mut target_to_callers: HashMap<String, HashSet<String>> = HashMap::new();
    for edge in edges {
        if edge.edge_kind == "calls" {
            target_to_callers
                .entry(edge.target_symbol_id.clone())
                .or_default()
                .insert(edge.source_symbol_id.clone());
        }
    }

    let mut result: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for (target_id, caller_ids) in &target_to_callers {
        for caller_id in caller_ids {
            let contracts = match store.list_active_contracts_for_symbol(caller_id) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if contracts.is_empty() {
                continue;
            }
            let caller_name = symbols_by_id
                .get(caller_id.as_str())
                .map(|s| s.qualified_name.as_str())
                .unwrap_or(caller_id.as_str());
            let entry = result.entry(target_id.clone()).or_default();
            for contract in contracts {
                entry.push((
                    caller_name.to_owned(),
                    contract.clause_type,
                    contract.clause_text,
                ));
            }
        }
    }

    result
}
