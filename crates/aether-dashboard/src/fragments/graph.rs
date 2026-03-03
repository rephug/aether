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
            (support::explanation_header(
                "How This Project's Pieces Connect",
                "Each dot is a component. Lines mean one depends on another. Bigger dots are more important.",
                "Nodes are components and edges are dependencies. Larger nodes indicate higher importance.",
                "Dependency graph view with centrality-informed node sizing."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Connected Components Graph" }
                    span class="intermediate-only" { "Project Dependency Graph" }
                    span class="expert-only" { "Dependency Graph" }
                }
                span class="badge badge-muted" {
                    "Depth: 2 "
                    (support::help_icon("Depth controls how many dependency hops are loaded from the selected root component."))
                }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div class="grid gap-3 md:grid-cols-[minmax(0,1fr)_auto]" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" {
                            span class="beginner-only" { "Root Component" }
                            span class="intermediate-only" { "Root Component (Symbol)" }
                            span class="expert-only" { "Root Symbol" }
                            " "
                            (support::help_icon("Start the graph from this component ID or qualified symbol name."))
                        }
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
                    "Click any node to open its full symbol deep dive in the main panel."
                }
            }

            div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_320px]" {
                div id="graph-container" class="chart-container min-h-[520px]" {}
                div id="graph-selection-panel" class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                    h3 class="text-sm font-semibold" { "Selection" }
                    p class="text-xs text-text-muted" {
                        "Graph node clicks load symbol deep dives directly in the main content area."
                    }
                    ul class="text-xs text-text-secondary space-y-1" {
                        li { "- circle color groups components by file" }
                        li { "- cyan outline means understanding data exists" }
                        li { "- zoom/pan available with mouse wheel and drag" }
                    }
                }
            }
        }
    })
}
