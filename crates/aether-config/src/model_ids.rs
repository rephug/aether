//! Model ids a provider has shut down or listed for retirement. A config that still names
//! one gets a warning from [`crate::validate_config`] at startup and from `aetherd doctor`.
//! Nothing here changes which model a request uses.

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

/// Every model id the config names, with the setting that names it, in config order.
/// Empty settings are left out.
pub fn configured_model_ids(config: &AetherConfig) -> Vec<(String, String)> {
    let mut models = Vec::new();
    let mut push = |setting: &str, model: Option<&str>| {
        if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
            models.push((setting.to_owned(), model.to_owned()));
        }
    };

    push("inference.model", config.inference.model.as_deref());
    if let Some(tiered) = &config.inference.tiered {
        push(
            "inference.tiered.primary_model",
            tiered.primary_model.as_deref(),
        );
        push(
            "inference.tiered.fallback_model",
            tiered.fallback_model.as_deref(),
        );
    }
    push(
        "sir_quality.triage_model",
        config.sir_quality.triage_model.as_deref(),
    );
    push(
        "sir_quality.deep_model",
        config.sir_quality.deep_model.as_deref(),
    );
    if let Some(watcher) = &config.watcher {
        push(
            "watcher.realtime_model",
            Some(watcher.realtime_model.as_str()),
        );
    }
    if let Some(batch) = &config.batch {
        push("batch.scan_model", Some(batch.scan_model.as_str()));
        push("batch.triage_model", Some(batch.triage_model.as_str()));
        push("batch.deep_model", Some(batch.deep_model.as_str()));
        for (name, provider) in [
            ("gemini", &batch.gemini),
            ("openai", &batch.openai),
            ("anthropic", &batch.anthropic),
        ] {
            push(
                &format!("batch.{name}.scan_model"),
                provider.scan_model.as_deref(),
            );
            push(
                &format!("batch.{name}.triage_model"),
                provider.triage_model.as_deref(),
            );
            push(
                &format!("batch.{name}.deep_model"),
                provider.deep_model.as_deref(),
            );
        }
    }
    push("embeddings.model", config.embeddings.model.as_deref());
    models
}

#[cfg(test)]
mod tests {
    use super::{ModelIdStatus, configured_model_ids, model_id_notice};
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
                ("inference.model", "gemini-3.1-flash-lite"),
                ("sir_quality.triage_model", "triage-model"),
                ("batch.scan_model", "scan-model"),
                ("batch.anthropic.deep_model", "deep-model"),
                ("embeddings.model", "gemini-embedding-2-preview"),
            ]
            .map(|(setting, model)| (setting.to_owned(), model.to_owned()))
        );
    }
}
