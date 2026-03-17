use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn coupling_chord_fragment(
    State(_state): State<Arc<DashboardState>>,
) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Coupling Chord Diagram",
                "This chart shows which parts of the code change together, displayed as connecting ribbons.",
                "Chord diagram of module-to-module coupling. Thicker ribbons mean stronger coupling. Colors indicate coupling type.",
                "d3.chord() with multi-signal ribbon coloring (structural=blue, semantic=green, temporal=orange). Threshold slider filters weak chords.",
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "How Modules Connect" }
                    span class="intermediate-only" { "Coupling Chord Diagram" }
                    span class="expert-only" { "Coupling Chord Diagram" }
                }
                span class="badge badge-orange" { "Module coupling" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div class="flex flex-wrap items-center gap-3 text-sm" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Min coupling" }
                        div class="flex items-center gap-2" {
                            input id="coupling-chord-threshold" type="range" min="0" max="1" step="0.05" value="0.3"
                                class="w-32 accent-cyan-500" {}
                            span id="coupling-chord-threshold-val" class="text-xs text-text-secondary font-mono" { "0.30" }
                        }
                    }
                }
            }

            div class="grid grid-cols-1 lg:grid-cols-3 gap-4" {
                div class="lg:col-span-2" {
                    div id="coupling-chord-chart" class="chart-container min-h-[560px]" {}
                }
                div id="coupling-chord-detail" class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 text-sm text-text-secondary" {
                    "Hover a module to highlight its connections. Click a chord for signal breakdown."
                }
            }
        }
    })
}
