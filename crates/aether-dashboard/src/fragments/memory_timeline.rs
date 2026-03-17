use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn memory_timeline_fragment(
    State(_state): State<Arc<DashboardState>>,
) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Project Memory Timeline",
                "A timeline showing important events in your project's history.",
                "Visual narrative of structural changes, semantic drift events, project decisions, and health milestones.",
                "Horizontal d3.scaleTime timeline with d3.zoom. Events categorized: structural (blue), semantic (green), memory (yellow), health (red).",
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Project Event History" }
                    span class="intermediate-only" { "Memory Timeline" }
                    span class="expert-only" { "Memory Timeline" }
                }
                span class="badge badge-purple" { "Event narrative" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div class="flex flex-wrap items-center gap-3 text-sm" {
                    span class="text-xs uppercase tracking-wider text-text-muted" { "Filter" }
                    label class="inline-flex items-center gap-1 cursor-pointer" {
                        input type="checkbox" class="memory-filter accent-blue-500" value="structural" checked {}
                        span class="text-blue-400" { "Structural" }
                    }
                    label class="inline-flex items-center gap-1 cursor-pointer" {
                        input type="checkbox" class="memory-filter accent-green-500" value="semantic" checked {}
                        span class="text-green-400" { "Semantic" }
                    }
                    label class="inline-flex items-center gap-1 cursor-pointer" {
                        input type="checkbox" class="memory-filter accent-yellow-500" value="memory" checked {}
                        span class="text-yellow-400" { "Memory" }
                    }
                    label class="inline-flex items-center gap-1 cursor-pointer" {
                        input type="checkbox" class="memory-filter accent-red-500" value="health" checked {}
                        span class="text-red-400" { "Health" }
                    }
                    label class="space-y-1 ml-3" {
                        span class="text-xs uppercase tracking-wider text-text-muted" { "Since" }
                        select id="memory-timeline-since" class="form-select text-xs rounded border border-surface-3 bg-surface-1 px-2 py-1" {
                            option value="30d" { "30 days" }
                            option value="90d" selected { "90 days" }
                            option value="all" { "All time" }
                        }
                    }
                }
            }

            div id="memory-timeline-chart" class="chart-container min-h-[420px]" {}
            div id="memory-timeline-detail" class="hidden rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 text-sm" {}
        }
    })
}
