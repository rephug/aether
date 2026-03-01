use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn causal_fragment(State(_state): State<Arc<DashboardState>>) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" data-page="causal" {
            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" { "Causal Explorer" }
                button id="causal-animate" class="px-2 py-1 rounded-md border border-surface-3/50 hover:bg-surface-3/40 text-xs" { "Animate" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div id="causal-symbol-search" data-target-input="causal-symbol-id" {
                    input id="causal-symbol-input" type="text" placeholder="Search target symbol..." class="w-full px-3 py-2 text-xs bg-surface-0/60 border border-surface-3/50 rounded-md text-text-primary placeholder-text-muted";
                    input id="causal-symbol-id" type="hidden";
                    div id="causal-symbol-results" class="hidden" {}
                }

                div class="grid gap-3 md:grid-cols-2" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Depth" }
                        input id="causal-depth" type="range" min="1" max="5" value="3" class="w-full";
                    }
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Lookback" }
                        select id="causal-lookback" class="w-full px-2 py-2 text-xs bg-surface-0/60 border border-surface-3/50 rounded-md" {
                            option value="7d" { "7d" }
                            option value="30d" selected { "30d" }
                            option value="90d" { "90d" }
                        }
                    }
                }
            }

            div id="causal-graph" class="chart-container min-h-[560px]" {}
        }
    })
}
