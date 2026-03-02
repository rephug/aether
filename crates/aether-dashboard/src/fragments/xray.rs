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
            (support::explanation_header(
                "Codebase X-Ray",
                "This page highlights where complexity and risk are concentrated so you can prioritize what to inspect first.",
                "Use these metrics and hotspots to focus on components with the highest operational risk.",
                "Cross-signal risk summary (pagerank, drift, tests, SIR, coupling)."
            ))

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
                    ("sir_coverage", "Understanding Coverage", "How much of your code has SIR understanding records."),
                    ("orphan_count", "Isolated Components", "Components that are disconnected from the largest project graph."),
                    ("avg_drift", "Average Change Risk", "Mean semantic drift level across analyzed components."),
                    ("graph_connectivity", "Connected Components", "How connected the code graph is. Higher usually means fewer isolated areas."),
                    ("high_coupling_pairs", "Strong Connections", "File pairs with high coupling scores that likely change together."),
                    ("sir_coverage_pct", "Understanding Coverage %", "Coverage ratio from 0 to 1 for SIR understanding."),
                    ("index_freshness_secs", "Analysis Freshness", "How recent the current analysis is."),
                    ("risk_grade", "Overall Risk Grade", "AETHER's rolled-up risk grade based on combined metrics."),
                ] {
                    div class="stat-card xray-metric-card" id={"metric-card-" (metric.0)} data-metric=(metric.0) {
                        div class="flex items-center justify-between gap-2" {
                            div class="text-xs uppercase tracking-wider text-text-muted inline-flex items-center gap-1" {
                                (metric.1)
                                (support::help_icon(metric.2))
                            }
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
                                th data-sort="qualified_name" class="cursor-pointer" { "Component" }
                                th data-sort="risk_score" class="cursor-pointer" { "Risk Score" }
                                th data-sort="pagerank" class="cursor-pointer" { "Importance Score" }
                                th data-sort="drift_score" class="cursor-pointer" { "Change Risk" }
                                th { "Tests" }
                                th { "Understanding" }
                            }
                        }
                        tbody id="xray-hotspots-body" {}
                    }
                }
            }
        }
    })
}
