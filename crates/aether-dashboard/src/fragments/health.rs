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
    let shared = state.shared.clone();
    let data = support::run_async_with_timeout(move || async move {
        Ok(health::load_health_data(shared.as_ref(), &query).await)
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(error = %err, "dashboard: failed to load health fragment data");
        health::HealthData {
            analysis_available: false,
            overall_score: None,
            notes: vec!["Health analysis timed out or failed to load.".to_owned()],
            dimensions: health::HealthDimensions {
                sir_coverage: 0.0,
                test_coverage: 0.0,
                graph_connectivity: 0.0,
                coupling_health: 0.0,
                drift_health: 0.0,
            },
            hotspots: Vec::new(),
            cycles: Vec::new(),
            orphans: Vec::new(),
        }
    });
    let staleness = support::compute_staleness(state.shared.workspace.as_path());
    let recommendation = primary_recommendation(&data, staleness.index_age_seconds);

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Project Health Check",
                "AETHER measures your codebase across several dimensions. Higher scores are better.",
                "These scores summarize quality, test coverage, connectivity, and freshness.",
                "Composite health dimensions across SIR, testing, connectivity, coupling, and drift."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Project Health Check" }
                    span class="intermediate-only" { "Project Health" }
                    span class="expert-only" { "Health" }
                }
                @if let Some(score) = data.overall_score {
                    span class="badge badge-green" { "Overall " (format!("{:.0}%", score * 100.0)) }
                } @else {
                    span class="badge badge-muted" { "No overall score" }
                }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                div class="flex flex-wrap gap-2 text-xs" {
                    span class="badge badge-cyan inline-flex gap-1 items-center" {
                        "Understanding Coverage " (format!("{:.0}%", data.dimensions.sir_coverage * 100.0))
                        (support::help_icon("How much of your code has SIR understanding records. Higher means AETHER understands more components."))
                    }
                    span class="badge badge-green inline-flex gap-1 items-center" {
                        "Test Coverage " (format!("{:.0}%", data.dimensions.test_coverage * 100.0))
                        (support::help_icon("Share of components linked to tests. Low coverage means more unverified behavior."))
                    }
                    span class="badge badge-purple inline-flex gap-1 items-center" {
                        "Connected Components " (format!("{:.0}%", data.dimensions.graph_connectivity * 100.0))
                        (support::help_icon("How much of the code graph is connected to the main component network. Low values may indicate isolated or dead code."))
                    }
                    span class="badge badge-yellow inline-flex gap-1 items-center" {
                        "Connection Health " (format!("{:.0}%", data.dimensions.coupling_health * 100.0))
                        (support::help_icon("Higher values mean lower harmful coupling across files."))
                    }
                    span class="badge badge-orange inline-flex gap-1 items-center" {
                        "Change Risk " (format!("{:.0}%", data.dimensions.drift_health * 100.0))
                        (support::help_icon("Higher values mean less unresolved semantic drift between code and last analysis."))
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

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                h3 class="text-sm font-semibold" { "What to Work on First" }
                p class="text-sm text-text-secondary" { (recommendation) }
            }

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
                                    tr {
                                        td {
                                            div class="font-medium" {
                                                span class="symbol-link text-blue-600 hover:underline cursor-pointer"
                                                    data-symbol=(hotspot.symbol_name.as_str()) {
                                                    (hotspot.symbol_name.as_str())
                                                }
                                            }
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

fn primary_recommendation(data: &health::HealthData, index_age_seconds: Option<i64>) -> String {
    let mut candidates: Vec<(&str, f64, &str)> = Vec::new();

    if data.dimensions.sir_coverage < 0.70 {
        candidates.push((
            "sir_coverage",
            data.dimensions.sir_coverage,
            "🔍 Many components haven't been analyzed. Run --index-once.",
        ));
    }
    if data.dimensions.test_coverage < 0.50 {
        candidates.push((
            "test_coverage",
            data.dimensions.test_coverage,
            "🧪 Less than half your components have tests.",
        ));
    }
    if data.dimensions.graph_connectivity < 0.60 {
        candidates.push((
            "graph_connectivity",
            data.dimensions.graph_connectivity,
            "🔗 Some components are isolated. May indicate dead code.",
        ));
    }
    if index_age_seconds.unwrap_or(0) > 24 * 60 * 60 {
        candidates.push(("staleness", 0.0, "⏰ Analysis is stale. Re-run indexing."));
    }

    candidates.sort_by(|left, right| {
        left.1
            .partial_cmp(&right.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(right.0))
    });

    if let Some((_, _, recommendation)) = candidates.first() {
        (*recommendation).to_owned()
    } else {
        "No urgent blockers detected. Keep indexing fresh and monitor high-risk hotspots."
            .to_owned()
    }
}
