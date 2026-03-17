use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn drift_timeline_fragment(
    State(_state): State<Arc<DashboardState>>,
) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Drift Timeline",
                "This chart shows how code meaning changes over time across different modules.",
                "Track semantic drift trends per module. Brush the timeline to zoom into specific periods.",
                "Multi-line time series of drift magnitude per module directory, with d3.brush for temporal selection.",
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "How Meaning Changes Over Time" }
                    span class="intermediate-only" { "Drift Timeline" }
                    span class="expert-only" { "Drift Timeline" }
                }
                span class="badge badge-orange" { "Temporal drift view" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div class="flex flex-wrap items-center gap-3 text-sm" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Top modules" }
                        select id="drift-timeline-top" class="form-select text-xs rounded border border-surface-3 bg-surface-1 px-2 py-1" {
                            option value="5" { "5" }
                            option value="10" selected { "10" }
                            option value="20" { "20" }
                        }
                    }
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Since" }
                        select id="drift-timeline-since" class="form-select text-xs rounded border border-surface-3 bg-surface-1 px-2 py-1" {
                            option value="7d" { "7 days" }
                            option value="30d" selected { "30 days" }
                            option value="90d" { "90 days" }
                            option value="all" { "All time" }
                        }
                    }
                }
            }

            div id="drift-timeline-chart" class="chart-container min-h-[480px]" {}
        }
    })
}
