use std::cmp::Ordering;
use std::path::Path;
use std::sync::Arc;

use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_memory::{
    AskInclude, AskQueryRequest, AskResultItem, AskResultKind, ProjectMemoryService, SemanticQuery,
};
use aether_store::SirStateStore;
use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::common;
use crate::narrative::classify_layer;
use crate::state::SharedState;
use crate::support::{self, DashboardState};

pub(crate) const ASK_NEEDS_INDEX_MESSAGE: &str = "AETHER needs to index this project before it can answer questions. Run: aetherd --workspace . --index-once";
const ASK_EMPTY_QUERY_MESSAGE: &str = "Type a question to search this codebase.";

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct AskRequest {
    pub question: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AskDashboardData {
    pub question: String,
    pub answer_type: String,
    pub results: Vec<AskDashboardResult>,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AskDashboardResult {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub snippet: String,
    pub relevance_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fused_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coupling_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sir_intent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sir_dependencies: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sir_error_modes: Option<Vec<String>>,
}

pub(crate) async fn ask_handler(
    State(state): State<Arc<DashboardState>>,
    Json(payload): Json<AskRequest>,
) -> impl IntoResponse {
    let question = payload.question.unwrap_or_default();
    let limit = payload.limit.unwrap_or(10);

    let shared = state.shared.clone();
    match support::run_async_with_timeout(move || async move {
        load_ask_data(shared.as_ref(), question, limit).await
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

pub(crate) async fn load_ask_data(
    shared: &SharedState,
    question: String,
    limit: u32,
) -> Result<AskDashboardData, String> {
    let trimmed = question.trim();
    if trimmed.is_empty() {
        return Ok(AskDashboardData {
            question: String::new(),
            answer_type: "empty_query".to_owned(),
            results: Vec::new(),
            summary: ASK_EMPTY_QUERY_MESSAGE.to_owned(),
            message: Some(ASK_EMPTY_QUERY_MESSAGE.to_owned()),
        });
    }

    if !ask_index_available(shared).await? {
        return Ok(AskDashboardData {
            question: trimmed.to_owned(),
            answer_type: "unavailable".to_owned(),
            results: Vec::new(),
            summary: ASK_NEEDS_INDEX_MESSAGE.to_owned(),
            message: Some(ASK_NEEDS_INDEX_MESSAGE.to_owned()),
        });
    }

    let semantic = build_semantic_query(shared.workspace.as_path(), trimmed).await;
    let service = ProjectMemoryService::new(shared.workspace.as_path());
    let raw = service
        .ask(AskQueryRequest {
            query: trimmed.to_owned(),
            limit: limit.clamp(1, 100),
            include: vec![
                AskInclude::Symbols,
                AskInclude::Notes,
                AskInclude::Coupling,
                AskInclude::Tests,
            ],
            now_ms: None,
            semantic,
        })
        .await
        .map_err(|err| err.to_string())?;

    let mut mapped = raw
        .results
        .into_iter()
        .map(|item| map_ask_result(shared, item))
        .collect::<Result<Vec<_>, _>>()?;

    mapped.sort_by(|left, right| {
        right
            .relevance_score
            .partial_cmp(&left.relevance_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.title.cmp(&right.title))
    });

    let summary = compose_summary(trimmed, mapped.as_slice());

    Ok(AskDashboardData {
        question: trimmed.to_owned(),
        answer_type: "search_results".to_owned(),
        results: mapped,
        summary,
        message: None,
    })
}

async fn ask_index_available(shared: &SharedState) -> Result<bool, String> {
    let overview = support::load_overview_data(shared).await?;
    Ok(overview.total_symbols > 0 && overview.sir_count > 0)
}

async fn build_semantic_query(workspace: &Path, question: &str) -> Option<SemanticQuery> {
    let loaded =
        match load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
        {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return None,
            Err(err) => {
                tracing::warn!(error = %err, "dashboard ask: embedding provider unavailable");
                return None;
            }
        };

    match loaded.provider.embed_text(question).await {
        Ok(embedding) if !embedding.is_empty() => Some(SemanticQuery {
            provider: loaded.provider_name,
            model: loaded.model_name,
            embedding,
        }),
        Ok(_) => {
            tracing::warn!("dashboard ask: embedding provider returned empty vector");
            None
        }
        Err(err) => {
            tracing::warn!(error = %err, "dashboard ask: failed to embed question");
            None
        }
    }
}

fn map_ask_result(shared: &SharedState, item: AskResultItem) -> Result<AskDashboardResult, String> {
    let kind = ask_kind_name(item.kind).to_owned();
    let mut result = AskDashboardResult {
        kind,
        id: item.id,
        title: item.title,
        snippet: item.snippet,
        relevance_score: item.relevance_score,
        file: item
            .file
            .map(|value| support::normalized_display_path(value.as_str())),
        language: item.language,
        tags: item.tags,
        source_type: item.source_type,
        test_file: item
            .test_file
            .map(|value| support::normalized_display_path(value.as_str())),
        fused_score: item.fused_score,
        coupling_type: item.coupling_type,
        symbol: None,
        layer: None,
        sir_intent: None,
        sir_dependencies: None,
        sir_error_modes: None,
    };

    if item.kind != AskResultKind::Symbol {
        return Ok(result);
    }

    let Some(symbol_id) = result.id.as_deref() else {
        return Ok(result);
    };

    let symbol_record = shared
        .store
        .get_symbol_record(symbol_id)
        .map_err(|err| err.to_string())?;

    let qualified_name = symbol_record
        .as_ref()
        .map(|record| record.qualified_name.as_str())
        .or(result.title.as_deref())
        .unwrap_or(symbol_id);
    let symbol_name = support::symbol_name_from_qualified(qualified_name);

    let file_path = symbol_record
        .as_ref()
        .map(|record| record.file_path.clone())
        .or_else(|| result.file.clone())
        .unwrap_or_default();

    if !file_path.is_empty() {
        result.file = Some(support::normalized_display_path(file_path.as_str()));
    }

    let mut sir_intent = String::new();
    let mut sir_dependencies = Vec::<String>::new();
    let mut sir_error_modes = Vec::<String>::new();

    if let Some(blob) = shared
        .store
        .read_sir_blob(symbol_id)
        .map_err(|err| err.to_string())?
    {
        let parsed: Value = serde_json::from_str(blob.as_str()).unwrap_or(Value::Null);
        if let Some(intent) = parsed
            .get("intent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            sir_intent = intent.to_owned();
        }
        sir_dependencies = as_string_vec(parsed.get("dependencies"));
        sir_error_modes = as_string_vec(parsed.get("error_modes"));
    }

    let layer = classify_layer(
        file_path.as_str(),
        qualified_name,
        (!sir_intent.is_empty()).then_some(sir_intent.as_str()),
    );

    result.symbol = Some(symbol_name);
    result.layer = Some(layer.name);
    result.sir_intent = (!sir_intent.is_empty()).then_some(sir_intent);
    result.sir_dependencies = (!sir_dependencies.is_empty()).then_some(sir_dependencies);
    result.sir_error_modes = (!sir_error_modes.is_empty()).then_some(sir_error_modes);

    Ok(result)
}

fn compose_summary(question: &str, results: &[AskDashboardResult]) -> String {
    let mut symbol_results = results
        .iter()
        .filter(|result| result.kind == "symbol")
        .collect::<Vec<_>>();
    symbol_results.sort_by(|left, right| {
        right
            .relevance_score
            .partial_cmp(&left.relevance_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.symbol.cmp(&right.symbol))
    });

    if symbol_results.is_empty() {
        if results.is_empty() {
            return "No matching components were found for this question.".to_owned();
        }

        let titles = results
            .iter()
            .filter_map(|row| {
                row.title
                    .as_deref()
                    .or(row.id.as_deref())
                    .map(ToOwned::to_owned)
            })
            .take(3)
            .collect::<Vec<_>>();

        if titles.is_empty() {
            return format!("No symbol matches were found for \"{question}\".");
        }

        return format!(
            "Top matches for \"{question}\" include {}.",
            join_human_list(titles.as_slice())
        );
    }

    let top = symbol_results.into_iter().take(5).collect::<Vec<_>>();
    let components = top
        .iter()
        .filter_map(|row| row.symbol.as_deref().or(row.title.as_deref()))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    let mut parts = vec![format!(
        "{question} is implemented through {}.",
        join_human_list(components.as_slice())
    )];

    let intent_sentences = top
        .iter()
        .filter_map(|row| row.sir_intent.as_deref())
        .map(common::first_sentence)
        .filter(|value| !value.trim().is_empty())
        .take(2)
        .collect::<Vec<_>>();

    for sentence in intent_sentences {
        parts.push(ensure_sentence(sentence));
    }

    let key_dependency = top
        .iter()
        .find_map(|row| row.sir_dependencies.as_ref())
        .and_then(|deps| deps.first())
        .cloned();

    if let Some(dep) = key_dependency {
        parts.push(format!("{dep} manages shared state used by this flow."));
    }

    parts.join(" ")
}

fn ensure_sentence(mut value: String) -> String {
    let trimmed = value.trim().to_owned();
    if trimmed.is_empty() {
        return String::new();
    }

    value = trimmed;
    if !value.ends_with('.') && !value.ends_with('!') && !value.ends_with('?') {
        value.push('.');
    }
    value
}

fn ask_kind_name(kind: AskResultKind) -> &'static str {
    match kind {
        AskResultKind::Symbol => "symbol",
        AskResultKind::Note => "note",
        AskResultKind::TestGuard => "test_guard",
        AskResultKind::CoupledFile => "coupled_file",
    }
}

fn as_string_vec(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn join_human_list(items: &[String]) -> String {
    let values = items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    match values.len() {
        0 => String::new(),
        1 => values[0].clone(),
        2 => format!("{} and {}", values[0], values[1]),
        _ => {
            let head = values[..values.len() - 1].join(", ");
            format!("{head}, and {}", values[values.len() - 1])
        }
    }
}
