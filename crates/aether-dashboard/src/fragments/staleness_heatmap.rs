use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn staleness_heatmap_fragment(
    State(_state): State<Arc<DashboardState>>,
) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Staleness Heatmap",
                "A heatmap showing which parts of the code are getting stale over time.",
                "Grid visualization of SIR staleness across modules and time. Red cells indicate areas where code meaning is drifting from reality.",
                "d3.scaleSequential(d3.interpolateRdYlGn) reversed. Y-axis: modules sorted by worst staleness. X-axis: daily time windows.",
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Where Code Gets Stale" }
                    span class="intermediate-only" { "Staleness Heatmap" }
                    span class="expert-only" { "Staleness Heatmap" }
                }
                span class="badge badge-red" { "Freshness view" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div class="flex flex-wrap items-center gap-3 text-sm" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Since" }
                        select id="staleness-heatmap-since" class="form-select text-xs rounded border border-surface-3 bg-surface-1 px-2 py-1" {
                            option value="7d" { "7 days" }
                            option value="30d" selected { "30 days" }
                            option value="90d" { "90 days" }
                        }
                    }
                    label class="inline-flex items-center gap-1 ml-3 cursor-pointer" {
                        input id="staleness-heatmap-stale-only" type="checkbox" class="accent-cyan-500" {}
                        span class="text-text-secondary" { "Stale only (>0.3)" }
                    }
                }
            }

            div id="staleness-heatmap-chart" class="chart-container min-h-[480px]" {}
        }
    })
}
