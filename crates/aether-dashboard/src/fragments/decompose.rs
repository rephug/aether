use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Html;
use maud::html;

use crate::api::decompose;
use crate::support::{self, DashboardState};

pub(crate) async fn decompose_fragment(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Html<String> {
    let shared = state.shared.clone();
    let selector_for_build = selector.clone();
    let data = match support::run_blocking_with_timeout(move || {
        decompose::build_decompose_data(shared.as_ref(), selector_for_build.as_str())
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
                (support::html_error_state("Failed to decompose prompt", detail.as_str()))
            });
        }
    };

    support::html_markup_response(html! {
        div class="space-y-4" data-page="decompose" {
            (support::explanation_header(
                "Prompt Decomposer",
                "Break complex components into dependency-ordered prompts.",
                "Each step narrows scope and adds checkpoints before the next prompt.",
                "Dependency-first decomposition and verification sequencing for agent workflows."
            ))

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-2" {
                h1 class="text-xl font-semibold" { "Build " (data.target.as_str()) " in " (data.step_count) " Steps" }
                div class="flex flex-wrap items-center gap-2 text-xs" {
                    span class={ "badge " (difficulty_badge_class(data.difficulty.label.as_str())) } {
                        (data.difficulty.emoji.as_str()) " " (data.difficulty.label.as_str())
                    }
                    span class="file-link text-blue-600 hover:underline cursor-pointer font-mono" data-path=(data.target_file.as_str()) {
                        (data.target_file.as_str())
                    }
                }
                p class="text-sm text-text-secondary" { (data.preamble.as_str()) }
            }

            div class="space-y-3" {
                @for step in &data.steps {
                    (render_step_card(step))
                }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 teaching-note" {
                h2 class="text-base font-semibold" { "Teaching Summary" }
                p class="text-sm text-text-secondary" { (data.teaching_summary.as_str()) }
            }
        }
    })
}

fn render_step_card(step: &crate::api::decompose::DecomposeStep) -> maud::Markup {
    let prompt_copy = step.prompt.clone();

    html! {
        article class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
            div class="flex flex-wrap items-start gap-3" {
                div class="inline-flex h-9 w-9 items-center justify-center rounded-full bg-accent-cyan/15 text-sm font-semibold text-accent-cyan" {
                    (step.number)
                }
                div class="space-y-1" {
                    h2 class="text-base font-semibold" { (step.title.as_str()) }
                    p class="text-sm text-text-secondary" { (step.subtitle.as_str()) }
                    div class="text-xs text-text-muted" { "Target: " (step.symbol_target.as_str()) " | " (step.difficulty.as_str()) }
                }
            }

            div class="rounded-lg border border-surface-3/30 bg-surface-0/70 p-3 space-y-2" {
                div class="flex items-center justify-between gap-2" {
                    span class="text-xs font-semibold uppercase tracking-wide text-text-muted" { "Prompt" }
                    button
                        type="button"
                        class="px-2 py-1 rounded-md border border-surface-3/40 hover:bg-surface-3/20 text-xs"
                        data-copy-text=(prompt_copy)
                        onclick="aetherCopyText(this)" {
                        "📋 Copy This Prompt"
                    }
                }
                pre class="text-sm text-text-secondary whitespace-pre-wrap" { (step.prompt.as_str()) }
            }

            p class="text-sm italic text-text-secondary" { (step.why_this_order.as_str()) }

            @if !step.context_needed.is_empty() {
                div class="text-sm text-text-secondary" {
                    span class="font-semibold text-text-primary" { "Context needed: " }
                    (step.context_needed.join(", "))
                }
            }

            p class="text-sm text-text-secondary" {
                span class="font-semibold text-text-primary" { "Expected output: " }
                (step.expected_output.as_str())
            }

            @if !step.checkpoints.is_empty() {
                details class="rounded-lg border border-surface-3/30 p-3" {
                    summary class="cursor-pointer text-sm font-semibold" { "✅ Verify Before Moving On" }
                    ul class="mt-2 space-y-2" {
                        @for checkpoint in &step.checkpoints {
                            li class={ "rounded-md border-l-4 p-2 text-sm " (checkpoint_class(checkpoint.severity.as_str())) } {
                                label class="inline-flex items-start gap-2" {
                                    input type="checkbox";
                                    span {
                                        span class="font-semibold" { (checkpoint.check.as_str()) }
                                        br;
                                        span class="text-xs text-text-secondary" { (checkpoint.why.as_str()) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn checkpoint_class(severity: &str) -> &'static str {
    if severity.eq_ignore_ascii_case("critical") {
        "border-l-red-600 bg-red-50/30"
    } else if severity.eq_ignore_ascii_case("warning") {
        "border-l-yellow-600 bg-yellow-50/30"
    } else {
        "border-l-slate-500 bg-slate-50/30"
    }
}

fn difficulty_badge_class(label: &str) -> &'static str {
    if label.eq_ignore_ascii_case("easy") {
        "badge-green"
    } else if label.eq_ignore_ascii_case("moderate") {
        "badge-yellow"
    } else {
        "badge-red"
    }
}
