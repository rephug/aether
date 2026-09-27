use std::env;
use std::path::{Path, PathBuf};

use aether_config::{
    AETHER_DIR_NAME, DEFAULT_COHERE_API_KEY_ENV, DEFAULT_GEMINI_API_KEY_ENV,
    DEFAULT_OMP_GATEWAY_ENDPOINT, DEFAULT_OMP_GATEWAY_TOKEN_ENV, DEFAULT_OPENAI_COMPAT_API_KEY_ENV,
    DEFAULT_QWEN_ENDPOINT, DEFAULT_QWEN_MODEL, EmbeddingProviderKind, InferenceProviderKind,
    OMP_GATEWAY_TOKEN_FILE, SearchRerankerKind, TieredConfig, ensure_workspace_config,
};
use aether_core::Secret;

use crate::embedding::Qwen3LocalEmbeddingProvider;
use crate::embedding::candle::CandleEmbeddingProvider;
use crate::embedding::gemini_native::GeminiNativeEmbeddingProvider;
use crate::embedding::openai_compat::OpenAiCompatEmbeddingProvider;
use crate::http::{is_ollama_reachable, is_ollama_reachable_blocking};
use crate::providers::gemini::{request_gemini_summary, resolve_gemini_model};
use crate::providers::qwen_local::request_qwen_summary;
use crate::providers::{
    GeminiProvider, MockInferenceProvider, OpenAiCompatProvider, Qwen3LocalProvider, TieredProvider,
};
use crate::reranker::candle::CandleRerankerProvider;
use crate::reranker::cohere::CohereRerankerProvider;
use crate::types::{
    EmbeddingProviderOverrides, InferError, InferenceProvider, LoadedEmbeddingProvider,
    LoadedProvider, LoadedRerankerProvider, ProviderOverrides, RerankerProviderOverrides,
    first_non_empty, normalize_optional,
};

fn resolve_inference_thinking(
    override_thinking: Option<String>,
    configured_thinking: Option<String>,
) -> Option<String> {
    first_non_empty(override_thinking, configured_thinking)
}

pub fn load_inference_provider_from_config(
    workspace_root: impl AsRef<Path>,
    overrides: ProviderOverrides,
) -> Result<LoadedProvider, InferError> {
    let config = ensure_workspace_config(workspace_root)?;

    let ProviderOverrides {
        provider,
        model,
        endpoint,
        api_key_env,
        thinking,
    } = overrides;
    let selected_provider = provider.unwrap_or(config.inference.provider);
    let selected_model = first_non_empty(model, config.inference.model);
    let selected_endpoint = first_non_empty(endpoint, config.inference.endpoint);
    let selected_thinking = resolve_inference_thinking(thinking, config.inference.thinking);
    let selected_api_key_env = resolve_inference_api_key_env(
        selected_provider,
        api_key_env,
        Some(config.inference.api_key_env),
    );

    match selected_provider {
        InferenceProviderKind::Auto => {
            let ollama_endpoint = normalize_optional(selected_endpoint.clone())
                .unwrap_or_else(|| DEFAULT_QWEN_ENDPOINT.to_owned());
            if is_ollama_reachable_blocking(&ollama_endpoint) {
                let provider =
                    Qwen3LocalProvider::new(Some(ollama_endpoint.clone()), selected_model.clone());
                tracing::info!(
                    endpoint = %ollama_endpoint,
                    model = %provider.model_name(),
                    "Auto provider selected qwen3_local after reaching Ollama"
                );
                Ok(LoadedProvider {
                    model_name: provider.model_name(),
                    provider: Box::new(provider),
                    provider_name: InferenceProviderKind::Qwen3Local.as_str().to_owned(),
                })
            } else if let Some(api_key) = read_env_non_empty(&selected_api_key_env) {
                let model = resolve_gemini_model(selected_model);
                tracing::info!(
                    api_key_env = %selected_api_key_env,
                    model = %model,
                    "Auto provider selected gemini after finding API key"
                );
                Ok(LoadedProvider {
                    provider: Box::new(GeminiProvider::new(
                        Secret::new(api_key),
                        model.clone(),
                        selected_thinking.clone(),
                    )),
                    provider_name: InferenceProviderKind::Gemini.as_str().to_owned(),
                    model_name: model,
                })
            } else {
                Err(InferError::NoProviderAvailable(
                    no_provider_available_message(&ollama_endpoint, &selected_api_key_env),
                ))
            }
        }
        InferenceProviderKind::Tiered => {
            let tiered = config.inference.tiered.as_ref().ok_or_else(|| {
                InferError::InvalidConfig(
                    "inference.provider=tiered requires [inference.tiered]".to_owned(),
                )
            })?;
            load_tiered_provider(
                tiered,
                selected_model,
                selected_endpoint,
                selected_api_key_env,
                selected_thinking,
            )
        }
        InferenceProviderKind::Gemini => {
            let provider = GeminiProvider::from_env_key(
                &selected_api_key_env,
                selected_model,
                selected_thinking,
            )?;
            Ok(LoadedProvider {
                model_name: provider.model_name(),
                provider: Box::new(provider),
                provider_name: InferenceProviderKind::Gemini.as_str().to_owned(),
            })
        }
        InferenceProviderKind::Qwen3Local => {
            let provider = Qwen3LocalProvider::new(selected_endpoint, selected_model);
            Ok(LoadedProvider {
                model_name: provider.model_name(),
                provider: Box::new(provider),
                provider_name: InferenceProviderKind::Qwen3Local.as_str().to_owned(),
            })
        }
        InferenceProviderKind::OpenAiCompat => {
            let api_key = read_env_non_empty(&selected_api_key_env)
                .ok_or_else(|| InferError::MissingApiKey(selected_api_key_env.clone()))?;
            let api_base = selected_endpoint.ok_or(InferError::MissingEndpoint)?;
            let model = selected_model.ok_or(InferError::MissingModel)?;
            let provider = OpenAiCompatProvider::new(Secret::new(api_key), api_base, model.clone());
            Ok(LoadedProvider {
                model_name: model,
                provider: Box::new(provider),
                provider_name: InferenceProviderKind::OpenAiCompat.as_str().to_owned(),
            })
        }
        InferenceProviderKind::Omp => {
            let provider = build_omp_provider(
                &selected_api_key_env,
                selected_endpoint,
                selected_model,
                selected_thinking,
            )?;
            Ok(LoadedProvider {
                model_name: provider.model_name(),
                provider: Box::new(provider),
                provider_name: InferenceProviderKind::Omp.as_str().to_owned(),
            })
        }
        InferenceProviderKind::Mock => {
            let provider = MockInferenceProvider::new();
            tracing::info!(
                "mock provider selected: placeholder [MOCK] SIRs at confidence 0.1, no API key needed"
            );
            Ok(LoadedProvider {
                model_name: provider.model_name(),
                provider_name: provider.provider_name(),
                provider: Box::new(provider),
            })
        }
    }
}

/// Build the omp gateway provider: OpenAI-compatible transport against
/// `omp auth-gateway serve`, model addressed as an omp route (`provider/model`).
fn build_omp_provider(
    api_key_env: &str,
    endpoint: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
) -> Result<OpenAiCompatProvider, InferError> {
    let token = resolve_omp_gateway_token(api_key_env)?;
    let api_base =
        normalize_optional(endpoint).unwrap_or_else(|| DEFAULT_OMP_GATEWAY_ENDPOINT.to_owned());
    let model = normalize_optional(model).ok_or_else(|| {
        InferError::InvalidConfig(
            "inference.provider=omp requires inference.model as an omp route (provider/model, \
             e.g. anthropic/claude-fable-5)"
                .to_owned(),
        )
    })?;
    if aether_config::split_omp_route(&model).is_none() {
        return Err(InferError::InvalidConfig(format!(
            "inference.model '{model}' is not an omp route; expected provider/model \
             (e.g. anthropic/claude-fable-5)"
        )));
    }
    tracing::info!(
        endpoint = %api_base,
        model = %model,
        reasoning_effort = ?omp_reasoning_effort(thinking.as_deref()),
        "omp provider selected (Oh My Pi auth gateway)"
    );
    Ok(
        OpenAiCompatProvider::new(Secret::new(token), api_base, model)
            .with_provider_name(InferenceProviderKind::Omp.as_str())
            .with_reasoning_effort(omp_reasoning_effort(thinking.as_deref())),
    )
}

/// Map `[inference].thinking` onto the gateway's `reasoning_effort` vocabulary.
///
/// Unknown values (including `dynamic`, `off`, `none`) omit the field so the gateway applies
/// the model's own default.
fn omp_reasoning_effort(thinking: Option<&str>) -> Option<String> {
    let value = thinking?.trim().to_ascii_lowercase();
    match value.as_str() {
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => Some(value),
        _ => None,
    }
}

/// Locate the gateway bearer token: the configured env var first, then the token file that
/// `omp auth-gateway token` writes under `$HOME`.
fn resolve_omp_gateway_token(api_key_env: &str) -> Result<String, InferError> {
    resolve_omp_gateway_token_from(api_key_env, omp_gateway_token_path())
}

fn resolve_omp_gateway_token_from(
    api_key_env: &str,
    token_file: Option<PathBuf>,
) -> Result<String, InferError> {
    if let Some(token) = read_env_non_empty(api_key_env) {
        return Ok(token);
    }
    if let Some(path) = token_file.as_ref()
        && let Ok(contents) = std::fs::read_to_string(path)
    {
        let token = contents.trim();
        if !token.is_empty() {
            return Ok(token.to_owned());
        }
    }
    Err(InferError::InvalidConfig(format!(
        "no omp gateway token: set {} or run `omp auth-gateway token` (looked for {})",
        api_key_env,
        token_file
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| format!("$HOME/{OMP_GATEWAY_TOKEN_FILE}"))
    )))
}

fn omp_gateway_token_path() -> Option<PathBuf> {
    // `OMP_GATEWAY_TOKEN_FILE` lets a container or CI point at a mounted token without
    // pretending to have a home directory.
    if let Some(explicit) = read_env_non_empty("OMP_GATEWAY_TOKEN_FILE") {
        return Some(PathBuf::from(explicit));
    }
    let home = read_env_non_empty("HOME").or_else(|| read_env_non_empty("USERPROFILE"))?;
    Some(Path::new(&home).join(OMP_GATEWAY_TOKEN_FILE))
}

pub fn load_provider_from_env_or_mock(
    workspace_root: impl AsRef<Path>,
    overrides: ProviderOverrides,
) -> Result<LoadedProvider, InferError> {
    load_inference_provider_from_config(workspace_root, overrides)
}

pub async fn summarize_text_with_config(
    workspace_root: impl AsRef<Path>,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<Option<String>, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let selected_provider = config.inference.provider;
    let selected_model = config.inference.model;
    let selected_endpoint = config.inference.endpoint;
    let selected_api_key_env =
        resolve_inference_api_key_env(selected_provider, None, Some(config.inference.api_key_env));
    let system_prompt = system_prompt.trim();
    let user_prompt = user_prompt.trim();
    if system_prompt.is_empty() || user_prompt.is_empty() {
        return Ok(None);
    }

    match selected_provider {
        InferenceProviderKind::Auto => {
            let endpoint = normalize_optional(selected_endpoint.clone())
                .unwrap_or_else(|| DEFAULT_QWEN_ENDPOINT.to_owned());
            if is_ollama_reachable(endpoint.as_str()).await {
                let model = normalize_optional(selected_model.clone())
                    .unwrap_or_else(|| DEFAULT_QWEN_MODEL.to_owned());
                tracing::info!(
                    endpoint = %endpoint,
                    model = %model,
                    "Auto provider selected qwen3_local summary path after reaching Ollama"
                );
                let summary = request_qwen_summary(
                    endpoint.as_str(),
                    model.as_str(),
                    system_prompt,
                    user_prompt,
                )
                .await?;
                Ok(clean_summary(summary))
            } else if let Some(api_key) = read_env_non_empty(selected_api_key_env.as_str()) {
                let model = resolve_gemini_model(selected_model);
                let api_key = Secret::new(api_key);
                tracing::info!(
                    api_key_env = %selected_api_key_env,
                    model = %model,
                    "Auto provider selected gemini summary path after finding API key"
                );
                let summary =
                    request_gemini_summary(&api_key, model.as_str(), system_prompt, user_prompt)
                        .await?;
                Ok(clean_summary(summary))
            } else {
                Err(InferError::NoProviderAvailable(
                    no_provider_available_message(endpoint.as_str(), selected_api_key_env.as_str()),
                ))
            }
        }
        InferenceProviderKind::Tiered => {
            let tiered = config.inference.tiered.as_ref().ok_or_else(|| {
                InferError::InvalidConfig(
                    "inference.provider=tiered requires [inference.tiered]".to_owned(),
                )
            })?;
            summarize_text_with_tiered(
                tiered,
                1.0,
                system_prompt,
                user_prompt,
                selected_model,
                selected_endpoint,
                selected_api_key_env,
            )
            .await
        }
        InferenceProviderKind::Gemini => {
            let Some(api_key) = read_env_non_empty(selected_api_key_env.as_str()) else {
                return Ok(None);
            };
            let model = resolve_gemini_model(selected_model);
            let api_key = Secret::new(api_key);
            let summary =
                request_gemini_summary(&api_key, model.as_str(), system_prompt, user_prompt)
                    .await?;
            Ok(clean_summary(summary))
        }
        InferenceProviderKind::Qwen3Local => {
            let endpoint = normalize_optional(selected_endpoint)
                .unwrap_or_else(|| DEFAULT_QWEN_ENDPOINT.to_owned());
            let model =
                normalize_optional(selected_model).unwrap_or_else(|| DEFAULT_QWEN_MODEL.to_owned());
            let summary = request_qwen_summary(
                endpoint.as_str(),
                model.as_str(),
                system_prompt,
                user_prompt,
            )
            .await?;
            Ok(clean_summary(summary))
        }
        InferenceProviderKind::OpenAiCompat => {
            let Some(api_key) = read_env_non_empty(selected_api_key_env.as_str()) else {
                return Ok(None);
            };
            let api_base = selected_endpoint.ok_or(InferError::MissingEndpoint)?;
            let model = selected_model.ok_or(InferError::MissingModel)?;
            let provider = OpenAiCompatProvider::new(Secret::new(api_key), api_base, model);
            let summary = provider.request_summary(system_prompt, user_prompt).await?;
            Ok(clean_summary(summary))
        }
        // The mock provider has no model to summarize with.
        InferenceProviderKind::Mock => Ok(None),
        InferenceProviderKind::Omp => {
            if resolve_omp_gateway_token(selected_api_key_env.as_str()).is_err() {
                return Ok(None);
            }
            let provider = build_omp_provider(
                selected_api_key_env.as_str(),
                selected_endpoint,
                selected_model,
                config.inference.thinking,
            )?;
            let summary = provider.request_summary(system_prompt, user_prompt).await?;
            Ok(clean_summary(summary))
        }
    }
}

pub fn load_embedding_provider_from_config(
    workspace_root: impl AsRef<Path>,
    overrides: EmbeddingProviderOverrides,
) -> Result<Option<LoadedEmbeddingProvider>, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let selected_enabled = overrides.enabled.unwrap_or(config.embeddings.enabled);
    if !selected_enabled {
        return Ok(None);
    }

    let selected_provider = overrides.provider.unwrap_or(config.embeddings.provider);
    let selected_model = first_non_empty(overrides.model, config.embeddings.model.clone());
    let selected_endpoint = first_non_empty(overrides.endpoint, config.embeddings.endpoint.clone());
    let selected_api_key_env =
        first_non_empty(overrides.api_key_env, config.embeddings.api_key_env.clone());
    let selected_task_type =
        first_non_empty(overrides.task_type, config.embeddings.task_type.clone());
    let selected_dimensions = overrides.dimensions.or(config.embeddings.dimensions);
    let selected_candle_model_dir = first_non_empty(
        overrides.candle_model_dir,
        config.embeddings.candle.model_dir.clone(),
    )
    .map(PathBuf::from);

    let loaded = match selected_provider {
        EmbeddingProviderKind::Qwen3Local => {
            let provider = Qwen3LocalEmbeddingProvider::new(selected_endpoint, selected_model);
            LoadedEmbeddingProvider {
                model_name: provider.model_name().to_owned(),
                provider: Box::new(provider),
                provider_name: EmbeddingProviderKind::Qwen3Local.as_str().to_owned(),
            }
        }
        EmbeddingProviderKind::Candle => {
            let model_dir = resolve_candle_model_dir(workspace_root, selected_candle_model_dir);
            let provider = CandleEmbeddingProvider::new(model_dir);
            let model_name = provider.model_name().to_owned();
            let provider_name = provider.provider_name().to_owned();
            LoadedEmbeddingProvider {
                model_name,
                provider: Box::new(provider),
                provider_name,
            }
        }
        EmbeddingProviderKind::OpenAiCompat => {
            let selected_api_key_env = selected_api_key_env
                .unwrap_or_else(|| DEFAULT_OPENAI_COMPAT_API_KEY_ENV.to_owned());
            let api_key = read_env_non_empty(&selected_api_key_env)
                .ok_or_else(|| InferError::MissingApiKey(selected_api_key_env.clone()))?;
            let endpoint = selected_endpoint.ok_or(InferError::MissingEndpoint)?;
            let model = selected_model.ok_or(InferError::MissingModel)?;
            let provider = OpenAiCompatEmbeddingProvider::new(
                endpoint,
                model,
                Secret::new(api_key),
                selected_task_type,
                selected_dimensions,
            );
            let model_name = provider.model_name().to_owned();
            let provider_name = provider.provider_name().to_owned();
            LoadedEmbeddingProvider {
                model_name,
                provider: Box::new(provider),
                provider_name,
            }
        }
        EmbeddingProviderKind::GeminiNative => {
            let api_key_env = selected_api_key_env.ok_or_else(|| {
                InferError::InvalidConfig(
                    "gemini_native provider requires embeddings.api_key_env".to_owned(),
                )
            })?;
            let api_key = read_env_non_empty(&api_key_env)
                .ok_or_else(|| InferError::MissingApiKey(api_key_env.clone()))?;
            let model = selected_model.ok_or(InferError::MissingModel)?;
            let provider = GeminiNativeEmbeddingProvider::new(
                model,
                Secret::new(api_key),
                selected_dimensions,
            );
            let model_name = provider.model_name().to_owned();
            let provider_name = provider.provider_name().to_owned();
            LoadedEmbeddingProvider {
                model_name,
                provider: Box::new(provider),
                provider_name,
            }
        }
    };

    Ok(Some(loaded))
}

pub fn load_reranker_provider_from_config(
    workspace_root: impl AsRef<Path>,
    overrides: RerankerProviderOverrides,
) -> Result<Option<LoadedRerankerProvider>, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let selected_provider = overrides.provider.unwrap_or(config.search.reranker);
    let selected_candle_model_dir = first_non_empty(
        overrides.candle_model_dir,
        first_non_empty(
            config.search.candle.model_dir.clone(),
            config.embeddings.candle.model_dir.clone(),
        ),
    )
    .map(PathBuf::from);
    let selected_cohere_api_key_env = first_non_empty(
        overrides.cohere_api_key_env,
        Some(config.providers.cohere.api_key_env.clone()),
    )
    .unwrap_or_else(|| DEFAULT_COHERE_API_KEY_ENV.to_owned());

    let loaded = match selected_provider {
        SearchRerankerKind::None => return Ok(None),
        SearchRerankerKind::Candle => {
            let model_dir = resolve_candle_model_dir(workspace_root, selected_candle_model_dir);
            let provider = CandleRerankerProvider::new(model_dir);
            LoadedRerankerProvider {
                model_name: provider.model_name().to_owned(),
                provider_name: provider.provider_name().to_owned(),
                provider: Box::new(provider),
            }
        }
        SearchRerankerKind::Cohere => {
            let provider = CohereRerankerProvider::from_env(&selected_cohere_api_key_env)?;
            LoadedRerankerProvider {
                model_name: provider.model_name().to_owned(),
                provider_name: provider.provider_name().to_owned(),
                provider: Box::new(provider),
            }
        }
    };

    Ok(Some(loaded))
}

pub fn download_candle_embedding_model(
    workspace_root: impl AsRef<Path>,
    model_dir_override: Option<PathBuf>,
) -> Result<PathBuf, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let configured_model_dir =
        model_dir_override.or_else(|| config.embeddings.candle.model_dir.map(PathBuf::from));
    let model_dir = resolve_candle_model_dir(workspace_root, configured_model_dir);

    let provider = CandleEmbeddingProvider::new(model_dir);
    provider.ensure_model_downloaded()
}

pub fn download_candle_reranker_model(
    workspace_root: impl AsRef<Path>,
    model_dir_override: Option<PathBuf>,
) -> Result<PathBuf, InferError> {
    let workspace_root = workspace_root.as_ref();
    let config = ensure_workspace_config(workspace_root)?;
    let configured_model_dir = model_dir_override
        .or_else(|| config.search.candle.model_dir.map(PathBuf::from))
        .or_else(|| config.embeddings.candle.model_dir.map(PathBuf::from));
    let model_dir = resolve_candle_model_dir(workspace_root, configured_model_dir);

    let provider = CandleRerankerProvider::new(model_dir);
    provider.ensure_model_downloaded()
}

fn load_tiered_provider(
    tiered: &TieredConfig,
    selected_model: Option<String>,
    selected_endpoint: Option<String>,
    selected_api_key_env: String,
    selected_thinking: Option<String>,
) -> Result<LoadedProvider, InferError> {
    let primary_kind = tiered.primary.trim().to_ascii_lowercase();
    let primary_model = first_non_empty(selected_model, tiered.primary_model.clone());
    let primary_endpoint = first_non_empty(selected_endpoint, tiered.primary_endpoint.clone());
    let primary_api_key_env = normalize_optional(Some(selected_api_key_env))
        .or_else(|| normalize_optional(Some(tiered.primary_api_key_env.clone())))
        .unwrap_or_else(|| DEFAULT_GEMINI_API_KEY_ENV.to_owned());

    let (primary_provider, primary_name, primary_model_name): (
        Box<dyn crate::types::InferenceProvider>,
        String,
        String,
    ) = match primary_kind.as_str() {
        "gemini" => {
            let provider = GeminiProvider::from_env_key(
                &primary_api_key_env,
                primary_model,
                selected_thinking,
            )?;
            let model_name = provider.model_name();
            (
                Box::new(provider),
                InferenceProviderKind::Gemini.as_str().to_owned(),
                model_name,
            )
        }
        "openai_compat" => {
            let api_key = read_env_non_empty(&primary_api_key_env)
                .ok_or_else(|| InferError::MissingApiKey(primary_api_key_env.clone()))?;
            let api_base = primary_endpoint.ok_or(InferError::MissingEndpoint)?;
            let model = primary_model.ok_or(InferError::MissingModel)?;
            (
                Box::new(OpenAiCompatProvider::new(
                    Secret::new(api_key),
                    api_base,
                    model.clone(),
                )),
                InferenceProviderKind::OpenAiCompat.as_str().to_owned(),
                model,
            )
        }
        "omp" => {
            let api_key_env = if primary_api_key_env == DEFAULT_GEMINI_API_KEY_ENV {
                DEFAULT_OMP_GATEWAY_TOKEN_ENV.to_owned()
            } else {
                primary_api_key_env
            };
            let provider = build_omp_provider(
                &api_key_env,
                primary_endpoint,
                primary_model,
                selected_thinking,
            )?;
            let model_name = provider.model_name();
            (
                Box::new(provider),
                InferenceProviderKind::Omp.as_str().to_owned(),
                model_name,
            )
        }
        other => {
            return Err(InferError::InvalidConfig(format!(
                "inference.tiered.primary must be 'gemini', 'openai_compat' or 'omp' (found '{other}')"
            )));
        }
    };

    let fallback = Qwen3LocalProvider::new(
        tiered.fallback_endpoint.clone(),
        tiered.fallback_model.clone(),
    );
    let fallback_model_name = fallback.model_name();
    let threshold = if tiered.primary_threshold.is_finite() {
        tiered.primary_threshold.clamp(0.0, 1.0)
    } else {
        0.8
    };

    let provider = TieredProvider::new(
        primary_provider,
        Box::new(fallback),
        threshold,
        tiered.retry_with_fallback,
        primary_name.clone(),
    );
    Ok(LoadedProvider {
        provider: Box::new(provider),
        provider_name: InferenceProviderKind::Tiered.as_str().to_owned(),
        model_name: format!("{primary_model_name}|{fallback_model_name}"),
    })
}

async fn summarize_text_with_tiered(
    tiered: &TieredConfig,
    score: f64,
    system_prompt: &str,
    user_prompt: &str,
    selected_model: Option<String>,
    selected_endpoint: Option<String>,
    selected_api_key_env: String,
) -> Result<Option<String>, InferError> {
    let threshold = if tiered.primary_threshold.is_finite() {
        tiered.primary_threshold.clamp(0.0, 1.0)
    } else {
        0.8
    };
    let primary_kind = tiered.primary.trim().to_ascii_lowercase();
    let primary_model = first_non_empty(selected_model, tiered.primary_model.clone());
    let primary_endpoint = first_non_empty(selected_endpoint, tiered.primary_endpoint.clone());
    let primary_api_key_env = normalize_optional(Some(selected_api_key_env))
        .or_else(|| normalize_optional(Some(tiered.primary_api_key_env.clone())))
        .unwrap_or_else(|| DEFAULT_GEMINI_API_KEY_ENV.to_owned());
    let fallback_endpoint = normalize_optional(tiered.fallback_endpoint.clone())
        .unwrap_or_else(|| DEFAULT_QWEN_ENDPOINT.to_owned());
    let fallback_model = normalize_optional(tiered.fallback_model.clone())
        .unwrap_or_else(|| DEFAULT_QWEN_MODEL.to_owned());

    if score >= threshold {
        let primary_result = match primary_kind.as_str() {
            "gemini" => {
                if let Some(api_key) = read_env_non_empty(primary_api_key_env.as_str()) {
                    let model = resolve_gemini_model(primary_model);
                    request_gemini_summary(
                        &Secret::new(api_key),
                        model.as_str(),
                        system_prompt,
                        user_prompt,
                    )
                    .await
                } else {
                    Err(InferError::MissingApiKey(primary_api_key_env))
                }
            }
            "openai_compat" => {
                let api_key = read_env_non_empty(primary_api_key_env.as_str())
                    .ok_or_else(|| InferError::MissingApiKey(primary_api_key_env.clone()))?;
                let api_base = primary_endpoint.ok_or(InferError::MissingEndpoint)?;
                let model = primary_model.ok_or(InferError::MissingModel)?;
                OpenAiCompatProvider::new(Secret::new(api_key), api_base, model)
                    .request_summary(system_prompt, user_prompt)
                    .await
            }
            other => {
                return Err(InferError::InvalidConfig(format!(
                    "inference.tiered.primary must be 'gemini' or 'openai_compat' (found '{other}')"
                )));
            }
        };

        match primary_result {
            Ok(summary) => return Ok(clean_summary(summary)),
            Err(err) if !tiered.retry_with_fallback => return Err(err),
            Err(err) => {
                tracing::warn!(error = %err, "tiered summary primary failed; using fallback");
            }
        }
    }

    let summary = request_qwen_summary(
        fallback_endpoint.as_str(),
        fallback_model.as_str(),
        system_prompt,
        user_prompt,
    )
    .await?;
    Ok(clean_summary(summary))
}

fn resolve_candle_model_dir(workspace_root: &Path, model_dir: Option<PathBuf>) -> PathBuf {
    let configured = model_dir.unwrap_or_else(|| PathBuf::from(AETHER_DIR_NAME).join("models"));
    if configured.is_absolute() {
        configured
    } else {
        workspace_root.join(configured)
    }
}

fn clean_summary(text: String) -> Option<String> {
    let normalized = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned();
    if normalized.is_empty() {
        return None;
    }
    Some(normalized)
}

fn no_provider_available_message(endpoint: &str, api_key_env: &str) -> String {
    format!(
        "start Ollama on {} or set {} / configure [inference] provider explicitly in .aether/config.toml",
        endpoint, api_key_env
    )
}

fn default_api_key_env_for_provider(provider: InferenceProviderKind) -> &'static str {
    match provider {
        InferenceProviderKind::OpenAiCompat => DEFAULT_OPENAI_COMPAT_API_KEY_ENV,
        InferenceProviderKind::Omp => DEFAULT_OMP_GATEWAY_TOKEN_ENV,
        _ => DEFAULT_GEMINI_API_KEY_ENV,
    }
}

fn resolve_inference_api_key_env(
    provider: InferenceProviderKind,
    override_api_key_env: Option<String>,
    config_api_key_env: Option<String>,
) -> String {
    let selected = first_non_empty(override_api_key_env, config_api_key_env);
    match selected {
        Some(value)
            if provider == InferenceProviderKind::OpenAiCompat
                && value == DEFAULT_GEMINI_API_KEY_ENV =>
        {
            DEFAULT_OPENAI_COMPAT_API_KEY_ENV.to_owned()
        }
        // The serde default for `api_key_env` is the Gemini variable; for omp that means
        // "unset", so fall through to the gateway token variable.
        Some(value)
            if provider == InferenceProviderKind::Omp && value == DEFAULT_GEMINI_API_KEY_ENV =>
        {
            DEFAULT_OMP_GATEWAY_TOKEN_ENV.to_owned()
        }
        Some(value) => value,
        None => default_api_key_env_for_provider(provider).to_owned(),
    }
}

fn read_env_non_empty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests;
