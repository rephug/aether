use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GraphFragmentQuery {
    pub root: Option<String>,
}

pub(crate) async fn graph_fragment(
    State(_state): State<Arc<DashboardState>>,
    Query(query): Query<GraphFragmentQuery>,
) -> Html<String> {
    let root_value = query.root.unwrap_or_default();

    support::html_markup_response(html! {
        div class="space-y-4" {
            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" { "Dependency Graph" }
                span class="badge badge-muted" { "Depth: 2" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div class="grid gap-3 md:grid-cols-[minmax(0,1fr)_auto]" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Root Symbol" }
                        input
                            id="graph-root"
                            name="root"
                            type="text"
                            value=(root_value)
                            placeholder="sym-123 or qualified name"
                            class="w-full px-3 py-2 text-xs bg-surface-0/60 border border-surface-3/50 rounded-md text-text-primary placeholder-text-muted focus:outline-none focus:border-accent-cyan/50 focus:ring-1 focus:ring-accent-cyan/20";
                    }
                    button
                        type="button"
                        onclick="initGraph()"
                        class="self-end px-3 py-2 rounded-md text-xs border border-surface-3/50 hover:bg-surface-3/40" {
                        "Load Graph"
                    }
                }
                p class="text-xs text-text-muted" {
                    "Click nodes in the graph to open symbol details in the right-side panel."
                }
            }

            div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_320px]" {
                div id="graph-container" class="chart-container min-h-[520px]" {}
                div id="graph-selection-panel" class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                    h3 class="text-sm font-semibold" { "Selection" }
                    p class="text-xs text-text-muted" {
                        "Node details appear in the side panel when you click a graph node."
                    }
                    ul class="text-xs text-text-secondary space-y-1" {
                        li { "- circle color groups nodes by file" }
                        li { "- cyan outline means SIR exists" }
                        li { "- zoom/pan available with mouse wheel and drag" }
                    }
                }
            }
        }
    })
}
