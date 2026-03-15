use crate::{
    constants::{DEFAULT_COHERE_API_KEY_ENV, DEFAULT_GEMINI_API_KEY_ENV},
    embeddings::EmbeddingProviderKind,
    inference::InferenceProviderKind,
    root::AetherConfig,
    search::SearchRerankerKind,
    storage::GraphBackend,
    verification::{VerifyContainerConfig, VerifyMicrovmConfig, VerifyMode},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub code: &'static str,
    pub message: String,
}

pub fn validate_config(config: &AetherConfig) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();

    if config.storage.graph_backend == GraphBackend::Cozo {
        warnings.push(ConfigWarning {
            code: "graph_backend_cozo_deprecated",
            message:
                "storage.graph_backend=cozo is deprecated; run `aether graph-migrate` and switch to surreal"
                    .to_owned(),
        });
    }

    if !config.embeddings.enabled {
        if config.embeddings.model.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_model_ignored",
                message:
                    "embeddings.model is set but embeddings.enabled=false; model will be ignored"
                        .to_owned(),
            });
        }
        if config.embeddings.endpoint.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_endpoint_ignored",
                message:
                    "embeddings.endpoint is set but embeddings.enabled=false; endpoint will be ignored"
                        .to_owned(),
            });
        }
        if config.embeddings.candle.model_dir.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_candle_model_dir_ignored",
                message:
                    "embeddings.candle.model_dir is set but embeddings.enabled=false; model_dir will be ignored"
                        .to_owned(),
            });
        }
    } else if matches!(
        config.embeddings.provider,
        EmbeddingProviderKind::Qwen3Local
    ) && config.embeddings.candle.model_dir.is_some()
    {
        warnings.push(ConfigWarning {
            code: "embeddings_candle_model_dir_unused_for_qwen3_local",
            message: "embeddings.provider=qwen3_local ignores embeddings.candle.model_dir"
                .to_owned(),
        });
    } else if matches!(
        config.embeddings.provider,
        EmbeddingProviderKind::OpenAiCompat
    ) {
        if config.embeddings.endpoint.is_none() {
            warnings.push(ConfigWarning {
                code: "embeddings_endpoint_missing_for_openai_compat",
                message: "embeddings.provider=openai_compat requires embeddings.endpoint"
                    .to_owned(),
            });
        }
    } else if matches!(
        config.embeddings.provider,
        EmbeddingProviderKind::GeminiNative
    ) {
        if config.embeddings.endpoint.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_endpoint_unused_for_gemini_native",
                message: "embeddings.provider=gemini_native ignores embeddings.endpoint".to_owned(),
            });
        }
        if config.embeddings.task_type.is_some() {
            warnings.push(ConfigWarning {
                code: "embeddings_task_type_unused_for_gemini_native",
                message: "embeddings.provider=gemini_native ignores embeddings.task_type"
                    .to_owned(),
            });
        }
    }

    if matches!(config.search.reranker, SearchRerankerKind::None)
        && config.search.candle.model_dir.is_some()
    {
        warnings.push(ConfigWarning {
            code: "search_candle_model_dir_unused_for_none",
            message: "search.reranker=none ignores search.candle.model_dir".to_owned(),
        });
    } else if matches!(config.search.reranker, SearchRerankerKind::Cohere)
        && config.search.candle.model_dir.is_some()
    {
        warnings.push(ConfigWarning {
            code: "search_candle_model_dir_unused_for_cohere",
            message: "search.reranker=cohere ignores search.candle.model_dir".to_owned(),
        });
    }

    if !matches!(config.search.reranker, SearchRerankerKind::Cohere)
        && config.providers.cohere.api_key_env != DEFAULT_COHERE_API_KEY_ENV
    {
        warnings.push(ConfigWarning {
            code: "providers_cohere_api_key_env_ignored",
            message: "providers.cohere.api_key_env is ignored unless search.reranker=cohere"
                .to_owned(),
        });
    }

    match config.inference.provider {
        InferenceProviderKind::Auto => {}
        InferenceProviderKind::Tiered => {
            if config.inference.tiered.is_none() {
                warnings.push(ConfigWarning {
                    code: "inference_tiered_config_missing",
                    message: "inference.provider=tiered requires [inference.tiered] config"
                        .to_owned(),
                });
            }
            if config.inference.model.is_some() {
                warnings.push(ConfigWarning {
                    code: "inference_model_ignored_for_tiered",
                    message: "inference.provider=tiered ignores inference.model".to_owned(),
                });
            }
            if config.inference.endpoint.is_some() {
                warnings.push(ConfigWarning {
                    code: "inference_endpoint_ignored_for_tiered",
                    message: "inference.provider=tiered ignores inference.endpoint".to_owned(),
                });
            }
            if config.inference.api_key_env != DEFAULT_GEMINI_API_KEY_ENV {
                warnings.push(ConfigWarning {
                    code: "inference_api_key_env_ignored_for_tiered",
                    message: "inference.api_key_env is ignored when inference.provider=tiered"
                        .to_owned(),
                });
            }
        }
        InferenceProviderKind::Gemini => {
            if config.inference.endpoint.is_some() {
                warnings.push(ConfigWarning {
                    code: "inference_endpoint_ignored_for_gemini",
                    message: "inference.provider=gemini ignores inference.endpoint".to_owned(),
                });
            }
        }
        InferenceProviderKind::Qwen3Local => {
            if config.inference.api_key_env != DEFAULT_GEMINI_API_KEY_ENV {
                warnings.push(ConfigWarning {
                    code: "inference_api_key_env_ignored_for_qwen3_local",
                    message: "inference.api_key_env is ignored when inference.provider=qwen3_local"
                        .to_owned(),
                });
            }
        }
        InferenceProviderKind::OpenAiCompat => {}
    }

    if config.verify.commands.is_empty() {
        warnings.push(ConfigWarning {
            code: "verify_commands_empty",
            message:
                "verify.commands is empty; aetherd --verify and aether_verify will have no commands to run"
                    .to_owned(),
        });
    }

    let container_defaults = VerifyContainerConfig::default();
    let container_settings_ignored =
        config.verify.mode == VerifyMode::Host && config.verify.container != container_defaults;
    if container_settings_ignored {
        warnings.push(ConfigWarning {
            code: "verify_container_settings_ignored_for_host",
            message: "verify.mode=host ignores verify.container settings".to_owned(),
        });
    }

    let microvm_defaults = VerifyMicrovmConfig::default();
    let microvm_settings_ignored =
        config.verify.mode != VerifyMode::Microvm && config.verify.microvm != microvm_defaults;
    if microvm_settings_ignored {
        warnings.push(ConfigWarning {
            code: "verify_microvm_settings_ignored_for_non_microvm",
            message: "verify.microvm settings are ignored unless verify.mode=microvm".to_owned(),
        });
    }

    if config.verify.mode == VerifyMode::Microvm
        && (config.verify.microvm.kernel_image.is_none()
            || config.verify.microvm.rootfs_image.is_none())
    {
        warnings.push(ConfigWarning {
            code: "verify_microvm_assets_missing",
            message:
                "verify.mode=microvm requires verify.microvm.kernel_image and verify.microvm.rootfs_image"
                    .to_owned(),
        });
    }

    let coupling_sum = config.coupling.temporal_weight
        + config.coupling.static_weight
        + config.coupling.semantic_weight;
    if (coupling_sum - 1.0).abs() > 0.01 {
        warnings.push(ConfigWarning {
            code: "coupling_weights_normalized",
            message: format!(
                "coupling weights should sum to 1.0 (found {coupling_sum:.3}); values will be normalized"
            ),
        });
    }

    if let Some(continuous) = &config.continuous {
        let schedule = continuous.schedule.trim().to_ascii_lowercase();
        if !matches!(schedule.as_str(), "hourly" | "nightly") {
            warnings.push(ConfigWarning {
                code: "continuous_schedule_invalid",
                message: format!(
                    "[continuous].schedule='{}' is unsupported; expected 'hourly' or 'nightly'",
                    continuous.schedule
                ),
            });
        }

        let requeue_pass = continuous.requeue_pass.trim().to_ascii_lowercase();
        if !matches!(requeue_pass.as_str(), "scan" | "triage" | "deep") {
            warnings.push(ConfigWarning {
                code: "continuous_requeue_pass_invalid",
                message: format!(
                    "[continuous].requeue_pass='{}' is unsupported; expected one of: scan, triage, deep",
                    continuous.requeue_pass
                ),
            });
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use crate::{
        AetherConfig, DEFAULT_GEMINI_API_KEY_ENV,
        analysis::{CouplingConfig, DriftConfig},
        continuous::ContinuousConfig,
        embeddings::{
            CandleEmbeddingsConfig, EmbeddingProviderKind, EmbeddingVectorBackend, EmbeddingsConfig,
        },
        health::{HealthConfig, HealthScoreConfig},
        inference::{InferenceConfig, InferenceProviderKind, default_sir_concurrency},
        planner::PlannerConfig,
        root::{DashboardConfig, GeneralConfig, parse_workspace_config_str},
        search::{ProvidersConfig, SearchConfig},
        sir_quality::SirQualityConfig,
        storage::{GraphBackend, StorageConfig},
        verification::{VerifyConfig, VerifyMode},
    };

    use super::{ConfigWarning, validate_config};

    fn warning_codes(warnings: &[ConfigWarning]) -> Vec<&'static str> {
        warnings.iter().map(|warning| warning.code).collect()
    }

    #[test]
    fn validate_config_reports_ignored_fields() {
        let config = AetherConfig {
            general: GeneralConfig::default(),
            inference: InferenceConfig {
                provider: InferenceProviderKind::Auto,
                model: None,
                endpoint: Some("http://127.0.0.1:11434".to_owned()),
                api_key_env: DEFAULT_GEMINI_API_KEY_ENV.to_owned(),
                concurrency: default_sir_concurrency(),
                tiered: None,
            },
            sir_quality: SirQualityConfig::default(),
            storage: StorageConfig {
                mirror_sir_files: true,
                graph_backend: GraphBackend::Cozo,
            },
            embeddings: EmbeddingsConfig {
                enabled: false,
                provider: EmbeddingProviderKind::Qwen3Local,
                vector_backend: EmbeddingVectorBackend::Lancedb,
                model: Some("mock-x".to_owned()),
                endpoint: Some("http://127.0.0.1:11434/api/embeddings".to_owned()),
                api_key_env: None,
                task_type: None,
                dimensions: None,
                candle: CandleEmbeddingsConfig::default(),
            },
            search: SearchConfig::default(),
            providers: ProvidersConfig::default(),
            verify: VerifyConfig {
                commands: vec!["cargo test".to_owned()],
                ..VerifyConfig::default()
            },
            coupling: CouplingConfig::default(),
            drift: DriftConfig::default(),
            health: HealthConfig::default(),
            planner: PlannerConfig::default(),
            health_score: HealthScoreConfig::default(),
            dashboard: DashboardConfig::default(),
            continuous: None,
            batch: None,
            watcher: None,
        };

        let codes = warning_codes(&validate_config(&config));
        assert!(codes.contains(&"embeddings_model_ignored"));
        assert!(codes.contains(&"embeddings_endpoint_ignored"));
        assert!(codes.contains(&"graph_backend_cozo_deprecated"));
    }

    #[test]
    fn validate_config_is_quiet_for_defaults() {
        let warnings = validate_config(&AetherConfig::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_config_warns_when_verify_commands_empty() {
        let config = AetherConfig {
            verify: VerifyConfig {
                commands: Vec::new(),
                ..VerifyConfig::default()
            },
            ..AetherConfig::default()
        };

        let codes = warning_codes(&validate_config(&config));
        assert!(codes.contains(&"verify_commands_empty"));
    }

    #[test]
    fn validate_config_warns_when_host_mode_ignores_container_settings() {
        let mut config = AetherConfig::default();
        config.verify.container.image = "rust:latest".to_owned();

        let codes = warning_codes(&validate_config(&config));
        assert!(codes.contains(&"verify_container_settings_ignored_for_host"));
    }

    #[test]
    fn validate_config_warns_when_non_microvm_mode_ignores_microvm_settings() {
        let mut config = AetherConfig::default();
        config.verify.mode = VerifyMode::Host;
        config.verify.microvm.runtime = "custom-runtime".to_owned();

        let codes = warning_codes(&validate_config(&config));
        assert!(codes.contains(&"verify_microvm_settings_ignored_for_non_microvm"));
    }

    #[test]
    fn validate_config_warns_when_microvm_assets_missing() {
        let mut config = AetherConfig::default();
        config.verify.mode = VerifyMode::Microvm;
        config.verify.microvm.kernel_image = Some("kernel".to_owned());
        config.verify.microvm.rootfs_image = None;

        let codes = warning_codes(&validate_config(&config));
        assert!(codes.contains(&"verify_microvm_assets_missing"));
    }

    #[test]
    fn validate_config_warns_on_openai_compat_without_endpoint() {
        let config = parse_workspace_config_str(
            r#"
[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
"#,
        )
        .expect("parse config");

        let codes = warning_codes(&validate_config(&config));
        assert!(codes.contains(&"embeddings_endpoint_missing_for_openai_compat"));
    }

    #[test]
    fn validate_config_warns_on_unused_gemini_native_fields() {
        let config = parse_workspace_config_str(
            r#"
[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = "GEMINI_API_KEY"
endpoint = "https://ignored.example/v1"
task_type = "RETRIEVAL_DOCUMENT"
"#,
        )
        .expect("parse config");

        let codes = warning_codes(&validate_config(&config));
        assert!(codes.contains(&"embeddings_endpoint_unused_for_gemini_native"));
        assert!(codes.contains(&"embeddings_task_type_unused_for_gemini_native"));
    }

    #[test]
    fn validate_config_warns_on_invalid_continuous_values() {
        let config = AetherConfig {
            continuous: Some(ContinuousConfig {
                schedule: "weekly".to_owned(),
                requeue_pass: "unknown".to_owned(),
                ..ContinuousConfig::default()
            }),
            ..AetherConfig::default()
        };

        let codes = warning_codes(&validate_config(&config));
        assert!(codes.contains(&"continuous_schedule_invalid"));
        assert!(codes.contains(&"continuous_requeue_pass_invalid"));
    }
}
