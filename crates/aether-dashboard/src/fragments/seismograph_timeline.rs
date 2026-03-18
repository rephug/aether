use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn seismograph_timeline_fragment(
    State(_state): State<Arc<DashboardState>>,
) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Seismograph Timeline",
                "This chart shows how fast code meaning is changing over time. Spikes mean lots of symbols changed at once.",
                "Track semantic velocity and codebase shift per batch. Red markers indicate cascade events where changes propagated through the call graph.",
                "Time series of Δ_sem velocity and codebase shift with cascade epicenter markers. Click markers for chain detail.",
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "How Fast Is Code Changing?" }
                    span class="intermediate-only" { "Seismograph Timeline" }
                    span class="expert-only" { "Seismograph Timeline" }
                }
                span class="badge badge-orange" { "Semantic velocity" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div class="flex flex-wrap items-center gap-3 text-sm" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Data points" }
                        select id="seismograph-timeline-limit" class="form-select text-xs rounded border border-surface-3 bg-surface-1 px-2 py-1" {
                            option value="20" { "20" }
                            option value="50" selected { "50" }
                            option value="100" { "100" }
                        }
                    }
                }
            }

            div id="seismograph-timeline-chart" class="chart-container min-h-[480px]" {}
        }
    })
}
