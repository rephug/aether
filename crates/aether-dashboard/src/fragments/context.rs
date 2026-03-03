use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Html;
use maud::html;

use crate::api::context;
use crate::support::{self, DashboardState};

pub(crate) async fn context_fragment(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Html<String> {
    let shared = state.shared.clone();
    let selector_for_build = selector.clone();
    let data = match support::run_blocking_with_timeout(move || {
        context::build_context_data(shared.as_ref(), selector_for_build.as_str())
    })
    .await
    {
        Ok(Some(data)) => data,
        Ok(None) => {
            return support::html_markup_response(html! {
                (support::html_empty_state("Symbol not found", None))
            });
        }
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to build context advisor", detail.as_str()))
            });
        }
    };

    let required_copy = data
        .required
        .iter()
        .map(|item| item.file.clone())
        .collect::<Vec<_>>()
        .join("\n");

    support::html_markup_response(html! {
        div class="space-y-4" data-page="context-advisor" {
            (support::explanation_header(
                "Context Window Advisor",
                "This identifies the minimum context an LLM needs for correct implementation work.",
                "Use required context first, then add optional context only if needed.",
                "Dependency-derived context minimization with line-estimate budgeting."
            ))

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                h1 class="text-xl font-semibold" { "What an LLM Needs to Build " (data.symbol.as_str()) }
                p class="text-sm text-text-secondary font-mono" {
                    "Required: " (data.total_required_lines) " lines | Optional: " (data.total_with_optional_lines.saturating_sub(data.total_required_lines)) " lines | Full project: " (data.full_codebase_lines) " lines | " (data.context_reduction.as_str())
                }

                button
                    type="button"
                    class="px-3 py-2 rounded-md border border-surface-3/40 hover:bg-surface-3/20 text-sm"
                    data-copy-text=(required_copy)
                    onclick="aetherCopyText(this)" {
                    "📋 Copy All Required Context"
                }
            }

            section class="space-y-3" {
                h2 class="text-base font-semibold" { "Required" }
                @if data.required.is_empty() {
                    (support::html_empty_state("No required dependency context found", None))
                } @else {
                    @for item in &data.required {
                        (context_card(item, "border-l-green-600"))
                    }
                }
            }

            section class="space-y-3" {
                h2 class="text-base font-semibold" { "Helpful but Optional" }
                @if data.helpful_but_optional.is_empty() {
                    (support::html_empty_state("No optional context identified", None))
                } @else {
                    @for item in &data.helpful_but_optional {
                        (context_card(item, "border-l-yellow-600"))
                    }
                }
            }

            section class="space-y-3" {
                details class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    summary class="cursor-pointer text-sm font-semibold" { "Not Needed (expand)" }
                    div class="mt-3 space-y-2" {
                        @for group in &data.not_needed {
                            div class="rounded-lg border-l-4 border-l-red-600 border border-surface-3/30 p-3" {
                                p class="text-sm text-text-secondary" { (group.reason.as_str()) }
                                ul class="list-disc pl-5 text-xs text-text-muted space-y-1" {
                                    @for file in &group.files {
                                        li { (file.as_str()) }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 teaching-note" {
                h3 class="text-sm font-semibold mb-1" { "Teaching Note" }
                p class="text-sm text-text-secondary" { (data.teaching_note.as_str()) }
            }
        }
    })
}

fn context_card(item: &crate::api::context::ContextFileItem, border_class: &str) -> maud::Markup {
    html! {
        article class={ "rounded-lg border-l-4 border border-surface-3/30 p-3 space-y-1 " (border_class) } {
            div class="flex flex-wrap items-center justify-between gap-2" {
                span class="file-link text-blue-600 hover:underline cursor-pointer font-mono text-xs" data-path=(item.file.as_str()) {
                    (item.file.as_str())
                }
                span class="badge badge-muted" { (item.estimated_lines) " lines" }
            }
            p class="text-sm text-text-secondary" { (item.reason.as_str()) }
            @if !item.symbols.is_empty() {
                div class="text-xs text-text-muted" {
                    "Symbols: " (item.symbols.join(", "))
                }
            }
        }
    }
}
