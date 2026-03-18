use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn seismograph_gauge_fragment(
    State(_state): State<Arc<DashboardState>>,
) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" hx-get="/dashboard/frag/seismograph-gauge" hx-trigger="every 30s" hx-target="this" hx-swap="outerHTML" hx-on-htmx-after-swap="if(window.initSeismographGauge) window.initSeismographGauge()" {
            (support::explanation_header(
                "Velocity Gauge",
                "A quick glance at how fast code meaning is changing right now. Green is calm, red is busy.",
                "Current semantic velocity with trend direction. Auto-refreshes every 30 seconds.",
                "Single-metric gauge: semantic_velocity with Δ trend (accelerating/stable/decelerating). 30s HTMX polling.",
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Change Speed" }
                    span class="intermediate-only" { "Velocity Gauge" }
                    span class="expert-only" { "Velocity Gauge" }
                }
                span class="badge badge-green" { "Auto-refresh 30s" }
            }

            div id="seismograph-gauge-container" class="flex items-center justify-center min-h-[280px]" {}
        }
    })
}
