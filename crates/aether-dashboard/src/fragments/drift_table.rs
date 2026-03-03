use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::api::drift::{self, DriftQuery};
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DriftFragmentQuery {
    pub threshold: Option<f64>,
}

pub(crate) async fn drift_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<DriftFragmentQuery>,
) -> Html<String> {
    let threshold = query.threshold.unwrap_or(0.0).clamp(0.0, 1.0);
    let data = drift::load_drift_data(
        state.shared.as_ref(),
        &DriftQuery {
            since: None,
            threshold: Some(threshold),
        },
    )
    .unwrap_or_else(|err| {
        tracing::warn!(error = %err, "dashboard: failed to load drift fragment data");
        drift::DriftData {
            analysis_available: false,
            drift_entries: Vec::new(),
            total_checked: 0,
            drifted_count: 0,
            threshold_used: threshold,
        }
    });

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "What's Changed Since Last Analysis",
                "Drift means the code changed but AETHER's understanding hasn't been updated yet.",
                "Higher change risk means bigger mismatch between recent edits and stored understanding.",
                "Drift report based on semantic delta magnitude."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "What's Changed Since Last Analysis" }
                    span class="intermediate-only" { "Change Risk Report" }
                    span class="expert-only" { "Drift Report" }
                }
                span class="badge badge-orange" {
                    (data.drift_entries.len()) " entries"
                }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                div class="grid gap-3 md:grid-cols-[minmax(0,1fr)_72px] md:items-end" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wider text-text-muted" {
                            "Change Risk Threshold "
                            (support::help_icon("Only show components at or above this risk level. Higher values are riskier changes."))
                        }
                        input
                            type="range"
                            min="0"
                            max="1"
                            step="0.01"
                            name="threshold"
                            value={(format!("{:.2}", threshold))}
                            hx-get="/dashboard/frag/drift-table"
                            hx-target="#main-content"
                            hx-trigger="input delay:200ms, change"
                            hx-include="this"
                            class="w-full";
                    }
                    div class="text-right text-xs font-mono text-text-secondary" {
                        (format!("{:.2}", threshold))
                    }
                }

                div class="flex flex-wrap gap-2 text-xs" {
                    span class="badge badge-muted" {
                        "Components Checked: " (data.total_checked)
                    }
                    span class="badge badge-yellow" {
                        "At-Risk Components: " (data.drifted_count)
                    }
                    @if !data.analysis_available {
                        span class="badge badge-red" { "Analysis unavailable" }
                    }
                }
            }

            div id="drift-chart" class="chart-container min-h-[320px]" {}

            @if data.drift_entries.is_empty() {
                (support::html_empty_state("No drift entries matched the current threshold", Some("aether drift-report")))
            } @else {
                table class="data-table" {
                    thead {
                        tr {
                            th { "Component" }
                            th { "Type" }
                            th { "Change Risk" }
                            th { "File" }
                        }
                    }
                    tbody {
                        @for entry in data.drift_entries {
                            tr {
                                td {
                                    div class="font-medium" {
                                        span class="symbol-link text-blue-600 hover:underline cursor-pointer"
                                            data-symbol=(entry.symbol_name.as_str()) {
                                            (entry.symbol_name.as_str())
                                        }
                                    }
                                    div class="text-xs text-text-muted font-mono" { (entry.symbol_id) }
                                    @if let Some(summary) = entry.drift_summary {
                                        div class="text-xs text-text-secondary mt-1" { (summary) }
                                    }
                                }
                                td { span class="badge badge-orange" { (entry.drift_type) } }
                                td class="font-mono" { (format!("{:.2}", entry.drift_magnitude)) }
                                td class="font-mono text-xs" {
                                    span class="file-link text-blue-600 hover:underline cursor-pointer"
                                        data-path=(entry.file_path.as_str()) {
                                        (entry.file_path.as_str())
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
