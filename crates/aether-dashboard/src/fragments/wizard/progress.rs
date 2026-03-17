use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

/// First-run progress page — shown after wizard restart while indexing runs.
/// This fragment runs on the MAIN dashboard router (has `DashboardState`).
pub(crate) async fn first_run_progress_fragment(
    State(state): State<Arc<DashboardState>>,
) -> Html<String> {
    let store = &state.shared.store;

    let (total_symbols, with_sir) = store.count_symbols_with_sir().unwrap_or((0, 0));

    let percent = if total_symbols > 0 {
        ((with_sir as f64 / total_symbols as f64) * 100.0).min(100.0) as u32
    } else {
        0
    };

    let is_complete = total_symbols > 0 && with_sir >= total_symbols;

    support::html_markup_response(html! {
        @if is_complete {
            div class="max-w-xl mx-auto mt-16 space-y-6" {
                (progress_content(total_symbols, with_sir, percent, true))
            }
        } @else {
            div
                class="max-w-xl mx-auto mt-16 space-y-6"
                hx-get="/dashboard/frag/first-run-progress"
                hx-trigger="every 3s"
                hx-target="this"
                hx-swap="outerHTML"
            {
                (progress_content(total_symbols, with_sir, percent, false))
            }
        }
    })
}

fn progress_content(
    total_symbols: usize,
    with_sir: usize,
    percent: u32,
    is_complete: bool,
) -> maud::Markup {
    html! {
        div class="text-center space-y-2" {
            h2 class="text-lg font-semibold text-text-primary dark:text-slate-100" {
                @if is_complete { "Indexing Complete!" }
                @else { "Initial Indexing in Progress" }
            }
            p class="text-sm text-text-muted dark:text-slate-400" {
                @if is_complete {
                    "AETHER has analyzed your codebase and is ready to use."
                } @else {
                    "AETHER is analyzing your codebase. This may take a few minutes."
                }
            }
        }

        // Progress bar
        div class="bg-surface-2 dark:bg-slate-700 rounded-full h-3 overflow-hidden" {
            div
                class="bg-accent-cyan h-full rounded-full transition-all duration-500"
                style={ "width: " (percent) "%" }
            {}
        }

        // Stats
        div class="flex justify-between text-xs text-text-muted dark:text-slate-400" {
            span { "Symbols: " (total_symbols) }
            span { "SIRs: " (with_sir) }
            span { (percent) "%" }
        }

        // Action button
        div class="text-center pt-4" {
            @if is_complete {
                a
                    class="px-6 py-2.5 bg-accent-cyan text-white rounded-lg hover:bg-accent-cyan/90 transition-colors font-medium inline-block cursor-pointer"
                    hx-get="/dashboard/frag/anatomy"
                    hx-target="#main-content"
                    hx-push-url="/dashboard/anatomy"
                {
                    "Show Dashboard \u{2192}"
                }
            } @else {
                span class="text-sm text-text-muted dark:text-slate-400" {
                    "You can start exploring while indexing continues."
                }
                div class="mt-2" {
                    a
                        class="text-sm text-accent-cyan hover:underline cursor-pointer"
                        hx-get="/dashboard/frag/anatomy"
                        hx-target="#main-content"
                        hx-push-url="/dashboard/anatomy"
                    {
                        "Go to Dashboard"
                    }
                }
            }
        }
    }
}
