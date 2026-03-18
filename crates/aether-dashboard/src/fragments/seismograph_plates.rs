use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn seismograph_plates_fragment(
    State(_state): State<Arc<DashboardState>>,
) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Tectonic Plates",
                "This map shows groups of related code. Green means stable, yellow means shifting, red means unstable.",
                "Louvain communities colored by stability score. Larger tiles have more symbols. Breaches indicate symbols crossing the noise threshold.",
                "Community treemap: stability ∈ [0,1] via RdYlGn interpolation, sized by symbol_count, breach_count overlay.",
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Code Community Stability" }
                    span class="intermediate-only" { "Tectonic Plates" }
                    span class="expert-only" { "Tectonic Plates" }
                }
                span class="badge badge-green" { "Community stability" }
            }

            div id="seismograph-plates-chart" class="chart-container min-h-[480px]" {}
        }
    })
}
