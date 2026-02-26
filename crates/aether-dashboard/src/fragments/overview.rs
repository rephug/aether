use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::{Markup, html};

use crate::support::{self, DashboardState};

pub(crate) async fn overview_fragment(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let overview = match support::load_overview_data(state.shared.as_ref()).await {
        Ok(data) => data,
        Err(err) => {
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load overview", &err))
            });
        }
    };
    let staleness = support::compute_staleness(state.shared.workspace.as_path());
    let age_text = support::format_age_seconds(staleness.index_age_seconds);

    support::html_markup_response(html! {
        div class="space-y-5" {
            div class="grid gap-4 md:grid-cols-2 xl:grid-cols-4" {
                (stat_card(&overview.total_symbols.to_string(), "Symbols"))
                (stat_card(&overview.total_files.to_string(), "Files"))
                (stat_card(&format!("{:.1}%", overview.sir_coverage_pct), "SIR Coverage"))
                (stat_card(&age_text, "Index Age"))
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                div class="flex flex-wrap items-center gap-2 text-xs" {
                    span class="badge badge-muted" { "Backend" }
                    span class={ "badge " (if overview.graph_backend == "surreal" { "badge-purple" } else { "badge-cyan" }) } { (overview.graph_backend) }
                    span class="badge badge-muted" { "Vector" }
                    span class={ "badge " (match overview.vector_status.as_str() { "available" => "badge-green", "disabled" => "badge-muted", _ => "badge-red" }) } { (overview.vector_status) }
                    span class="badge badge-yellow" { "Drift: " (overview.drift_count) }
                    span class="badge badge-orange" { "Coupling: " (overview.coupling_count) }
                    @if staleness.stale {
                        span class="badge badge-red" { "Stale" }
                    }
                }
            }

            div class="grid gap-5 xl:grid-cols-[minmax(0,1.2fr)_minmax(0,0.8fr)]" {
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    h2 class="text-sm font-semibold mb-3" { "Language Breakdown" }
                    @if overview.languages.is_empty() {
                        (support::html_empty_state("No indexed files yet", Some("aether index")))
                    } @else {
                        table class="data-table" {
                            thead {
                                tr {
                                    th { "Language" }
                                    th { "Files" }
                                    th { "Share" }
                                }
                            }
                            tbody {
                                @for (lang, count) in &overview.languages {
                                    tr {
                                        td {
                                            span class={ "badge " (support::badge_class_for_language(lang)) } { (lang) }
                                        }
                                        td class="font-mono" { (count) }
                                        td class="font-mono" {
                                            @let pct = if overview.total_files > 0 { (*count as f64 / overview.total_files as f64) * 100.0 } else { 0.0 };
                                            (format!("{pct:.1}%"))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    h2 class="text-sm font-semibold mb-3" { "Overview Chart" }
                    div id="overview-chart" class="min-h-[260px]" {
                        @if overview.languages.is_empty() {
                            (support::html_empty_state("No indexed files yet", Some("aether index")))
                        }
                    }
                }
            }
        }
    })
}

fn stat_card(value: &str, label: &str) -> Markup {
    html! {
        div class="stat-card" {
            div class="stat-value" { (value) }
            div class="stat-label" { (label) }
        }
    }
}
