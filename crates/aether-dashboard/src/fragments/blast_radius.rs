use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct BlastRadiusFragmentQuery {
    pub symbol_id: Option<String>,
    pub depth: Option<u32>,
    pub min_coupling: Option<f64>,
}

pub(crate) async fn blast_radius_fragment(
    State(_state): State<Arc<DashboardState>>,
    Query(query): Query<BlastRadiusFragmentQuery>,
) -> Html<String> {
    let symbol_id = query.symbol_id.unwrap_or_default();
    let depth = query.depth.unwrap_or(3).clamp(1, 5);
    let min_coupling = query.min_coupling.unwrap_or(0.2).clamp(0.0, 1.0);

    support::html_markup_response(html! {
        div class="space-y-4" data-page="blast-radius" data-symbol-id=(symbol_id.clone()) {
            (support::explanation_header(
                "Impact Radius",
                "See which components are most likely to be affected when this component changes.",
                "Rings represent dependency distance and estimated impact.",
                "Blast radius traversal across dependency and coupling signals."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Impact Radius Explorer" }
                    span class="intermediate-only" { "Blast Radius Explorer" }
                    span class="expert-only" { "Blast Radius Explorer" }
                }
                span class="badge badge-purple" { "Symbol impact rings" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div id="blast-radius-search" class="space-y-1" data-target-input="blast-symbol-id" {
                    label class="text-xs uppercase tracking-wider text-text-muted" { "Symbol" }
                    input
                        id="blast-symbol-input"
                        type="text"
                        value=(symbol_id)
                        placeholder="Search symbol..."
                        class="w-full px-3 py-2 text-xs bg-surface-0/60 border border-surface-3/50 rounded-md text-text-primary placeholder-text-muted";
                    input id="blast-symbol-id" type="hidden" value=(symbol_id);
                    div id="blast-symbol-results" class="hidden" {}
                }

                div class="grid gap-3 md:grid-cols-2" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Depth" }
                        input id="blast-depth" type="range" min="1" max="5" step="1" value=(depth) class="w-full";
                        div class="text-xs text-text-secondary font-mono" { (depth) " hops" }
                    }
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Min Coupling" }
                        input id="blast-min-coupling" type="range" min="0" max="1" step="0.05" value=(format!("{:.2}", min_coupling)) class="w-full";
                        div class="text-xs text-text-secondary font-mono" { (format!("{:.2}", min_coupling)) }
                    }
                }
            }

            div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_340px]" {
                div id="blast-radius-chart" class="chart-container min-h-[560px]" {}
                div id="blast-radius-detail" class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 text-xs text-text-secondary" {
                    "Shift+click a node to inspect symbol details here."
                }
            }
        }
    })
}
