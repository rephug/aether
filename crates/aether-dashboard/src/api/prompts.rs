use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::api::catalog::{CatalogSymbol, SymbolCatalog, load_symbol_catalog};
use crate::api::difficulty::{DifficultyView, difficulty_for_symbol};
use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptGoal {
    UnderstandComponent,
    UnderstandFlow,
    FindRelated,
    AssessRisk,
    Debug,
    PlanRefactor,
    HealthCheck,
    UnderstandHistory,
    BuildStepByStep,
}

impl PromptGoal {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim() {
            "understand_component" => Some(Self::UnderstandComponent),
            "understand_flow" => Some(Self::UnderstandFlow),
            "find_related" => Some(Self::FindRelated),
            "assess_risk" => Some(Self::AssessRisk),
            "debug" => Some(Self::Debug),
            "plan_refactor" => Some(Self::PlanRefactor),
            "health_check" => Some(Self::HealthCheck),
            "understand_history" => Some(Self::UnderstandHistory),
            "build_step_by_step" => Some(Self::BuildStepByStep),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UnderstandComponent => "understand_component",
            Self::UnderstandFlow => "understand_flow",
            Self::FindRelated => "find_related",
            Self::AssessRisk => "assess_risk",
            Self::Debug => "debug",
            Self::PlanRefactor => "plan_refactor",
            Self::HealthCheck => "health_check",
            Self::UnderstandHistory => "understand_history",
            Self::BuildStepByStep => "build_step_by_step",
        }
    }

    pub(crate) fn tool_name(self) -> &'static str {
        match self {
            Self::UnderstandComponent => "aether_explain",
            Self::UnderstandFlow => "aether_dependencies",
            Self::FindRelated => "aether_search",
            Self::AssessRisk => "aether_blast_radius",
            Self::Debug => "aether_explain + aether_dependencies",
            Self::PlanRefactor => "aether_blast_radius + aether_coupling",
            Self::HealthCheck => "aether_health",
            Self::UnderstandHistory => "aether_ask",
            Self::BuildStepByStep => "aether_dependencies + aether_explain",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PromptSearchQuery {
    pub q: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PromptGenerateQuery {
    pub goal: Option<String>,
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PromptSearchItem {
    pub symbol_id: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file: String,
    pub intent: String,
    pub difficulty: DifficultyView,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PromptSearchData {
    pub results: Vec<PromptSearchItem>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PromptGenerateData {
    pub goal: String,
    pub mcp_tool: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<PromptSearchItem>,
}

pub(crate) async fn prompt_search_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<PromptSearchQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || {
        build_prompt_search_data(shared.as_ref(), query.q.as_deref().unwrap_or_default())
    })
    .await
    {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

pub(crate) async fn prompt_generate_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<PromptGenerateQuery>,
) -> Response {
    let Some(goal_raw) = query
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "invalid_request",
                "message": "query parameter 'goal' is required"
            })),
        )
            .into_response();
    };

    let Some(goal) = PromptGoal::from_str(goal_raw) else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "invalid_request",
                "message": format!("unsupported goal '{}'", goal_raw)
            })),
        )
            .into_response();
    };

    let shared = state.shared.clone();
    let symbol = query.symbol.clone();
    match support::run_blocking_with_timeout(move || {
        build_generated_prompt(shared.as_ref(), goal, symbol.as_deref())
    })
    .await
    {
        Ok(Some(data)) => support::api_json(state.shared.as_ref(), data).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "not_found",
                "message": "symbol not found"
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

pub(crate) fn build_prompt_search_data(
    shared: &SharedState,
    query: &str,
) -> Result<PromptSearchData, String> {
    let catalog = load_symbol_catalog(shared)?;
    Ok(build_prompt_search_from_catalog(&catalog, query, 20))
}

pub(crate) fn build_prompt_search_from_catalog(
    catalog: &SymbolCatalog,
    query: &str,
    limit: usize,
) -> PromptSearchData {
    let q = query.trim().to_ascii_lowercase();

    let mut results = catalog
        .symbols
        .iter()
        .filter(|symbol| {
            if q.is_empty() {
                return true;
            }
            let searchable = format!(
                "{}\n{}\n{}\n{}",
                symbol.name, symbol.qualified_name, symbol.file_path, symbol.sir.intent
            )
            .to_ascii_lowercase();
            searchable.contains(q.as_str())
        })
        .map(to_search_item)
        .collect::<Vec<_>>();

    results.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    results.truncate(limit.max(1));

    PromptSearchData {
        count: results.len(),
        results,
    }
}

pub(crate) fn build_generated_prompt(
    shared: &SharedState,
    goal: PromptGoal,
    symbol_selector: Option<&str>,
) -> Result<Option<PromptGenerateData>, String> {
    let catalog = load_symbol_catalog(shared)?;
    build_generated_prompt_from_catalog(&catalog, goal, symbol_selector)
}

pub(crate) fn build_generated_prompt_from_catalog(
    catalog: &SymbolCatalog,
    goal: PromptGoal,
    symbol_selector: Option<&str>,
) -> Result<Option<PromptGenerateData>, String> {
    if goal == PromptGoal::HealthCheck {
        return Ok(Some(PromptGenerateData {
            goal: goal.as_str().to_owned(),
            mcp_tool: goal.tool_name().to_owned(),
            prompt: "Run an AETHER health check. Show the overall score, weakest areas, and what to work on first.".to_owned(),
            symbol: None,
        }));
    }

    let Some(selector) = symbol_selector
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let Some(resolved) = catalog.resolve_symbol_selector(selector) else {
        return Ok(None);
    };
    let Some(symbol) = catalog.symbol(resolved.primary_index) else {
        return Ok(None);
    };

    let prompt = match goal {
        PromptGoal::UnderstandComponent => format!(
            "Explain what {} in {} does. Use the AETHER explain tool to get its semantic analysis, then describe its purpose, what it depends on, and what depends on it, in plain English.",
            symbol.name, symbol.file_path
        ),
        PromptGoal::UnderstandFlow => format!(
            "Trace the data flow starting from {}. Use AETHER's dependency tool to show what it calls and what calls it, then explain the complete flow.",
            symbol.name
        ),
        PromptGoal::FindRelated => format!(
            "Search AETHER for everything related to {}. Show semantically similar symbols and dependency connections.",
            symbol.name
        ),
        PromptGoal::AssessRisk => format!(
            "What's the blast radius if I change {} in {}? Explain the risk level and what tests to run.",
            symbol.name, symbol.file_path
        ),
        PromptGoal::Debug => format!(
            "I'm debugging an issue with {}. Explain what it does, its error modes, and trace dependencies to find likely failure points.",
            symbol.name
        ),
        PromptGoal::PlanRefactor => format!(
            "I want to refactor {}. Show what would be affected and suggest a safe refactoring plan.",
            symbol.name
        ),
        PromptGoal::UnderstandHistory => format!(
            "Why was {} built this way? What design decisions led to its current structure?",
            symbol.name
        ),
        PromptGoal::BuildStepByStep => format!(
            "Decompose {} in {} into an implementation sequence. Start with dependency-free foundations, then add each dependent layer. Provide explicit checkpoints after each step.",
            symbol.name, symbol.file_path
        ),
        PromptGoal::HealthCheck => String::new(),
    };

    Ok(Some(PromptGenerateData {
        goal: goal.as_str().to_owned(),
        mcp_tool: goal.tool_name().to_owned(),
        prompt,
        symbol: Some(to_search_item(symbol)),
    }))
}

fn to_search_item(symbol: &CatalogSymbol) -> PromptSearchItem {
    PromptSearchItem {
        symbol_id: symbol.id.clone(),
        name: symbol.name.clone(),
        qualified_name: symbol.qualified_name.clone(),
        kind: symbol.kind.clone(),
        file: symbol.file_path.clone(),
        intent: first_sentence(symbol.sir.intent.as_str()),
        difficulty: difficulty_for_symbol(symbol),
    }
}

fn first_sentence(intent: &str) -> String {
    let first = crate::api::common::first_sentence(intent);
    if first.trim().is_empty() {
        "No SIR intent summary available".to_owned()
    } else {
        first
    }
}
