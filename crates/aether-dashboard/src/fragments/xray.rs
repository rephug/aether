use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct XrayFragmentQuery {
    pub window: Option<String>,
}

pub(crate) async fn xray_fragment(
    State(_state): State<Arc<DashboardState>>,
    Query(query): Query<XrayFragmentQuery>,
) -> Html<String> {
    let window = query.window.unwrap_or_else(|| "7d".to_owned());

    support::html_markup_response(html! {
        div class="space-y-5" data-page="xray" data-window=(window.clone()) {
            div class="flex flex-wrap items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" { "Codebase X-Ray" }
                div class="flex items-center gap-2 text-xs" {
                    @for option in ["7d", "30d", "90d", "all"] {
                        button
                            class={"px-2 py-1 rounded-md border border-surface-3/50 hover:bg-surface-3/40 " (if window == option { "bg-surface-3/40" } else { "" })}
                            hx-get={"/dashboard/frag/xray?window=" (option)}
                            hx-target="#main-content" {
                            (option.to_ascii_uppercase())
                        }
                    }
                }
            }

            div id="xray-metrics-grid" class="grid gap-3 md:grid-cols-2 xl:grid-cols-4" {
                @for metric in [
                    ("sir_coverage", "SIR Coverage"),
                    ("orphan_count", "Orphan Count"),
                    ("avg_drift", "Avg Drift"),
                    ("graph_connectivity", "Graph Connectivity"),
                    ("high_coupling_pairs", "High Coupling Pairs"),
                    ("sir_coverage_pct", "SIR Coverage %"),
                    ("index_freshness_secs", "Index Freshness"),
                    ("risk_grade", "Risk Grade"),
                ] {
                    div class="stat-card xray-metric-card" id={"metric-card-" (metric.0)} data-metric=(metric.0) {
                        div class="flex items-center justify-between gap-2" {
                            div class="text-xs uppercase tracking-wider text-text-muted" { (metric.1) }
                            span class="text-xs font-mono text-text-secondary" id={"metric-trend-" (metric.0)} { "—" }
                        }
                        div class="stat-value mt-2" id={"metric-value-" (metric.0)} { "—" }
                        div class="mt-3" id={"sparkline-" (metric.0)} style="height:24px;" {}
                    }
                }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div class="flex items-center justify-between gap-3" {
                    h3 class="text-sm font-semibold" { "Risk Hotspots" }
                    span class="badge badge-muted" id="xray-hotspot-count" { "0 symbols" }
                }
                div class="overflow-x-auto" {
                    table class="data-table" {
                        thead {
                            tr {
                                th data-sort="qualified_name" class="cursor-pointer" { "Symbol" }
                                th data-sort="risk_score" class="cursor-pointer" { "Risk" }
                                th data-sort="pagerank" class="cursor-pointer" { "PageRank" }
                                th data-sort="drift_score" class="cursor-pointer" { "Drift" }
                                th { "Tests" }
                                th { "SIR" }
                            }
                        }
                        tbody id="xray-hotspots-body" {}
                    }
                }
            }
        }
    })
}
