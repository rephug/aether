use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ArchitectureFragmentQuery {
    pub granularity: Option<String>,
}

pub(crate) async fn architecture_fragment(
    State(_state): State<Arc<DashboardState>>,
    Query(query): Query<ArchitectureFragmentQuery>,
) -> Html<String> {
    let granularity = query
        .granularity
        .unwrap_or_else(|| "symbol".to_owned())
        .to_ascii_lowercase();

    support::html_markup_response(html! {
        div class="space-y-4" data-page="architecture" data-granularity=(granularity.clone()) {
            (support::explanation_header(
                "Architecture Neighborhoods",
                "AETHER groups related components into communities so you can spot boundaries and misplaced code quickly.",
                "Communities represent clusters of strongly connected components.",
                "Community detection view for structural cohesion and boundary drift."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Architecture Neighborhoods" }
                    span class="intermediate-only" { "Architecture Map" }
                    span class="expert-only" { "Architecture Map" }
                }
                div class="flex items-center gap-2" {
                    button
                        class={"px-2 py-1 text-xs rounded-md border border-surface-3/50 hover:bg-surface-3/40 " (if granularity == "symbol" { "bg-surface-3/40" } else { "" })}
                        hx-get="/dashboard/frag/architecture?granularity=symbol"
                        hx-target="#main-content" {
                        "Logical view"
                    }
                    button
                        class={"px-2 py-1 text-xs rounded-md border border-surface-3/50 hover:bg-surface-3/40 " (if granularity != "symbol" { "bg-surface-3/40" } else { "" })}
                        hx-get="/dashboard/frag/architecture?granularity=file"
                        hx-target="#main-content" {
                        "Directory view"
                    }
                }
            }

            div class="flex flex-wrap gap-2 text-xs" {
                span id="architecture-community-count" class="badge badge-muted" { "Neighborhoods: —" }
                span id="architecture-misplaced-count" class="badge badge-orange" { "Misplaced Components: —" }
                label class="inline-flex items-center gap-1 rounded-md border border-surface-3/40 px-2 py-1" {
                    input id="architecture-show-misplaced" type="checkbox";
                    "Show misplaced only"
                }
            }

            div id="architecture-treemap" class="chart-container min-h-[620px]" {}
        }
    })
}
