use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::api::coupling;
use crate::support::{self, DashboardState};

pub(crate) async fn coupling_fragment(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let shared = state.shared.clone();
    let data = support::run_async_with_timeout(move || async move {
        coupling::load_coupling_data(shared.as_ref(), 0.0, 100).await
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(error = %err, "dashboard: failed to load coupling fragment data");
        coupling::CouplingData {
            analysis_available: false,
            pairs: Vec::new(),
            total_pairs: 0,
            commits_scanned: 0,
            last_mined_at: None,
            last_commit_hash: None,
        }
    });

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Which Files Change Together",
                "When two files frequently change at the same time, they're coupled.",
                "High coupling means edits in one file often impact another.",
                "Temporal, structural, and semantic co-change coupling view."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Which Files Change Together" }
                    span class="intermediate-only" { "File Coupling Map" }
                    span class="expert-only" { "Coupling Map" }
                }
                span class="badge badge-orange" { (data.total_pairs) " pairs" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                div class="flex flex-wrap gap-2 text-xs" {
                    span class="badge badge-muted" { "Commits scanned: " (data.commits_scanned) }
                    @if let Some(last_mined_at) = data.last_mined_at {
                        span class="badge badge-muted" { "Last mined at: " (last_mined_at) }
                    }
                    @if let Some(last_commit_hash) = &data.last_commit_hash {
                        span class="badge badge-muted" { "Commit: " (last_commit_hash) }
                    }
                    @if !data.analysis_available {
                        span class="badge badge-red" { "Analysis unavailable" }
                    }
                }
            }

            @if !data.pairs.is_empty() {
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                    h3 class="text-sm font-semibold" { "Key Connections" }
                    ul class="space-y-1 text-sm text-text-secondary" {
                        @for pair in data.pairs.iter().take(5) {
                            li {
                                @let qualitative = support::coupling_strength_label(pair.coupling_score);
                                @let reason = strongest_signal_reason(pair);
                                (pair.file_a) " and " (pair.file_b) " are " (qualitative) " (" (format!("{:.2}", pair.coupling_score)) ") — " (reason)
                            }
                        }
                    }
                }
            }

            div id="heatmap-container" class="chart-container min-h-[420px]" {}

            @if data.pairs.is_empty() {
                (support::html_empty_state("No coupling pairs available", Some("aether mine-coupling")))
            } @else {
                table class="data-table" {
                    thead {
                        tr {
                            th { "File A" }
                            th { "File B" }
                            th { "Connection Strength" }
                            th { "Type" }
                        }
                    }
                    tbody {
                        @for pair in data.pairs.iter().take(25) {
                            tr {
                                td class="font-mono text-xs" { (pair.file_a) }
                                td class="font-mono text-xs" { (pair.file_b) }
                                td class="font-mono" {
                                    @let qualitative = support::coupling_strength_label(pair.coupling_score);
                                    "Connection Strength: " (qualitative) " (" (format!("{:.2}", pair.coupling_score)) ")"
                                }
                                td { span class="badge badge-yellow" { (pair.coupling_type.clone()) } }
                            }
                        }
                    }
                }
            }
        }
    })
}

fn strongest_signal_reason(pair: &coupling::CouplingPair) -> &'static str {
    let temporal = pair.signals.temporal.max(pair.signals.co_change);
    let structural = pair.signals.structural.max(pair.signals.static_signal);
    let semantic = pair.signals.semantic;

    if temporal >= structural && temporal >= semantic {
        "commit history shows they change together frequently"
    } else if structural >= temporal && structural >= semantic {
        "their dependency structure is tightly linked"
    } else {
        "their behavior and intent patterns are strongly related"
    }
}
