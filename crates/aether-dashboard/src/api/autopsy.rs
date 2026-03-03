use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::catalog::{CatalogSymbol, load_symbol_catalog};
use crate::api::difficulty::{DifficultyView, difficulty_for_symbol};
use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AutopsyPrompt {
    pub level: String,
    pub emoji: String,
    pub label: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why_it_works: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub what_goes_wrong: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub key_elements: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_elements: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AutopsyData {
    pub symbol: String,
    pub kind: String,
    pub file: String,
    pub difficulty: DifficultyView,
    pub prompts: Vec<AutopsyPrompt>,
    pub teaching_summary: String,
    pub pattern_name: String,
    pub pattern_rule: String,
}

pub(crate) async fn autopsy_handler(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Response {
    let shared = state.shared.clone();
    let selector_for_build = selector.clone();
    match support::run_blocking_with_timeout(move || {
        build_autopsy_data(shared.as_ref(), selector_for_build.as_str())
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

pub(crate) fn build_autopsy_data(
    shared: &SharedState,
    selector: &str,
) -> Result<Option<AutopsyData>, String> {
    let catalog = load_symbol_catalog(shared)?;
    let Some(resolved) = catalog.resolve_symbol_selector(selector) else {
        return Ok(None);
    };
    let Some(symbol) = catalog.symbol(resolved.primary_index) else {
        return Ok(None);
    };

    Ok(Some(build_autopsy_from_symbol(symbol)))
}

pub(crate) fn build_autopsy_from_symbol(symbol: &CatalogSymbol) -> AutopsyData {
    let difficulty = difficulty_for_symbol(symbol);

    let control_flow_hints = detect_control_flow_hints(symbol);
    let dependency_types = if symbol.sir.dependencies.is_empty() {
        vec!["existing project types".to_owned()]
    } else {
        symbol.sir.dependencies.clone()
    };

    let good_prompt = compose_good_prompt(symbol, control_flow_hints.as_slice());
    let key_elements = compose_key_elements(symbol, control_flow_hints.as_slice());

    let partial_prompt = compose_partial_prompt(symbol);
    let partial_missing = compose_partial_missing(symbol, control_flow_hints.as_slice());

    let bad_prompt = format!(
        "Build the {} component for my project.",
        symbol.name.to_ascii_lowercase()
    );
    let bad_missing = vec![
        "No type information -> model invents incompatible types".to_owned(),
        "No existing integration constraints -> output diverges from project interfaces".to_owned(),
        "No dependency context -> wrong calls and missing contracts".to_owned(),
        "No control-flow detail -> happy-path only implementation".to_owned(),
    ];

    let (pattern_name, pattern_rule) = pick_pattern(symbol, control_flow_hints.as_slice());

    let prompts = vec![
        AutopsyPrompt {
            level: "good".to_owned(),
            emoji: "✅".to_owned(),
            label: "This Would Work".to_owned(),
            prompt: good_prompt,
            why_it_works: Some("This prompt specifies concrete types, control flow structure, and failure/termination behavior. That gives the model enough constraints to generate code that integrates with the existing codebase.".to_owned()),
            what_goes_wrong: None,
            key_elements,
            missing_elements: Vec::new(),
        },
        AutopsyPrompt {
            level: "partial".to_owned(),
            emoji: "⚠️".to_owned(),
            label: "This Would Have Bugs".to_owned(),
            prompt: partial_prompt,
            why_it_works: None,
            what_goes_wrong: Some("This prompt states the goal but omits structure and edge conditions. The likely output handles the happy path and misses shutdown/error branches or contract details.".to_owned()),
            key_elements: Vec::new(),
            missing_elements: partial_missing,
        },
        AutopsyPrompt {
            level: "bad".to_owned(),
            emoji: "❌".to_owned(),
            label: "This Would Fail".to_owned(),
            prompt: bad_prompt,
            why_it_works: None,
            what_goes_wrong: Some("Without context, the model will invent abstractions that do not match project types, interfaces, or flow conventions. Integration work will outweigh any speed gain.".to_owned()),
            key_elements: Vec::new(),
            missing_elements: bad_missing,
        },
    ];

    let teaching_summary = format!(
        "The working prompt succeeds because it supplies HOW as well as WHAT: concrete dependencies ({}) and explicit flow details ({}). As difficulty increases, specify more control-flow and error-handling detail.",
        dependency_types.join(", "),
        if control_flow_hints.is_empty() {
            "branching and termination rules".to_owned()
        } else {
            control_flow_hints.join(", ")
        }
    );

    AutopsyData {
        symbol: symbol.name.clone(),
        kind: symbol.kind.clone(),
        file: symbol.file_path.clone(),
        difficulty,
        prompts,
        teaching_summary,
        pattern_name,
        pattern_rule,
    }
}

fn compose_good_prompt(symbol: &CatalogSymbol, control_flow_hints: &[String]) -> String {
    let mut prompt = format!(
        "Implement {} in {} with the existing project interfaces.",
        symbol.name, symbol.file_path
    );

    if !symbol.sir.dependencies.is_empty() {
        prompt.push(' ');
        prompt.push_str(
            format!(
                "Use these dependencies/types explicitly: {}.",
                symbol.sir.dependencies.join(", ")
            )
            .as_str(),
        );
    }

    if !control_flow_hints.is_empty() {
        prompt.push(' ');
        prompt.push_str(
            format!(
                "Control flow requirements: {}.",
                control_flow_hints.join(", ")
            )
            .as_str(),
        );
    }

    if !symbol.sir.error_modes.is_empty() {
        prompt.push(' ');
        prompt.push_str(
            format!(
                "Handle these failure modes: {}.",
                symbol.sir.error_modes.join(", ")
            )
            .as_str(),
        );
    }

    if !symbol.sir.side_effects.is_empty() {
        prompt.push(' ');
        prompt.push_str(
            format!(
                "Preserve side effects: {}.",
                symbol.sir.side_effects.join(", ")
            )
            .as_str(),
        );
    }

    prompt
}

fn compose_partial_prompt(symbol: &CatalogSymbol) -> String {
    let first_sentence = crate::api::common::first_sentence(symbol.sir.intent.as_str());
    if first_sentence.trim().is_empty() {
        format!("Implement {} for this codebase.", symbol.name)
    } else {
        format!("Implement {}. {}", symbol.name, first_sentence)
    }
}

fn compose_key_elements(symbol: &CatalogSymbol, control_flow_hints: &[String]) -> Vec<String> {
    let mut elements = Vec::<String>::new();

    if !symbol.sir.dependencies.is_empty() {
        elements.push(format!(
            "Dependency/type context included ({})",
            symbol.sir.dependencies.join(", ")
        ));
    }

    if !control_flow_hints.is_empty() {
        elements.push(format!(
            "Control flow specified ({})",
            control_flow_hints.join(", ")
        ));
    }

    if !symbol.sir.error_modes.is_empty() {
        elements.push(format!(
            "Error cases enumerated ({})",
            symbol.sir.error_modes.join(", ")
        ));
    }

    if !symbol.sir.side_effects.is_empty() {
        elements.push(format!(
            "Side effects constrained ({})",
            symbol.sir.side_effects.join(", ")
        ));
    }

    if elements.is_empty() {
        elements.push("Project integration constraints explicitly requested".to_owned());
    }

    elements
}

fn compose_partial_missing(symbol: &CatalogSymbol, control_flow_hints: &[String]) -> Vec<String> {
    let mut missing = Vec::<String>::new();

    if !symbol.sir.dependencies.is_empty() {
        missing
            .push("No explicit type/dependency list -> interface mismatches are likely".to_owned());
    }

    if !control_flow_hints.is_empty() {
        missing.push(
            "No explicit control-flow shape -> model may choose incorrect branch structure"
                .to_owned(),
        );
    }

    if !symbol.sir.error_modes.is_empty() {
        missing.push("No explicit error mode list -> happy-path bias".to_owned());
    }

    if !symbol.sir.side_effects.is_empty() {
        missing.push(
            "No explicit side-effect requirements -> missing notifications/cleanup".to_owned(),
        );
    }

    if missing.is_empty() {
        missing.push("Prompt is high-level and leaves integration details ambiguous".to_owned());
    }

    missing
}

fn pick_pattern(symbol: &CatalogSymbol, control_flow_hints: &[String]) -> (String, String) {
    let intent = symbol.sir.intent.to_ascii_lowercase();
    let kind = symbol.kind.to_ascii_lowercase();

    if control_flow_hints
        .iter()
        .any(|hint| hint.contains("select") || hint.contains("loop") || hint.contains("channel"))
    {
        return (
            "Async State Machine".to_owned(),
            "For async operations with multiple event sources, specify the loop/select structure, each branch, and all termination conditions.".to_owned(),
        );
    }

    if intent.contains("arc")
        || intent.contains("mutex")
        || intent.contains("lock")
        || intent.contains("shared")
        || intent.contains("concurrent")
    {
        return (
            "Concurrent Shared State".to_owned(),
            "For shared-state code, specify wrapping pattern, locking boundaries, and lifecycle ownership.".to_owned(),
        );
    }

    if symbol.sir.error_modes.len() > 3 {
        return (
            "Error-Heavy Operations".to_owned(),
            "Enumerate each failure mode explicitly. LLMs default to happy-path implementations when errors are underspecified.".to_owned(),
        );
    }

    if intent.contains("parse")
        || intent.contains("frame")
        || intent.contains("serialize")
        || kind.contains("protocol")
    {
        return (
            "Protocol Implementation".to_owned(),
            "For protocol code, include format constraints and at least one expected input/output mapping.".to_owned(),
        );
    }

    (
        "Data Transformation".to_owned(),
        "For lower-side-effect transforms, specify input type, output type, and transformation rule.".to_owned(),
    )
}

fn detect_control_flow_hints(symbol: &CatalogSymbol) -> Vec<String> {
    let mut hints = Vec::<String>::new();
    let intent = symbol.sir.intent.to_ascii_lowercase();

    for (needle, label) in [
        ("loop", "loop"),
        ("select", "select!"),
        ("match", "match branching"),
        ("await", "async await"),
        ("spawn", "task spawn"),
        ("channel", "channel receive/send"),
        ("iterate", "iteration"),
    ] {
        if intent.contains(needle) {
            hints.push(label.to_owned());
        }
    }

    hints
}
