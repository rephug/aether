use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;

use crate::api::health::{self, HealthQuery};
use crate::support::{self, DashboardState};

pub(crate) async fn health_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<HealthQuery>,
) -> Html<String> {
    let data = health::load_health_data(state.shared.as_ref(), &query).await;

    support::html_markup_response(html! {
        div class="space-y-4" {
            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" { "Health" }
                @if let Some(score) = data.overall_score {
                    span class="badge badge-green" { "Overall " (format!("{:.0}%", score * 100.0)) }
                } @else {
                    span class="badge badge-muted" { "No overall score" }
                }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                div class="flex flex-wrap gap-2 text-xs" {
                    span class="badge badge-cyan" {
                        "SIR " (format!("{:.0}%", data.dimensions.sir_coverage * 100.0))
                    }
                    span class="badge badge-green" {
                        "Tests " (format!("{:.0}%", data.dimensions.test_coverage * 100.0))
                    }
                    span class="badge badge-yellow" {
                        "Coupling " (format!("{:.0}%", data.dimensions.coupling_health * 100.0))
                    }
                    span class="badge badge-orange" {
                        "Drift " (format!("{:.0}%", data.dimensions.drift_health * 100.0))
                    }
                    @if !data.analysis_available {
                        span class="badge badge-red" { "Analysis unavailable" }
                    }
                }
                @if !data.notes.is_empty() {
                    ul class="mt-3 space-y-1 text-xs text-text-secondary" {
                        @for note in data.notes.iter().take(6) {
                            li { "- " (note) }
                        }
                    }
                }
            }

            div id="health-chart" class="chart-container min-h-[260px]" {}

            div class="grid gap-4 xl:grid-cols-2" {
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    h3 class="text-sm font-semibold mb-3" { "Risk Hotspots" }
                    @if data.hotspots.is_empty() {
                        (support::html_empty_state("No hotspots found", None))
                    } @else {
                        table class="data-table" {
                            thead {
                                tr {
                                    th { "Symbol" }
                                    th { "Risk" }
                                    th { "PageRank" }
                                    th { "Top Factor" }
                                }
                            }
                            tbody {
                                @for hotspot in data.hotspots.iter().take(20) {
                                    tr class="clickable"
                                        hx-get={"/dashboard/frag/symbol/" (hotspot.symbol_id)}
                                        hx-target="#detail-panel" {
                                        td {
                                            div class="font-medium" { (hotspot.symbol_name.clone()) }
                                            div class="text-xs text-text-muted font-mono" { (hotspot.symbol_id.clone()) }
                                        }
                                        td class="font-mono" { (format!("{:.2}", hotspot.risk_score)) }
                                        td class="font-mono" { (format!("{:.2}", hotspot.pagerank)) }
                                        td class="text-xs" { (hotspot.top_factor_description.clone()) }
                                    }
                                }
                            }
                        }
                    }
                }

                div class="space-y-4" {
                    div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                        h3 class="text-sm font-semibold mb-2" { "Cycles" }
                        @if data.cycles.is_empty() {
                            (support::html_empty_state("No cycles detected", None))
                        } @else {
                            ul class="space-y-2 text-xs" {
                                @for cycle in data.cycles.iter().take(12) {
                                    li class="rounded-md border border-surface-3/30 p-2" {
                                        div class="font-medium" { "Cycle #" (cycle.cycle_id) " (" (cycle.edge_count) " edges)" }
                                        div class="text-text-secondary" { (cycle.symbols.join(" -> ")) }
                                    }
                                }
                            }
                        }
                    }

                    div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                        h3 class="text-sm font-semibold mb-2" { "Orphans" }
                        @if data.orphans.is_empty() {
                            (support::html_empty_state("No orphan subgraphs detected", None))
                        } @else {
                            ul class="space-y-2 text-xs" {
                                @for orphan in data.orphans.iter().take(12) {
                                    li class="rounded-md border border-surface-3/30 p-2" {
                                        div class="font-medium" { "Subgraph #" (orphan.subgraph_id) }
                                        div class="text-text-secondary" { (orphan.symbols.join(", ")) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}
