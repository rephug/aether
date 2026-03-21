use crate::{
    analysis::{
        CouplingConfig, default_coupling_bulk_commit_threshold, default_coupling_commit_window,
        default_coupling_exclude_patterns, default_coupling_min_co_change_count,
        default_coupling_semantic_weight, default_coupling_static_weight,
        default_coupling_temporal_weight, default_drift_analysis_window, default_drift_threshold,
    },
    constants::{
        DEFAULT_OPENAI_COMPAT_API_KEY_ENV, GEMINI_DEFAULT_CONCURRENCY, MAX_SEARCH_THRESHOLD,
        MIN_SEARCH_THRESHOLD,
    },
    embeddings::EmbeddingProviderKind,
    health::{
        HealthConfig, HealthScoreConfig, RiskWeights, default_health_drift_weight,
        default_health_no_sir_weight, default_health_pagerank_weight,
        default_health_recency_weight, default_health_score_author_count_high,
        default_health_score_blame_age_spread_high_secs,
        default_health_score_boundary_leakage_high, default_health_score_churn_30d_high,
        default_health_score_churn_90d_high, default_health_score_dead_feature_fail,
        default_health_score_dead_feature_warn, default_health_score_drift_density_high,
        default_health_score_file_loc_fail, default_health_score_file_loc_warn,
        default_health_score_internal_dep_fail, default_health_score_internal_dep_warn,
        default_health_score_stale_ref_fail, default_health_score_stale_ref_patterns,
        default_health_score_stale_ref_warn, default_health_score_stale_sir_high,
        default_health_score_test_gap_high, default_health_score_todo_density_fail,
        default_health_score_todo_density_warn, default_health_score_trait_method_fail,
        default_health_score_trait_method_warn, default_health_test_gap_weight,
    },
    inference::{
        InferenceProviderKind, default_api_key_env, default_sir_concurrency,
        default_tiered_fallback_endpoint, default_tiered_fallback_model,
        default_tiered_primary_threshold,
    },
    planner::{
        PlannerConfig, default_planner_community_resolution, default_planner_min_community_size,
        default_planner_semantic_rescue_max_k, default_planner_semantic_rescue_threshold,
    },
    root::{AetherConfig, default_log_level},
    search::{
        default_cohere_api_key_env, default_rerank_window, default_search_threshold_default,
        default_search_threshold_python, default_search_threshold_rust,
        default_search_threshold_typescript,
    },
    sir_quality::{
        SirQualityConfig, default_deep_concurrency, default_deep_confidence_threshold,
        default_deep_max_neighbors, default_deep_priority_threshold, default_deep_timeout_secs,
        default_triage_concurrency, default_triage_confidence_threshold,
        default_triage_priority_threshold, default_triage_timeout_secs,
    },
    verification::{
        default_verify_container_image, default_verify_container_runtime,
        default_verify_container_workdir, default_verify_microvm_memory_mib,
        default_verify_microvm_runtime, default_verify_microvm_vcpu_count,
        default_verify_microvm_workdir,
    },
};

pub(crate) fn normalize_optional(input: Option<String>) -> Option<String> {
    input
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_with_default(input: String, default: String) -> String {
    let normalized = input.trim();
    if normalized.is_empty() {
        default
    } else {
        normalized.to_owned()
    }
}

fn normalize_commands(commands: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();

    for raw in commands {
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }

        if !normalized.iter().any(|existing| existing == value) {
            normalized.push(value.to_owned());
        }
    }

    normalized
}

pub(crate) fn normalize_threshold_language(language: &str) -> &'static str {
    let normalized = language.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "rust" | "rs" => "rust",
        "typescript" | "ts" | "tsx" | "javascript" | "js" => "typescript",
        "python" | "py" => "python",
        _ => "default",
    }
}

fn normalize_threshold_value(value: f32, fallback: f32) -> f32 {
    if !value.is_finite() {
        return fallback;
    }

    value.clamp(MIN_SEARCH_THRESHOLD, MAX_SEARCH_THRESHOLD)
}

fn normalize_optional_threshold(value: Option<f32>) -> Option<f32> {
    value.and_then(|inner| {
        if !inner.is_finite() {
            None
        } else {
            Some(inner.clamp(MIN_SEARCH_THRESHOLD, MAX_SEARCH_THRESHOLD))
        }
    })
}

fn normalize_probability(value: f64, fallback: f64) -> f64 {
    if !value.is_finite() {
        fallback
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn normalize_provider_concurrency(provider: InferenceProviderKind, concurrency: usize) -> usize {
    if provider == InferenceProviderKind::Gemini && concurrency == default_sir_concurrency() {
        GEMINI_DEFAULT_CONCURRENCY
    } else {
        concurrency.max(1)
    }
}

fn normalize_sir_quality_config(config: &mut SirQualityConfig) {
    config.triage_priority_threshold = normalize_probability(
        config.triage_priority_threshold,
        default_triage_priority_threshold(),
    );
    config.triage_confidence_threshold = normalize_probability(
        config.triage_confidence_threshold,
        default_triage_confidence_threshold(),
    );
    config.triage_provider = normalize_optional(config.triage_provider.take());
    config.triage_model = normalize_optional(config.triage_model.take());
    config.triage_endpoint = normalize_optional(config.triage_endpoint.take());
    config.triage_api_key_env = normalize_optional(config.triage_api_key_env.take());
    if config.triage_concurrency == 0 {
        config.triage_concurrency = default_triage_concurrency();
    }
    if config.triage_timeout_secs == 0 {
        config.triage_timeout_secs = default_triage_timeout_secs();
    }

    config.deep_priority_threshold = normalize_probability(
        config.deep_priority_threshold,
        default_deep_priority_threshold(),
    );
    config.deep_confidence_threshold = normalize_probability(
        config.deep_confidence_threshold,
        default_deep_confidence_threshold(),
    );
    config.deep_provider = normalize_optional(config.deep_provider.take());
    config.deep_model = normalize_optional(config.deep_model.take());
    config.deep_endpoint = normalize_optional(config.deep_endpoint.take());
    config.deep_api_key_env = normalize_optional(config.deep_api_key_env.take());
    if config.deep_max_neighbors == 0 {
        config.deep_max_neighbors = default_deep_max_neighbors();
    }
    if config.deep_concurrency == 0 {
        config.deep_concurrency = default_deep_concurrency();
    }
    if config.deep_timeout_secs == 0 {
        config.deep_timeout_secs = default_deep_timeout_secs();
    }
}

fn normalize_patterns(patterns: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for raw in patterns {
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }
        if !normalized.iter().any(|existing| existing == value) {
            normalized.push(value.to_owned());
        }
    }
    normalized
}

fn normalize_weight_value(value: f32, fallback: f32) -> f32 {
    if !value.is_finite() || value < 0.0 {
        fallback
    } else {
        value
    }
}

fn normalize_coupling_weights(config: &mut CouplingConfig) {
    config.temporal_weight =
        normalize_weight_value(config.temporal_weight, default_coupling_temporal_weight());
    config.static_weight =
        normalize_weight_value(config.static_weight, default_coupling_static_weight());
    config.semantic_weight =
        normalize_weight_value(config.semantic_weight, default_coupling_semantic_weight());

    let sum = config.temporal_weight + config.static_weight + config.semantic_weight;
    if (sum - 1.0).abs() <= 0.01 {
        return;
    }

    if sum <= f32::EPSILON {
        config.temporal_weight = default_coupling_temporal_weight();
        config.static_weight = default_coupling_static_weight();
        config.semantic_weight = default_coupling_semantic_weight();
        eprintln!("AETHER config warning: coupling weights summed to {sum:.3}; reset to defaults");
        return;
    }

    config.temporal_weight /= sum;
    config.static_weight /= sum;
    config.semantic_weight /= sum;
    eprintln!("AETHER config warning: coupling weights summed to {sum:.3}; normalized to 1.0");
}

fn normalize_health_weight_value(value: f64, fallback: f64) -> f64 {
    if !value.is_finite() || value < 0.0 {
        fallback
    } else {
        value
    }
}

fn normalize_health_weights(config: &mut HealthConfig) {
    config.risk_weights.pagerank = normalize_health_weight_value(
        config.risk_weights.pagerank,
        default_health_pagerank_weight(),
    );
    config.risk_weights.test_gap = normalize_health_weight_value(
        config.risk_weights.test_gap,
        default_health_test_gap_weight(),
    );
    config.risk_weights.drift =
        normalize_health_weight_value(config.risk_weights.drift, default_health_drift_weight());
    config.risk_weights.no_sir =
        normalize_health_weight_value(config.risk_weights.no_sir, default_health_no_sir_weight());
    config.risk_weights.recency =
        normalize_health_weight_value(config.risk_weights.recency, default_health_recency_weight());

    let sum = config.risk_weights.pagerank
        + config.risk_weights.test_gap
        + config.risk_weights.drift
        + config.risk_weights.no_sir
        + config.risk_weights.recency;
    if (sum - 1.0).abs() <= 0.000_001 {
        return;
    }

    if sum <= f64::EPSILON {
        config.risk_weights = RiskWeights::default();
        eprintln!(
            "AETHER config warning: health risk weights summed to {sum:.3}; reset to defaults"
        );
        return;
    }

    config.risk_weights.pagerank /= sum;
    config.risk_weights.test_gap /= sum;
    config.risk_weights.drift /= sum;
    config.risk_weights.no_sir /= sum;
    config.risk_weights.recency /= sum;
    eprintln!("AETHER config warning: health risk weights summed to {sum:.3}; normalized to 1.0");
}

fn normalize_health_score_usize_pair(
    warn: &mut usize,
    fail: &mut usize,
    default_warn: usize,
    default_fail: usize,
) {
    if *warn == 0 || *fail == 0 || *fail <= *warn {
        *warn = default_warn;
        *fail = default_fail;
    }
}

fn normalize_health_score_f32_pair(
    warn: &mut f32,
    fail: &mut f32,
    default_warn: f32,
    default_fail: f32,
) {
    if !warn.is_finite() || !fail.is_finite() || *warn <= 0.0 || *fail <= *warn {
        *warn = default_warn;
        *fail = default_fail;
    }
}

fn normalize_health_score_positive_f32(value: &mut f32, default_value: f32) {
    if !value.is_finite() || *value <= 0.0 {
        *value = default_value;
    }
}

fn normalize_health_score_positive_u64(value: &mut u64, default_value: u64) {
    if *value == 0 {
        *value = default_value;
    }
}

fn normalize_optional_positive_f64(value: &mut Option<f64>) {
    if value.is_some_and(|raw| !raw.is_finite() || raw <= 0.0) {
        *value = None;
    }
}

fn normalize_planner_config(config: &mut PlannerConfig) {
    if !config.semantic_rescue_threshold.is_finite() {
        config.semantic_rescue_threshold = default_planner_semantic_rescue_threshold();
    }
    config.semantic_rescue_threshold = config.semantic_rescue_threshold.clamp(0.3, 0.95);

    if config.semantic_rescue_max_k == 0 {
        config.semantic_rescue_max_k = default_planner_semantic_rescue_max_k();
    }
    config.semantic_rescue_max_k = config.semantic_rescue_max_k.clamp(1, 10);

    if !config.community_resolution.is_finite() {
        config.community_resolution = default_planner_community_resolution();
    }
    config.community_resolution = config.community_resolution.clamp(0.1, 3.0);

    if config.min_community_size == 0 {
        config.min_community_size = default_planner_min_community_size();
    }
    config.min_community_size = config.min_community_size.clamp(1, 20);
}

fn normalize_health_score_config(config: &mut HealthScoreConfig) {
    normalize_health_score_usize_pair(
        &mut config.file_loc_warn,
        &mut config.file_loc_fail,
        default_health_score_file_loc_warn(),
        default_health_score_file_loc_fail(),
    );
    normalize_health_score_usize_pair(
        &mut config.trait_method_warn,
        &mut config.trait_method_fail,
        default_health_score_trait_method_warn(),
        default_health_score_trait_method_fail(),
    );
    normalize_health_score_usize_pair(
        &mut config.internal_dep_warn,
        &mut config.internal_dep_fail,
        default_health_score_internal_dep_warn(),
        default_health_score_internal_dep_fail(),
    );
    normalize_health_score_f32_pair(
        &mut config.todo_density_warn,
        &mut config.todo_density_fail,
        default_health_score_todo_density_warn(),
        default_health_score_todo_density_fail(),
    );
    normalize_health_score_usize_pair(
        &mut config.dead_feature_warn,
        &mut config.dead_feature_fail,
        default_health_score_dead_feature_warn(),
        default_health_score_dead_feature_fail(),
    );
    normalize_health_score_usize_pair(
        &mut config.stale_ref_warn,
        &mut config.stale_ref_fail,
        default_health_score_stale_ref_warn(),
        default_health_score_stale_ref_fail(),
    );
    config.stale_ref_patterns = normalize_patterns(std::mem::take(&mut config.stale_ref_patterns));
    if config.stale_ref_patterns.is_empty() {
        config.stale_ref_patterns = default_health_score_stale_ref_patterns();
    }
    normalize_health_score_usize_pair(
        &mut config.churn_30d_high,
        &mut config.churn_90d_high,
        default_health_score_churn_30d_high(),
        default_health_score_churn_90d_high(),
    );
    if config.author_count_high <= 1 {
        config.author_count_high = default_health_score_author_count_high();
    }
    normalize_health_score_positive_u64(
        &mut config.blame_age_spread_high_secs,
        default_health_score_blame_age_spread_high_secs(),
    );
    normalize_health_score_positive_f32(
        &mut config.drift_density_high,
        default_health_score_drift_density_high(),
    );
    normalize_health_score_positive_f32(
        &mut config.stale_sir_high,
        default_health_score_stale_sir_high(),
    );
    normalize_health_score_positive_f32(
        &mut config.test_gap_high,
        default_health_score_test_gap_high(),
    );
    normalize_health_score_positive_f32(
        &mut config.boundary_leakage_high,
        default_health_score_boundary_leakage_high(),
    );
    normalize_optional_positive_f64(&mut config.structural_weight);
    normalize_optional_positive_f64(&mut config.git_weight);
    normalize_optional_positive_f64(&mut config.semantic_weight);
}

pub(crate) fn normalize_config(mut config: AetherConfig) -> AetherConfig {
    config.general.log_level = normalize_with_default(
        std::mem::take(&mut config.general.log_level),
        default_log_level(),
    );
    config.inference.model = normalize_optional(config.inference.model.take());
    config.inference.endpoint = normalize_optional(config.inference.endpoint.take());
    if config.inference.concurrency == 0 {
        config.inference.concurrency = default_sir_concurrency();
    }
    config.inference.concurrency =
        normalize_provider_concurrency(config.inference.provider, config.inference.concurrency);
    if let Some(tiered) = config.inference.tiered.as_mut() {
        tiered.primary =
            normalize_with_default(std::mem::take(&mut tiered.primary), "gemini".to_owned());
        tiered.primary_model = normalize_optional(tiered.primary_model.take());
        tiered.primary_endpoint = normalize_optional(tiered.primary_endpoint.take());
        tiered.primary_api_key_env = normalize_with_default(
            std::mem::take(&mut tiered.primary_api_key_env),
            default_api_key_env(),
        );
        if !tiered.primary_threshold.is_finite() {
            tiered.primary_threshold = default_tiered_primary_threshold();
        }
        tiered.primary_threshold = tiered.primary_threshold.clamp(0.0, 1.0);
        tiered.fallback_model =
            normalize_optional(tiered.fallback_model.take()).or_else(default_tiered_fallback_model);
        tiered.fallback_endpoint = normalize_optional(tiered.fallback_endpoint.take())
            .or_else(default_tiered_fallback_endpoint);
    }
    normalize_sir_quality_config(&mut config.sir_quality);
    config.embeddings.model = normalize_optional(config.embeddings.model.take());
    config.embeddings.endpoint = normalize_optional(config.embeddings.endpoint.take());
    config.embeddings.api_key_env = normalize_optional(config.embeddings.api_key_env.take());
    config.embeddings.task_type = normalize_optional(config.embeddings.task_type.take());
    config.embeddings.candle.model_dir =
        normalize_optional(config.embeddings.candle.model_dir.take());
    config.search.candle.model_dir = normalize_optional(config.search.candle.model_dir.take());
    if config.search.rerank_window == 0 {
        config.search.rerank_window = default_rerank_window();
    }
    config.search.thresholds.default = normalize_threshold_value(
        config.search.thresholds.default,
        default_search_threshold_default(),
    );
    config.search.thresholds.rust = normalize_threshold_value(
        config.search.thresholds.rust,
        default_search_threshold_rust(),
    );
    config.search.thresholds.typescript = normalize_threshold_value(
        config.search.thresholds.typescript,
        default_search_threshold_typescript(),
    );
    config.search.thresholds.python = normalize_threshold_value(
        config.search.thresholds.python,
        default_search_threshold_python(),
    );
    config.search.calibrated_thresholds.default =
        normalize_optional_threshold(config.search.calibrated_thresholds.default.take());
    config.search.calibrated_thresholds.rust =
        normalize_optional_threshold(config.search.calibrated_thresholds.rust.take());
    config.search.calibrated_thresholds.typescript =
        normalize_optional_threshold(config.search.calibrated_thresholds.typescript.take());
    config.search.calibrated_thresholds.python =
        normalize_optional_threshold(config.search.calibrated_thresholds.python.take());
    config.verify.commands = normalize_commands(std::mem::take(&mut config.verify.commands));
    config.verify.container.runtime = normalize_with_default(
        std::mem::take(&mut config.verify.container.runtime),
        default_verify_container_runtime(),
    );
    config.verify.container.image = normalize_with_default(
        std::mem::take(&mut config.verify.container.image),
        default_verify_container_image(),
    );
    config.verify.container.workdir = normalize_with_default(
        std::mem::take(&mut config.verify.container.workdir),
        default_verify_container_workdir(),
    );
    config.verify.microvm.runtime = normalize_with_default(
        std::mem::take(&mut config.verify.microvm.runtime),
        default_verify_microvm_runtime(),
    );
    config.verify.microvm.kernel_image =
        normalize_optional(config.verify.microvm.kernel_image.take());
    config.verify.microvm.rootfs_image =
        normalize_optional(config.verify.microvm.rootfs_image.take());
    config.verify.microvm.workdir = normalize_with_default(
        std::mem::take(&mut config.verify.microvm.workdir),
        default_verify_microvm_workdir(),
    );
    if config.verify.microvm.vcpu_count == 0 {
        config.verify.microvm.vcpu_count = default_verify_microvm_vcpu_count();
    }
    if config.verify.microvm.memory_mib == 0 {
        config.verify.microvm.memory_mib = default_verify_microvm_memory_mib();
    }
    if config.coupling.commit_window == 0 {
        config.coupling.commit_window = default_coupling_commit_window();
    }
    if config.coupling.min_co_change_count == 0 {
        config.coupling.min_co_change_count = default_coupling_min_co_change_count();
    }
    if config.coupling.bulk_commit_threshold == 0 {
        config.coupling.bulk_commit_threshold = default_coupling_bulk_commit_threshold();
    }
    config.coupling.exclude_patterns =
        normalize_patterns(std::mem::take(&mut config.coupling.exclude_patterns));
    if config.coupling.exclude_patterns.is_empty() {
        config.coupling.exclude_patterns = default_coupling_exclude_patterns();
    }
    normalize_coupling_weights(&mut config.coupling);
    config.drift.analysis_window = normalize_with_default(
        std::mem::take(&mut config.drift.analysis_window),
        default_drift_analysis_window(),
    );
    if !config.drift.drift_threshold.is_finite() {
        config.drift.drift_threshold = default_drift_threshold();
    } else {
        config.drift.drift_threshold = config.drift.drift_threshold.clamp(0.0, 1.0);
    }
    config.drift.hub_percentile = config.drift.hub_percentile.clamp(1, 100);
    normalize_health_weights(&mut config.health);
    normalize_planner_config(&mut config.planner);
    normalize_health_score_config(&mut config.health_score);

    let api_key_env = config.inference.api_key_env.trim();
    if api_key_env.is_empty() {
        config.inference.api_key_env = default_api_key_env();
    } else {
        config.inference.api_key_env = api_key_env.to_owned();
    }

    if config.embeddings.provider == EmbeddingProviderKind::OpenAiCompat
        && config.embeddings.api_key_env.is_none()
    {
        config.embeddings.api_key_env = Some(DEFAULT_OPENAI_COMPAT_API_KEY_ENV.to_owned());
    }

    let cohere_api_key_env = config.providers.cohere.api_key_env.trim();
    if cohere_api_key_env.is_empty() {
        config.providers.cohere.api_key_env = default_cohere_api_key_env();
    } else {
        config.providers.cohere.api_key_env = cohere_api_key_env.to_owned();
    }

    config
}

#[cfg(test)]
mod tests {
    use crate::{
        DEFAULT_HEALTH_SCORE_AUTHOR_COUNT_HIGH, DEFAULT_HEALTH_SCORE_BLAME_AGE_SPREAD_HIGH_SECS,
        DEFAULT_HEALTH_SCORE_BOUNDARY_LEAKAGE_HIGH, DEFAULT_HEALTH_SCORE_CHURN_30D_HIGH,
        DEFAULT_HEALTH_SCORE_CHURN_90D_HIGH, DEFAULT_HEALTH_SCORE_DRIFT_DENSITY_HIGH,
        DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL, DEFAULT_HEALTH_SCORE_FILE_LOC_WARN,
        DEFAULT_HEALTH_SCORE_INTERNAL_DEP_FAIL, DEFAULT_HEALTH_SCORE_INTERNAL_DEP_WARN,
        DEFAULT_HEALTH_SCORE_STALE_SIR_HIGH, DEFAULT_HEALTH_SCORE_TEST_GAP_HIGH,
        DEFAULT_HEALTH_SCORE_TODO_DENSITY_FAIL, DEFAULT_HEALTH_SCORE_TODO_DENSITY_WARN,
        DEFAULT_HEALTH_SCORE_TRAIT_METHOD_FAIL, DEFAULT_HEALTH_SCORE_TRAIT_METHOD_WARN,
        GEMINI_DEFAULT_CONCURRENCY, MAX_SEARCH_THRESHOLD, MIN_SEARCH_THRESHOLD,
        health::default_health_score_stale_ref_patterns, root::parse_workspace_config_str,
    };

    use super::normalize_config;

    fn normalize(raw: &str) -> crate::AetherConfig {
        let parsed = parse_workspace_config_str(raw).expect("parse config");
        normalize_config(parsed)
    }

    #[test]
    fn normalize_preserves_explicit_api_key_env() {
        let config = normalize(
            r#"
[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
endpoint = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
"#,
        );

        assert_eq!(
            config.embeddings.api_key_env.as_deref(),
            Some("OPENROUTER_API_KEY")
        );
    }

    #[test]
    fn planner_config_normalizes_new_fields() {
        let normalized = normalize(
            r#"
[planner]
semantic_rescue_threshold = 42.0
semantic_rescue_max_k = 0
community_resolution = -5.0
min_community_size = 99
"#,
        );

        assert_eq!(normalized.planner.semantic_rescue_threshold, 0.95);
        assert_eq!(normalized.planner.semantic_rescue_max_k, 3);
        assert_eq!(normalized.planner.community_resolution, 0.1);
        assert_eq!(normalized.planner.min_community_size, 20);
    }

    #[test]
    fn planner_config_section_parses() {
        let normalized = normalize(
            r#"
[planner]
semantic_rescue_threshold = 0.82
semantic_rescue_max_k = 5
community_resolution = 0.65
min_community_size = 4
"#,
        );

        assert_eq!(normalized.planner.semantic_rescue_threshold, 0.82);
        assert_eq!(normalized.planner.semantic_rescue_max_k, 5);
        assert_eq!(normalized.planner.community_resolution, 0.65);
        assert_eq!(normalized.planner.min_community_size, 4);
    }

    #[test]
    fn normalize_coupling_weights_when_sum_is_invalid() {
        let config = normalize(
            r#"
[coupling]
temporal_weight = 2.0
static_weight = 1.0
semantic_weight = 1.0
"#,
        );

        let sum = config.coupling.temporal_weight
            + config.coupling.static_weight
            + config.coupling.semantic_weight;
        assert!((sum - 1.0).abs() < 1e-6);
        assert!((config.coupling.temporal_weight - 0.5).abs() < 1e-6);
        assert!((config.coupling.static_weight - 0.25).abs() < 1e-6);
        assert!((config.coupling.semantic_weight - 0.25).abs() < 1e-6);
    }

    #[test]
    fn normalize_health_weights_when_sum_is_invalid() {
        let config = normalize(
            r#"
[health]
risk_weights = { pagerank = 2.0, test_gap = 1.0, drift = 1.0, no_sir = 1.0, recency = 1.0 }
"#,
        );

        let sum = config.health.risk_weights.pagerank
            + config.health.risk_weights.test_gap
            + config.health.risk_weights.drift
            + config.health.risk_weights.no_sir
            + config.health.risk_weights.recency;
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalize_health_score_values() {
        let config = normalize(
            r#"
[health_score]
file_loc_warn = 0
file_loc_fail = 0
trait_method_warn = 50
trait_method_fail = 20
internal_dep_warn = 9
internal_dep_fail = 9
todo_density_warn = 0.0
todo_density_fail = 0.0
dead_feature_warn = 0
dead_feature_fail = 0
stale_ref_warn = 0
stale_ref_fail = 0
stale_ref_patterns = ["", "  "]
churn_30d_high = 0
churn_90d_high = 0
author_count_high = 1
blame_age_spread_high_secs = 0
drift_density_high = 0.0
stale_sir_high = 0.0
test_gap_high = 0.0
boundary_leakage_high = 0.0
structural_weight = -1.0
git_weight = 0.0
semantic_weight = -2.0
"#,
        );

        assert_eq!(
            config.health_score.file_loc_warn,
            DEFAULT_HEALTH_SCORE_FILE_LOC_WARN
        );
        assert_eq!(
            config.health_score.file_loc_fail,
            DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL
        );
        assert_eq!(
            config.health_score.trait_method_warn,
            DEFAULT_HEALTH_SCORE_TRAIT_METHOD_WARN
        );
        assert_eq!(
            config.health_score.trait_method_fail,
            DEFAULT_HEALTH_SCORE_TRAIT_METHOD_FAIL
        );
        assert_eq!(
            config.health_score.internal_dep_warn,
            DEFAULT_HEALTH_SCORE_INTERNAL_DEP_WARN
        );
        assert_eq!(
            config.health_score.internal_dep_fail,
            DEFAULT_HEALTH_SCORE_INTERNAL_DEP_FAIL
        );
        assert_eq!(
            config.health_score.todo_density_warn,
            DEFAULT_HEALTH_SCORE_TODO_DENSITY_WARN
        );
        assert_eq!(
            config.health_score.todo_density_fail,
            DEFAULT_HEALTH_SCORE_TODO_DENSITY_FAIL
        );
        assert_eq!(
            config.health_score.stale_ref_patterns,
            default_health_score_stale_ref_patterns()
        );
        assert_eq!(
            config.health_score.churn_30d_high,
            DEFAULT_HEALTH_SCORE_CHURN_30D_HIGH
        );
        assert_eq!(
            config.health_score.churn_90d_high,
            DEFAULT_HEALTH_SCORE_CHURN_90D_HIGH
        );
        assert_eq!(
            config.health_score.author_count_high,
            DEFAULT_HEALTH_SCORE_AUTHOR_COUNT_HIGH
        );
        assert_eq!(
            config.health_score.blame_age_spread_high_secs,
            DEFAULT_HEALTH_SCORE_BLAME_AGE_SPREAD_HIGH_SECS
        );
        assert_eq!(
            config.health_score.drift_density_high,
            DEFAULT_HEALTH_SCORE_DRIFT_DENSITY_HIGH
        );
        assert_eq!(
            config.health_score.stale_sir_high,
            DEFAULT_HEALTH_SCORE_STALE_SIR_HIGH
        );
        assert_eq!(
            config.health_score.test_gap_high,
            DEFAULT_HEALTH_SCORE_TEST_GAP_HIGH
        );
        assert_eq!(
            config.health_score.boundary_leakage_high,
            DEFAULT_HEALTH_SCORE_BOUNDARY_LEAKAGE_HIGH
        );
        assert!(config.health_score.structural_weight.is_none());
        assert!(config.health_score.git_weight.is_none());
        assert!(config.health_score.semantic_weight.is_none());
    }

    #[test]
    fn normalize_sir_quality_and_inference_concurrency() {
        let config = normalize(
            r#"
[inference]
provider = "gemini"
concurrency = 0

[sir_quality]
deep_pass = true
deep_priority_threshold = 9.9
deep_confidence_threshold = -3.0
deep_max_neighbors = 0
deep_concurrency = 0
"#,
        );

        assert_eq!(config.inference.concurrency, GEMINI_DEFAULT_CONCURRENCY);
        assert!(config.sir_quality.triage_pass);
        assert_eq!(config.sir_quality.triage_priority_threshold, 1.0);
        assert_eq!(config.sir_quality.triage_confidence_threshold, 0.0);
        assert_eq!(
            config.sir_quality.triage_concurrency,
            GEMINI_DEFAULT_CONCURRENCY
        );
        assert_eq!(config.sir_quality.deep_max_neighbors, 10);
        assert_eq!(
            config.sir_quality.deep_concurrency,
            GEMINI_DEFAULT_CONCURRENCY
        );
        assert_eq!(config.sir_quality.deep_timeout_secs, 180);
    }

    #[test]
    fn normalize_clamps_manual_and_calibrated_thresholds() {
        let config = normalize(
            r#"
[search]
reranker = "none"
rerank_window = 50

[search.thresholds]
default = 1.2
rust = 0.2
typescript = 0.66
python = -10.0

[search.calibrated_thresholds]
default = 0.1
rust = 0.97
typescript = 0.67
"#,
        );

        assert_eq!(config.search.thresholds.default, MAX_SEARCH_THRESHOLD);
        assert_eq!(config.search.thresholds.rust, MIN_SEARCH_THRESHOLD);
        assert_eq!(config.search.thresholds.typescript, 0.66);
        assert_eq!(config.search.thresholds.python, MIN_SEARCH_THRESHOLD);
        assert_eq!(
            config.search.calibrated_thresholds.default,
            Some(MIN_SEARCH_THRESHOLD)
        );
        assert_eq!(
            config.search.calibrated_thresholds.rust,
            Some(MAX_SEARCH_THRESHOLD)
        );
        assert_eq!(config.search.calibrated_thresholds.typescript, Some(0.67));
        assert_eq!(config.search.calibrated_thresholds.python, None);
    }
}
