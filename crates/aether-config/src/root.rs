use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    analysis::{CouplingConfig, DriftConfig},
    batch::BatchConfig,
    constants::{AETHER_DIR_NAME, CONFIG_FILE_NAME, DEFAULT_DASHBOARD_PORT, DEFAULT_LOG_LEVEL},
    continuous::ContinuousConfig,
    contracts::ContractsConfig,
    embeddings::EmbeddingsConfig,
    health::{HealthConfig, HealthScoreConfig},
    inference::InferenceConfig,
    normalize::normalize_config,
    planner::PlannerConfig,
    search::{ProvidersConfig, SearchConfig},
    seismograph::SeismographConfig,
    sir_quality::SirQualityConfig,
    storage::StorageConfig,
    verification::VerifyConfig,
    watcher::WatcherConfig,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AetherConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub inference: InferenceConfig,
    #[serde(default)]
    pub sir_quality: SirQualityConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub embeddings: EmbeddingsConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub verify: VerifyConfig,
    #[serde(default)]
    pub coupling: CouplingConfig,
    #[serde(default)]
    pub drift: DriftConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub planner: PlannerConfig,
    #[serde(default)]
    pub health_score: HealthScoreConfig,
    #[serde(default)]
    pub dashboard: DashboardConfig,
    #[serde(default)]
    pub continuous: Option<ContinuousConfig>,
    #[serde(default)]
    pub batch: Option<BatchConfig>,
    #[serde(default)]
    pub seismograph: Option<SeismographConfig>,
    #[serde(default)]
    pub contracts: Option<ContractsConfig>,
    #[serde(default, rename = "watcher")]
    pub watcher: Option<WatcherConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardConfig {
    #[serde(default = "default_dashboard_port")]
    pub port: u16,
    #[serde(default = "default_dashboard_enabled")]
    pub enabled: bool,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            port: default_dashboard_port(),
            enabled: default_dashboard_enabled(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config TOML: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("failed to serialize config TOML: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("toml edit error: {0}")]
    TomlEdit(#[from] toml_edit::TomlError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn aether_dir(workspace_root: impl AsRef<Path>) -> PathBuf {
    workspace_root.as_ref().join(AETHER_DIR_NAME)
}

pub fn config_path(workspace_root: impl AsRef<Path>) -> PathBuf {
    aether_dir(workspace_root).join(CONFIG_FILE_NAME)
}

pub fn load_workspace_config(
    workspace_root: impl AsRef<Path>,
) -> Result<AetherConfig, ConfigError> {
    let path = config_path(workspace_root);
    if !path.exists() {
        return Ok(AetherConfig::default());
    }

    let raw = fs::read_to_string(path)?;
    let parsed = parse_workspace_config_str(&raw)?;
    Ok(normalize_config(parsed))
}

pub fn ensure_workspace_config(
    workspace_root: impl AsRef<Path>,
) -> Result<AetherConfig, ConfigError> {
    let workspace_root = workspace_root.as_ref();
    fs::create_dir_all(aether_dir(workspace_root))?;

    let path = config_path(workspace_root);
    if path.exists() {
        return load_workspace_config(workspace_root);
    }

    let config = AetherConfig::default();
    save_workspace_config(workspace_root, &config)?;

    Ok(config)
}

pub fn save_workspace_config(
    workspace_root: impl AsRef<Path>,
    config: &AetherConfig,
) -> Result<(), ConfigError> {
    let workspace_root = workspace_root.as_ref();
    fs::create_dir_all(aether_dir(workspace_root))?;
    let normalized = normalize_config(config.clone());
    let content = toml::to_string_pretty(&normalized)?;
    fs::write(config_path(workspace_root), content)?;
    Ok(())
}

/// Save a single config section while preserving TOML comments and formatting.
///
/// Uses `toml_edit` to parse the existing file, merge the given JSON values into
/// the specified section, and write back. Falls back to `save_workspace_config()`
/// if the config file does not yet exist.
pub fn save_workspace_config_preserving_comments(
    workspace_root: impl AsRef<Path>,
    section: &str,
    values: &serde_json::Value,
) -> Result<(), ConfigError> {
    let workspace_root = workspace_root.as_ref();
    let path = config_path(workspace_root);

    if !path.exists() {
        // No existing file — create one via the standard path, then merge.
        fs::create_dir_all(aether_dir(workspace_root))?;
        let config = AetherConfig::default();
        save_workspace_config(workspace_root, &config)?;
    }

    let raw = fs::read_to_string(&path)?;
    let mut doc: toml_edit::DocumentMut = raw.parse()?;

    // Get or create the target section table.
    if doc.get(section).is_none() {
        doc[section] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let table = doc[section].as_table_mut().ok_or_else(|| {
        std::io::Error::other(format!("config section '{section}' is not a table"))
    })?;

    if let serde_json::Value::Object(map) = values {
        merge_json_into_toml_table(table, map);
    }

    fs::write(&path, doc.to_string())?;
    Ok(())
}

/// Reset a config section to its defaults while preserving other sections.
pub fn reset_section_to_defaults(
    workspace_root: impl AsRef<Path>,
    section: &str,
) -> Result<(), ConfigError> {
    let defaults = AetherConfig::default();
    let defaults_value = serde_json::to_value(&defaults)?;
    let section_defaults = defaults_value
        .get(section)
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let workspace_root = workspace_root.as_ref();
    let path = config_path(workspace_root);

    if !path.exists() {
        return save_workspace_config(workspace_root, &defaults);
    }

    let raw = fs::read_to_string(&path)?;
    let mut doc: toml_edit::DocumentMut = raw.parse()?;

    // Replace section entirely with defaults.
    doc.remove(section);
    doc[section] = toml_edit::Item::Table(toml_edit::Table::new());

    let table = doc[section].as_table_mut().unwrap();
    if let serde_json::Value::Object(map) = &section_defaults {
        merge_json_into_toml_table(table, map);
    }

    fs::write(&path, doc.to_string())?;
    Ok(())
}

fn merge_json_into_toml_table(
    table: &mut toml_edit::Table,
    map: &serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in map {
        match value {
            serde_json::Value::Object(inner) => {
                // Nested table (e.g., search.thresholds).
                if table.get(key).is_none() {
                    table[key] = toml_edit::Item::Table(toml_edit::Table::new());
                }
                if let Some(sub) = table[key].as_table_mut() {
                    merge_json_into_toml_table(sub, inner);
                }
            }
            serde_json::Value::String(s) => {
                table[key] = toml_edit::value(s.as_str());
            }
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    table[key] = toml_edit::value(i);
                } else if let Some(f) = n.as_f64() {
                    table[key] = toml_edit::value(f);
                }
            }
            serde_json::Value::Bool(b) => {
                table[key] = toml_edit::value(*b);
            }
            serde_json::Value::Array(arr) => {
                let mut toml_arr = toml_edit::Array::new();
                for item in arr {
                    match item {
                        serde_json::Value::String(s) => {
                            toml_arr.push(s.as_str());
                        }
                        serde_json::Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                toml_arr.push(i);
                            } else if let Some(f) = n.as_f64() {
                                toml_arr.push(f);
                            }
                        }
                        serde_json::Value::Bool(b) => {
                            toml_arr.push(*b);
                        }
                        _ => {}
                    }
                }
                table[key] = toml_edit::value(toml_arr);
            }
            serde_json::Value::Null => {
                table.remove(key);
            }
        }
    }
}

pub(crate) fn parse_workspace_config_str(raw: &str) -> Result<AetherConfig, ConfigError> {
    let mut parsed: toml::Value = toml::from_str(raw)?;
    rewrite_legacy_sir_quality_keys(&mut parsed);
    parsed.try_into().map_err(Into::into)
}

fn rewrite_legacy_sir_quality_keys(parsed: &mut toml::Value) {
    let Some(root) = parsed.as_table_mut() else {
        return;
    };
    let Some(sir_quality) = root
        .get_mut("sir_quality")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };

    let has_new_triage_schema = sir_quality.keys().any(|key| key.starts_with("triage_"));
    if has_new_triage_schema {
        return;
    }

    for (legacy_key, triage_key) in [
        ("deep_pass", "triage_pass"),
        ("deep_provider", "triage_provider"),
        ("deep_model", "triage_model"),
        ("deep_endpoint", "triage_endpoint"),
        ("deep_api_key_env", "triage_api_key_env"),
        ("deep_priority_threshold", "triage_priority_threshold"),
        ("deep_confidence_threshold", "triage_confidence_threshold"),
        ("deep_max_symbols", "triage_max_symbols"),
        ("deep_concurrency", "triage_concurrency"),
    ] {
        if let Some(value) = sir_quality.remove(legacy_key) {
            sir_quality.insert(triage_key.to_owned(), value);
        }
    }
}

pub(crate) fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_owned()
}

fn default_dashboard_port() -> u16 {
    DEFAULT_DASHBOARD_PORT
}

fn default_dashboard_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        aether_dir, config_path, ensure_workspace_config, load_workspace_config,
        parse_workspace_config_str, save_workspace_config,
    };
    use crate::{
        AetherConfig, BatchConfig, BatchProviderConfig, ContinuousConfig,
        DEFAULT_COHERE_API_KEY_ENV, DEFAULT_DASHBOARD_PORT, DEFAULT_DRIFT_ANALYSIS_WINDOW,
        DEFAULT_DRIFT_HUB_PERCENTILE, DEFAULT_DRIFT_THRESHOLD, DEFAULT_GEMINI_API_KEY_ENV,
        DEFAULT_HEALTH_DRIFT_WEIGHT, DEFAULT_HEALTH_NO_SIR_WEIGHT, DEFAULT_HEALTH_PAGERANK_WEIGHT,
        DEFAULT_HEALTH_RECENCY_WEIGHT, DEFAULT_HEALTH_SCORE_AUTHOR_COUNT_HIGH,
        DEFAULT_HEALTH_SCORE_BLAME_AGE_SPREAD_HIGH_SECS,
        DEFAULT_HEALTH_SCORE_BOUNDARY_LEAKAGE_HIGH, DEFAULT_HEALTH_SCORE_CHURN_30D_HIGH,
        DEFAULT_HEALTH_SCORE_CHURN_90D_HIGH, DEFAULT_HEALTH_SCORE_DEAD_FEATURE_FAIL,
        DEFAULT_HEALTH_SCORE_DEAD_FEATURE_WARN, DEFAULT_HEALTH_SCORE_DRIFT_DENSITY_HIGH,
        DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL, DEFAULT_HEALTH_SCORE_FILE_LOC_WARN,
        DEFAULT_HEALTH_SCORE_INTERNAL_DEP_FAIL, DEFAULT_HEALTH_SCORE_INTERNAL_DEP_WARN,
        DEFAULT_HEALTH_SCORE_STALE_REF_FAIL, DEFAULT_HEALTH_SCORE_STALE_REF_WARN,
        DEFAULT_HEALTH_SCORE_STALE_SIR_HIGH, DEFAULT_HEALTH_SCORE_TEST_GAP_HIGH,
        DEFAULT_HEALTH_SCORE_TODO_DENSITY_FAIL, DEFAULT_HEALTH_SCORE_TODO_DENSITY_WARN,
        DEFAULT_HEALTH_SCORE_TRAIT_METHOD_FAIL, DEFAULT_HEALTH_SCORE_TRAIT_METHOD_WARN,
        DEFAULT_HEALTH_TEST_GAP_WEIGHT, DEFAULT_LOG_LEVEL, DEFAULT_OPENAI_COMPAT_API_KEY_ENV,
        DEFAULT_SEARCH_THRESHOLD_DEFAULT, DEFAULT_SEARCH_THRESHOLD_PYTHON,
        DEFAULT_SEARCH_THRESHOLD_RUST, DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT,
        DEFAULT_SIR_CONCURRENCY, DEFAULT_VERIFY_CONTAINER_IMAGE, DEFAULT_VERIFY_CONTAINER_RUNTIME,
        DEFAULT_VERIFY_CONTAINER_WORKDIR, DEFAULT_VERIFY_MICROVM_MEMORY_MIB,
        DEFAULT_VERIFY_MICROVM_RUNTIME, DEFAULT_VERIFY_MICROVM_VCPU_COUNT,
        DEFAULT_VERIFY_MICROVM_WORKDIR, EmbeddingProviderKind, EmbeddingVectorBackend,
        GraphBackend, InferenceProviderKind, SearchRerankerKind, VerifyMode, WatcherConfig,
        health::default_health_score_stale_ref_patterns,
    };

    const LEGACY_COZO_GRAPH_STORE: &str = concat!("Cozo", "GraphStore");

    #[test]
    fn ensure_workspace_config_creates_default_file() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        let config = ensure_workspace_config(workspace).expect("ensure config");

        assert_eq!(config.general.log_level, DEFAULT_LOG_LEVEL);
        assert_eq!(config.inference.provider, InferenceProviderKind::Auto);
        assert_eq!(config.inference.api_key_env, DEFAULT_GEMINI_API_KEY_ENV);
        assert_eq!(config.inference.concurrency, DEFAULT_SIR_CONCURRENCY);
        assert!(!config.sir_quality.triage_pass);
        assert_eq!(config.sir_quality.triage_priority_threshold, 0.7);
        assert_eq!(config.sir_quality.triage_confidence_threshold, 0.85);
        assert_eq!(config.sir_quality.triage_max_symbols, 0);
        assert_eq!(config.sir_quality.triage_concurrency, 4);
        assert_eq!(config.sir_quality.triage_timeout_secs, 180);
        assert!(!config.sir_quality.deep_pass);
        assert_eq!(config.sir_quality.deep_priority_threshold, 0.9);
        assert_eq!(config.sir_quality.deep_confidence_threshold, 0.85);
        assert_eq!(config.sir_quality.deep_max_symbols, 20);
        assert_eq!(config.sir_quality.deep_max_neighbors, 10);
        assert_eq!(config.sir_quality.deep_concurrency, 4);
        assert_eq!(config.sir_quality.deep_timeout_secs, 180);
        assert!(config.storage.mirror_sir_files);
        assert_eq!(config.storage.graph_backend, GraphBackend::Surreal);
        assert!(!config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::Qwen3Local
        );
        assert_eq!(config.search.reranker, SearchRerankerKind::None);
        assert_eq!(config.search.rerank_window, 50);
        assert_eq!(
            config.search.thresholds.default,
            DEFAULT_SEARCH_THRESHOLD_DEFAULT
        );
        assert_eq!(config.search.thresholds.rust, DEFAULT_SEARCH_THRESHOLD_RUST);
        assert_eq!(
            config.search.thresholds.typescript,
            DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT
        );
        assert_eq!(
            config.search.thresholds.python,
            DEFAULT_SEARCH_THRESHOLD_PYTHON
        );
        assert!(config.search.calibrated_thresholds.is_empty());
        assert_eq!(
            config.providers.cohere.api_key_env,
            DEFAULT_COHERE_API_KEY_ENV
        );
        assert_eq!(
            config.verify.commands,
            vec![
                "cargo fmt --all --check".to_owned(),
                "cargo clippy --workspace -- -D warnings".to_owned(),
                "cargo test --workspace".to_owned()
            ]
        );
        assert_eq!(config.verify.mode, VerifyMode::Host);
        assert_eq!(
            config.verify.container.runtime,
            DEFAULT_VERIFY_CONTAINER_RUNTIME
        );
        assert_eq!(
            config.verify.container.image,
            DEFAULT_VERIFY_CONTAINER_IMAGE
        );
        assert_eq!(
            config.verify.container.workdir,
            DEFAULT_VERIFY_CONTAINER_WORKDIR
        );
        assert!(!config.verify.container.fallback_to_host_on_unavailable);
        assert_eq!(
            config.verify.microvm.runtime,
            DEFAULT_VERIFY_MICROVM_RUNTIME
        );
        assert_eq!(config.verify.microvm.kernel_image, None);
        assert_eq!(config.verify.microvm.rootfs_image, None);
        assert_eq!(
            config.verify.microvm.workdir,
            DEFAULT_VERIFY_MICROVM_WORKDIR
        );
        assert_eq!(
            config.verify.microvm.vcpu_count,
            DEFAULT_VERIFY_MICROVM_VCPU_COUNT
        );
        assert_eq!(
            config.verify.microvm.memory_mib,
            DEFAULT_VERIFY_MICROVM_MEMORY_MIB
        );
        assert!(!config.verify.microvm.fallback_to_container_on_unavailable);
        assert!(!config.verify.microvm.fallback_to_host_on_unavailable);
        assert!(config.coupling.enabled);
        assert_eq!(config.coupling.commit_window, 500);
        assert_eq!(config.coupling.min_co_change_count, 3);
        assert_eq!(config.coupling.bulk_commit_threshold, 30);
        assert!((config.coupling.temporal_weight - 0.5).abs() < 1e-6);
        assert!((config.coupling.static_weight - 0.3).abs() < 1e-6);
        assert!((config.coupling.semantic_weight - 0.2).abs() < 1e-6);
        assert_eq!(
            config.coupling.exclude_patterns,
            vec![
                "*.lock".to_owned(),
                "*.generated.*".to_owned(),
                ".gitignore".to_owned()
            ]
        );
        assert!(config.drift.enabled);
        assert_eq!(config.drift.drift_threshold, DEFAULT_DRIFT_THRESHOLD);
        assert_eq!(
            config.drift.analysis_window,
            DEFAULT_DRIFT_ANALYSIS_WINDOW.to_owned()
        );
        assert!(!config.drift.auto_analyze);
        assert_eq!(config.drift.hub_percentile, DEFAULT_DRIFT_HUB_PERCENTILE);
        assert!(config.health.enabled);
        assert_eq!(
            config.health.risk_weights.pagerank,
            DEFAULT_HEALTH_PAGERANK_WEIGHT
        );
        assert_eq!(
            config.health.risk_weights.test_gap,
            DEFAULT_HEALTH_TEST_GAP_WEIGHT
        );
        assert_eq!(
            config.health.risk_weights.drift,
            DEFAULT_HEALTH_DRIFT_WEIGHT
        );
        assert_eq!(
            config.health.risk_weights.no_sir,
            DEFAULT_HEALTH_NO_SIR_WEIGHT
        );
        assert_eq!(
            config.health.risk_weights.recency,
            DEFAULT_HEALTH_RECENCY_WEIGHT
        );
        assert_eq!(config.planner.semantic_rescue_threshold, 0.70);
        assert_eq!(config.planner.semantic_rescue_max_k, 3);
        assert_eq!(config.planner.community_resolution, 0.5);
        assert_eq!(config.planner.min_community_size, 3);
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
            config.health_score.dead_feature_warn,
            DEFAULT_HEALTH_SCORE_DEAD_FEATURE_WARN
        );
        assert_eq!(
            config.health_score.dead_feature_fail,
            DEFAULT_HEALTH_SCORE_DEAD_FEATURE_FAIL
        );
        assert_eq!(
            config.health_score.stale_ref_warn,
            DEFAULT_HEALTH_SCORE_STALE_REF_WARN
        );
        assert_eq!(
            config.health_score.stale_ref_fail,
            DEFAULT_HEALTH_SCORE_STALE_REF_FAIL
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
        assert_eq!(config.dashboard.port, DEFAULT_DASHBOARD_PORT);
        assert!(config.dashboard.enabled);
        assert_eq!(config.continuous, None);
        assert!(config_path(workspace).exists());

        let content = fs::read_to_string(config_path(workspace)).expect("read config file");
        assert!(content.contains("[general]"));
        assert!(content.contains("log_level = \"info\""));
        assert!(content.contains("[inference]"));
        assert!(content.contains("provider = \"auto\""));
        assert!(content.contains("concurrency = 2"));
        assert!(content.contains("[sir_quality]"));
        assert!(content.contains("triage_pass = false"));
        assert!(content.contains("deep_pass = false"));
        assert!(content.contains("[storage]"));
        assert!(content.contains("mirror_sir_files = true"));
        assert!(content.contains("graph_backend = \"surreal\""));
        assert!(content.contains("[embeddings]"));
        assert!(content.contains("enabled = false"));
        assert!(content.contains("provider = \"qwen3_local\""));
        assert!(content.contains("vector_backend = \"lancedb\""));
        assert!(content.contains("[search]"));
        assert!(content.contains("reranker = \"none\""));
        assert!(content.contains("rerank_window = 50"));
        assert!(content.contains("[search.thresholds]"));
        assert!(content.contains("default = "));
        assert!(content.contains("rust = "));
        assert!(content.contains("typescript = "));
        assert!(content.contains("python = "));
        assert!(!content.contains("[search.calibrated_thresholds]"));
        assert!(content.contains("[providers.cohere]"));
        assert!(content.contains("api_key_env = \"COHERE_API_KEY\""));
        assert!(content.contains("[verify]"));
        assert!(content.contains("commands = ["));
        assert!(content.contains("mode = \"host\""));
        assert!(content.contains("[verify.container]"));
        assert!(content.contains("runtime = \"docker\""));
        assert!(content.contains("image = \"rust:1-bookworm\""));
        assert!(content.contains("workdir = \"/workspace\""));
        assert!(content.contains("[verify.microvm]"));
        assert!(content.contains("runtime = \"firecracker\""));
        assert!(content.contains("workdir = \"/workspace\""));
        assert!(content.contains("vcpu_count = 1"));
        assert!(content.contains("memory_mib = 1024"));
        assert!(content.contains("[coupling]"));
        assert!(content.contains("enabled = true"));
        assert!(content.contains("commit_window = 500"));
        assert!(content.contains("min_co_change_count = 3"));
        assert!(content.contains("bulk_commit_threshold = 30"));
        assert!(content.contains("temporal_weight = 0.5"));
        assert!(content.contains("static_weight = 0.3"));
        assert!(content.contains("semantic_weight = 0.2"));
        assert!(content.contains("[drift]"));
        assert!(content.contains("drift_threshold = 0.85"));
        assert!(content.contains("analysis_window = \"100 commits\""));
        assert!(content.contains("auto_analyze = false"));
        assert!(content.contains("hub_percentile = 95"));
        assert!(content.contains("[health]"));
        assert!(content.contains("enabled = true"));
        assert!(content.contains("risk_weights = {") || content.contains("[health.risk_weights]"));
        assert!(content.contains("pagerank = 0.3"));
        assert!(content.contains("test_gap = 0.25"));
        assert!(content.contains("drift = 0.2"));
        assert!(content.contains("no_sir = 0.15"));
        assert!(content.contains("recency = 0.1"));
        assert!(content.contains("[planner]"));
        assert!(content.contains("semantic_rescue_threshold"));
        assert!(content.contains("semantic_rescue_max_k = 3"));
        assert!(content.contains("community_resolution"));
        assert!(content.contains("min_community_size = 3"));
        assert!(content.contains("[health_score]"));
        assert!(content.contains("file_loc_warn = 800"));
        assert!(content.contains("trait_method_fail = 35"));
        assert!(content.contains("stale_ref_patterns = ["));
        assert!(content.contains(&format!("\"{LEGACY_COZO_GRAPH_STORE}\"")));
        assert!(content.contains("\"CozoDB\""));
        assert!(content.contains("[dashboard]"));
        assert!(content.contains("port = 9730"));
        assert!(content.contains("enabled = true"));
        assert!(content.contains("\"cargo fmt --all --check\""));
        assert!(content.contains("\"cargo clippy --workspace -- -D warnings\""));
        assert!(content.contains("\"cargo test --workspace\""));
    }

    #[test]
    fn load_workspace_config_parses_inference_storage_and_embedding_values() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[general]
log_level = " debug "

[inference]
provider = "qwen3_local"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434"
api_key_env = "CUSTOM_GEMINI_KEY"

[storage]
mirror_sir_files = false
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434/api/embeddings"

[search]
reranker = "cohere"
rerank_window = 0

[search.candle]
model_dir = " .aether/models "

[providers.cohere]
api_key_env = " CUSTOM_COHERE_KEY "

[verify]
mode = "microvm"
commands = [
    " cargo test ",
    "",
    "cargo clippy --workspace -- -D warnings",
    "cargo test"
]

[verify.container]
runtime = " docker "
image = " rust:1-bookworm "
workdir = " /workspace "
fallback_to_host_on_unavailable = true

[verify.microvm]
runtime = " firecracker "
kernel_image = " ./assets/vmlinux "
rootfs_image = " ./assets/rootfs.ext4 "
workdir = " /workspace "
vcpu_count = 0
memory_mib = 0
fallback_to_container_on_unavailable = true
fallback_to_host_on_unavailable = true

[drift]
enabled = false
drift_threshold = 0.9
analysis_window = " 50 commits "
auto_analyze = true
hub_percentile = 0

[health]
enabled = false
risk_weights = { pagerank = 3.0, test_gap = 1.0, drift = 1.0, no_sir = 1.0, recency = 2.0 }

[health_score]
file_loc_warn = 1000
file_loc_fail = 2000
trait_method_warn = 25
trait_method_fail = 40
internal_dep_warn = 7
internal_dep_fail = 11
todo_density_warn = 6.0
todo_density_fail = 16.0
dead_feature_warn = 2
dead_feature_fail = 6
stale_ref_warn = 2
stale_ref_fail = 4
stale_ref_patterns = [" __LEGACY_COZO_GRAPH_STORE__ ", "", "LegacyStore"]

[dashboard]
port = 9800
enabled = false
"#
        .replace("__LEGACY_COZO_GRAPH_STORE__", LEGACY_COZO_GRAPH_STORE);
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");

        assert_eq!(config.general.log_level, "debug");
        assert_eq!(config.inference.provider, InferenceProviderKind::Qwen3Local);
        assert_eq!(
            config.inference.model.as_deref(),
            Some("qwen3-embeddings-4B")
        );
        assert_eq!(
            config.inference.endpoint.as_deref(),
            Some("http://127.0.0.1:11434")
        );
        assert_eq!(config.inference.api_key_env, "CUSTOM_GEMINI_KEY");
        assert!(!config.storage.mirror_sir_files);
        assert_eq!(config.storage.graph_backend, GraphBackend::Sqlite);
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::Qwen3Local
        );
        assert_eq!(
            config.embeddings.vector_backend,
            EmbeddingVectorBackend::Sqlite
        );
        assert_eq!(
            config.embeddings.model.as_deref(),
            Some("qwen3-embeddings-4B")
        );
        assert_eq!(
            config.embeddings.endpoint.as_deref(),
            Some("http://127.0.0.1:11434/api/embeddings")
        );
        assert_eq!(config.search.reranker, SearchRerankerKind::Cohere);
        assert_eq!(config.search.rerank_window, 50);
        assert_eq!(
            config.search.thresholds.default,
            DEFAULT_SEARCH_THRESHOLD_DEFAULT
        );
        assert_eq!(config.search.thresholds.rust, DEFAULT_SEARCH_THRESHOLD_RUST);
        assert_eq!(
            config.search.thresholds.typescript,
            DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT
        );
        assert_eq!(
            config.search.thresholds.python,
            DEFAULT_SEARCH_THRESHOLD_PYTHON
        );
        assert_eq!(
            config.search.candle.model_dir.as_deref(),
            Some(".aether/models")
        );
        assert_eq!(config.providers.cohere.api_key_env, "CUSTOM_COHERE_KEY");
        assert_eq!(
            config.verify.commands,
            vec![
                "cargo test".to_owned(),
                "cargo clippy --workspace -- -D warnings".to_owned(),
            ]
        );
        assert_eq!(config.verify.mode, VerifyMode::Microvm);
        assert_eq!(config.verify.container.runtime, "docker");
        assert_eq!(config.verify.container.image, "rust:1-bookworm");
        assert_eq!(config.verify.container.workdir, "/workspace");
        assert!(config.verify.container.fallback_to_host_on_unavailable);
        assert_eq!(config.verify.microvm.runtime, "firecracker");
        assert_eq!(
            config.verify.microvm.kernel_image.as_deref(),
            Some("./assets/vmlinux")
        );
        assert_eq!(
            config.verify.microvm.rootfs_image.as_deref(),
            Some("./assets/rootfs.ext4")
        );
        assert_eq!(config.verify.microvm.workdir, "/workspace");
        assert_eq!(config.verify.microvm.vcpu_count, 1);
        assert_eq!(config.verify.microvm.memory_mib, 1024);
        assert!(config.verify.microvm.fallback_to_container_on_unavailable);
        assert!(config.verify.microvm.fallback_to_host_on_unavailable);
        assert!(config.coupling.enabled);
        assert_eq!(config.coupling.commit_window, 500);
        assert_eq!(config.coupling.min_co_change_count, 3);
        assert_eq!(config.coupling.bulk_commit_threshold, 30);
        assert!((config.coupling.temporal_weight - 0.5).abs() < 1e-6);
        assert!((config.coupling.static_weight - 0.3).abs() < 1e-6);
        assert!((config.coupling.semantic_weight - 0.2).abs() < 1e-6);
        assert!(!config.drift.enabled);
        assert_eq!(config.drift.drift_threshold, 0.9);
        assert_eq!(config.drift.analysis_window, "50 commits");
        assert!(config.drift.auto_analyze);
        assert_eq!(config.drift.hub_percentile, 1);
        assert!(!config.health.enabled);
        let health_sum = config.health.risk_weights.pagerank
            + config.health.risk_weights.test_gap
            + config.health.risk_weights.drift
            + config.health.risk_weights.no_sir
            + config.health.risk_weights.recency;
        assert!((health_sum - 1.0).abs() < 1e-6);
        assert_eq!(config.health_score.file_loc_warn, 1000);
        assert_eq!(config.health_score.file_loc_fail, 2000);
        assert_eq!(config.health_score.trait_method_warn, 25);
        assert_eq!(config.health_score.trait_method_fail, 40);
        assert_eq!(config.health_score.internal_dep_warn, 7);
        assert_eq!(config.health_score.internal_dep_fail, 11);
        assert_eq!(config.health_score.todo_density_warn, 6.0);
        assert_eq!(config.health_score.todo_density_fail, 16.0);
        assert_eq!(config.health_score.dead_feature_warn, 2);
        assert_eq!(config.health_score.dead_feature_fail, 6);
        assert_eq!(config.health_score.stale_ref_warn, 2);
        assert_eq!(config.health_score.stale_ref_fail, 4);
        assert_eq!(
            config.health_score.stale_ref_patterns,
            vec![LEGACY_COZO_GRAPH_STORE.to_owned(), "LegacyStore".to_owned()]
        );
        assert_eq!(config.dashboard.port, 9800);
        assert!(!config.dashboard.enabled);
    }

    #[test]
    fn parse_workspace_config_accepts_empty_continuous_section() {
        let config = parse_workspace_config_str("").expect("parse empty config");
        assert_eq!(config.continuous, None);
    }

    #[test]
    fn parse_workspace_config_parses_full_continuous_section() {
        let config = parse_workspace_config_str(
            r#"
[continuous]
enabled = true
schedule = "hourly"
staleness_half_life_days = 21.5
staleness_sigmoid_k = 0.45
neighbor_decay = 0.6
neighbor_cutoff = 0.2
coupling_predict_threshold = 0.9
priority_pagerank_alpha = 0.15
max_requeue_per_run = 250
auto_submit = true
requeue_pass = "deep"
"#,
        )
        .expect("parse continuous config");

        assert_eq!(
            config.continuous,
            Some(ContinuousConfig {
                enabled: true,
                schedule: "hourly".to_owned(),
                staleness_half_life_days: 21.5,
                staleness_sigmoid_k: 0.45,
                neighbor_decay: 0.6,
                neighbor_cutoff: 0.2,
                coupling_predict_threshold: 0.9,
                priority_pagerank_alpha: 0.15,
                max_requeue_per_run: 250,
                auto_submit: true,
                requeue_pass: "deep".to_owned(),
            })
        );
    }

    #[test]
    fn load_workspace_config_parses_candle_embedding_provider() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[embeddings]
enabled = true
provider = "candle"
vector_backend = "lancedb"

[embeddings.candle]
model_dir = " .aether/models "
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert!(config.embeddings.enabled);
        assert_eq!(config.embeddings.provider, EmbeddingProviderKind::Candle);
        assert_eq!(
            config.embeddings.candle.model_dir.as_deref(),
            Some(".aether/models")
        );
    }

    #[test]
    fn ensure_workspace_config_does_not_overwrite_existing_values() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[general]
log_level = ""

[inference]
provider = "qwen3_local"
api_key_env = "CUSTOM_KEY"

[storage]
mirror_sir_files = false
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
model = "qwen3-embeddings-4B"

[verify]
mode = "container"
commands = ["cargo --version"]
[verify.container]
runtime = "docker"
image = "rust:1-bookworm"
workdir = "/workspace"
fallback_to_host_on_unavailable = true
[verify.microvm]
runtime = "firecracker"
kernel_image = "./kernel"
rootfs_image = "./rootfs.ext4"
workdir = "/workspace"
vcpu_count = 2
memory_mib = 2048
fallback_to_container_on_unavailable = true
fallback_to_host_on_unavailable = true
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let before = fs::read_to_string(config_path(workspace)).expect("read before");
        let config = ensure_workspace_config(workspace).expect("ensure config");
        let after = fs::read_to_string(config_path(workspace)).expect("read after");

        assert_eq!(before, after);
        assert_eq!(config.general.log_level, DEFAULT_LOG_LEVEL);
        assert_eq!(config.inference.provider, InferenceProviderKind::Qwen3Local);
        assert_eq!(config.inference.api_key_env, "CUSTOM_KEY");
        assert!(!config.storage.mirror_sir_files);
        assert_eq!(config.storage.graph_backend, GraphBackend::Sqlite);
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::Qwen3Local
        );
        assert_eq!(
            config.embeddings.vector_backend,
            EmbeddingVectorBackend::Sqlite
        );
        assert_eq!(config.search.reranker, SearchRerankerKind::None);
        assert_eq!(config.search.rerank_window, 50);
        assert_eq!(
            config.search.thresholds.default,
            DEFAULT_SEARCH_THRESHOLD_DEFAULT
        );
        assert_eq!(config.search.thresholds.rust, DEFAULT_SEARCH_THRESHOLD_RUST);
        assert_eq!(
            config.search.thresholds.typescript,
            DEFAULT_SEARCH_THRESHOLD_TYPESCRIPT
        );
        assert_eq!(
            config.search.thresholds.python,
            DEFAULT_SEARCH_THRESHOLD_PYTHON
        );
        assert_eq!(
            config.providers.cohere.api_key_env,
            DEFAULT_COHERE_API_KEY_ENV
        );
        assert_eq!(config.verify.commands, vec!["cargo --version".to_owned()]);
        assert_eq!(config.verify.mode, VerifyMode::Container);
        assert!(config.verify.container.fallback_to_host_on_unavailable);
        assert!(config.verify.microvm.fallback_to_container_on_unavailable);
        assert!(config.verify.microvm.fallback_to_host_on_unavailable);
        assert!(config.coupling.enabled);
        assert_eq!(config.coupling.commit_window, 500);
        assert_eq!(config.coupling.min_co_change_count, 3);
        assert_eq!(config.coupling.bulk_commit_threshold, 30);
        assert!((config.coupling.temporal_weight - 0.5).abs() < 1e-6);
        assert!((config.coupling.static_weight - 0.3).abs() < 1e-6);
        assert!((config.coupling.semantic_weight - 0.2).abs() < 1e-6);
        assert!(config.drift.enabled);
        assert_eq!(config.drift.drift_threshold, DEFAULT_DRIFT_THRESHOLD);
        assert_eq!(config.drift.analysis_window, DEFAULT_DRIFT_ANALYSIS_WINDOW);
        assert!(!config.drift.auto_analyze);
        assert_eq!(config.drift.hub_percentile, DEFAULT_DRIFT_HUB_PERCENTILE);
        assert!(config.health.enabled);
        assert_eq!(
            config.health.risk_weights.pagerank,
            DEFAULT_HEALTH_PAGERANK_WEIGHT
        );
        assert_eq!(
            config.health_score.file_loc_warn,
            DEFAULT_HEALTH_SCORE_FILE_LOC_WARN
        );
        assert_eq!(
            config.health_score.file_loc_fail,
            DEFAULT_HEALTH_SCORE_FILE_LOC_FAIL
        );
        assert!(config.batch.is_none());
        assert!(config.watcher.is_none());
    }

    #[test]
    fn parse_workspace_config_str_rewrites_legacy_sir_quality_keys() {
        let config = parse_workspace_config_str(
            r#"
[sir_quality]
deep_pass = true
deep_priority_threshold = 0.75
deep_concurrency = 9
"#,
        )
        .expect("parse config");

        assert!(config.sir_quality.triage_pass);
        assert_eq!(config.sir_quality.triage_priority_threshold, 0.75);
        assert_eq!(config.sir_quality.triage_concurrency, 9);
        assert!(!config.sir_quality.deep_pass);
        assert_eq!(config.sir_quality.deep_priority_threshold, 0.9);
    }

    #[test]
    fn load_workspace_config_keeps_new_triage_and_deep_schema_distinct() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");

        let raw = r#"
[sir_quality]
triage_pass = true
triage_priority_threshold = 0.25
triage_confidence_threshold = 0.5
triage_concurrency = 0
triage_timeout_secs = 0
deep_pass = true
deep_priority_threshold = 0.95
deep_confidence_threshold = 0.8
deep_max_symbols = 7
deep_concurrency = 0
deep_timeout_secs = 0
"#;
        fs::write(config_path(workspace), raw).expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert!(config.sir_quality.triage_pass);
        assert_eq!(config.sir_quality.triage_priority_threshold, 0.25);
        assert_eq!(config.sir_quality.triage_confidence_threshold, 0.5);
        assert_eq!(config.sir_quality.triage_concurrency, 4);
        assert_eq!(config.sir_quality.triage_timeout_secs, 180);
        assert!(config.sir_quality.deep_pass);
        assert_eq!(config.sir_quality.deep_priority_threshold, 0.95);
        assert_eq!(config.sir_quality.deep_confidence_threshold, 0.8);
        assert_eq!(config.sir_quality.deep_max_symbols, 7);
        assert_eq!(config.sir_quality.deep_concurrency, 4);
        assert_eq!(config.sir_quality.deep_timeout_secs, 180);
    }

    #[test]
    fn save_workspace_config_writes_search_threshold_sections() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        let mut config = AetherConfig::default();
        config.search.thresholds.rust = 0.73;
        config.search.calibrated_thresholds.rust = Some(0.71);

        save_workspace_config(workspace, &config).expect("save config");
        let stored = load_workspace_config(workspace).expect("load config");
        assert_eq!(stored.search.thresholds.rust, 0.73);
        assert_eq!(stored.search.calibrated_thresholds.rust, Some(0.71));

        let rendered = fs::read_to_string(config_path(workspace)).expect("read config");
        assert!(rendered.contains("[search.thresholds]"));
        assert!(rendered.contains("[search.calibrated_thresholds]"));
    }

    #[test]
    fn save_workspace_config_writes_health_score_section() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        let mut config = AetherConfig::default();
        config.health_score.file_loc_warn = 900;
        config.health_score.stale_ref_patterns =
            vec![LEGACY_COZO_GRAPH_STORE.to_owned(), "LegacyStore".to_owned()];

        save_workspace_config(workspace, &config).expect("save config");
        let stored = load_workspace_config(workspace).expect("load config");
        assert_eq!(stored.health_score.file_loc_warn, 900);
        assert_eq!(
            stored.health_score.stale_ref_patterns,
            vec![LEGACY_COZO_GRAPH_STORE.to_owned(), "LegacyStore".to_owned()]
        );

        let rendered = fs::read_to_string(config_path(workspace)).expect("read config");
        assert!(rendered.contains("[health_score]"));
        assert!(rendered.contains("file_loc_warn = 900"));
    }

    #[test]
    fn load_workspace_config_parses_openai_compat_provider() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");
        fs::write(
            config_path(workspace),
            r#"
[inference]
provider = "openai_compat"
model = "glm-4.7"
endpoint = "https://api.z.ai/api/paas/v4"
"#,
        )
        .expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert_eq!(
            config.inference.provider,
            InferenceProviderKind::OpenAiCompat
        );
    }

    #[test]
    fn load_workspace_config_parses_openai_compat_embedding_provider() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");
        fs::write(
            config_path(workspace),
            r#"
[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
endpoint = "https://openrouter.ai/api/v1"
task_type = " CODE_RETRIEVAL "
dimensions = 3072
"#,
        )
        .expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::OpenAiCompat
        );
        assert_eq!(
            config.embeddings.api_key_env.as_deref(),
            Some(DEFAULT_OPENAI_COMPAT_API_KEY_ENV)
        );
        assert_eq!(
            config.embeddings.task_type.as_deref(),
            Some("CODE_RETRIEVAL")
        );
        assert_eq!(config.embeddings.dimensions, Some(3072));
    }

    #[test]
    fn load_workspace_config_parses_gemini_native_embedding_provider() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(aether_dir(workspace)).expect("create .aether");
        fs::write(
            config_path(workspace),
            r#"
[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = " GEMINI_API_KEY "
dimensions = 3072
"#,
        )
        .expect("write config");

        let config = load_workspace_config(workspace).expect("load config");
        assert!(config.embeddings.enabled);
        assert_eq!(
            config.embeddings.provider,
            EmbeddingProviderKind::GeminiNative
        );
        assert_eq!(
            config.embeddings.model.as_deref(),
            Some("gemini-embedding-2-preview")
        );
        assert_eq!(
            config.embeddings.api_key_env.as_deref(),
            Some("GEMINI_API_KEY")
        );
        assert_eq!(config.embeddings.dimensions, Some(3072));
    }

    #[test]
    fn parse_workspace_config_str_accepts_empty_toml_with_batch_and_watcher_absent() {
        let config = parse_workspace_config_str("").expect("parse empty config");
        assert_eq!(config, AetherConfig::default());
        assert!(config.batch.is_none());
        assert!(config.watcher.is_none());
    }

    #[test]
    fn parse_workspace_config_str_reads_batch_and_watcher_sections() {
        let config = parse_workspace_config_str(
            r#"
[batch]
scan_model = "gemini-3.1-flash-lite-preview"
triage_model = "gemini-3.1-pro-preview"
deep_model = "gemini-3.1-pro-preview"
scan_thinking = "low"
triage_thinking = "medium"
deep_thinking = "high"
triage_neighbor_depth = 2
deep_neighbor_depth = 3
scan_max_chars = 9000
triage_max_chars = 12000
deep_max_chars = 16000
passes = ["scan", "triage", "deep"]
auto_chain = false
batch_dir = ".aether/custom-batch"
poll_interval_secs = 120
jsonl_chunk_size = 1234

[watcher]
realtime_model = "gemini-3.1-pro-preview"
realtime_provider = "gemini"
trigger_on_branch_switch = false
trigger_on_git_pull = true
trigger_on_merge = false
git_trigger_changed_files_only = false
git_debounce_secs = 4.5
trigger_on_build_success = true
"#,
        )
        .expect("parse config with batch and watcher");

        assert_eq!(
            config.batch,
            Some(BatchConfig {
                scan_model: "gemini-3.1-flash-lite-preview".to_owned(),
                triage_model: "gemini-3.1-pro-preview".to_owned(),
                deep_model: "gemini-3.1-pro-preview".to_owned(),
                scan_thinking: "low".to_owned(),
                triage_thinking: "medium".to_owned(),
                deep_thinking: "high".to_owned(),
                triage_neighbor_depth: 2,
                deep_neighbor_depth: 3,
                scan_max_chars: 9000,
                triage_max_chars: 12000,
                deep_max_chars: 16000,
                passes: vec!["scan".to_owned(), "triage".to_owned(), "deep".to_owned()],
                auto_chain: false,
                batch_dir: ".aether/custom-batch".to_owned(),
                poll_interval_secs: 120,
                jsonl_chunk_size: 1234,
                prompt_tier: "auto".to_owned(),
                provider: "gemini".to_owned(),
                gemini: BatchProviderConfig::default(),
                openai: BatchProviderConfig::default(),
                anthropic: BatchProviderConfig::default(),
            })
        );
        assert_eq!(
            config.watcher,
            Some(WatcherConfig {
                realtime_model: "gemini-3.1-pro-preview".to_owned(),
                realtime_provider: "gemini".to_owned(),
                trigger_on_branch_switch: false,
                trigger_on_git_pull: true,
                trigger_on_merge: false,
                git_trigger_changed_files_only: false,
                git_debounce_secs: 4.5,
                trigger_on_build_success: true,
            })
        );
    }
}
