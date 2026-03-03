use std::sync::Arc;

use axum::extract::{Form, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::api::ask as ask_api;
use crate::api::common;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct AskForm {
    pub question: Option<String>,
    pub limit: Option<u32>,
}

pub(crate) async fn ask_fragment(
    State(state): State<Arc<DashboardState>>,
    Form(form): Form<AskForm>,
) -> Html<String> {
    let question = form.question.unwrap_or_default();
    let limit = form.limit.unwrap_or(10);

    let shared = state.shared.clone();
    let data = match support::run_async_with_timeout(move || async move {
        ask_api::load_ask_data(shared.as_ref(), question, limit).await
    })
    .await
    {
        Ok(data) => data,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to run Ask AETHER", detail.as_str()))
            });
        }
    };

    if data.answer_type != "search_results" {
        let message = data
            .message
            .unwrap_or_else(|| ask_api::ASK_NEEDS_INDEX_MESSAGE.to_owned());

        return support::html_markup_response(html! {
            div class="rounded-xl border border-amber-300/60 bg-amber-50/70 dark:bg-amber-900/20 p-4 space-y-2" {
                div class="text-sm font-semibold text-amber-900 dark:text-amber-200" { "Ask AETHER" }
                p class="text-sm text-amber-900/90 dark:text-amber-100" { (message) }
            }
        });
    }

    let symbol_results = data
        .results
        .iter()
        .filter(|row| row.kind == "symbol")
        .collect::<Vec<_>>();

    support::html_markup_response(html! {
        div class="space-y-4" {
            div class="rounded-xl border border-blue-300/60 bg-blue-50/70 dark:bg-blue-900/20 p-4" {
                p class="text-sm text-blue-900 dark:text-blue-100" { (data.summary) }
            }

            div class="space-y-2" {
                h3 class="text-sm font-semibold" { "Related Components" }

                @if symbol_results.is_empty() {
                    (support::html_empty_state("No symbol matches were found for this question", None))
                } @else {
                    div class="grid gap-3" {
                        @for row in symbol_results {
                            @let symbol_label = row.symbol.as_deref().or(row.title.as_deref()).unwrap_or("unknown");
                            @let symbol_target = row.symbol.as_deref().or(row.title.as_deref()).unwrap_or("");
                            @let layer_label = row.layer.as_deref().unwrap_or("Core Logic");
                            @let relevance = format!("{:.2}", row.relevance_score);
                            @let intent = row
                                .sir_intent
                                .as_deref()
                                .map(common::first_sentence)
                                .filter(|value| !value.trim().is_empty())
                                .unwrap_or_else(|| row.snippet.clone());

                            article class="rounded-xl border border-surface-3/40 bg-surface-1/50 p-4 space-y-2" {
                                div class="flex flex-wrap items-start justify-between gap-2" {
                                    div class="space-y-1" {
                                        span class="symbol-link text-blue-600 hover:underline cursor-pointer font-semibold"
                                            data-symbol=(symbol_target) {
                                            (symbol_label)
                                        }
                                        @if let Some(file) = &row.file {
                                            div class="file-link text-xs font-mono text-text-muted hover:underline cursor-pointer"
                                                data-path=(file.as_str()) {
                                                (file.as_str())
                                            }
                                        }
                                    }
                                    div class="flex items-center gap-2" {
                                        span class="badge badge-cyan" { (layer_label) }
                                        span class="badge badge-muted" { "Rel " (relevance) }
                                    }
                                }
                                p class="text-xs text-text-secondary" { (intent) }
                            }
                        }
                    }
                }
            }
        }
    })
}
