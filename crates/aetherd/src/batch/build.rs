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
use aether_store::{GraphDependencyEdgeRecord, SqliteStore};
use anyhow::{Context, Result, anyhow};

use crate::batch::hash::compute_prompt_hash;
use crate::batch::{BatchProvider, BatchRuntimeConfig, PassConfig};
use crate::cli::BatchPass;
use crate::observer::ObserverState;
use crate::sir_pipeline::{SirIdentity, build_job};

/// The keymap sidecar of one build: each request's provider key mapped to its full key
/// (see `BuildSummary::keymap`).
pub(crate) fn keymap_sidecar_name(pass: &str, build_id: &str) -> String {
    format!("{pass}.{build_id}.{KEYMAP_SIDECAR_KIND}.json")
}

/// The origin sidecar of one build: what each request was built from (see
/// `BuildSummary::origins`).
pub(crate) fn origin_sidecar_name(pass: &str, build_id: &str) -> String {
    format!("{pass}.{build_id}.{ORIGIN_SIDECAR_KIND}.json")
}

/// The sidecar kinds ingest looks for beside a pass's results: every
/// `<pass>.<build_id>.<kind>.json` in the directory (one per build, written once and
/// never modified) plus the pre-build-id `<pass>.<kind>.json` for legacy results.
pub(crate) const KEYMAP_SIDECAR_KIND: &str = "keymap";
pub(crate) const ORIGIN_SIDECAR_KIND: &str = "origin";

/// Remove one build's sidecars once every result of that build has been ingested, so
/// a directory used for repeated builds does not accumulate them. A sidecar that is
/// already gone is not an error.
pub(crate) fn remove_build_sidecars(batch_dir: &Path, pass: &str, build_id: &str) -> Result<()> {
    for name in [
        keymap_sidecar_name(pass, build_id),
        origin_sidecar_name(pass, build_id),
    ] {
        let path = batch_dir.join(name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to remove batch sidecar {}", path.display()));
            }
        }
    }
    Ok(())
}

/// A short identifier unique to one build of a pass, carried in its request keys.
fn new_build_id(pass: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let material = format!("{pass}:{nanos}:{}", std::process::id());
    aether_core::content_hash(material.as_str())
        .chars()
        .take(12)
        .collect()
}

/// Write a build's sidecar: serialized to a temporary file beside `path` and renamed
/// into place, so a reader never sees a partial file. Each build writes its own files
/// (the build id is in the name), so nothing is read, merged or overwritten.
fn write_sidecar<V: serde::Serialize>(
    path: &Path,
    entries: &HashMap<String, V>,
    what: &str,
) -> Result<()> {
    let json =
        serde_json::to_string(entries).with_context(|| format!("failed to serialize {what}"))?;
    let temp_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&temp_path, json)
        .with_context(|| format!("failed to write {what} {}", temp_path.display()))?;
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to publish {what} {} as {}",
            temp_path.display(),
            path.display()
        )
    })
}

#[derive(Debug, Clone)]
pub(crate) struct BuildSummary {
    pub files: Vec<PathBuf>,
    pub written: usize,
    pub skipped: usize,
    pub unresolved_symbols: usize,
    /// Maps symbol_id → full batch key (`symbol_id|prompt_hash`) for providers
    /// that truncate the key in their custom_id field (e.g. Anthropic's 64-char limit).
    /// Keyed by the provider's request key for each full key (`BatchProvider::request_key`).
    pub keymap: HashMap<String, String>,
    /// This build's identifier, carried in every request key (`symbol_id|prompt_hash|build_id`)
    /// and so in every result, so a result is matched to the sidecar entries of the
    /// build that produced it and never to a later build's for the same pass.
    pub build_id: String,
    /// What each request was built from, keyed by the full request key, so ingest can
    /// tell a result whose symbol was written or edited meanwhile and leave the newer
    /// state alone (see [`BatchRequestOrigin`]).
    pub origins: HashMap<String, BatchRequestOrigin>,
}

/// What one batch request was built from: the SIR the symbol held (`None`: no SIR)
/// and the content hash of the symbol source the prompt was built from. Ingest applies
/// a result only while the symbol still holds exactly that SIR and that source; a
/// symbol injected or edited meanwhile keeps its newer state (and, for an edit, the
/// daemon's regeneration from the new source is not pre-empted by a stale result).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BatchRequestOrigin {
    pub prior_sir: Option<SirIdentity>,
    pub source_hash: String,
}

pub(crate) fn snapshot_workspace_symbols(workspace: &Path) -> Result<HashMap<String, Symbol>> {
    let mut observer = ObserverState::new(workspace.to_path_buf())
        .context("failed to initialize batch symbol observer")?;
    observer
        .seed_from_disk()
        .context("failed to snapshot workspace symbols for batch build")?;

    let mut symbols_by_id = HashMap::new();
    for event in observer.initial_symbol_events() {
        for symbol in event.added.into_iter().chain(event.updated) {
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
        build_id: new_build_id(pass_config.pass.as_str()),
        origins: HashMap::new(),
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

        // The symbol's current SIR and its identity come from one row read before the
        // prompt is built: the prompt (for triage and deep passes) describes this SIR,
        // and ingest applies the result only while the symbol still holds exactly it.
        let (existing_meta, baseline_blob) = match store
            .get_sir_meta_with_blob(symbol_id.as_str())
            .with_context(|| format!("failed to read SIR state for {symbol_id}"))?
        {
            Some((meta, blob)) => (Some(meta), blob),
            None => (None, None),
        };

        // Build per-symbol user prompt and collect neighbor entries for hash.
        let (neighbor_entries, user_prompt) = match pass_config.pass {
            BatchPass::Scan => (
                Vec::new(),
                sir_scan_user_prompt(&job.symbol_text, &job.context),
            ),
            BatchPass::Triage | BatchPass::Deep => {
                let baseline_sir = baseline_blob
                    .as_deref()
                    .and_then(|blob| match serde_json::from_str::<SirAnnotation>(blob) {
                        Ok(sir) => Some(sir),
                        Err(err) => {
                            tracing::warn!(symbol_id = %symbol_id, error = %err, "skipping invalid SIR blob during batch build");
                            None
                        }
                    });
                let Some(baseline_sir) = baseline_sir else {
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
        let existing_hash = existing_meta
            .as_ref()
            .and_then(|record| record.prompt_hash.as_deref());
        if existing_hash == Some(prompt_hash.as_str()) {
            summary.skipped += 1;
            continue;
        }

        if writer.is_none() || current_lines >= runtime.jsonl_chunk_size {
            chunk_index += 1;
            current_lines = 0;
            // Named per build: a second build of the same pass in this directory must
            // not truncate a chunk an earlier run has yet to submit.
            let file_path = runtime.batch_dir.join(format!(
                "{}-{}-{:04}.jsonl",
                pass_config.pass.as_str(),
                summary.build_id,
                chunk_index
            ));
            let file = File::create(&file_path).with_context(|| {
                format!("failed to create batch JSONL file {}", file_path.display())
            })?;
            writer = Some(BufWriter::new(file));
            summary.files.push(file_path);
        }

        let key_str = format!("{}|{}|{}", symbol_id, prompt_hash, summary.build_id);
        summary
            .keymap
            .insert(provider.request_key(&key_str), key_str.clone());
        // The origin's source hash is of the text `build_job` actually read for the
        // prompt, not of the snapshot the symbol came from: an edit between the two is
        // then reflected in what ingest demands the file to hold, and a source that
        // returns to the snapshot's contents cannot make a prompt of the intermediate
        // body look current.
        summary.origins.insert(
            key_str.clone(),
            BatchRequestOrigin {
                prior_sir: existing_meta.as_ref().map(SirIdentity::of),
                source_hash: job.source_hash.clone(),
            },
        );
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

    // Write the keymap sidecar so ingest can recover full keys from providers that
    // truncate custom_id (e.g. Anthropic's 64-char limit), and the origin sidecar: the
    // SIR identity (hash and history version) and symbol source hash each request was
    // built from, which ingest compares under the inject lock right before persisting a
    // result, skipping results for symbols written or edited in the meantime. Each
    // build writes its own two files (named by build id, published atomically, never
    // modified afterwards), so concurrent builds of one pass cannot lose each other's
    // entries and a result of an earlier build always finds its own; the full run and
    // the continuous monitor remove a build's files once its results are ingested.
    if !summary.keymap.is_empty() {
        let keymap_path = runtime.batch_dir.join(keymap_sidecar_name(
            pass_config.pass.as_str(),
            summary.build_id.as_str(),
        ));
        write_sidecar(&keymap_path, &summary.keymap, "batch keymap")?;
    }
    if !summary.origins.is_empty() {
        let origin_path = runtime.batch_dir.join(origin_sidecar_name(
            pass_config.pass.as_str(),
            summary.build_id.as_str(),
        ));
        write_sidecar(&origin_path, &summary.origins, "batch origin sidecar")?;
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aether_store::SirIdentity;
    use tempfile::tempdir;

    use std::path::Path;

    use super::{new_build_id, origin_sidecar_name, remove_build_sidecars, write_sidecar};

    #[test]
    fn build_ids_are_short_and_distinct() {
        let first = new_build_id("triage");
        let second = new_build_id("triage");
        assert_eq!(first.len(), 12);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn each_build_publishes_its_own_sidecar_and_can_remove_it() {
        let temp = tempdir().expect("tempdir");
        let dir = temp.path();
        let first = HashMap::from([(
            "sym|p|build-1".to_owned(),
            Some(SirIdentity {
                sir_hash: "h1".to_owned(),
                sir_version: 1,
            }),
        )]);
        let second = HashMap::from([(
            "sym|p|build-2".to_owned(),
            Some(SirIdentity {
                sir_hash: "h2".to_owned(),
                sir_version: 2,
            }),
        )]);
        let first_path = dir.join(origin_sidecar_name("triage", "build-1"));
        let second_path = dir.join(origin_sidecar_name("triage", "build-2"));
        write_sidecar(&first_path, &first, "batch origin sidecar").expect("first build");
        write_sidecar(&second_path, &second, "batch origin sidecar").expect("second build");

        // Two builds of one pass leave two files, each holding only its own entries,
        // and no temporary file behind.
        let read = |path: &Path| -> HashMap<String, Option<SirIdentity>> {
            serde_json::from_str(&std::fs::read_to_string(path).expect("read sidecar"))
                .expect("parse sidecar")
        };
        assert_eq!(read(&first_path), first);
        assert_eq!(read(&second_path), second);
        let names = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("utf8")
            })
            .collect::<Vec<_>>();
        assert!(
            names.iter().all(|name| !name.contains(".tmp-")),
            "temporary files must be renamed away: {names:?}"
        );

        // Removing one build's sidecars leaves the other's, and is idempotent.
        remove_build_sidecars(dir, "triage", "build-1").expect("remove first");
        assert!(!first_path.exists());
        assert!(second_path.exists());
        remove_build_sidecars(dir, "triage", "build-1").expect("remove again");
    }
}
