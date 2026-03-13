use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;

use aether_health::Severity;

use crate::api::health_score::{self, HealthScoreQuery};
use crate::support::{self, DashboardState};

pub(crate) async fn health_score_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<HealthScoreQuery>,
) -> Html<String> {
    let shared = state.shared.clone();
    let query = query.clone();
    let data = match support::run_async_with_timeout(move || {
        let shared = shared.clone();
        let query = query.clone();
        async move { health_score::load_health_score_data(shared.as_ref(), &query).await }
    })
    .await
    {
        Ok(data) => data,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load health score", &detail))
            });
        }
    };

    let trend_json = serde_json::to_string(&data.trend).unwrap_or_else(|_| "[]".to_owned());
    let delta_label = match data.delta.cmp(&0) {
        std::cmp::Ordering::Less => format!("↓ {}", data.delta.abs()),
        std::cmp::Ordering::Equal => "→ 0".to_owned(),
        std::cmp::Ordering::Greater => format!("↑ {}", data.delta),
    };

    support::html_markup_response(html! {
        div class="rounded-2xl border border-surface-3/40 bg-surface-1/60 p-5 shadow-sm space-y-5" style="font-family: 'IBM Plex Sans', sans-serif;" {
            div class="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between" {
                div class="space-y-2" {
                    div class="text-[11px] uppercase tracking-[0.24em] text-text-muted" { "Health Gauge" }
                    h3 class="text-xl font-semibold text-text-primary" { "Workspace Health Score" }
                    p class="text-sm text-text-secondary max-w-2xl" {
                        "Structural, git, and semantic hotspot scoring across workspace crates. Higher scores are healthier."
                    }
                }
                div class={ "rounded-2xl border px-5 py-4 min-w-[220px] " (severity_panel_class(data.severity)) } {
                    div class="flex items-start justify-between gap-4" {
                        div {
                            div class="text-[11px] uppercase tracking-[0.22em] opacity-80" { (data.severity.as_label()) }
                            div class="text-5xl font-semibold leading-none mt-1" { (data.workspace_score) }
                            div class="text-xs mt-2 opacity-85" { "Current workspace health" }
                        }
                        span class={ "rounded-full px-2.5 py-1 text-xs font-semibold " (delta_class(data.delta)) } {
                            (delta_label)
                        }
                    }
                }
            }

            div class="grid gap-5 xl:grid-cols-[minmax(0,1.15fr)_minmax(0,0.85fr)]" {
                div class="space-y-3" {
                    div class="flex items-center justify-between gap-3" {
                        h4 class="text-sm font-semibold" { "Hotspot Crates" }
                        span class="text-xs text-text-muted" { (data.crates.len()) " shown" }
                    }
                    @if data.crates.is_empty() {
                        (support::html_empty_state("No crates exceeded the current threshold", None))
                    } @else {
                        table class="data-table" {
                            thead {
                                tr {
                                    th { "Crate" }
                                    th { "Score" }
                                    th { "Archetypes" }
                                    th { "Top Violation" }
                                }
                            }
                            tbody {
                                @for crate_row in &data.crates {
                                    tr {
                                        td class="align-top" {
                                            div class="font-medium text-text-primary" { (&crate_row.name) }
                                            div class={ "text-xs mt-1 " (severity_text_class(crate_row.severity)) } {
                                                (crate_row.severity.as_label())
                                            }
                                        }
                                        td class="align-top min-w-[180px]" {
                                            div class="flex items-center gap-3" {
                                                div class="h-2.5 flex-1 rounded-full bg-surface-3/35 overflow-hidden" {
                                                    div class={ "h-full rounded-full " (severity_bar_class(crate_row.severity)) }
                                                        style={ "width: " (crate_row.score) "%" } {}
                                                }
                                                span class="font-mono text-xs text-text-secondary min-w-[48px] text-right" {
                                                    (crate_row.score) "/100"
                                                }
                                            }
                                        }
                                        td class="align-top" {
                                            div class="flex flex-wrap gap-1.5" {
                                                @if crate_row.archetypes.is_empty() {
                                                    span class="badge badge-muted" { "None" }
                                                } @else {
                                                    @for archetype in &crate_row.archetypes {
                                                        span class="badge badge-purple" { (archetype) }
                                                    }
                                                }
                                            }
                                        }
                                        td class="align-top text-xs text-text-secondary leading-5" {
                                            (&crate_row.top_violation)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div class="space-y-4" {
                    div class="rounded-xl border border-surface-3/30 bg-surface-0/55 p-4 space-y-3" {
                        div class="flex items-center justify-between gap-3" {
                            h4 class="text-sm font-semibold" { "Archetype Distribution" }
                            span class="text-xs text-text-muted" { "Filtered crates" }
                        }
                        @if data.archetype_distribution.is_empty() {
                            (support::html_empty_state("No archetypes above the current threshold", None))
                        } @else {
                            @let max_count = data.archetype_distribution.values().copied().max().unwrap_or(1);
                            div class="space-y-3" {
                                @for (name, count) in data.archetype_distribution.iter() {
                                    div class="space-y-1.5" {
                                        div class="flex items-center justify-between gap-3 text-xs" {
                                            span class="font-medium text-text-primary" { (name) }
                                            span class="font-mono text-text-secondary" { (count) }
                                        }
                                        div class="h-2.5 rounded-full bg-surface-3/35 overflow-hidden" {
                                            div class="h-full rounded-full bg-accent-cyan"
                                                style={ "width: " (((*count as f64 / max_count as f64) * 100.0).round()) "%" } {}
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div class="rounded-xl border border-surface-3/30 bg-surface-0/55 p-4 space-y-3" {
                        div class="flex items-center justify-between gap-3" {
                            h4 class="text-sm font-semibold" { "Recent Trend" }
                            span class="text-xs text-text-muted" { (data.trend.len()) " points" }
                        }
                        div
                            data-health-score-trend=(trend_json)
                            data-health-score-severity=(serde_json::to_string(&data.severity).unwrap_or_else(|_| "\"moderate\"".to_owned()).replace('"', ""))
                            class="chart-container min-h-[68px]" {}
                    }
                }
            }
        }
    })
}

fn severity_panel_class(severity: Severity) -> &'static str {
    match severity {
        Severity::Healthy => {
            "border-emerald-500/35 bg-emerald-500/12 text-emerald-100 dark:text-emerald-100"
        }
        Severity::Watch => "border-lime-500/35 bg-lime-500/12 text-lime-100 dark:text-lime-100",
        Severity::Moderate => {
            "border-amber-500/35 bg-amber-500/12 text-amber-100 dark:text-amber-100"
        }
        Severity::High => {
            "border-orange-500/35 bg-orange-500/12 text-orange-100 dark:text-orange-100"
        }
        Severity::Critical => "border-red-500/35 bg-red-500/12 text-red-100 dark:text-red-100",
    }
}

fn delta_class(delta: i32) -> &'static str {
    match delta.cmp(&0) {
        std::cmp::Ordering::Less => "bg-red-500/20 text-red-200",
        std::cmp::Ordering::Equal => "bg-surface-3/50 text-text-secondary",
        std::cmp::Ordering::Greater => "bg-emerald-500/20 text-emerald-200",
    }
}

fn severity_text_class(severity: Severity) -> &'static str {
    match severity {
        Severity::Healthy => "text-emerald-400",
        Severity::Watch => "text-lime-400",
        Severity::Moderate => "text-amber-400",
        Severity::High => "text-orange-400",
        Severity::Critical => "text-red-400",
    }
}

fn severity_bar_class(severity: Severity) -> &'static str {
    match severity {
        Severity::Healthy => "bg-emerald-500",
        Severity::Watch => "bg-lime-500",
        Severity::Moderate => "bg-amber-500",
        Severity::High => "bg-orange-500",
        Severity::Critical => "bg-red-500",
    }
}
