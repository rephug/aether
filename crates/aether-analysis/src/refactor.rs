use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::load_workspace_config;
use aether_core::{GitContext, Symbol, content_hash, normalize_path};
use aether_health::{RefactorSelectionInput, select_refactor_targets};
use aether_infer::{
    EmbeddingProvider, EmbeddingProviderOverrides, EmbeddingPurpose,
    load_embedding_provider_from_config, sir_prompt::SirEnrichmentContext,
};
use aether_parse::{SymbolExtractor, language_for_path};
use aether_sir::{
    FileSir, SirAnnotation, canonicalize_sir_json, synthetic_file_sir_id, validate_sir,
};
use aether_store::{
    IntentSnapshot, SirStateStore, SnapshotEntry, SnapshotStore, SqliteStore, TestIntentStore,
};
use serde::{Deserialize, Serialize};

use crate::coupling::AnalysisError;
use crate::graph_algorithms::{
    GraphAlgorithmEdge, betweenness_centrality, page_rank, strongly_connected_components,
};
use crate::{HealthAnalyzer, HealthInclude, HealthReport, HealthRequest};

const DEFAULT_HEALTH_LIMIT: u32 = 200;
const DEFAULT_MAX_NEIGHBORS: usize = 8;
const DEFAULT_PRIORITY_THRESHOLD: f64 = 0.5;
const DEFAULT_CONFIDENCE_THRESHOLD: f64 = 0.6;
const SIR_GENERATION_PASS_DEEP: &str = "deep";
const SIR_GENERATION_PASS_SCAN: &str = "scan";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefactorScope {
    File { path: String },
    Crate { name: String },
}

impl RefactorScope {
    pub fn label(&self) -> String {
        match self {
            Self::File { path } => format!("file:{}", normalize_path(path)),
            Self::Crate { name } => format!("crate:{}", name.trim()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefactorPreparationRequest {
    pub scope: RefactorScope,
    pub top_n: usize,
}

#[derive(Debug, Clone)]
pub struct PreparedRefactorCandidate {
    pub symbol: Symbol,
    pub refactor_risk: f64,
    pub risk_factors: Vec<String>,
    pub needs_deep_scan: bool,
    pub in_cycle: bool,
    pub enrichment: SirEnrichmentContext,
    pub current_generation_pass: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefactorPreparation {
    pub scope: RefactorScope,
    pub scope_label: String,
    pub scope_symbols: Vec<Symbol>,
    pub candidates: Vec<PreparedRefactorCandidate>,
    pub forced_cycle_members: usize,
    pub skipped_fresh: usize,
    pub health_report: HealthReport,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSymbolSummary {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentVerificationEntry {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub similarity: f64,
    pub threshold: f64,
    pub passed: bool,
    pub method: String,
    pub issue: Option<String>,
    pub generation_pass: Option<String>,
    pub was_deep_scanned: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyIntentReport {
    pub snapshot_id: String,
    pub scope: String,
    pub threshold: f64,
    pub passed: bool,
    pub compared_entries: usize,
    pub failed_entries: usize,
    pub disappeared_symbols: Vec<IntentSymbolSummary>,
    pub new_symbols: Vec<IntentSymbolSummary>,
    pub entries: Vec<IntentVerificationEntry>,
    pub used_embeddings: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
struct ResolvedRefactorScope {
    scope: RefactorScope,
    symbols: Vec<Symbol>,
}

#[derive(Debug, Clone)]
struct ScopeSymbolMetrics {
    symbol: Symbol,
    risk_score: f64,
    pagerank: f64,
    betweenness: f64,
    test_count: u32,
    risk_factors: Vec<String>,
    in_cycle: bool,
    has_fresh_deep_sir: bool,
    baseline_sir: Option<SirAnnotation>,
    current_generation_pass: Option<String>,
}

#[derive(Debug, Clone)]
struct HealthMetricsFallback {
    pagerank_scores: HashMap<String, f64>,
    betweenness_scores: HashMap<String, f64>,
    cycle_members: HashSet<String>,
}

pub fn prepare_refactor_prep(
    workspace: &Path,
    store: &SqliteStore,
    request: RefactorPreparationRequest,
) -> Result<RefactorPreparation, AnalysisError> {
    let resolved = resolve_scope_symbols(workspace, &request.scope)?;
    let top_n = request.top_n.max(1);
    let health_report = compute_health_report(workspace)?;
    let metrics = collect_scope_metrics(store, &resolved.symbols, &health_report)?;
    let priority_scores = metrics
        .iter()
        .map(|metric| (metric.symbol.id.clone(), metric.risk_score))
        .collect::<HashMap<_, _>>();
    let metrics_by_id = metrics
        .iter()
        .map(|metric| (metric.symbol.id.clone(), metric.clone()))
        .collect::<HashMap<_, _>>();
    let selection_inputs = metrics
        .iter()
        .map(|metric| RefactorSelectionInput {
            symbol_id: metric.symbol.id.clone(),
            qualified_name: metric.symbol.qualified_name.clone(),
            file_path: metric.symbol.file_path.clone(),
            risk_score: metric.risk_score,
            pagerank: metric.pagerank,
            betweenness: metric.betweenness,
            test_count: metric.test_count,
            risk_factors: metric.risk_factors.clone(),
            in_cycle: metric.in_cycle,
            has_fresh_deep_sir: metric.has_fresh_deep_sir,
        })
        .collect::<Vec<_>>();
    let selection = select_refactor_targets(&selection_inputs, top_n);
    let grouped_symbols = group_symbols_by_file(&resolved.symbols);
    let enrichment_settings = load_enrichment_settings(workspace)?;

    let mut candidates = Vec::with_capacity(selection.selected.len());
    for selected in selection.selected {
        let Some(metric) = metrics_by_id.get(selected.symbol_id.as_str()) else {
            continue;
        };
        let file_symbols = grouped_symbols
            .get(metric.symbol.file_path.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let enrichment = build_enrichment_context(
            store,
            EnrichmentBuildInput {
                symbol: &metric.symbol,
                file_symbols,
                priority_scores: &priority_scores,
                baseline_sir: metric.baseline_sir.clone(),
                priority_score: metric.risk_score,
                current_generation_pass: &metric.current_generation_pass,
                settings: &enrichment_settings,
            },
        )?;

        candidates.push(PreparedRefactorCandidate {
            symbol: metric.symbol.clone(),
            refactor_risk: selected.refactor_risk,
            risk_factors: selected.risk_factors,
            needs_deep_scan: selected.needs_deep_scan,
            in_cycle: selected.in_cycle,
            enrichment,
            current_generation_pass: metric.current_generation_pass.clone(),
        });
    }

    let mut notes = health_report.notes.clone();
    if candidates.is_empty() {
        notes.push("No in-scope symbols qualified for refactor-prep selection.".to_owned());
    } else if selection.skipped_fresh > 0 {
        notes.push(format!(
            "{} selected symbols already have a fresh deep SIR and will skip the deep pass.",
            selection.skipped_fresh
        ));
    }

    Ok(RefactorPreparation {
        scope: resolved.scope.clone(),
        scope_label: resolved.scope.label(),
        scope_symbols: resolved.symbols,
        candidates,
        forced_cycle_members: selection.forced_cycle_members,
        skipped_fresh: selection.skipped_fresh,
        health_report,
        notes,
    })
}

pub fn collect_intent_snapshot(
    workspace: &Path,
    store: &SqliteStore,
    scope: &RefactorScope,
    scope_symbols: &[Symbol],
    deep_scanned_symbol_ids: &HashSet<String>,
) -> Result<IntentSnapshot, AnalysisError> {
    let git_commit = GitContext::open(workspace)
        .and_then(|context| context.head_commit_hash())
        .unwrap_or_else(|| "unknown".to_owned());
    let created_at = unix_timestamp_secs();
    let scope_label = scope.label();
    let snapshot_material = format!("{git_commit}\n{scope_label}\n{created_at}");
    let snapshot_hash = content_hash(snapshot_material.as_str());
    let snapshot_id = format!("refactor-prep-{}", &snapshot_hash[..16]);

    let mut entries = Vec::with_capacity(scope_symbols.len());
    for symbol in scope_symbols {
        let Some(sir_json) = store.read_sir_blob(symbol.id.as_str())? else {
            return Err(AnalysisError::Message(format!(
                "cannot create snapshot: missing SIR for {} ({})",
                symbol.qualified_name, symbol.id
            )));
        };
        let sir = parse_valid_sir(symbol.id.as_str(), sir_json.as_str())?;
        let canonical = canonicalize_sir_json(&sir);
        let generation_pass = store
            .get_sir_meta(symbol.id.as_str())?
            .map(|meta| normalize_generation_pass(meta.generation_pass.as_str()))
            .unwrap_or_else(|| SIR_GENERATION_PASS_SCAN.to_owned());

        entries.push(SnapshotEntry {
            symbol_id: symbol.id.clone(),
            qualified_name: symbol.qualified_name.clone(),
            file_path: symbol.file_path.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            sir_json: canonical,
            generation_pass,
            was_deep_scanned: deep_scanned_symbol_ids.contains(symbol.id.as_str()),
        });
    }

    Ok(IntentSnapshot {
        snapshot_id,
        git_commit,
        created_at,
        scope: scope_label,
        symbol_count: entries.len(),
        deep_count: entries
            .iter()
            .filter(|entry| entry.was_deep_scanned)
            .count(),
        symbols: entries,
    })
}

pub fn verify_intent_snapshot(
    workspace: &Path,
    store: &SqliteStore,
    snapshot_id: &str,
    threshold: f64,
) -> Result<VerifyIntentReport, AnalysisError> {
    let Some(snapshot) = store.get_snapshot(snapshot_id)? else {
        return Err(AnalysisError::Message(format!(
            "snapshot '{snapshot_id}' was not found"
        )));
    };
    let scope = parse_snapshot_scope(snapshot.scope.as_str())?;
    let resolved = resolve_scope_symbols(workspace, &scope)?;
    let current_by_id = resolved
        .symbols
        .into_iter()
        .map(|symbol| (symbol.id.clone(), symbol))
        .collect::<BTreeMap<_, _>>();
    let snapshot_by_id = snapshot
        .symbols
        .iter()
        .map(|entry| (entry.symbol_id.clone(), entry))
        .collect::<BTreeMap<_, _>>();

    let mut engine = SimilarityEngine::new(workspace)?;
    let mut entries = Vec::new();
    for snapshot_entry in &snapshot.symbols {
        let Some(current_symbol) = current_by_id.get(snapshot_entry.symbol_id.as_str()) else {
            continue;
        };
        let current_blob = store.read_sir_blob(current_symbol.id.as_str())?;
        let current_generation_pass = store
            .get_sir_meta(current_symbol.id.as_str())?
            .map(|meta| normalize_generation_pass(meta.generation_pass.as_str()));

        let (similarity, method, issue) = match current_blob {
            Some(blob) => engine.compare(snapshot_entry.sir_json.as_str(), blob.as_str()),
            None => (
                0.0,
                "missing_current_sir".to_owned(),
                Some("missing_current_sir".to_owned()),
            ),
        };
        let passed = similarity >= threshold && issue.is_none();
        entries.push(IntentVerificationEntry {
            symbol_id: snapshot_entry.symbol_id.clone(),
            qualified_name: current_symbol.qualified_name.clone(),
            file_path: current_symbol.file_path.clone(),
            similarity,
            threshold,
            passed,
            method,
            issue,
            generation_pass: current_generation_pass,
            was_deep_scanned: snapshot_entry.was_deep_scanned,
        });
    }

    entries.sort_by(|left, right| {
        left.file_path
            .cmp(&right.file_path)
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });

    let disappeared_symbols = snapshot
        .symbols
        .iter()
        .filter(|entry| !current_by_id.contains_key(entry.symbol_id.as_str()))
        .map(|entry| IntentSymbolSummary {
            symbol_id: entry.symbol_id.clone(),
            qualified_name: entry.qualified_name.clone(),
            file_path: entry.file_path.clone(),
        })
        .collect::<Vec<_>>();
    let new_symbols = current_by_id
        .values()
        .filter(|symbol| !snapshot_by_id.contains_key(symbol.id.as_str()))
        .map(|symbol| IntentSymbolSummary {
            symbol_id: symbol.id.clone(),
            qualified_name: symbol.qualified_name.clone(),
            file_path: symbol.file_path.clone(),
        })
        .collect::<Vec<_>>();
    let failed_entries = entries.iter().filter(|entry| !entry.passed).count();
    let passed = failed_entries == 0 && disappeared_symbols.is_empty() && new_symbols.is_empty();

    Ok(VerifyIntentReport {
        snapshot_id: snapshot.snapshot_id,
        scope: snapshot.scope,
        threshold,
        passed,
        compared_entries: entries.len(),
        failed_entries,
        disappeared_symbols,
        new_symbols,
        entries,
        used_embeddings: engine.used_embeddings,
        notes: engine.notes,
    })
}

fn compute_health_report(workspace: &Path) -> Result<HealthReport, AnalysisError> {
    let analyzer = HealthAnalyzer::new(workspace)?;
    let request = HealthRequest {
        include: vec![
            HealthInclude::CriticalSymbols,
            HealthInclude::Bottlenecks,
            HealthInclude::Cycles,
            HealthInclude::RiskHotspots,
        ],
        limit: DEFAULT_HEALTH_LIMIT,
        min_risk: 0.0,
    };
    run_with_runtime(async move { analyzer.analyze(&request).await })
}

fn collect_scope_metrics(
    store: &SqliteStore,
    scope_symbols: &[Symbol],
    health_report: &HealthReport,
) -> Result<Vec<ScopeSymbolMetrics>, AnalysisError> {
    let fallback = collect_graph_metrics(store);
    let max_pagerank = fallback
        .pagerank_scores
        .values()
        .copied()
        .fold(0.0_f64, f64::max);
    let max_betweenness = fallback
        .betweenness_scores
        .values()
        .copied()
        .fold(0.0_f64, f64::max);
    let report_critical_by_id = health_report
        .critical_symbols
        .iter()
        .map(|entry| (entry.symbol_id.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let mut report_risk_factors = HashMap::<String, Vec<String>>::new();
    for entry in &health_report.risk_hotspots {
        report_risk_factors.insert(entry.symbol_id.clone(), entry.risk_factors.clone());
    }
    let report_cycle_members = health_report
        .cycles
        .iter()
        .flat_map(|cycle| cycle.symbols.iter().map(|symbol| symbol.id.clone()))
        .collect::<HashSet<_>>();

    let mut metrics = Vec::with_capacity(scope_symbols.len());
    for symbol in scope_symbols {
        let test_count = store
            .list_test_intents_for_symbol(symbol.id.as_str())?
            .len() as u32;
        let baseline_sir = store
            .read_sir_blob(symbol.id.as_str())?
            .map(|blob| parse_valid_sir(symbol.id.as_str(), blob.as_str()))
            .transpose()?;
        let current_meta = store.get_sir_meta(symbol.id.as_str())?;
        let current_generation_pass = current_meta
            .as_ref()
            .map(|meta| normalize_generation_pass(meta.generation_pass.as_str()));
        let in_cycle = report_cycle_members.contains(symbol.id.as_str())
            || fallback.cycle_members.contains(symbol.id.as_str());
        let has_fresh_deep_sir = baseline_sir.is_some()
            && current_meta.as_ref().is_some_and(|meta| {
                normalize_generation_pass(meta.generation_pass.as_str()) == SIR_GENERATION_PASS_DEEP
            });
        let pagerank = report_critical_by_id
            .get(symbol.id.as_str())
            .map(|entry| entry.pagerank)
            .unwrap_or_else(|| {
                fallback
                    .pagerank_scores
                    .get(symbol.id.as_str())
                    .copied()
                    .unwrap_or(0.0)
            });
        let betweenness = report_critical_by_id
            .get(symbol.id.as_str())
            .map(|entry| entry.betweenness)
            .unwrap_or_else(|| {
                fallback
                    .betweenness_scores
                    .get(symbol.id.as_str())
                    .copied()
                    .unwrap_or(0.0)
            });

        let (fallback_risk, mut fallback_factors) = fallback_risk_score(
            pagerank,
            max_pagerank,
            betweenness,
            max_betweenness,
            test_count,
            in_cycle,
            baseline_sir.is_some(),
        );
        let risk_score = report_critical_by_id
            .get(symbol.id.as_str())
            .map(|entry| entry.risk_score)
            .unwrap_or(fallback_risk);

        if let Some(report_factors) = report_critical_by_id
            .get(symbol.id.as_str())
            .map(|entry| entry.risk_factors.clone())
            .or_else(|| report_risk_factors.get(symbol.id.as_str()).cloned())
        {
            fallback_factors = merge_risk_factors(report_factors, fallback_factors);
        }

        metrics.push(ScopeSymbolMetrics {
            symbol: symbol.clone(),
            risk_score,
            pagerank,
            betweenness,
            test_count,
            risk_factors: fallback_factors,
            in_cycle,
            has_fresh_deep_sir,
            baseline_sir,
            current_generation_pass,
        });
    }

    metrics.sort_by(|left, right| {
        left.symbol
            .file_path
            .cmp(&right.symbol.file_path)
            .then_with(|| left.symbol.qualified_name.cmp(&right.symbol.qualified_name))
            .then_with(|| left.symbol.id.cmp(&right.symbol.id))
    });
    Ok(metrics)
}

fn collect_graph_metrics(store: &SqliteStore) -> HealthMetricsFallback {
    let Ok(edges) = store.list_graph_dependency_edges() else {
        return HealthMetricsFallback {
            pagerank_scores: HashMap::new(),
            betweenness_scores: HashMap::new(),
            cycle_members: HashSet::new(),
        };
    };
    let algo_edges = edges
        .into_iter()
        .map(|edge| GraphAlgorithmEdge {
            source_id: edge.source_symbol_id,
            target_id: edge.target_symbol_id,
            edge_kind: edge.edge_kind,
        })
        .collect::<Vec<_>>();
    if algo_edges.is_empty() {
        return HealthMetricsFallback {
            pagerank_scores: HashMap::new(),
            betweenness_scores: HashMap::new(),
            cycle_members: HashSet::new(),
        };
    }

    let pagerank_scores = page_rank(&algo_edges, 0.85, 20);
    let betweenness_scores = betweenness_centrality(&algo_edges)
        .into_iter()
        .collect::<HashMap<_, _>>();
    let cycle_members = strongly_connected_components(&algo_edges)
        .into_iter()
        .filter(|component| component.len() > 1)
        .flatten()
        .collect::<HashSet<_>>();

    HealthMetricsFallback {
        pagerank_scores,
        betweenness_scores,
        cycle_members,
    }
}

fn fallback_risk_score(
    pagerank: f64,
    max_pagerank: f64,
    betweenness: f64,
    max_betweenness: f64,
    test_count: u32,
    in_cycle: bool,
    has_sir: bool,
) -> (f64, Vec<String>) {
    let pagerank_norm = normalize_signal(pagerank, max_pagerank);
    let betweenness_norm = normalize_signal(betweenness, max_betweenness);
    let test_gap = if test_count == 0 {
        1.0
    } else {
        (1.0 / (test_count as f64 + 1.0)).clamp(0.0, 1.0)
    };
    let mut risk_factors = Vec::new();
    if in_cycle {
        risk_factors.push("cycle_member".to_owned());
    }
    if pagerank_norm >= 0.6 {
        risk_factors.push("high_pagerank".to_owned());
    }
    if betweenness_norm >= 0.6 {
        risk_factors.push("high_betweenness".to_owned());
    }
    if test_count == 0 {
        risk_factors.push("missing_test_coverage".to_owned());
    }
    if !has_sir {
        risk_factors.push("missing_sir".to_owned());
    }

    let mut risk = pagerank_norm * 0.35
        + betweenness_norm * 0.35
        + test_gap * 0.2
        + if in_cycle { 0.1 } else { 0.0 };
    if !has_sir {
        risk += 0.1;
    }
    (risk.clamp(0.0, 1.0), risk_factors)
}

struct EnrichmentBuildInput<'a> {
    symbol: &'a Symbol,
    file_symbols: &'a [Symbol],
    priority_scores: &'a HashMap<String, f64>,
    baseline_sir: Option<SirAnnotation>,
    priority_score: f64,
    current_generation_pass: &'a Option<String>,
    settings: &'a EnrichmentSettings,
}

fn build_enrichment_context(
    store: &SqliteStore,
    input: EnrichmentBuildInput<'_>,
) -> Result<SirEnrichmentContext, AnalysisError> {
    let EnrichmentBuildInput {
        symbol,
        file_symbols,
        priority_scores,
        baseline_sir,
        priority_score,
        current_generation_pass,
        settings,
    } = input;
    let file_rollup_id = synthetic_file_sir_id(symbol.language.as_str(), symbol.file_path.as_str());
    let file_intent = store
        .read_sir_blob(file_rollup_id.as_str())?
        .and_then(|blob| serde_json::from_str::<FileSir>(&blob).ok())
        .map(|sir| sir.intent.trim().to_owned())
        .filter(|value| !value.is_empty());

    let mut neighbors = file_symbols
        .iter()
        .filter(|peer| peer.id != symbol.id)
        .filter_map(|peer| {
            let blob = store.read_sir_blob(peer.id.as_str()).ok().flatten()?;
            let peer_sir = serde_json::from_str::<SirAnnotation>(&blob).ok()?;
            validate_sir(&peer_sir).ok()?;
            Some((
                priority_scores
                    .get(peer.id.as_str())
                    .copied()
                    .unwrap_or(0.0),
                peer.qualified_name.clone(),
                peer_sir.intent,
            ))
        })
        .collect::<Vec<_>>();
    neighbors.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    if settings.max_neighbors > 0 && neighbors.len() > settings.max_neighbors {
        neighbors.truncate(settings.max_neighbors);
    }

    Ok(SirEnrichmentContext {
        file_intent,
        neighbor_intents: neighbors
            .into_iter()
            .map(|(_, name, intent)| (name, intent))
            .collect(),
        baseline_sir: baseline_sir.clone(),
        priority_reason: format_priority_reason(
            store,
            symbol.id.as_str(),
            priority_score,
            baseline_sir
                .as_ref()
                .map(|sir| f64::from(sir.confidence.clamp(0.0, 1.0)))
                .unwrap_or(0.0),
            settings.priority_threshold,
            settings.confidence_threshold,
            current_generation_pass.as_deref(),
        ),
    })
}

fn load_enrichment_settings(workspace: &Path) -> Result<EnrichmentSettings, AnalysisError> {
    let config = load_workspace_config(workspace)?;
    Ok(EnrichmentSettings {
        max_neighbors: config
            .sir_quality
            .deep_max_neighbors
            .max(DEFAULT_MAX_NEIGHBORS),
        priority_threshold: config
            .sir_quality
            .deep_priority_threshold
            .clamp(0.0, 1.0)
            .max(DEFAULT_PRIORITY_THRESHOLD.min(1.0)),
        confidence_threshold: config
            .sir_quality
            .deep_confidence_threshold
            .clamp(0.0, 1.0)
            .max(DEFAULT_CONFIDENCE_THRESHOLD.min(1.0)),
    })
}

#[derive(Debug, Clone, Copy)]
struct EnrichmentSettings {
    max_neighbors: usize,
    priority_threshold: f64,
    confidence_threshold: f64,
}

fn format_priority_reason(
    store: &SqliteStore,
    symbol_id: &str,
    priority_score: f64,
    confidence: f64,
    priority_threshold: f64,
    confidence_threshold: f64,
    generation_pass: Option<&str>,
) -> String {
    let mut reasons = Vec::<String>::new();
    if priority_score >= priority_threshold {
        reasons.push(format!(
            "priority {:.2} at or above threshold {:.2}",
            priority_score, priority_threshold
        ));
    }
    if confidence <= confidence_threshold {
        reasons.push(format!(
            "baseline confidence {:.2} at or below threshold {:.2}",
            confidence, confidence_threshold
        ));
    }
    if generation_pass == Some(SIR_GENERATION_PASS_SCAN) {
        reasons.push("only scan-pass SIR exists".to_owned());
    }

    if let Ok(Some(metadata)) = store.get_symbol_metadata(symbol_id) {
        if metadata.is_public {
            reasons.push("public API symbol".to_owned());
        }
        let kind = metadata.kind.to_ascii_lowercase();
        if kind == "function" || kind == "method" {
            reasons.push("function/method".to_owned());
        }
    }

    if reasons.is_empty() {
        "selected for deeper analysis".to_owned()
    } else {
        reasons.join(" + ")
    }
}

fn resolve_scope_symbols(
    workspace: &Path,
    scope: &RefactorScope,
) -> Result<ResolvedRefactorScope, AnalysisError> {
    match scope {
        RefactorScope::File { path } => {
            let absolute = resolve_workspace_file_path(workspace, path)?;
            let display_path = workspace_relative_display_path(workspace, absolute.as_path())?;
            let symbols = extract_symbols_for_file(absolute.as_path(), display_path.as_str())?;
            Ok(ResolvedRefactorScope {
                scope: RefactorScope::File { path: display_path },
                symbols,
            })
        }
        RefactorScope::Crate { name } => {
            let Some(crate_root) = find_crate_root_by_name(workspace, name)? else {
                return Err(AnalysisError::Message(format!(
                    "crate '{name}' was not found under {}",
                    workspace.display()
                )));
            };
            let src_root = crate_root.join("src");
            if !src_root.exists() {
                return Ok(ResolvedRefactorScope {
                    scope: RefactorScope::Crate {
                        name: name.trim().to_owned(),
                    },
                    symbols: Vec::new(),
                });
            }

            let mut files = Vec::new();
            collect_supported_source_files(&src_root, &mut files)?;
            let mut all_symbols = Vec::new();
            for file in files {
                let display_path = workspace_relative_display_path(workspace, file.as_path())?;
                let mut symbols = extract_symbols_for_file(file.as_path(), display_path.as_str())?;
                all_symbols.append(&mut symbols);
            }
            sort_symbols(&mut all_symbols);
            Ok(ResolvedRefactorScope {
                scope: RefactorScope::Crate {
                    name: name.trim().to_owned(),
                },
                symbols: all_symbols,
            })
        }
    }
}

fn parse_snapshot_scope(value: &str) -> Result<RefactorScope, AnalysisError> {
    let trimmed = value.trim();
    if let Some(path) = trimmed.strip_prefix("file:") {
        return Ok(RefactorScope::File {
            path: normalize_path(path.trim()),
        });
    }
    if let Some(name) = trimmed.strip_prefix("crate:") {
        let name = name.trim();
        if name.is_empty() {
            return Err(AnalysisError::Message(
                "snapshot scope 'crate:' is missing a crate name".to_owned(),
            ));
        }
        return Ok(RefactorScope::Crate {
            name: name.to_owned(),
        });
    }
    Err(AnalysisError::Message(format!(
        "unsupported snapshot scope '{trimmed}'"
    )))
}

fn resolve_workspace_file_path(
    workspace: &Path,
    file_path: &str,
) -> Result<PathBuf, AnalysisError> {
    let path = PathBuf::from(file_path);
    let joined = if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    };
    let absolute = joined.canonicalize()?;
    if !absolute.starts_with(workspace) {
        return Err(AnalysisError::Message(format!(
            "file path must stay under workspace {}",
            workspace.display()
        )));
    }
    Ok(absolute)
}

fn workspace_relative_display_path(
    workspace: &Path,
    absolute: &Path,
) -> Result<String, AnalysisError> {
    let relative = absolute.strip_prefix(workspace).map_err(|_| {
        AnalysisError::Message(format!(
            "path {} is not under workspace {}",
            absolute.display(),
            workspace.display()
        ))
    })?;
    Ok(normalize_path(&relative.to_string_lossy()))
}

fn extract_symbols_for_file(
    file_path: &Path,
    display_path: &str,
) -> Result<Vec<Symbol>, AnalysisError> {
    let language = language_for_path(file_path).ok_or_else(|| {
        AnalysisError::Message(format!(
            "unsupported file extension for {}",
            file_path.display()
        ))
    })?;
    let source = fs::read_to_string(file_path)?;
    let mut extractor =
        SymbolExtractor::new().map_err(|err| AnalysisError::Message(err.to_string()))?;
    let mut symbols = extractor
        .extract_from_source(language, display_path, &source)
        .map_err(|err| AnalysisError::Message(err.to_string()))?;
    sort_symbols(&mut symbols);
    Ok(symbols)
}

fn collect_supported_source_files(
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), AnalysisError> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_supported_source_files(path.as_path(), files)?;
            continue;
        }
        if entry.file_type()?.is_file() && language_for_path(path.as_path()).is_some() {
            files.push(path);
        }
    }
    files.sort();
    Ok(())
}

fn find_crate_root_by_name(
    workspace: &Path,
    crate_name: &str,
) -> Result<Option<PathBuf>, AnalysisError> {
    let mut stack = vec![workspace.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if matches!(name.as_str(), ".git" | ".aether" | "target") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !file_type.is_file() || entry.file_name() != "Cargo.toml" {
                continue;
            }
            let content = fs::read_to_string(&path)?;
            if cargo_package_name(content.as_str()).as_deref() == Some(crate_name.trim()) {
                return Ok(path.parent().map(Path::to_path_buf));
            }
        }
    }
    Ok(None)
}

fn cargo_package_name(content: &str) -> Option<String> {
    let mut in_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package || trimmed.starts_with('#') {
            continue;
        }
        let (key, value) = trimmed.split_once('=')?;
        if key.trim() == "name" {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

fn group_symbols_by_file(symbols: &[Symbol]) -> BTreeMap<String, Vec<Symbol>> {
    let mut grouped = BTreeMap::<String, Vec<Symbol>>::new();
    for symbol in symbols {
        grouped
            .entry(symbol.file_path.clone())
            .or_default()
            .push(symbol.clone());
    }
    for file_symbols in grouped.values_mut() {
        sort_symbols(file_symbols);
    }
    grouped
}

fn sort_symbols(symbols: &mut Vec<Symbol>) {
    symbols.sort_by(|left, right| {
        left.file_path
            .cmp(&right.file_path)
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
            .then_with(|| left.id.cmp(&right.id))
    });
    symbols.dedup_by(|left, right| left.id == right.id);
}

fn parse_valid_sir(symbol_id: &str, blob: &str) -> Result<SirAnnotation, AnalysisError> {
    let sir = serde_json::from_str::<SirAnnotation>(blob).map_err(|err| {
        AnalysisError::Message(format!("invalid SIR JSON for {symbol_id}: {err}"))
    })?;
    validate_sir(&sir).map_err(|err| {
        AnalysisError::Message(format!("invalid SIR annotation for {symbol_id}: {err}"))
    })?;
    Ok(sir)
}

fn normalize_generation_pass(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        SIR_GENERATION_PASS_SCAN.to_owned()
    } else {
        normalized
    }
}

fn normalize_signal(value: f64, max_value: f64) -> f64 {
    if max_value <= f64::EPSILON {
        0.0
    } else {
        (value / max_value).clamp(0.0, 1.0)
    }
}

fn merge_risk_factors(primary: Vec<String>, secondary: Vec<String>) -> Vec<String> {
    let mut merged = BTreeSet::new();
    for factor in primary.into_iter().chain(secondary) {
        let trimmed = factor.trim();
        if !trimmed.is_empty() {
            merged.insert(trimmed.to_owned());
        }
    }
    merged.into_iter().collect()
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn run_with_runtime<F, T>(future: F) -> Result<T, AnalysisError>
where
    F: Future<Output = Result<T, AnalysisError>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| AnalysisError::Message(format!("failed to build tokio runtime: {err}")))?;
    runtime.block_on(future)
}

struct SimilarityEngine {
    runtime: tokio::runtime::Runtime,
    provider: Option<Box<dyn EmbeddingProvider>>,
    notes: Vec<String>,
    used_embeddings: bool,
}

impl SimilarityEngine {
    fn new(workspace: &Path) -> Result<Self, AnalysisError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| {
                AnalysisError::Message(format!("failed to build tokio runtime: {err}"))
            })?;
        let mut notes = Vec::new();
        let provider = match load_embedding_provider_from_config(
            workspace,
            EmbeddingProviderOverrides::default(),
        ) {
            Ok(Some(loaded)) => Some(loaded.provider),
            Ok(None) => None,
            Err(err) => {
                notes.push(format!(
                    "embedding similarity unavailable, falling back to string diff: {err}"
                ));
                None
            }
        };

        Ok(Self {
            runtime,
            provider,
            notes,
            used_embeddings: false,
        })
    }

    fn compare(
        &mut self,
        snapshot_json: &str,
        current_json: &str,
    ) -> (f64, String, Option<String>) {
        if let Some(provider) = self.provider.as_ref()
            && let (Ok(snapshot_sir), Ok(current_sir)) = (
                serde_json::from_str::<SirAnnotation>(snapshot_json),
                serde_json::from_str::<SirAnnotation>(current_json),
            )
            && !snapshot_sir.intent.trim().is_empty()
            && !current_sir.intent.trim().is_empty()
        {
            let snapshot_intent = snapshot_sir.intent.clone();
            let current_intent = current_sir.intent.clone();
            let result = self.runtime.block_on(async {
                let left = provider
                    .embed_text_with_purpose(snapshot_intent.as_str(), EmbeddingPurpose::Document)
                    .await;
                let right = provider
                    .embed_text_with_purpose(current_intent.as_str(), EmbeddingPurpose::Document)
                    .await;
                match (left, right) {
                    (Ok(left), Ok(right)) if !left.is_empty() && !right.is_empty() => {
                        Ok(cosine_similarity(left.as_slice(), right.as_slice()) as f64)
                    }
                    (Ok(_), Ok(_)) => Err("received empty embedding vector".to_owned()),
                    (Err(err), _) | (_, Err(err)) => Err(err.to_string()),
                }
            });

            match result {
                Ok(similarity) => {
                    self.used_embeddings = true;
                    return (
                        similarity.clamp(0.0, 1.0),
                        "embedding_intent".to_owned(),
                        None,
                    );
                }
                Err(err) => {
                    let note = format!(
                        "embedding similarity failed for one or more entries, falling back to string diff: {err}"
                    );
                    if !self.notes.iter().any(|existing| existing == &note) {
                        self.notes.push(note);
                    }
                }
            }
        }

        (
            normalized_string_similarity(snapshot_json, current_json),
            "string_dice".to_owned(),
            None,
        )
    }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (left_value, right_value) in left.iter().zip(right.iter()) {
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    if left_norm <= f32::EPSILON || right_norm <= f32::EPSILON {
        return 0.0;
    }
    (dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(0.0, 1.0)
}

fn normalized_string_similarity(left: &str, right: &str) -> f64 {
    if left == right {
        return 1.0;
    }
    if left.trim().is_empty() || right.trim().is_empty() {
        return 0.0;
    }

    let left_grams = dice_grams(left);
    let right_grams = dice_grams(right);
    if left_grams.is_empty() || right_grams.is_empty() {
        return 0.0;
    }

    let mut left_counts = HashMap::<String, usize>::new();
    for gram in left_grams {
        *left_counts.entry(gram).or_insert(0) += 1;
    }
    let mut right_counts = HashMap::<String, usize>::new();
    for gram in right_grams {
        *right_counts.entry(gram).or_insert(0) += 1;
    }

    let intersection = left_counts
        .iter()
        .map(|(gram, left_count)| {
            right_counts
                .get(gram)
                .copied()
                .map(|right_count| (*left_count).min(right_count))
                .unwrap_or(0)
        })
        .sum::<usize>();
    let left_total = left_counts.values().sum::<usize>();
    let right_total = right_counts.values().sum::<usize>();

    ((2 * intersection) as f64 / (left_total + right_total) as f64).clamp(0.0, 1.0)
}

fn dice_grams(value: &str) -> Vec<String> {
    let normalized = value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    let chars = normalized.chars().collect::<Vec<_>>();
    if chars.len() < 2 {
        return Vec::new();
    }
    chars
        .windows(2)
        .map(|window| window.iter().collect::<String>())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;

    use super::{
        RefactorPreparationRequest, RefactorScope, collect_intent_snapshot, prepare_refactor_prep,
        verify_intent_snapshot,
    };
    use aether_core::{Language, SourceRange, Symbol, SymbolKind};
    use aether_store::{
        SirMetaRecord, SirStateStore, SnapshotStore, SqliteStore, SymbolCatalogStore, SymbolRecord,
    };
    use tempfile::tempdir;

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn write_demo_file(workspace: &Path) -> String {
        let relative = "crates/demo/src/lib.rs";
        let absolute = workspace.join(relative);
        fs::create_dir_all(absolute.parent().expect("parent")).expect("mkdirs");
        fs::write(
            &absolute,
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\npub fn sub(a: i32, b: i32) -> i32 { a - b }\n",
        )
        .expect("write source");
        fs::write(
            workspace.join("crates/demo/Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("write cargo");
        relative.to_owned()
    }

    fn symbol_record(symbol: &Symbol) -> SymbolRecord {
        SymbolRecord {
            id: symbol.id.clone(),
            file_path: symbol.file_path.clone(),
            language: symbol.language.as_str().to_owned(),
            kind: symbol.kind.as_str().to_owned(),
            qualified_name: symbol.qualified_name.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            last_seen_at: 1_700_000_000,
        }
    }

    fn seed_sir(store: &SqliteStore, symbol: &Symbol, intent: &str) {
        let sir_json = format!(
            "{{\"confidence\":0.95,\"dependencies\":[],\"error_modes\":[],\"inputs\":[],\"intent\":\"{intent}\",\"outputs\":[],\"side_effects\":[]}}"
        );
        store
            .write_sir_blob(symbol.id.as_str(), &sir_json)
            .expect("write sir");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol.id.clone(),
                sir_hash: format!("hash-{}", symbol.id),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                generation_pass: "deep".to_owned(),
                updated_at: 1_700_000_001,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_001,
            })
            .expect("upsert sir meta");
    }

    fn parse_demo_symbols(workspace: &Path, relative: &str) -> Vec<Symbol> {
        let absolute = workspace.join(relative);
        let source = fs::read_to_string(&absolute).expect("read source");
        let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
        extractor
            .extract_from_source(Language::Rust, relative, &source)
            .expect("parse symbols")
    }

    #[test]
    fn prepare_refactor_prep_resolves_file_scope_symbols() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_file(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        let symbols = parse_demo_symbols(temp.path(), &relative);
        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            seed_sir(&store, symbol, symbol.qualified_name.as_str());
        }

        let prep = prepare_refactor_prep(
            temp.path(),
            &store,
            RefactorPreparationRequest {
                scope: RefactorScope::File {
                    path: relative.clone(),
                },
                top_n: 2,
            },
        )
        .expect("prepare refactor prep");

        assert_eq!(prep.scope_label, format!("file:{relative}"));
        assert_eq!(prep.scope_symbols.len(), 2);
    }

    #[test]
    fn collect_intent_snapshot_records_deep_count() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_file(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        let symbols = parse_demo_symbols(temp.path(), &relative);
        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            seed_sir(&store, symbol, symbol.qualified_name.as_str());
        }

        let deep_ids = HashSet::from([symbols[0].id.clone()]);
        let snapshot = collect_intent_snapshot(
            temp.path(),
            &store,
            &RefactorScope::File {
                path: relative.clone(),
            },
            &symbols,
            &deep_ids,
        )
        .expect("collect snapshot");
        store.create_snapshot(&snapshot).expect("persist snapshot");

        assert_eq!(snapshot.symbol_count, 2);
        assert_eq!(snapshot.deep_count, 1);
        assert_eq!(snapshot.scope, format!("file:{relative}"));
    }

    #[test]
    fn verify_intent_snapshot_passes_when_unchanged() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_file(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        let symbols = parse_demo_symbols(temp.path(), &relative);
        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            seed_sir(&store, symbol, symbol.qualified_name.as_str());
        }
        let snapshot = collect_intent_snapshot(
            temp.path(),
            &store,
            &RefactorScope::File {
                path: relative.clone(),
            },
            &symbols,
            &HashSet::new(),
        )
        .expect("collect snapshot");
        let snapshot_id = snapshot.snapshot_id.clone();
        store.create_snapshot(&snapshot).expect("persist snapshot");

        let report = verify_intent_snapshot(temp.path(), &store, snapshot_id.as_str(), 0.85)
            .expect("verify");
        assert!(report.passed);
        assert_eq!(report.failed_entries, 0);
        assert!(report.disappeared_symbols.is_empty());
        assert!(report.new_symbols.is_empty());
    }

    #[test]
    fn verify_intent_snapshot_flags_similarity_failures() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_file(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        let symbols = parse_demo_symbols(temp.path(), &relative);
        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            seed_sir(&store, symbol, symbol.qualified_name.as_str());
        }
        let snapshot = collect_intent_snapshot(
            temp.path(),
            &store,
            &RefactorScope::File {
                path: relative.clone(),
            },
            &symbols,
            &HashSet::new(),
        )
        .expect("collect snapshot");
        let snapshot_id = snapshot.snapshot_id.clone();
        store.create_snapshot(&snapshot).expect("persist snapshot");

        let changed = format!(
            "{{\"confidence\":0.1,\"dependencies\":[\"sqlx\"],\"error_modes\":[\"panic\"],\"inputs\":[\"db\"],\"intent\":\"mutates persistent state with retries and failure handling\",\"outputs\":[\"result\"],\"side_effects\":[\"database\"]}}"
        );
        store
            .write_sir_blob(symbols[0].id.as_str(), &changed)
            .expect("rewrite sir");

        let report = verify_intent_snapshot(temp.path(), &store, snapshot_id.as_str(), 0.95)
            .expect("verify");
        assert!(!report.passed);
        assert!(report.failed_entries >= 1);
    }

    #[test]
    fn verify_intent_snapshot_reports_new_symbols() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_file(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        let mut symbols = parse_demo_symbols(temp.path(), &relative);
        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            seed_sir(&store, symbol, symbol.qualified_name.as_str());
        }
        let snapshot = collect_intent_snapshot(
            temp.path(),
            &store,
            &RefactorScope::File {
                path: relative.clone(),
            },
            &symbols,
            &HashSet::new(),
        )
        .expect("collect snapshot");
        let snapshot_id = snapshot.snapshot_id.clone();
        store.create_snapshot(&snapshot).expect("persist snapshot");

        fs::write(
            temp.path().join(&relative),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\npub fn sub(a: i32, b: i32) -> i32 { a - b }\n\npub fn mul(a: i32, b: i32) -> i32 { a * b }\n",
        )
        .expect("rewrite source");
        symbols = parse_demo_symbols(temp.path(), &relative);
        let new_symbol = symbols
            .iter()
            .find(|symbol| symbol.qualified_name.ends_with("mul"))
            .expect("new symbol");
        store
            .upsert_symbol(symbol_record(new_symbol))
            .expect("upsert new symbol");
        seed_sir(&store, new_symbol, new_symbol.qualified_name.as_str());

        let report = verify_intent_snapshot(temp.path(), &store, snapshot_id.as_str(), 0.85)
            .expect("verify");
        assert!(!report.passed);
        assert_eq!(report.new_symbols.len(), 1);
    }

    #[test]
    fn string_similarity_is_high_for_identical_strings() {
        let score =
            super::normalized_string_similarity("{\"intent\":\"same\"}", "{\"intent\":\"same\"}");
        assert_eq!(score, 1.0);
    }

    #[test]
    fn string_similarity_drops_for_distinct_strings() {
        let score = super::normalized_string_similarity(
            "{\"intent\":\"reads file\"}",
            "{\"intent\":\"mutates database\"}",
        );
        assert!(score < 0.85);
    }

    #[test]
    fn parse_snapshot_scope_rejects_unknown_format() {
        let err = super::parse_snapshot_scope("unknown").expect_err("expected error");
        assert!(err.to_string().contains("unsupported snapshot scope"));
    }

    #[test]
    fn collect_snapshot_fails_when_sir_is_missing() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let symbol = Symbol {
            id: "sym-a".to_owned(),
            language: Language::Rust,
            file_path: "src/lib.rs".to_owned(),
            kind: SymbolKind::Function,
            name: "demo".to_owned(),
            qualified_name: "demo".to_owned(),
            signature_fingerprint: "sig-a".to_owned(),
            content_hash: "hash-a".to_owned(),
            range: SourceRange {
                start: aether_core::Position { line: 1, column: 1 },
                end: aether_core::Position {
                    line: 1,
                    column: 10,
                },
                start_byte: Some(0),
                end_byte: Some(10),
            },
        };
        let store = SqliteStore::open(temp.path()).expect("open store");
        let err = collect_intent_snapshot(
            temp.path(),
            &store,
            &RefactorScope::File {
                path: "src/lib.rs".to_owned(),
            },
            &[symbol],
            &HashSet::new(),
        )
        .expect_err("expected missing sir error");
        assert!(err.to_string().contains("missing SIR"));
    }
}
