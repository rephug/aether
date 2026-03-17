use std::collections::HashMap;
use std::sync::Arc;

use aether_config::{
    load_workspace_config, reset_section_to_defaults, save_workspace_config_preserving_comments,
};
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use maud::html;
use serde_json::{Map, Value};

use crate::support::{self, DashboardState};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const RESTART_REQUIRED_FIELDS: &[(&str, &str)] = &[
    ("embeddings", "provider"),
    ("embeddings", "vector_backend"),
    ("dashboard", "port"),
    ("storage", "graph_backend"),
];

// ---------------------------------------------------------------------------
// Handler 1: GET /api/v1/settings/:section
// ---------------------------------------------------------------------------

pub(crate) async fn get_section_handler(
    State(state): State<Arc<DashboardState>>,
    Path(section): Path<String>,
) -> impl IntoResponse {
    let config = match load_workspace_config(&state.shared.workspace) {
        Ok(c) => c,
        Err(err) => {
            return support::json_internal_error(format!("Failed to load config: {err}"))
                .into_response();
        }
    };

    let full_value = match serde_json::to_value(&config) {
        Ok(v) => v,
        Err(err) => {
            return support::json_internal_error(format!("Failed to serialize config: {err}"))
                .into_response();
        }
    };

    let section_value = match section.as_str() {
        "inference" => extract_key(&full_value, "inference"),
        "embeddings" => extract_key(&full_value, "embeddings"),
        "search" => {
            let mut obj = Map::new();
            if let Some(s) = full_value.get("search") {
                obj.insert("search".into(), s.clone());
            }
            if let Some(p) = full_value.get("providers") {
                obj.insert("providers".into(), p.clone());
            }
            Value::Object(obj)
        }
        "indexing" => extract_key(&full_value, "watcher"),
        "dashboard" => extract_key(&full_value, "dashboard"),
        "generation" => extract_key(&full_value, "sir_quality"),
        "advanced" => {
            let mut obj = Map::new();
            if let Some(v) = full_value.get("general") {
                obj.insert("general".into(), v.clone());
            }
            if let Some(v) = full_value.get("storage") {
                obj.insert("storage".into(), v.clone());
            }
            if let Some(v) = full_value.get("verify") {
                obj.insert("verify".into(), v.clone());
            }
            Value::Object(obj)
        }
        "coupling" => extract_key(&full_value, "coupling"),
        "drift" => extract_key(&full_value, "drift"),
        "health" => {
            let mut obj = Map::new();
            if let Some(v) = full_value.get("health") {
                obj.insert("health".into(), v.clone());
            }
            if let Some(v) = full_value.get("planner") {
                obj.insert("planner".into(), v.clone());
            }
            if let Some(v) = full_value.get("health_score") {
                obj.insert("health_score".into(), v.clone());
            }
            Value::Object(obj)
        }
        "batch" => extract_key(&full_value, "batch"),
        "watcher" => extract_key(&full_value, "watcher"),
        "continuous" => extract_key(&full_value, "continuous"),
        _ => Value::Null,
    };

    support::api_json(state.shared.as_ref(), section_value).into_response()
}

fn extract_key(root: &Value, key: &str) -> Value {
    root.get(key).cloned().unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Handler 2: POST /api/v1/settings/:section
// ---------------------------------------------------------------------------

pub(crate) async fn save_section_handler(
    State(state): State<Arc<DashboardState>>,
    Path(section): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let sections = match build_section_json(&form, &section) {
        Ok(s) => s,
        Err(errors) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                support::html_markup_response(crate::fragments::settings::helpers::error_banner(
                    &errors,
                )),
            )
                .into_response();
        }
    };

    // Save each TOML section
    for (toml_key, values) in &sections {
        if let Err(err) =
            save_workspace_config_preserving_comments(&state.shared.workspace, toml_key, values)
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                support::html_markup_response(crate::fragments::settings::helpers::error_banner(
                    &[format!("Failed to save [{toml_key}]: {err}")],
                )),
            )
                .into_response();
        }
    }

    // Detect restart-required fields
    let mut restart_fields = Vec::new();
    for (toml_section, field_name) in RESTART_REQUIRED_FIELDS {
        let field_in_form = match section.as_str() {
            "advanced" if *toml_section == "storage" => {
                form.contains_key(&format!("storage.{field_name}"))
            }
            s => {
                // Map UI section to TOML section(s) and check
                let mapped = ui_section_to_toml_keys(s);
                mapped.contains(&(*toml_section).to_owned()) && form.contains_key(*field_name)
            }
        };
        if field_in_form {
            restart_fields.push(format!("{toml_section}.{field_name}"));
        }
    }

    let markup = if restart_fields.is_empty() {
        crate::fragments::settings::helpers::success_banner(
            "Settings saved. Changes apply on next use.",
        )
    } else {
        let names = restart_fields.join(", ");
        html! {
            (crate::fragments::settings::helpers::success_banner("Settings saved."))
            (crate::fragments::settings::helpers::restart_required_banner(&names))
        }
    };

    (StatusCode::OK, support::html_markup_response(markup)).into_response()
}

// ---------------------------------------------------------------------------
// Handler 3: POST /api/v1/settings/:section/reset
// ---------------------------------------------------------------------------

pub(crate) async fn reset_section_handler(
    State(state): State<Arc<DashboardState>>,
    Path(section): Path<String>,
) -> impl IntoResponse {
    let toml_keys = ui_section_to_toml_keys(&section);

    for key in &toml_keys {
        if let Err(err) = reset_section_to_defaults(&state.shared.workspace, key) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                support::html_markup_response(crate::fragments::settings::helpers::error_banner(
                    &[format!("Failed to reset [{key}]: {err}")],
                )),
            )
                .into_response();
        }
    }

    // Re-load the config and render the section form with the fresh defaults
    let config = load_workspace_config(&state.shared.workspace).unwrap_or_default();
    let workspace = state.shared.workspace.clone();

    let markup = match section.as_str() {
        "inference" => crate::fragments::settings::inference::render(&config),
        "embeddings" => crate::fragments::settings::embeddings::render(&config),
        "search" => crate::fragments::settings::search::render(&config),
        "indexing" => crate::fragments::settings::indexing::render(&config, &workspace),
        "dashboard" => crate::fragments::settings::dashboard_cfg::render(&config),
        "generation" => crate::fragments::settings::generation::render(&config),
        "advanced" => crate::fragments::settings::advanced::render(&config),
        "coupling" => crate::fragments::settings::coupling::render(&config),
        "drift" => crate::fragments::settings::drift::render(&config),
        "health" => crate::fragments::settings::health::render(&config),
        "batch" => crate::fragments::settings::batch::render(&config),
        "watcher" => crate::fragments::settings::watcher::render(&config),
        "continuous" => crate::fragments::settings::continuous::render(&config),
        _ => html! {
            div class="text-text-muted text-sm py-8 text-center" {
                "Unknown settings section: " (section)
            }
        },
    };

    (StatusCode::OK, support::html_markup_response(markup)).into_response()
}

// ---------------------------------------------------------------------------
// Section-to-TOML key mapping
// ---------------------------------------------------------------------------

fn ui_section_to_toml_keys(section: &str) -> Vec<String> {
    match section {
        "inference" => vec!["inference".into()],
        "embeddings" => vec!["embeddings".into()],
        "search" => vec!["search".into(), "providers".into()],
        "indexing" => vec!["watcher".into()],
        "dashboard" => vec!["dashboard".into()],
        "generation" => vec!["sir_quality".into()],
        "advanced" => vec!["general".into(), "storage".into(), "verify".into()],
        "coupling" => vec!["coupling".into()],
        "drift" => vec!["drift".into()],
        "health" => vec!["health".into(), "planner".into(), "health_score".into()],
        "batch" => vec!["batch".into()],
        "watcher" => vec!["watcher".into()],
        "continuous" => vec!["continuous".into()],
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Form helpers
// ---------------------------------------------------------------------------

fn form_bool(form: &HashMap<String, String>, key: &str) -> bool {
    form.get(key).is_some_and(|v| v == "true" || v == "on")
}

fn form_str(form: &HashMap<String, String>, key: &str) -> String {
    form.get(key).cloned().unwrap_or_default()
}

fn json_str(form: &HashMap<String, String>, key: &str) -> Value {
    Value::String(form.get(key).cloned().unwrap_or_default())
}

fn insert_opt_str(obj: &mut Map<String, Value>, form: &HashMap<String, String>, key: &str) {
    if let Some(v) = form.get(key).filter(|s| !s.is_empty()) {
        obj.insert(key.into(), Value::String(v.clone()));
    }
}

fn parse_port(form: &HashMap<String, String>, key: &str) -> Result<u16, String> {
    let raw = form.get(key).map(|s| s.as_str()).unwrap_or("0");
    let port: u16 = raw
        .parse()
        .map_err(|_| format!("{key}: must be a valid port number"))?;
    if !(1..=65535).contains(&port) {
        return Err(format!("{key}: must be between 1 and 65535"));
    }
    Ok(port)
}

fn parse_usize_range(
    form: &HashMap<String, String>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    let raw = form.get(key).map(|s| s.as_str()).unwrap_or("0");
    let val: usize = raw
        .parse()
        .map_err(|_| format!("{key}: must be a number"))?;
    if val < min || val > max {
        return Err(format!("{key}: must be between {min} and {max}"));
    }
    Ok(val)
}

fn parse_usize_min(form: &HashMap<String, String>, key: &str, min: usize) -> Result<usize, String> {
    let raw = form.get(key).map(|s| s.as_str()).unwrap_or("0");
    let val: usize = raw
        .parse()
        .map_err(|_| format!("{key}: must be a number"))?;
    if val < min {
        return Err(format!("{key}: must be >= {min}"));
    }
    Ok(val)
}

fn parse_threshold(form: &HashMap<String, String>, key: &str) -> Result<f64, String> {
    let raw = form.get(key).map(|s| s.as_str()).unwrap_or("0");
    let val: f64 = raw
        .parse()
        .map_err(|_| format!("{key}: must be a number"))?;
    if !(0.0..=1.0).contains(&val) {
        return Err(format!("{key}: must be between 0.0 and 1.0"));
    }
    Ok(val)
}

fn parse_f64_range(
    form: &HashMap<String, String>,
    key: &str,
    min: f64,
    max: f64,
) -> Result<f64, String> {
    let raw = form.get(key).map(|s| s.as_str()).unwrap_or("0");
    let val: f64 = raw
        .parse()
        .map_err(|_| format!("{key}: must be a number"))?;
    if val < min || val > max {
        return Err(format!("{key}: must be between {min} and {max}"));
    }
    Ok(val)
}

fn parse_f64_nonneg(form: &HashMap<String, String>, key: &str) -> Result<f64, String> {
    let raw = form.get(key).map(|s| s.as_str()).unwrap_or("0");
    let val: f64 = raw
        .parse()
        .map_err(|_| format!("{key}: must be a number"))?;
    if val < 0.0 {
        return Err(format!("{key}: must be >= 0"));
    }
    Ok(val)
}

fn parse_u64_min(form: &HashMap<String, String>, key: &str, min: u64) -> Result<u64, String> {
    let raw = form.get(key).map(|s| s.as_str()).unwrap_or("0");
    let val: u64 = raw
        .parse()
        .map_err(|_| format!("{key}: must be a number"))?;
    if val < min {
        return Err(format!("{key}: must be >= {min}"));
    }
    Ok(val)
}

// ---------------------------------------------------------------------------
// Section-specific JSON builders
// ---------------------------------------------------------------------------

fn build_section_json(
    form: &HashMap<String, String>,
    section: &str,
) -> Result<HashMap<String, Value>, Vec<String>> {
    let mut sections: HashMap<String, Value> = HashMap::new();
    let mut errors = Vec::new();

    match section {
        "inference" => build_inference(form, &mut sections, &mut errors),
        "embeddings" => build_embeddings(form, &mut sections, &mut errors),
        "search" => build_search(form, &mut sections, &mut errors),
        "indexing" => build_indexing(form, &mut sections, &mut errors),
        "dashboard" => build_dashboard(form, &mut sections, &mut errors),
        "generation" => build_generation(form, &mut sections, &mut errors),
        "advanced" => build_advanced(form, &mut sections, &mut errors),
        "coupling" => build_coupling(form, &mut sections, &mut errors),
        "drift" => build_drift(form, &mut sections, &mut errors),
        "health" => build_health(form, &mut sections, &mut errors),
        "batch" => build_batch(form, &mut sections, &mut errors),
        "watcher" => build_watcher(form, &mut sections, &mut errors),
        "continuous" => build_continuous(form, &mut sections, &mut errors),
        _ => errors.push(format!("Unknown section: {section}")),
    }

    if errors.is_empty() {
        Ok(sections)
    } else {
        Err(errors)
    }
}

// --- Inference ---

fn build_inference(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();
    obj.insert("provider".into(), json_str(form, "provider"));
    insert_opt_str(&mut obj, form, "model");
    insert_opt_str(&mut obj, form, "endpoint");
    obj.insert("api_key_env".into(), json_str(form, "api_key_env"));

    match parse_usize_range(form, "concurrency", 1, 24) {
        Ok(v) => {
            obj.insert("concurrency".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }

    // Tiered sub-config
    if form_str(form, "provider") == "tiered" {
        let mut tiered = Map::new();
        tiered.insert("primary".into(), json_str(form, "tiered.primary"));
        insert_opt_str_prefixed(&mut tiered, form, "tiered.primary_model", "primary_model");
        insert_opt_str_prefixed(
            &mut tiered,
            form,
            "tiered.primary_endpoint",
            "primary_endpoint",
        );
        tiered.insert(
            "primary_api_key_env".into(),
            json_str(form, "tiered.primary_api_key_env"),
        );

        match parse_threshold(form, "tiered.primary_threshold") {
            Ok(v) => {
                tiered.insert(
                    "primary_threshold".into(),
                    Value::Number(serde_json::Number::from_f64(v).unwrap_or_else(|| 0.into())),
                );
            }
            Err(e) => errors.push(e),
        }

        insert_opt_str_prefixed(&mut tiered, form, "tiered.fallback_model", "fallback_model");
        insert_opt_str_prefixed(
            &mut tiered,
            form,
            "tiered.fallback_endpoint",
            "fallback_endpoint",
        );
        tiered.insert(
            "retry_with_fallback".into(),
            Value::Bool(form_bool(form, "tiered.retry_with_fallback")),
        );

        obj.insert("tiered".into(), Value::Object(tiered));
    }

    sections.insert("inference".into(), Value::Object(obj));
}

// --- Embeddings ---

fn build_embeddings(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();
    obj.insert("enabled".into(), Value::Bool(form_bool(form, "enabled")));
    obj.insert("provider".into(), json_str(form, "provider"));
    insert_opt_str(&mut obj, form, "model");
    insert_opt_str(&mut obj, form, "endpoint");
    insert_opt_str(&mut obj, form, "api_key_env");
    obj.insert("vector_backend".into(), json_str(form, "vector_backend"));
    insert_opt_str(&mut obj, form, "task_type");

    if let Some(dims_str) = form.get("dimensions").filter(|s| !s.is_empty()) {
        match dims_str.parse::<u32>() {
            Ok(d) if d >= 1 => {
                obj.insert("dimensions".into(), Value::Number(d.into()));
            }
            _ => errors.push("dimensions: must be a positive integer".into()),
        }
    }

    // Candle sub-config
    let candle_model_dir = form.get("candle.model_dir").filter(|s| !s.is_empty());
    if let Some(dir) = candle_model_dir {
        let mut candle = Map::new();
        candle.insert("model_dir".into(), Value::String(dir.clone()));
        obj.insert("candle".into(), Value::Object(candle));
    }

    sections.insert("embeddings".into(), Value::Object(obj));
}

// --- Search ---

fn build_search(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();
    obj.insert("reranker".into(), json_str(form, "reranker"));

    match parse_usize_min(form, "rerank_window", 1) {
        Ok(v) => {
            obj.insert("rerank_window".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }

    // Thresholds sub-object
    let mut thresholds = Map::new();
    for key in &["default", "rust", "typescript", "python"] {
        let form_key = format!("thresholds.{key}");
        match parse_f64_range(form, &form_key, 0.0, 1.0) {
            Ok(v) => {
                if let Some(n) = serde_json::Number::from_f64(v) {
                    thresholds.insert((*key).into(), Value::Number(n));
                }
            }
            Err(e) => errors.push(e),
        }
    }
    obj.insert("thresholds".into(), Value::Object(thresholds));

    // Candle reranker config
    if let Some(dir) = form.get("candle.model_dir").filter(|s| !s.is_empty()) {
        let mut candle = Map::new();
        candle.insert("model_dir".into(), Value::String(dir.clone()));
        obj.insert("candle".into(), Value::Object(candle));
    }

    sections.insert("search".into(), Value::Object(obj));

    // Providers (Cohere API key)
    if form.contains_key("providers.cohere.api_key_env") {
        let mut providers = Map::new();
        let mut cohere = Map::new();
        cohere.insert(
            "api_key_env".into(),
            json_str(form, "providers.cohere.api_key_env"),
        );
        providers.insert("cohere".into(), Value::Object(cohere));
        sections.insert("providers".into(), Value::Object(providers));
    }
}

// --- Indexing ---

fn build_indexing(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();

    if form.contains_key("git_debounce_secs") {
        match parse_f64_nonneg(form, "git_debounce_secs") {
            Ok(v) => {
                if let Some(n) = serde_json::Number::from_f64(v) {
                    obj.insert("git_debounce_secs".into(), Value::Number(n));
                }
            }
            Err(e) => errors.push(e),
        }
    }

    sections.insert("watcher".into(), Value::Object(obj));
}

// --- Dashboard ---

fn build_dashboard(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();

    match parse_port(form, "port") {
        Ok(p) => {
            obj.insert("port".into(), Value::Number(p.into()));
        }
        Err(e) => errors.push(e),
    }

    obj.insert("enabled".into(), Value::Bool(form_bool(form, "enabled")));

    sections.insert("dashboard".into(), Value::Object(obj));
}

// --- Generation (sir_quality) ---

fn build_generation(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();

    // Triage pass
    obj.insert(
        "triage_pass".into(),
        Value::Bool(form_bool(form, "triage_pass")),
    );
    insert_opt_str(&mut obj, form, "triage_provider");
    insert_opt_str(&mut obj, form, "triage_model");
    insert_opt_str(&mut obj, form, "triage_endpoint");
    insert_opt_str(&mut obj, form, "triage_api_key_env");

    match parse_threshold(form, "triage_priority_threshold") {
        Ok(v) => insert_f64(&mut obj, "triage_priority_threshold", v),
        Err(e) => errors.push(e),
    }
    match parse_threshold(form, "triage_confidence_threshold") {
        Ok(v) => insert_f64(&mut obj, "triage_confidence_threshold", v),
        Err(e) => errors.push(e),
    }
    match parse_usize_min(form, "triage_max_symbols", 0) {
        Ok(v) => {
            obj.insert("triage_max_symbols".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    match parse_usize_range(form, "triage_concurrency", 1, 24) {
        Ok(v) => {
            obj.insert("triage_concurrency".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    match parse_usize_min(form, "triage_timeout_secs", 1) {
        Ok(v) => {
            obj.insert("triage_timeout_secs".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }

    // Deep pass
    obj.insert(
        "deep_pass".into(),
        Value::Bool(form_bool(form, "deep_pass")),
    );
    insert_opt_str(&mut obj, form, "deep_provider");
    insert_opt_str(&mut obj, form, "deep_model");
    insert_opt_str(&mut obj, form, "deep_endpoint");
    insert_opt_str(&mut obj, form, "deep_api_key_env");

    match parse_threshold(form, "deep_priority_threshold") {
        Ok(v) => insert_f64(&mut obj, "deep_priority_threshold", v),
        Err(e) => errors.push(e),
    }
    match parse_threshold(form, "deep_confidence_threshold") {
        Ok(v) => insert_f64(&mut obj, "deep_confidence_threshold", v),
        Err(e) => errors.push(e),
    }
    match parse_usize_min(form, "deep_max_symbols", 0) {
        Ok(v) => {
            obj.insert("deep_max_symbols".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    match parse_usize_range(form, "deep_max_neighbors", 1, 50) {
        Ok(v) => {
            obj.insert("deep_max_neighbors".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    match parse_usize_range(form, "deep_concurrency", 1, 24) {
        Ok(v) => {
            obj.insert("deep_concurrency".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    match parse_usize_min(form, "deep_timeout_secs", 1) {
        Ok(v) => {
            obj.insert("deep_timeout_secs".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }

    sections.insert("sir_quality".into(), Value::Object(obj));
}

// --- Advanced (general + storage + verify) ---

fn build_advanced(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    // General
    let mut general = Map::new();
    general.insert("log_level".into(), json_str(form, "general.log_level"));
    sections.insert("general".into(), Value::Object(general));

    // Storage
    let mut storage = Map::new();
    storage.insert(
        "graph_backend".into(),
        json_str(form, "storage.graph_backend"),
    );
    storage.insert(
        "mirror_sir_files".into(),
        Value::Bool(form_bool(form, "storage.mirror_sir_files")),
    );
    sections.insert("storage".into(), Value::Object(storage));

    // Verify
    let mut verify = Map::new();
    verify.insert("mode".into(), json_str(form, "verify.mode"));
    sections.insert("verify".into(), Value::Object(verify));

    let _ = errors; // no special validation beyond type parsing
}

// --- Coupling ---

fn build_coupling(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();
    obj.insert("enabled".into(), Value::Bool(form_bool(form, "enabled")));

    match parse_usize_min(form, "commit_window", 1) {
        Ok(v) => {
            obj.insert("commit_window".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    match parse_usize_min(form, "min_co_change_count", 1) {
        Ok(v) => {
            obj.insert("min_co_change_count".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    match parse_usize_min(form, "bulk_commit_threshold", 1) {
        Ok(v) => {
            obj.insert("bulk_commit_threshold".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }

    match parse_threshold(form, "temporal_weight") {
        Ok(v) => insert_f64(&mut obj, "temporal_weight", v),
        Err(e) => errors.push(e),
    }
    match parse_threshold(form, "static_weight") {
        Ok(v) => insert_f64(&mut obj, "static_weight", v),
        Err(e) => errors.push(e),
    }
    match parse_threshold(form, "semantic_weight") {
        Ok(v) => insert_f64(&mut obj, "semantic_weight", v),
        Err(e) => errors.push(e),
    }

    sections.insert("coupling".into(), Value::Object(obj));
}

// --- Drift ---

fn build_drift(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();
    obj.insert("enabled".into(), Value::Bool(form_bool(form, "enabled")));

    match parse_threshold(form, "drift_threshold") {
        Ok(v) => insert_f64(&mut obj, "drift_threshold", v),
        Err(e) => errors.push(e),
    }

    obj.insert("analysis_window".into(), json_str(form, "analysis_window"));
    obj.insert(
        "auto_analyze".into(),
        Value::Bool(form_bool(form, "auto_analyze")),
    );

    match parse_usize_range(form, "hub_percentile", 1, 100) {
        Ok(v) => {
            obj.insert("hub_percentile".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }

    sections.insert("drift".into(), Value::Object(obj));
}

// --- Health (health + planner + health_score) ---

fn build_health(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    // Health config (risk weights)
    let mut health = Map::new();
    health.insert(
        "enabled".into(),
        Value::Bool(form_bool(form, "health.enabled")),
    );

    let mut risk_weights = Map::new();
    for key in &["pagerank", "test_gap", "drift", "no_sir", "recency"] {
        let form_key = format!("health.risk_weights.{key}");
        match parse_threshold(form, &form_key) {
            Ok(v) => {
                if let Some(n) = serde_json::Number::from_f64(v) {
                    risk_weights.insert((*key).into(), Value::Number(n));
                }
            }
            Err(e) => errors.push(e),
        }
    }
    health.insert("risk_weights".into(), Value::Object(risk_weights));
    sections.insert("health".into(), Value::Object(health));

    // Planner config
    let mut planner = Map::new();
    match parse_f64_range(form, "planner.semantic_rescue_threshold", 0.0, 1.0) {
        Ok(v) => insert_f64(&mut planner, "semantic_rescue_threshold", v),
        Err(e) => errors.push(e),
    }
    match parse_usize_range(form, "planner.semantic_rescue_max_k", 1, 10) {
        Ok(v) => {
            planner.insert("semantic_rescue_max_k".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    match parse_f64_range(form, "planner.community_resolution", 0.1, 3.0) {
        Ok(v) => insert_f64(&mut planner, "community_resolution", v),
        Err(e) => errors.push(e),
    }
    match parse_usize_range(form, "planner.min_community_size", 1, 20) {
        Ok(v) => {
            planner.insert("min_community_size".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    sections.insert("planner".into(), Value::Object(planner));

    // Health score config (structural thresholds)
    let mut hs = Map::new();

    // Warn/fail pairs — validate that fail > warn
    let warn_fail_usize_pairs = &[
        ("file_loc_warn", "file_loc_fail"),
        ("trait_method_warn", "trait_method_fail"),
        ("internal_dep_warn", "internal_dep_fail"),
        ("dead_feature_warn", "dead_feature_fail"),
        ("stale_ref_warn", "stale_ref_fail"),
    ];

    for (warn_key, fail_key) in warn_fail_usize_pairs {
        let hs_warn_key = format!("health_score.{warn_key}");
        let hs_fail_key = format!("health_score.{fail_key}");
        let warn_val = parse_usize_min(form, &hs_warn_key, 1);
        let fail_val = parse_usize_min(form, &hs_fail_key, 1);

        match (&warn_val, &fail_val) {
            (Ok(w), Ok(f)) if f <= w => {
                errors.push(format!(
                    "health_score.{fail_key} must be greater than health_score.{warn_key}"
                ));
            }
            _ => {}
        }

        match warn_val {
            Ok(v) => {
                hs.insert((*warn_key).into(), Value::Number(v.into()));
            }
            Err(e) => errors.push(e),
        }
        match fail_val {
            Ok(v) => {
                hs.insert((*fail_key).into(), Value::Number(v.into()));
            }
            Err(e) => errors.push(e),
        }
    }

    // TODO density warn/fail (f32 values, stored as f64 in JSON)
    let td_warn_key = "health_score.todo_density_warn";
    let td_fail_key = "health_score.todo_density_fail";
    let td_warn = parse_f64_nonneg(form, td_warn_key);
    let td_fail = parse_f64_nonneg(form, td_fail_key);

    match (&td_warn, &td_fail) {
        (Ok(w), Ok(f)) if f <= w => {
            errors.push(
                "health_score.todo_density_fail must be greater than health_score.todo_density_warn"
                    .into(),
            );
        }
        _ => {}
    }
    match td_warn {
        Ok(v) => insert_f64(&mut hs, "todo_density_warn", v),
        Err(e) => errors.push(e),
    }
    match td_fail {
        Ok(v) => insert_f64(&mut hs, "todo_density_fail", v),
        Err(e) => errors.push(e),
    }

    // Non-paired usize fields
    for key in &["churn_30d_high", "churn_90d_high", "author_count_high"] {
        let form_key = format!("health_score.{key}");
        match parse_usize_min(form, &form_key, 0) {
            Ok(v) => {
                hs.insert((*key).into(), Value::Number(v.into()));
            }
            Err(e) => errors.push(e),
        }
    }

    // u64 field
    if form.contains_key("health_score.blame_age_spread_high_secs") {
        match parse_u64_min(form, "health_score.blame_age_spread_high_secs", 0) {
            Ok(v) => {
                hs.insert("blame_age_spread_high_secs".into(), Value::Number(v.into()));
            }
            Err(e) => errors.push(e),
        }
    }

    // Float thresholds (0.0-1.0)
    for key in &[
        "drift_density_high",
        "stale_sir_high",
        "test_gap_high",
        "boundary_leakage_high",
    ] {
        let form_key = format!("health_score.{key}");
        match parse_threshold(form, &form_key) {
            Ok(v) => insert_f64(&mut hs, key, v),
            Err(e) => errors.push(e),
        }
    }

    sections.insert("health_score".into(), Value::Object(hs));
}

// --- Batch ---

fn build_batch(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();

    // Models
    obj.insert("scan_model".into(), json_str(form, "scan_model"));
    obj.insert("triage_model".into(), json_str(form, "triage_model"));
    obj.insert("deep_model".into(), json_str(form, "deep_model"));

    // Thinking levels
    obj.insert("scan_thinking".into(), json_str(form, "scan_thinking"));
    obj.insert("triage_thinking".into(), json_str(form, "triage_thinking"));
    obj.insert("deep_thinking".into(), json_str(form, "deep_thinking"));

    // Pipeline
    obj.insert(
        "auto_chain".into(),
        Value::Bool(form_bool(form, "auto_chain")),
    );

    match parse_usize_min(form, "jsonl_chunk_size", 1) {
        Ok(v) => {
            obj.insert("jsonl_chunk_size".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    match parse_usize_min(form, "poll_interval_secs", 1) {
        Ok(v) => {
            obj.insert("poll_interval_secs".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }

    sections.insert("batch".into(), Value::Object(obj));
}

// --- Watcher ---

fn build_watcher(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();

    obj.insert("realtime_model".into(), json_str(form, "realtime_model"));
    obj.insert(
        "realtime_provider".into(),
        json_str(form, "realtime_provider"),
    );

    // Git triggers (boolean fields — absent = false)
    obj.insert(
        "trigger_on_branch_switch".into(),
        Value::Bool(form_bool(form, "trigger_on_branch_switch")),
    );
    obj.insert(
        "trigger_on_git_pull".into(),
        Value::Bool(form_bool(form, "trigger_on_git_pull")),
    );
    obj.insert(
        "trigger_on_merge".into(),
        Value::Bool(form_bool(form, "trigger_on_merge")),
    );
    obj.insert(
        "trigger_on_build_success".into(),
        Value::Bool(form_bool(form, "trigger_on_build_success")),
    );
    obj.insert(
        "git_trigger_changed_files_only".into(),
        Value::Bool(form_bool(form, "git_trigger_changed_files_only")),
    );

    match parse_f64_nonneg(form, "git_debounce_secs") {
        Ok(v) => {
            if let Some(n) = serde_json::Number::from_f64(v) {
                obj.insert("git_debounce_secs".into(), Value::Number(n));
            }
        }
        Err(e) => errors.push(e),
    }

    sections.insert("watcher".into(), Value::Object(obj));
}

// --- Continuous ---

fn build_continuous(
    form: &HashMap<String, String>,
    sections: &mut HashMap<String, Value>,
    errors: &mut Vec<String>,
) {
    let mut obj = Map::new();

    obj.insert("enabled".into(), Value::Bool(form_bool(form, "enabled")));
    obj.insert("schedule".into(), json_str(form, "schedule"));

    // Staleness scoring
    match parse_f64_nonneg(form, "staleness_half_life_days") {
        Ok(v) => insert_f64(&mut obj, "staleness_half_life_days", v),
        Err(e) => errors.push(e),
    }
    match parse_f64_nonneg(form, "staleness_sigmoid_k") {
        Ok(v) => insert_f64(&mut obj, "staleness_sigmoid_k", v),
        Err(e) => errors.push(e),
    }
    match parse_threshold(form, "neighbor_decay") {
        Ok(v) => insert_f64(&mut obj, "neighbor_decay", v),
        Err(e) => errors.push(e),
    }
    match parse_threshold(form, "neighbor_cutoff") {
        Ok(v) => insert_f64(&mut obj, "neighbor_cutoff", v),
        Err(e) => errors.push(e),
    }
    match parse_threshold(form, "coupling_predict_threshold") {
        Ok(v) => insert_f64(&mut obj, "coupling_predict_threshold", v),
        Err(e) => errors.push(e),
    }

    // Requeue
    match parse_usize_min(form, "max_requeue_per_run", 0) {
        Ok(v) => {
            obj.insert("max_requeue_per_run".into(), Value::Number(v.into()));
        }
        Err(e) => errors.push(e),
    }
    obj.insert(
        "auto_submit".into(),
        Value::Bool(form_bool(form, "auto_submit")),
    );
    obj.insert("requeue_pass".into(), json_str(form, "requeue_pass"));

    match parse_threshold(form, "priority_pagerank_alpha") {
        Ok(v) => insert_f64(&mut obj, "priority_pagerank_alpha", v),
        Err(e) => errors.push(e),
    }

    sections.insert("continuous".into(), Value::Object(obj));
}

// ---------------------------------------------------------------------------
// Utility: insert f64 into a JSON map
// ---------------------------------------------------------------------------

fn insert_f64(obj: &mut Map<String, Value>, key: &str, val: f64) {
    if let Some(n) = serde_json::Number::from_f64(val) {
        obj.insert(key.into(), Value::Number(n));
    }
}

fn insert_opt_str_prefixed(
    obj: &mut Map<String, Value>,
    form: &HashMap<String, String>,
    form_key: &str,
    obj_key: &str,
) {
    if let Some(v) = form.get(form_key).filter(|s| !s.is_empty()) {
        obj.insert(obj_key.into(), Value::String(v.clone()));
    }
}
