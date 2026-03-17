use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn health_scorecard_fragment(
    State(_state): State<Arc<DashboardState>>,
) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" hx-get="/dashboard/frag/health-scorecard" hx-trigger="every 30s" hx-target="this" hx-swap="outerHTML" {
            (support::explanation_header(
                "Health Scorecard",
                "At-a-glance project health metrics with trend indicators.",
                "Composite health scorecard with sparklines for each dimension. Green means healthy, yellow needs attention, red is critical.",
                "11 metric cards with d3.area sparklines, auto-refreshing every 30s. Thresholds from project health configuration.",
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Project Health Overview" }
                    span class="intermediate-only" { "Health Scorecard" }
                    span class="expert-only" { "Health Scorecard" }
                }
                span class="badge badge-green" { "Auto-refresh 30s" }
            }

            div id="health-scorecard-grid" class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-3" {}
        }
    })
}
