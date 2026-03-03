use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Html;
use maud::html;

use crate::api::autopsy;
use crate::support::{self, DashboardState};

pub(crate) async fn autopsy_fragment(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Html<String> {
    let shared = state.shared.clone();
    let selector_for_build = selector.clone();
    let data = match support::run_blocking_with_timeout(move || {
        autopsy::build_autopsy_data(shared.as_ref(), selector_for_build.as_str())
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
                (support::html_error_state("Failed to build prompt autopsy", detail.as_str()))
            });
        }
    };

    support::html_markup_response(html! {
        div class="space-y-4" data-page="autopsy" {
            (support::explanation_header(
                "Prompt Autopsy",
                "Compare strong, partial, and vague prompts to understand why agent output quality changes.",
                "Use this to calibrate prompt specificity based on component complexity.",
                "Template-composed prompt quality analysis from SIR and dependency context."
            ))

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-2" {
                h1 class="text-xl font-semibold" { "Prompt Autopsy: " (data.symbol.as_str()) }
                div class="flex flex-wrap items-center gap-2 text-xs" {
                    span class={ "badge " (difficulty_badge_class(data.difficulty.label.as_str())) } {
                        (data.difficulty.emoji.as_str()) " " (data.difficulty.label.as_str())
                    }
                    span class="badge badge-cyan" { (data.pattern_name.as_str()) }
                    span class="file-link text-blue-600 hover:underline cursor-pointer font-mono" data-path=(data.file.as_str()) { (data.file.as_str()) }
                }
            }

            div class="grid gap-3 xl:grid-cols-3" {
                @for prompt in &data.prompts {
                    (prompt_card(prompt))
                }
            }

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 teaching-note" {
                h2 class="text-base font-semibold" { "Teaching Summary" }
                p class="text-sm text-text-secondary" { (data.teaching_summary.as_str()) }
                p class="text-sm text-text-secondary mt-2" {
                    span class="font-semibold text-text-primary" { "Pattern rule: " }
                    (data.pattern_rule.as_str())
                }
            }
        }
    })
}

fn prompt_card(prompt: &crate::api::autopsy::AutopsyPrompt) -> maud::Markup {
    let border = if prompt.level == "good" {
        "border-green-600"
    } else if prompt.level == "partial" {
        "border-yellow-600"
    } else {
        "border-red-600"
    };

    html! {
        article class={ "rounded-xl border-2 p-4 space-y-3 bg-surface-1/40 " (border) } {
            h2 class="text-base font-semibold" { (prompt.emoji.as_str()) " " (prompt.label.as_str()) }
            div class="rounded-lg border border-surface-3/30 bg-surface-0/70 p-3 space-y-2" {
                button
                    type="button"
                    class="px-2 py-1 rounded-md border border-surface-3/40 hover:bg-surface-3/20 text-xs"
                    data-copy-text=(prompt.prompt.as_str())
                    onclick="aetherCopyText(this)" {
                    "Copy Prompt"
                }
                pre class="text-sm text-text-secondary whitespace-pre-wrap" { (prompt.prompt.as_str()) }
            }

            @if let Some(why) = prompt.why_it_works.as_deref() {
                p class="text-sm text-text-secondary" {
                    span class="font-semibold text-text-primary" { "Why it works: " }
                    (why)
                }
            }

            @if let Some(wrong) = prompt.what_goes_wrong.as_deref() {
                p class="text-sm text-text-secondary" {
                    span class="font-semibold text-text-primary" { "What goes wrong: " }
                    (wrong)
                }
            }

            @if !prompt.key_elements.is_empty() {
                ul class="list-disc pl-5 text-sm text-text-secondary space-y-1" {
                    @for item in &prompt.key_elements {
                        li { (item.as_str()) }
                    }
                }
            }

            @if !prompt.missing_elements.is_empty() {
                ul class="list-disc pl-5 text-sm text-text-secondary space-y-1" {
                    @for item in &prompt.missing_elements {
                        li { (item.as_str()) }
                    }
                }
            }
        }
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
