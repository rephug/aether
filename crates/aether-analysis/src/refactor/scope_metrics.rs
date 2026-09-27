//! Per-symbol metrics of a refactor scope: health signals with a graph-derived
//! fallback, plus the SIR each candidate's deep pass is built from and bound to.

use super::*;

#[derive(Debug, Clone)]
pub(super) struct ScopeSymbolMetrics {
    pub(super) symbol: Symbol,
    pub(super) risk_score: f64,
    pub(super) pagerank: f64,
    pub(super) betweenness: f64,
    pub(super) test_count: u32,
    pub(super) risk_factors: Vec<String>,
    pub(super) in_cycle: bool,
    pub(super) has_fresh_deep_sir: bool,
    pub(super) baseline_sir: Option<SirAnnotation>,
    pub(super) baseline_sir_identity: Option<SirIdentity>,
    pub(super) current_generation_pass: Option<String>,
}

#[derive(Debug, Clone)]
struct HealthMetricsFallback {
    pagerank_scores: HashMap<String, f64>,
    betweenness_scores: HashMap<String, f64>,
    cycle_members: HashSet<String>,
}

pub(super) fn collect_scope_metrics(
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
        // Blob and metadata come from the one row that holds both, so the identity
        // recorded here is the identity of the SIR the enrichment is built from.
        let (current_meta, baseline_sir_identity, baseline_blob) =
            match store.get_sir_meta_with_blob(symbol.id.as_str())? {
                Some(row) => (Some(row.meta), Some(row.identity), row.blob),
                None => (None, None, None),
            };
        let baseline_sir = baseline_blob
            .map(|blob| parse_valid_sir(symbol.id.as_str(), blob.as_str()))
            .transpose()?;
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
            baseline_sir_identity,
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
