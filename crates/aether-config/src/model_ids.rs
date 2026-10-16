//! Model ids a provider has shut down or listed for retirement. A config that still names
//! one gets a warning from [`crate::validate_config`] at startup and from `aetherd doctor`.
//! Nothing here changes which model a request uses.

use crate::embeddings::EmbeddingProviderKind;
use crate::inference::{InferenceProviderKind, TieredConfig};
use crate::root::AetherConfig;

/// Whether the provider still serves a model id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelIdStatus {
    /// No longer served: every request naming it fails.
    ShutDown,
    /// Listed for retirement with a replacement. The provider's date is the earliest it
    /// might stop serving the id, so it may or may not still answer.
    Retiring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelIdNotice {
    pub id: &'static str,
    pub status: ModelIdStatus,
    pub replacement: &'static str,
    /// What happened and when, in the provider's words where they matter.
    pub detail: &'static str,
    /// What moving to `replacement` takes besides changing the id, when it takes more.
    pub switching: Option<&'static str>,
}

pub const MODEL_ID_NOTICES: &[ModelIdNotice] = &[
    ModelIdNotice {
        id: "gemini-3.1-flash-lite-preview",
        status: ModelIdStatus::ShutDown,
        replacement: "gemini-3.1-flash-lite",
        detail: "Google shut it down on 2026-05-25; gemini-3.1-flash-lite is GA since 2026-05-07",
        switching: None,
    },
    ModelIdNotice {
        id: "gemini-embedding-2-preview",
        status: ModelIdStatus::Retiring,
        replacement: "gemini-embedding-2",
        detail: "Google's deprecations page lists it (as embedding-2-preview) with 2026-08-10 \
                 as the earliest possible date on which it might be retired",
        switching: Some(
            "gemini-embedding-2 doesn't take the taskType field AETHER's gemini_native \
             provider sends, so requests can fail or lose the query/document distinction \
             until the provider puts the task in the text instead (Google's code retrieval \
             form: 'task: code retrieval | query: ...' for queries, 'title: none | text: ...' \
             for documents); switching also re-embeds every symbol",
        ),
    },
];

/// The notice for `model`, also when it is written as an omp route (`google/<id>`) or a
/// Gemini resource name (`models/<id>`).
pub fn model_id_notice(model: &str) -> Option<&'static ModelIdNotice> {
    let model = model.trim();
    let bare = model.rsplit('/').next().unwrap_or(model);
    MODEL_ID_NOTICES
        .iter()
        .find(|notice| notice.id.eq_ignore_ascii_case(bare))
}

/// The notice for `model` when Google serves it: through the Gemini API (`gemini_api`), or
/// as a `google/` route through a gateway (omp, an OpenAI-compatible router). The same
/// name on another backend, such as an Ollama model or an OpenAI-compatible endpoint, is
/// that backend's model, and Google's notices don't apply to it.
pub fn model_id_notice_for(model: &str, gemini_api: bool) -> Option<&'static ModelIdNotice> {
    let model = model.trim();
    let mut segments = model.split('/').collect::<Vec<_>>();
    segments.pop();
    let google_route = segments
        .iter()
        .any(|segment| segment.eq_ignore_ascii_case("google"));
    if gemini_api || google_route {
        model_id_notice(model)
    } else {
        None
    }
}

/// A model id the config names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredModel {
    /// The setting that names it, such as `inference.model`.
    pub setting: String,
    pub model: String,
    /// Whether the setting's provider sends the id to Google's Gemini API: provider
    /// `gemini`, `auto` (which falls back to Gemini), a tiered primary of `gemini`,
    /// `gemini_native` embeddings, or Gemini batches.
    pub gemini_api: bool,
}

impl ConfiguredModel {
    /// The notice for this model, where Google serves it.
    pub fn notice(&self) -> Option<&'static ModelIdNotice> {
        model_id_notice_for(&self.model, self.gemini_api)
    }
}

fn inference_uses_gemini(provider: InferenceProviderKind, tiered: Option<&TieredConfig>) -> bool {
    match provider {
        InferenceProviderKind::Gemini | InferenceProviderKind::Auto => true,
        InferenceProviderKind::Tiered => {
            tiered.is_some_and(|tiered| tiered.primary.trim().eq_ignore_ascii_case("gemini"))
        }
        InferenceProviderKind::Qwen3Local
        | InferenceProviderKind::OpenAiCompat
        | InferenceProviderKind::Omp
        | InferenceProviderKind::Mock => false,
    }
}

/// Whether a pass with its own optional provider (a quality pass, the watcher's realtime
/// model) sends to Gemini; without one it inherits the inference provider.
fn pass_uses_gemini(config: &AetherConfig, pass_provider: Option<&str>) -> bool {
    let tiered = config.inference.tiered.as_ref();
    match pass_provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    {
        Some(provider) => provider
            .parse::<InferenceProviderKind>()
            .is_ok_and(|provider| inference_uses_gemini(provider, tiered)),
        None => inference_uses_gemini(config.inference.provider, tiered),
    }
}

/// Every model id the config names, with the setting that names it and whether Google's
/// Gemini API serves it, in config order. Empty settings are left out.
pub fn configured_model_ids(config: &AetherConfig) -> Vec<ConfiguredModel> {
    let mut models = Vec::new();
    let mut push = |setting: &str, model: Option<&str>, gemini_api: bool| {
        if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
            models.push(ConfiguredModel {
                setting: setting.to_owned(),
                model: model.to_owned(),
                gemini_api,
            });
        }
    };

    let tiered = config.inference.tiered.as_ref();
    push(
        "inference.model",
        config.inference.model.as_deref(),
        inference_uses_gemini(config.inference.provider, tiered),
    );
    if let Some(tiered) = tiered {
        push(
            "inference.tiered.primary_model",
            tiered.primary_model.as_deref(),
            tiered.primary.trim().eq_ignore_ascii_case("gemini"),
        );
        // The fallback is always local (Ollama).
        push(
            "inference.tiered.fallback_model",
            tiered.fallback_model.as_deref(),
            false,
        );
    }
    push(
        "sir_quality.triage_model",
        config.sir_quality.triage_model.as_deref(),
        pass_uses_gemini(config, config.sir_quality.triage_provider.as_deref()),
    );
    push(
        "sir_quality.deep_model",
        config.sir_quality.deep_model.as_deref(),
        pass_uses_gemini(config, config.sir_quality.deep_provider.as_deref()),
    );
    if let Some(watcher) = &config.watcher {
        push(
            "watcher.realtime_model",
            Some(watcher.realtime_model.as_str()),
            pass_uses_gemini(config, Some(watcher.realtime_provider.as_str())),
        );
    }
    if let Some(batch) = &config.batch {
        // `auto` follows the omp route in the model id, which `google/` already marks.
        let gemini_batch = batch.provider.trim().eq_ignore_ascii_case("gemini");
        push(
            "batch.scan_model",
            Some(batch.scan_model.as_str()),
            gemini_batch,
        );
        push(
            "batch.triage_model",
            Some(batch.triage_model.as_str()),
            gemini_batch,
        );
        push(
            "batch.deep_model",
            Some(batch.deep_model.as_str()),
            gemini_batch,
        );
        for (name, provider, gemini_api) in [
            ("gemini", &batch.gemini, true),
            ("openai", &batch.openai, false),
            ("anthropic", &batch.anthropic, false),
        ] {
            push(
                &format!("batch.{name}.scan_model"),
                provider.scan_model.as_deref(),
                gemini_api,
            );
            push(
                &format!("batch.{name}.triage_model"),
                provider.triage_model.as_deref(),
                gemini_api,
            );
            push(
                &format!("batch.{name}.deep_model"),
                provider.deep_model.as_deref(),
                gemini_api,
            );
        }
    }
    push(
        "embeddings.model",
        config.embeddings.model.as_deref(),
        config.embeddings.provider == EmbeddingProviderKind::GeminiNative,
    );
    models
}

#[cfg(test)]
mod tests {
    use super::{
        ConfiguredModel, ModelIdStatus, configured_model_ids, model_id_notice, model_id_notice_for,
    };
    use crate::root::parse_workspace_config_str;

    #[test]
    fn notices_match_bare_ids_routes_and_resource_names() {
        for model in [
            "gemini-3.1-flash-lite-preview",
            " google/gemini-3.1-flash-lite-preview ",
            "models/gemini-3.1-flash-lite-preview",
        ] {
            let notice = model_id_notice(model).expect(model);
            assert_eq!(notice.status, ModelIdStatus::ShutDown);
            assert_eq!(notice.replacement, "gemini-3.1-flash-lite");
        }
        let embedding = model_id_notice("gemini-embedding-2-preview").expect("embedding notice");
        assert_eq!(embedding.status, ModelIdStatus::Retiring);
        assert!(embedding.detail.contains("earliest possible date"));
        let switching = embedding
            .switching
            .expect("the switch needs a request change");
        assert!(switching.contains("taskType"), "{switching}");
        assert!(
            switching.contains("task: code retrieval | query:"),
            "{switching}"
        );
        assert!(model_id_notice("gemini-3.1-flash-lite").is_none());
        assert!(model_id_notice("gemini-embedding-2").is_none());
    }

    #[test]
    fn configured_model_ids_lists_every_model_setting() {
        let config = parse_workspace_config_str(
            r#"
[inference]
provider = "gemini"
model = "gemini-3.1-flash-lite"

[sir_quality]
triage_model = "triage-model"

[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"

[batch]
scan_model = "scan-model"

[batch.anthropic]
deep_model = "deep-model"
"#,
        )
        .expect("parse config");

        assert_eq!(
            configured_model_ids(&config),
            [
                ("inference.model", "gemini-3.1-flash-lite", true),
                ("sir_quality.triage_model", "triage-model", true),
                // The batch provider defaults to Gemini.
                ("batch.scan_model", "scan-model", true),
                ("batch.anthropic.deep_model", "deep-model", false),
                ("embeddings.model", "gemini-embedding-2-preview", true),
            ]
            .map(|(setting, model, gemini_api)| ConfiguredModel {
                setting: setting.to_owned(),
                model: model.to_owned(),
                gemini_api,
            })
        );
    }

    /// Google's notices are about Google's models: the same name served by another
    /// backend gets none, while a `google/` route through a gateway still does.
    #[test]
    fn notices_apply_only_where_google_serves_the_model() {
        let preview = "gemini-3.1-flash-lite-preview";
        assert!(model_id_notice_for(preview, true).is_some());
        assert!(model_id_notice_for(preview, false).is_none());
        assert!(model_id_notice_for(&format!("google/{preview}"), false).is_some());
        assert!(model_id_notice_for(&format!("openrouter/google/{preview}"), false).is_some());
        assert!(model_id_notice_for(&format!("acme/{preview}"), false).is_none());

        let config = parse_workspace_config_str(
            r#"
[inference]
provider = "qwen3_local"
model = "gemini-3.1-flash-lite-preview"

[sir_quality]
triage_provider = "gemini"
triage_model = "gemini-3.1-flash-lite-preview"

[embeddings]
enabled = true
provider = "openai_compat"
model = "gemini-embedding-2-preview"
"#,
        )
        .expect("parse config");
        let noticed = configured_model_ids(&config)
            .into_iter()
            .filter(|model| model.notice().is_some())
            .map(|model| model.setting)
            .collect::<Vec<_>>();
        assert_eq!(noticed, ["sir_quality.triage_model"]);
    }
}
