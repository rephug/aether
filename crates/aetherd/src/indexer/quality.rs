//! The quality passes of a full index run: triage and deep regeneration of the
//! symbols the priority scores single out, each job bound to the SIR observed once.

use super::*;

#[derive(Debug, Clone)]
struct QualityPassCandidate {
    symbol: Symbol,
    priority_score: f64,
    baseline_sir: SirAnnotation,
    /// The identity of `baseline_sir`'s stored write, read from the same row.
    baseline_sir_identity: SirIdentity,
}

pub(super) fn run_triage_pass(
    config: &IndexerConfig,
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
    priority_scores: &HashMap<String, f64>,
    quality: &aether_config::SirQualityConfig,
    out: &mut dyn std::io::Write,
    contracts_enabled: bool,
) -> Result<()> {
    let eligible_candidates =
        collect_quality_pass_candidates(store, symbols_by_id, priority_scores, |pass| {
            pass == SIR_GENERATION_PASS_TRIAGE
                || pass == SIR_GENERATION_PASS_DEEP
                || pass == SIR_GENERATION_PASS_REGENERATED
        })?;
    let candidates = select_quality_pass_candidates(
        "Triage pass",
        eligible_candidates,
        quality.triage_priority_threshold,
        quality.triage_confidence_threshold,
        quality.triage_max_symbols,
    );
    if candidates.is_empty() {
        tracing::info!("Triage pass: 0 symbols selected");
        return Ok(());
    }

    let triage_provider = parse_quality_provider(
        quality.triage_provider.clone(),
        "sir_quality.triage_provider",
    )?
    .or(config.inference_provider);
    let triage_pipeline = SirPipeline::new(
        config.workspace.clone(),
        quality.triage_concurrency.max(1),
        ProviderOverrides {
            provider: triage_provider,
            model: quality
                .triage_model
                .clone()
                .or_else(|| config.inference_model.clone()),
            endpoint: quality
                .triage_endpoint
                .clone()
                .or_else(|| config.inference_endpoint.clone()),
            api_key_env: quality
                .triage_api_key_env
                .clone()
                .or_else(|| config.inference_api_key_env.clone()),
            thinking: quality.triage_thinking.clone(),
        },
    )
    .map(|pipeline| pipeline.with_inference_timeout_secs(quality.triage_timeout_secs))
    .context("failed to initialize triage-pass provider pipeline")?;
    run_quality_pass(
        "Triage pass",
        store,
        &triage_pipeline,
        candidates,
        priority_scores,
        quality.deep_max_neighbors,
        quality.triage_priority_threshold,
        quality.triage_confidence_threshold,
        config.print_sir,
        out,
        SIR_GENERATION_PASS_TRIAGE,
        false,
        contracts_enabled,
    )
}

pub(super) fn run_deep_pass(
    config: &IndexerConfig,
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
    priority_scores: &HashMap<String, f64>,
    quality: &aether_config::SirQualityConfig,
    out: &mut dyn std::io::Write,
    contracts_enabled: bool,
) -> Result<()> {
    let eligible_candidates =
        collect_quality_pass_candidates(store, symbols_by_id, priority_scores, |pass| {
            pass == SIR_GENERATION_PASS_DEEP || pass == SIR_GENERATION_PASS_REGENERATED
        })?;
    let candidates = select_quality_pass_candidates(
        "Deep pass",
        eligible_candidates,
        quality.deep_priority_threshold,
        quality.deep_confidence_threshold,
        quality.deep_max_symbols,
    );
    if candidates.is_empty() {
        tracing::info!("Deep pass: 0 symbols selected");
        return Ok(());
    }

    let deep_provider =
        parse_quality_provider(quality.deep_provider.clone(), "sir_quality.deep_provider")?
            .or(config.inference_provider);
    let deep_pipeline = SirPipeline::new(
        config.workspace.clone(),
        quality.deep_concurrency.max(1),
        ProviderOverrides {
            provider: deep_provider,
            model: quality
                .deep_model
                .clone()
                .or_else(|| config.inference_model.clone()),
            endpoint: quality
                .deep_endpoint
                .clone()
                .or_else(|| config.inference_endpoint.clone()),
            api_key_env: quality
                .deep_api_key_env
                .clone()
                .or_else(|| config.inference_api_key_env.clone()),
            thinking: quality.deep_thinking.clone(),
        },
    )
    .map(|pipeline| pipeline.with_inference_timeout_secs(quality.deep_timeout_secs))
    .context("failed to initialize deep-pass provider pipeline")?;
    run_quality_pass(
        "Deep pass",
        store,
        &deep_pipeline,
        candidates,
        priority_scores,
        quality.deep_max_neighbors,
        quality.deep_priority_threshold,
        quality.deep_confidence_threshold,
        config.print_sir,
        out,
        SIR_GENERATION_PASS_DEEP,
        true,
        contracts_enabled,
    )
}

fn collect_quality_pass_candidates<F>(
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
    priority_scores: &HashMap<String, f64>,
    should_skip_pass: F,
) -> Result<Vec<QualityPassCandidate>>
where
    F: Fn(&str) -> bool,
{
    let mut candidates = Vec::new();
    for symbol_id in store.list_all_symbol_ids()? {
        let Some(symbol) = symbols_by_id.get(symbol_id.as_str()) else {
            tracing::warn!(
                symbol_id = %symbol_id,
                "Quality pass symbol missing from initial snapshot; skipping"
            );
            continue;
        };
        // Metadata and blob from one row, so the identity recorded with the baseline
        // is the identity of the SIR the enrichment will be built from.
        let Some(row) = store.get_sir_meta_with_blob(symbol.id.as_str())? else {
            continue;
        };
        let (meta, identity, blob) = (row.meta, row.identity, row.blob);
        let pass = meta.generation_pass.to_ascii_lowercase();
        if should_skip_pass(pass.as_str()) {
            continue;
        }

        let Some(blob) = blob else {
            continue;
        };
        let baseline_sir = match serde_json::from_str::<SirAnnotation>(&blob) {
            Ok(sir) => sir,
            Err(err) => {
                tracing::warn!(
                    symbol_id = %symbol.id,
                    error = %err,
                    "failed to parse baseline SIR while selecting quality pass candidates"
                );
                continue;
            }
        };
        candidates.push(QualityPassCandidate {
            symbol: symbol.clone(),
            priority_score: priority_scores
                .get(symbol.id.as_str())
                .copied()
                .unwrap_or(0.0),
            baseline_sir,
            baseline_sir_identity: identity,
        });
    }

    Ok(candidates)
}

fn select_quality_pass_candidates(
    pass_label: &str,
    mut eligible_candidates: Vec<QualityPassCandidate>,
    priority_threshold: f64,
    confidence_threshold: f64,
    max_symbols: usize,
) -> Vec<QualityPassCandidate> {
    let mut candidates = eligible_candidates
        .iter()
        .filter(|candidate| {
            let low_confidence = (candidate.baseline_sir.confidence as f64) <= confidence_threshold;
            let high_priority = candidate.priority_score >= priority_threshold;
            high_priority || low_confidence
        })
        .cloned()
        .collect::<Vec<_>>();

    sort_quality_pass_candidates(&mut candidates);
    if candidates.is_empty() && max_symbols > 0 {
        sort_quality_pass_candidates(&mut eligible_candidates);
        candidates = eligible_candidates.into_iter().take(max_symbols).collect();
        tracing::info!(
            "{pass_label}: threshold selected 0, using top-{max_symbols} by priority as floor"
        );
    } else if max_symbols > 0 && candidates.len() > max_symbols {
        candidates.truncate(max_symbols);
    }

    candidates
}

fn sort_quality_pass_candidates(candidates: &mut [QualityPassCandidate]) {
    candidates.sort_by(|left, right| {
        right
            .priority_score
            .total_cmp(&left.priority_score)
            .then_with(|| left.symbol.id.cmp(&right.symbol.id))
    });
}

fn parse_quality_provider(
    provider_raw: Option<String>,
    field_name: &str,
) -> Result<Option<InferenceProviderKind>> {
    provider_raw
        .map(|provider_raw| {
            provider_raw
                .parse::<InferenceProviderKind>()
                .map_err(|error| anyhow::anyhow!("invalid {field_name} '{provider_raw}': {error}"))
        })
        .transpose()
}

#[allow(clippy::too_many_arguments)]
fn run_quality_pass(
    pass_label: &str,
    store: &SqliteStore,
    pipeline: &SirPipeline,
    candidates: Vec<QualityPassCandidate>,
    priority_scores: &HashMap<String, f64>,
    max_neighbors: usize,
    priority_threshold: f64,
    confidence_threshold: f64,
    print_sir: bool,
    out: &mut dyn std::io::Write,
    generation_pass: &str,
    use_cot: bool,
    contracts_enabled: bool,
) -> Result<()> {
    let use_cot = use_cot && pipeline.provider_name() == InferenceProviderKind::Qwen3Local.as_str();
    let total = candidates.len();
    let mut batch_items = Vec::with_capacity(total);

    for candidate in candidates {
        let enrichment = build_enrichment_context(
            store,
            &candidate.symbol,
            Some(candidate.baseline_sir),
            priority_scores,
            max_neighbors,
            priority_threshold,
            confidence_threshold,
            candidate.priority_score,
            contracts_enabled,
        )?;
        batch_items.push(QualityBatchItem {
            symbol: candidate.symbol,
            priority_score: candidate.priority_score,
            enrichment,
            use_cot,
            baseline_sir_identity: Some(candidate.baseline_sir_identity),
        });
    }

    tracing::info!("{pass_label}: submitting {total} symbols for concurrent inference");

    let stats =
        pipeline.process_quality_batch(store, batch_items, generation_pass, print_sir, out)?;

    tracing::info!(
        "{pass_label}: complete - {} improved, {} failed out of {} total",
        stats.success_count,
        stats.failure_count,
        total
    );

    Ok(())
}
