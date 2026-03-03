use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::catalog::{CatalogSymbol, load_symbol_catalog, parse_dependency_entry};
use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GeneratedSpec {
    pub purpose: String,
    pub requirements: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_handling: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SpecData {
    pub symbol: String,
    pub kind: String,
    pub file: String,
    pub spec: GeneratedSpec,
}

pub(crate) async fn spec_handler(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Response {
    let shared = state.shared.clone();
    let selector_for_build = selector.clone();
    match support::run_blocking_with_timeout(move || {
        build_spec_data(shared.as_ref(), selector_for_build.as_str())
    })
    .await
    {
        Ok(Some(data)) => support::api_json(state.shared.as_ref(), data).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "not_found",
                "message": format!("symbol '{}' not found", selector)
            })),
        )
            .into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

pub(crate) fn build_spec_data(
    shared: &SharedState,
    selector: &str,
) -> Result<Option<SpecData>, String> {
    let catalog = load_symbol_catalog(shared)?;
    let Some(resolved) = catalog.resolve_symbol_selector(selector) else {
        return Ok(None);
    };
    let Some(symbol) = catalog.symbol(resolved.primary_index) else {
        return Ok(None);
    };
    Ok(Some(build_spec_from_symbol(symbol)))
}

pub(crate) fn build_spec_from_symbol(symbol: &CatalogSymbol) -> SpecData {
    let purpose = first_nonempty_sentence(
        symbol.sir.intent.as_str(),
        "No SIR intent summary available",
    );
    let mut requirements = to_requirement_sentences(symbol.sir.intent.as_str());
    if requirements.is_empty() {
        requirements.push("MUST satisfy the documented symbol intent.".to_owned());
    }

    let inputs = if symbol.sir.inputs.is_empty() {
        vec!["No explicit inputs documented in SIR".to_owned()]
    } else {
        symbol.sir.inputs.clone()
    };

    let outputs = if symbol.sir.outputs.is_empty() {
        vec!["No explicit outputs documented in SIR".to_owned()]
    } else {
        symbol.sir.outputs.clone()
    };

    let dependencies = if symbol.sir.dependencies.is_empty() {
        vec!["No explicit dependencies documented in SIR".to_owned()]
    } else {
        symbol
            .sir
            .dependencies
            .iter()
            .map(|dep| {
                let (name, reason) = parse_dependency_entry(dep.as_str());
                if let Some(reason) = reason {
                    format!("{name} ({reason})")
                } else {
                    name
                }
            })
            .collect()
    };

    let error_handling = if symbol.sir.error_modes.is_empty() {
        vec!["No explicit error modes documented in SIR".to_owned()]
    } else {
        symbol
            .sir
            .error_modes
            .iter()
            .map(|mode| format!("MUST handle {mode}"))
            .collect()
    };

    SpecData {
        symbol: symbol.name.clone(),
        kind: symbol.kind.clone(),
        file: symbol.file_path.clone(),
        spec: GeneratedSpec {
            purpose,
            requirements,
            inputs,
            outputs,
            dependencies,
            error_handling,
        },
    }
}

fn to_requirement_sentences(intent: &str) -> Vec<String> {
    intent
        .split(['.', '!', '?'])
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            if text.to_ascii_uppercase().starts_with("MUST ") {
                text.to_owned()
            } else {
                format!("MUST {}", normalize_requirement_sentence(text))
            }
        })
        .collect()
}

fn normalize_requirement_sentence(sentence: &str) -> String {
    let mut chars = sentence.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!(
        "{}{}",
        first.to_ascii_lowercase(),
        chars.collect::<String>()
    )
}

fn first_nonempty_sentence(input: &str, fallback: &str) -> String {
    let sentence = crate::api::common::first_sentence(input);
    if sentence.trim().is_empty() {
        fallback.to_owned()
    } else {
        sentence
    }
}
