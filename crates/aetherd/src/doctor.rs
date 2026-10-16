//! `aetherd doctor`: whether the model ids the config would call exist.
//!
//! Shut-down and deprecated ids are reported from the config alone. Then each provider
//! the config selects is resolved as indexing resolves it, and its service is asked to
//! list its models. Listing is the only request made: nothing is generated or embedded,
//! so a check spends no tokens.

use std::fmt;
use std::path::Path;

use aether_config::{
    AetherConfig, EmbeddingProviderKind, InferenceProviderKind, ModelIdStatus, model_id_notice,
    validate_config,
};
use aether_infer::{
    EmbeddingProviderOverrides, ModelCatalog, ModelListing, ProviderOverrides,
    load_embedding_provider_from_config, load_inference_provider_from_config,
};
use anyhow::{Context, Result};

/// Most similar ids a "not listed" result suggests.
const MAX_SUGGESTIONS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
            Self::Skip => "skip",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    /// The setting or role checked, such as `inference` or `embeddings.model`.
    pub subject: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn failures(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == CheckStatus::Fail)
            .count()
    }

    pub fn exit_code(&self) -> i32 {
        i32::from(self.failures() > 0)
    }

    fn push(&mut self, subject: impl Into<String>, status: CheckStatus, detail: impl Into<String>) {
        self.checks.push(DoctorCheck {
            subject: subject.into(),
            status,
            detail: detail.into(),
        });
    }
}

/// The models one role (inference, a quality pass, embeddings) would call.
struct RoleModels {
    role: String,
    provider: String,
    listings: Vec<ModelListing>,
}

pub fn run_doctor(workspace: &Path, config: &AetherConfig) -> Result<DoctorReport> {
    let mut report = DoctorReport::default();

    for warning in validate_config(config) {
        let status = match warning.code {
            "model_id_shut_down" => CheckStatus::Fail,
            "model_id_deprecated" => CheckStatus::Warn,
            _ => continue,
        };
        let subject = warning
            .message
            .split_once('=')
            .map(|(setting, _)| setting.to_owned())
            .unwrap_or_else(|| "config".to_owned());
        report.push(subject, status, warning.message);
    }

    let mut roles = Vec::new();
    collect_inference_role(workspace, config, &mut roles, &mut report);
    collect_quality_roles(workspace, config, &mut roles, &mut report);
    collect_embedding_role(workspace, config, &mut roles, &mut report);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to start the model-listing runtime")?;
    // Each service is listed once, however many roles use it.
    let mut listed: Vec<(ModelCatalog, Result<Vec<String>, String>)> = Vec::new();
    for role in roles {
        for listing in role.listings {
            let ids = match listed
                .iter()
                .find(|(catalog, _)| *catalog == listing.catalog)
            {
                Some((_, ids)) => ids.clone(),
                None => {
                    let ids = runtime
                        .block_on(listing.catalog.list_model_ids())
                        .map_err(|err| err.to_string());
                    listed.push((listing.catalog.clone(), ids.clone()));
                    ids
                }
            };
            let (status, detail) = listing_result(&role.provider, &listing, ids);
            report.push(role.role.clone(), status, detail);
        }
    }

    Ok(report)
}

fn collect_inference_role(
    workspace: &Path,
    config: &AetherConfig,
    roles: &mut Vec<RoleModels>,
    report: &mut DoctorReport,
) {
    if config.inference.provider == InferenceProviderKind::Mock {
        report.push(
            "inference",
            CheckStatus::Skip,
            "provider mock calls no model",
        );
        return;
    }
    load_inference_role(
        workspace,
        "inference",
        ProviderOverrides::default(),
        roles,
        report,
    );
}

/// A quality pass with a provider, model, endpoint or key of its own uses a model the
/// inference check doesn't cover; one without inherits the inference model.
fn collect_quality_roles(
    workspace: &Path,
    config: &AetherConfig,
    roles: &mut Vec<RoleModels>,
    report: &mut DoctorReport,
) {
    let quality = &config.sir_quality;
    for (role, enabled, provider, model, endpoint, api_key_env) in [
        (
            "sir_quality.triage",
            quality.triage_pass,
            &quality.triage_provider,
            &quality.triage_model,
            &quality.triage_endpoint,
            &quality.triage_api_key_env,
        ),
        (
            "sir_quality.deep",
            quality.deep_pass,
            &quality.deep_provider,
            &quality.deep_model,
            &quality.deep_endpoint,
            &quality.deep_api_key_env,
        ),
    ] {
        if provider.is_none() && model.is_none() && endpoint.is_none() && api_key_env.is_none() {
            continue;
        }
        let provider = match provider.as_deref().map(str::parse::<InferenceProviderKind>) {
            Some(Ok(provider)) => Some(provider),
            Some(Err(err)) => {
                report.push(role, CheckStatus::Fail, format!("invalid provider: {err}"));
                continue;
            }
            None => None,
        };
        if provider.unwrap_or(config.inference.provider) == InferenceProviderKind::Mock {
            report.push(role, CheckStatus::Skip, "provider mock calls no model");
            continue;
        }
        let role = if enabled {
            role.to_owned()
        } else {
            format!("{role} (pass disabled)")
        };
        load_inference_role(
            workspace,
            &role,
            ProviderOverrides {
                provider,
                model: model.clone(),
                endpoint: endpoint.clone(),
                api_key_env: api_key_env.clone(),
                thinking: None,
            },
            roles,
            report,
        );
    }
}

fn load_inference_role(
    workspace: &Path,
    role: &str,
    overrides: ProviderOverrides,
    roles: &mut Vec<RoleModels>,
    report: &mut DoctorReport,
) {
    match load_inference_provider_from_config(workspace, overrides) {
        Ok(loaded) => roles.push(RoleModels {
            role: role.to_owned(),
            listings: loaded.provider.model_listings(),
            provider: loaded.provider_name,
        }),
        Err(err) => report.push(role, CheckStatus::Fail, format!("cannot check: {err}")),
    }
}

fn collect_embedding_role(
    workspace: &Path,
    config: &AetherConfig,
    roles: &mut Vec<RoleModels>,
    report: &mut DoctorReport,
) {
    if !config.embeddings.enabled {
        return;
    }
    if config.embeddings.provider == EmbeddingProviderKind::Candle {
        report.push(
            "embeddings",
            CheckStatus::Skip,
            "provider candle runs a local model file",
        );
        return;
    }
    match load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default()) {
        Ok(Some(loaded)) => roles.push(RoleModels {
            role: "embeddings".to_owned(),
            listings: loaded.provider.model_listings(),
            provider: loaded.provider_name,
        }),
        Ok(None) => {}
        Err(err) => report.push(
            "embeddings",
            CheckStatus::Fail,
            format!("cannot check: {err}"),
        ),
    }
}

fn listing_result(
    provider: &str,
    listing: &ModelListing,
    ids: Result<Vec<String>, String>,
) -> (CheckStatus, String) {
    let model = listing.model.as_str();
    let source = listing.catalog.describe();
    let ids = match ids {
        Ok(ids) => ids,
        Err(err) => {
            return (
                CheckStatus::Fail,
                format!("{provider} model '{model}': could not list models at {source}: {err}"),
            );
        }
    };
    if listing.catalog.lists(model, &ids) {
        return (
            CheckStatus::Ok,
            format!("{provider} model '{model}' is listed by {source}"),
        );
    }

    let mut detail = format!("{provider} model '{model}' is not listed by {source}");
    if let Some(notice) = model_id_notice(model) {
        let state = match notice.status {
            ModelIdStatus::ShutDown => "shut down",
            ModelIdStatus::Deprecated => "deprecated",
        };
        detail.push_str(&format!(" ({state}; use '{}')", notice.replacement));
    } else {
        let similar = similar_ids(model, &ids);
        if !similar.is_empty() {
            detail.push_str(&format!("; similar: {}", similar.join(", ")));
        }
    }
    (CheckStatus::Fail, detail)
}

/// The listed ids sharing the longest prefix with `model`, at least four characters.
fn similar_ids(model: &str, ids: &[String]) -> Vec<String> {
    let model = model.strip_prefix("models/").unwrap_or(model);
    let shared = |id: &str| {
        model
            .chars()
            .zip(id.chars())
            .take_while(|(left, right)| left == right)
            .count()
    };
    let best = ids.iter().map(|id| shared(id)).max().unwrap_or(0);
    if best < 4 {
        return Vec::new();
    }
    let mut similar = ids
        .iter()
        .filter(|id| shared(id) == best)
        .cloned()
        .collect::<Vec<_>>();
    similar.sort();
    similar.truncate(MAX_SUGGESTIONS);
    similar
}

pub fn print_doctor_report(report: &DoctorReport) {
    println!("AETHER doctor: configured models (lists models only, generates nothing)");
    if report.checks.is_empty() {
        println!("  nothing to check: no provider calls a hosted model");
    }
    for check in &report.checks {
        println!("  [{}] {}: {}", check.status, check.subject, check.detail);
    }
    match report.failures() {
        0 => println!("No problems found."),
        1 => println!("1 problem found."),
        failures => println!("{failures} problems found."),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use aether_config::ensure_workspace_config;
    use tempfile::tempdir;

    use super::{CheckStatus, run_doctor, similar_ids};

    fn write_config(workspace: &std::path::Path, config: &str) {
        std::fs::create_dir_all(workspace.join(".aether")).expect("aether dir");
        std::fs::write(workspace.join(".aether/config.toml"), config).expect("write config");
    }

    #[test]
    fn doctor_lists_ollama_models_once_and_fails_a_missing_model() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 4096];
            let read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let body = r#"{"models":[{"name":"qwen3.5:4b"},{"name":"qwen3-embedding:0.6b"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("write response");
            request
        });

        let temp = tempdir().expect("tempdir");
        write_config(
            temp.path(),
            &format!(
                r#"
[inference]
provider = "qwen3_local"
model = "qwen3.5:4b"
endpoint = "{endpoint}"

[embeddings]
enabled = true
provider = "qwen3_local"
model = "qwen3-embedding:4b"
endpoint = "{endpoint}/api/embeddings"
"#
            ),
        );
        let config = ensure_workspace_config(temp.path()).expect("config");

        let report = run_doctor(temp.path(), &config).expect("doctor");
        // Both roles share one Ollama server, listed once; only /api/tags is called.
        let request = server.join().expect("server");
        assert!(request.starts_with("GET /api/tags "), "{request}");

        let inference = report
            .checks
            .iter()
            .find(|check| check.subject == "inference")
            .expect("inference check");
        assert_eq!(inference.status, CheckStatus::Ok, "{report:?}");
        let embeddings = report
            .checks
            .iter()
            .find(|check| check.subject == "embeddings")
            .expect("embeddings check");
        assert_eq!(embeddings.status, CheckStatus::Fail, "{report:?}");
        assert!(
            embeddings
                .detail
                .contains("'qwen3-embedding:4b' is not listed")
        );
        assert!(embeddings.detail.contains("similar: qwen3-embedding:0.6b"));
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn doctor_fails_a_shut_down_id_without_any_request() {
        let temp = tempdir().expect("tempdir");
        write_config(
            temp.path(),
            r#"
[inference]
provider = "mock"
model = "gemini-3.1-flash-lite-preview"
"#,
        );
        let config = ensure_workspace_config(temp.path()).expect("config");

        let report = run_doctor(temp.path(), &config).expect("doctor");
        let statuses = report
            .checks
            .iter()
            .map(|check| (check.subject.as_str(), check.status))
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            [
                ("inference.model", CheckStatus::Fail),
                ("inference", CheckStatus::Skip),
            ]
        );
        assert!(
            report.checks[0]
                .detail
                .contains("use 'gemini-3.1-flash-lite'")
        );
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn similar_ids_share_the_longest_prefix() {
        let ids = [
            "gemini-3.1-flash-lite".to_owned(),
            "gemini-3.1-pro".to_owned(),
            "text-embedding-004".to_owned(),
        ];
        assert_eq!(
            similar_ids("gemini-3.1-flash", &ids),
            ["gemini-3.1-flash-lite"]
        );
        assert!(similar_ids("llama3", &ids).is_empty());
    }
}
