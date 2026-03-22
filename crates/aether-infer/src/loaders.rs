use std::env;
use std::path::{Path, PathBuf};

use aether_config::{
    AETHER_DIR_NAME, DEFAULT_COHERE_API_KEY_ENV, DEFAULT_GEMINI_API_KEY_ENV,
    DEFAULT_OPENAI_COMPAT_API_KEY_ENV, DEFAULT_QWEN_ENDPOINT, DEFAULT_QWEN_MODEL,
    EmbeddingProviderKind, InferenceProviderKind, SearchRerankerKind, TieredConfig,
    ensure_workspace_config,
};
use aether_core::Secret;

use crate::embedding::Qwen3LocalEmbeddingProvider;
use crate::embedding::candle::CandleEmbeddingProvider;
use crate::embedding::gemini_native::GeminiNativeEmbeddingProvider;
use crate::embedding::openai_compat::OpenAiCompatEmbeddingProvider;
use crate::http::{is_ollama_reachable, is_ollama_reachable_blocking};
use crate::providers::gemini::{request_gemini_summary, resolve_gemini_model};
use crate::providers::qwen_local::request_qwen_summary;
use crate::providers::{GeminiProvider, OpenAiCompatProvider, Qwen3LocalProvider, TieredProvider};
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
    }
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
        other => {
            return Err(InferError::InvalidConfig(format!(
                "inference.tiered.primary must be 'gemini' or 'openai_compat' (found '{other}')"
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
mod tests {
    use std::io::{Read, Write};
    use std::time::{SystemTime, UNIX_EPOCH};

    use aether_config::{
        EmbeddingProviderKind, InferenceProviderKind, SearchRerankerKind, ensure_workspace_config,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::providers::gemini::GEMINI_DEFAULT_MODEL;

    #[test]
    fn load_embedding_provider_defaults_to_disabled() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let loaded =
            load_embedding_provider_from_config(temp.path(), EmbeddingProviderOverrides::default())
                .expect("load embedding provider");
        assert!(loaded.is_none());
    }

    #[test]
    fn load_embedding_provider_reads_enabled_qwen_settings() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");

        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "auto"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true

[embeddings]
enabled = true
provider = "qwen3_local"
model = "qwen3-embeddings-4B"
endpoint = "http://127.0.0.1:11434/api/embeddings"
"#,
        )
        .expect("write config");

        let loaded =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
                .expect("load embedding provider")
                .expect("embedding provider should be enabled");

        assert_eq!(
            loaded.provider_name,
            EmbeddingProviderKind::Qwen3Local.as_str()
        );
        assert_eq!(loaded.model_name, "qwen3-embeddings-4B");
    }

    #[test]
    fn load_embedding_provider_reads_enabled_candle_settings() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");

        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[embeddings]
enabled = true
provider = "candle"

[embeddings.candle]
model_dir = ".aether/models"
"#,
        )
        .expect("write config");

        let loaded =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
                .expect("load embedding provider")
                .expect("embedding provider should be enabled");

        assert_eq!(loaded.provider_name, EmbeddingProviderKind::Candle.as_str());
        assert_eq!(loaded.model_name, "qwen3-embedding-0.6b");
    }

    #[test]
    fn load_embedding_provider_openai_compat_requires_endpoint() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");
        let env_name = format!(
            "AETHER_TEST_OPENAI_COMPAT_EMBED_KEY_ENDPOINT_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );

        unsafe {
            env::set_var(&env_name, "test-key");
        }

        std::fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
api_key_env = "{env_name}"
"#
            ),
        )
        .expect("write config");

        let result =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

        match result {
            Err(InferError::MissingEndpoint) => {}
            Ok(_) => panic!("expected missing endpoint"),
            Err(err) => panic!("expected missing endpoint, got {err}"),
        }

        unsafe {
            env::remove_var(env_name);
        }
    }

    #[test]
    fn load_embedding_provider_openai_compat_requires_model() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");
        let env_name = format!(
            "AETHER_TEST_OPENAI_COMPAT_EMBED_KEY_MODEL_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );

        unsafe {
            env::set_var(&env_name, "test-key");
        }

        std::fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[embeddings]
enabled = true
provider = "openai_compat"
endpoint = "https://api.example.com/v1"
api_key_env = "{env_name}"
"#
            ),
        )
        .expect("write config");

        let result =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

        match result {
            Err(InferError::MissingModel) => {}
            Ok(_) => panic!("expected missing model"),
            Err(err) => panic!("expected missing model, got {err}"),
        }

        unsafe {
            env::remove_var(env_name);
        }
    }

    #[test]
    fn load_embedding_provider_openai_compat_requires_api_key() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");
        let env_name = "AETHER_TEST_OPENAI_COMPAT_EMBED_KEY_ZZZZZ".to_owned();

        std::fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
endpoint = "https://api.example.com/v1"
api_key_env = "{env_name}"
"#
            ),
        )
        .expect("write config");

        let result =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

        match result {
            Err(InferError::MissingApiKey(name)) => assert_eq!(name, env_name),
            Ok(_) => panic!("expected missing api key"),
            Err(err) => panic!("expected missing api key, got {err}"),
        }
    }

    #[test]
    fn load_embedding_provider_openai_compat_constructs_with_valid_config() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");
        let env_name = format!(
            "AETHER_TEST_OPENAI_COMPAT_EMBED_KEY_VALID_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );

        unsafe {
            env::set_var(&env_name, "test-key");
        }

        std::fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[embeddings]
enabled = true
provider = "openai_compat"
model = "text-embedding-3-large"
endpoint = "https://api.example.com/v1/embeddings"
api_key_env = "{env_name}"
task_type = "CODE_RETRIEVAL"
dimensions = 3072
"#
            ),
        )
        .expect("write config");

        let loaded =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
                .expect("load embedding provider")
                .expect("embedding provider should be enabled");

        assert_eq!(
            loaded.provider_name,
            EmbeddingProviderKind::OpenAiCompat.as_str()
        );
        assert_eq!(loaded.model_name, "text-embedding-3-large");

        unsafe {
            env::remove_var(env_name);
        }
    }

    #[test]
    fn load_embedding_provider_gemini_native_requires_model() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");
        let env_name = format!(
            "AETHER_TEST_GEMINI_NATIVE_EMBED_KEY_MODEL_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );

        unsafe {
            env::set_var(&env_name, "test-key");
        }

        std::fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[embeddings]
enabled = true
provider = "gemini_native"
api_key_env = "{env_name}"
"#
            ),
        )
        .expect("write config");

        let result =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

        match result {
            Err(InferError::MissingModel) => {}
            Ok(_) => panic!("expected missing model"),
            Err(err) => panic!("expected missing model, got {err}"),
        }

        unsafe {
            env::remove_var(env_name);
        }
    }

    #[test]
    fn load_embedding_provider_gemini_native_requires_api_key_env() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");

        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
"#,
        )
        .expect("write config");

        let result =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default());

        match result {
            Err(InferError::InvalidConfig(message)) => {
                assert!(message.contains("embeddings.api_key_env"));
            }
            Ok(_) => panic!("expected invalid config"),
            Err(err) => panic!("expected invalid config, got {err}"),
        }
    }

    #[test]
    fn load_embedding_provider_gemini_native_constructs_with_valid_config() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");
        let env_name = format!(
            "AETHER_TEST_GEMINI_NATIVE_EMBED_KEY_VALID_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );

        unsafe {
            env::set_var(&env_name, "test-key");
        }

        std::fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = "{env_name}"
dimensions = 3072
"#
            ),
        )
        .expect("write config");

        let loaded =
            load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
                .expect("load embedding provider")
                .expect("embedding provider should be enabled");

        assert_eq!(
            loaded.provider_name,
            EmbeddingProviderKind::GeminiNative.as_str()
        );
        assert_eq!(loaded.model_name, "gemini-embedding-2-preview");

        unsafe {
            env::remove_var(env_name);
        }
    }

    #[test]
    fn load_provider_auto_errors_when_key_missing() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let result = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::Auto),
                endpoint: Some("http://127.0.0.1:9".to_owned()),
                api_key_env: Some("AETHER_TEST_NONEXISTENT_KEY_ZZZZZ".to_owned()),
                ..ProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::NoProviderAvailable(message)) => {
                assert!(message.contains("configure [inference] provider explicitly"))
            }
            Ok(_) => panic!("expected no provider available error, got Ok result"),
            Err(err) => panic!("expected no provider available error, got {err}"),
        }
    }

    #[test]
    fn resolve_inference_thinking_prefers_override() {
        assert_eq!(
            resolve_inference_thinking(Some(" high ".to_owned()), Some("low".to_owned())),
            Some("high".to_owned())
        );
    }

    #[test]
    fn resolve_inference_thinking_falls_back_to_config() {
        assert_eq!(
            resolve_inference_thinking(None, Some(" medium ".to_owned())),
            Some("medium".to_owned())
        );
    }

    #[test]
    fn load_provider_auto_chooses_gemini_when_key_present() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let env_name = format!(
            "AETHER_TEST_GEMINI_KEY_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );

        unsafe {
            env::set_var(&env_name, "test-key");
        }

        let loaded = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::Auto),
                endpoint: Some("http://127.0.0.1:9".to_owned()),
                api_key_env: Some(env_name.clone()),
                ..ProviderOverrides::default()
            },
        )
        .expect("load provider");

        assert_eq!(loaded.provider_name, InferenceProviderKind::Gemini.as_str());
        assert_eq!(loaded.model_name, GEMINI_DEFAULT_MODEL);

        unsafe {
            env::remove_var(env_name);
        }
    }

    #[test]
    fn load_provider_auto_prefers_qwen_when_ollama_is_reachable() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind test ollama listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept health check");
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]")
                .expect("write health response");
        });

        let loaded = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::Auto),
                endpoint: Some(endpoint),
                ..ProviderOverrides::default()
            },
        )
        .expect("load provider");

        assert_eq!(
            loaded.provider_name,
            InferenceProviderKind::Qwen3Local.as_str()
        );
        assert_eq!(loaded.model_name, DEFAULT_QWEN_MODEL);
        server.join().expect("join health server");
    }

    #[test]
    fn load_provider_openai_compat_requires_api_key_with_fabricated_env_var() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");
        let env_name = "AETHER_TEST_OPENAI_COMPAT_KEY_ZZZZZ".to_owned();

        let result = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::OpenAiCompat),
                endpoint: Some("https://api.example.com/v1".to_owned()),
                model: Some("glm-4.7".to_owned()),
                api_key_env: Some(env_name.clone()),
                ..ProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::MissingApiKey(name)) => assert_eq!(name, env_name),
            _ => panic!("expected missing api key"),
        }
    }

    #[test]
    fn load_provider_openai_compat_requires_endpoint() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");
        let env_name = format!(
            "AETHER_TEST_OPENAI_COMPAT_KEY_ENDPOINT_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        unsafe {
            env::set_var(&env_name, "test-key");
        }

        let result = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::OpenAiCompat),
                model: Some("glm-4.7".to_owned()),
                api_key_env: Some(env_name.clone()),
                ..ProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::MissingEndpoint) => {}
            _ => panic!("expected missing endpoint"),
        }

        unsafe {
            env::remove_var(env_name);
        }
    }

    #[test]
    fn load_provider_openai_compat_requires_model() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");
        let env_name = format!(
            "AETHER_TEST_OPENAI_COMPAT_KEY_MODEL_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        unsafe {
            env::set_var(&env_name, "test-key");
        }

        let result = load_provider_from_env_or_mock(
            temp.path(),
            ProviderOverrides {
                provider: Some(InferenceProviderKind::OpenAiCompat),
                endpoint: Some("https://api.example.com/v1".to_owned()),
                api_key_env: Some(env_name.clone()),
                ..ProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::MissingModel) => {}
            _ => panic!("expected missing model"),
        }

        unsafe {
            env::remove_var(env_name);
        }
    }

    #[test]
    fn load_reranker_provider_defaults_to_none() {
        let temp = tempdir().expect("tempdir");
        ensure_workspace_config(temp.path()).expect("ensure config");

        let loaded =
            load_reranker_provider_from_config(temp.path(), RerankerProviderOverrides::default())
                .expect("load reranker provider");
        assert!(loaded.is_none());
    }

    #[test]
    fn load_reranker_provider_reads_enabled_candle_settings() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");

        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[search]
reranker = "candle"

[search.candle]
model_dir = ".aether/models"
"#,
        )
        .expect("write config");

        let loaded =
            load_reranker_provider_from_config(workspace, RerankerProviderOverrides::default())
                .expect("load reranker provider")
                .expect("reranker provider should be enabled");

        assert_eq!(loaded.provider_name, SearchRerankerKind::Candle.as_str());
        assert_eq!(loaded.model_name, "qwen3-reranker-0.6b");
    }

    #[test]
    fn load_reranker_provider_requires_cohere_api_key() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        ensure_workspace_config(workspace).expect("ensure config");

        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[search]
reranker = "cohere"
"#,
        )
        .expect("write config");

        let env_name = format!(
            "AETHER_TEST_COHERE_KEY_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        unsafe {
            env::remove_var(&env_name);
        }

        let result = load_reranker_provider_from_config(
            workspace,
            RerankerProviderOverrides {
                cohere_api_key_env: Some(env_name.clone()),
                ..RerankerProviderOverrides::default()
            },
        );

        match result {
            Err(InferError::MissingCohereApiKey(var)) => assert_eq!(var, env_name),
            _ => panic!("expected missing cohere key error"),
        }
    }
}
