use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn time_machine_fragment(
    State(_state): State<Arc<DashboardState>>,
) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" data-page="time-machine" {
            (support::explanation_header(
                "Project Timeline Snapshot",
                "Move through time to see what the code graph looked like around earlier analysis events.",
                "Use the slider to inspect prior graph state and nearby events.",
                "Temporal graph snapshot with drift and history overlays."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Project Timeline Snapshot" }
                    span class="intermediate-only" { "Time Machine" }
                    span class="expert-only" { "Time Machine" }
                }
                div class="flex items-center gap-2 text-xs" {
                    button id="time-machine-play" class="px-2 py-1 rounded-md border border-surface-3/50 hover:bg-surface-3/40" { "Play" }
                    select id="time-machine-speed" class="px-2 py-1 rounded-md bg-surface-0/60 border border-surface-3/50" {
                        option value="1" { "1x" }
                        option value="2" { "2x" }
                        option value="5" { "5x" }
                    }
                }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                input id="time-machine-at" type="range" min="0" max="100" value="100" class="w-full";
                div class="flex flex-wrap gap-2 text-xs" {
                    label class="inline-flex items-center gap-1" { input id="layer-deps" type="checkbox" checked; "Dependencies" }
                    label class="inline-flex items-center gap-1" { input id="layer-drift" type="checkbox" checked; "Drift" }
                    label class="inline-flex items-center gap-1" { input id="layer-communities" type="checkbox" checked; "Communities" }
                }
            }

            div id="time-machine-graph" class="chart-container min-h-[520px]" {}
            div id="time-machine-events" class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 max-h-[200px] overflow-y-auto text-xs" {}
        }
    })
}
