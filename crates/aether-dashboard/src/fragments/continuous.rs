use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::api::continuous;
use crate::support::{self, DashboardState};

pub(crate) async fn continuous_fragment(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let shared = state.shared.clone();
    let data = support::run_blocking_with_timeout(move || {
        continuous::load_continuous_data(shared.as_ref())
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(error = %err, "dashboard: failed to load continuous fragment data");
        continuous::ContinuousData {
            has_data: false,
            last_started_at: None,
            last_completed_at: None,
            total_symbols: 0,
            symbols_with_sir: 0,
            scored_symbols: 0,
            score_bands: continuous::ScoreBands::default(),
            most_stale: None,
            selected_symbols: 0,
            requeue_pass: String::new(),
            last_error: None,
        }
    });

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Continuous Intelligence Monitor",
                "AETHER continuously watches for stale or outdated understanding and refreshes it automatically.",
                "The continuous monitor scores symbol staleness using noisy-OR probability and requeues the worst offenders for re-analysis.",
                "Noisy-OR staleness scoring with semantic gate, priority-weighted requeue selection."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Continuous Intelligence Monitor" }
                    span class="intermediate-only" { "Continuous Monitor Status" }
                    span class="expert-only" { "Continuous Monitor" }
                }
                @if data.has_data {
                    span class="badge badge-cyan" {
                        "Last scan: " (support::format_age_seconds(data.last_completed_at.map(now_minus))) " ago"
                    }
                } @else {
                    span class="badge badge-muted" { "No scans yet" }
                }
            }

            @if !data.has_data {
                (support::html_empty_state("No continuous monitor data", Some("aether continuous run-once")))
            } @else {
                // Overview metrics
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                    h3 class="text-sm font-semibold" { "Symbol Coverage" }
                    div class="grid grid-cols-2 md:grid-cols-4 gap-3" {
                        (metric_card("Total Symbols", &data.total_symbols.to_string()))
                        (metric_card("With SIR", &data.symbols_with_sir.to_string()))
                        (metric_card("Scored", &data.scored_symbols.to_string()))
                        (metric_card("Requeued", &data.selected_symbols.to_string()))
                    }
                }

                // Staleness distribution
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                    h3 class="text-sm font-semibold" { "Staleness Distribution" }
                    @let total = data.score_bands.critical + data.score_bands.high + data.score_bands.medium + data.score_bands.low;
                    @if total > 0 {
                        div class="w-full h-6 rounded-full overflow-hidden flex" {
                            @let pct = |n: usize| format!("{}%", (n as f64 / total as f64 * 100.0).round() as u64);
                            @if data.score_bands.critical > 0 {
                                div class="bg-red-500 dark:bg-red-600 h-full flex items-center justify-center text-[10px] text-white font-bold"
                                    style=(format!("width: {}", pct(data.score_bands.critical)))
                                    title=(format!("Critical: {}", data.score_bands.critical)) {}
                            }
                            @if data.score_bands.high > 0 {
                                div class="bg-orange-400 dark:bg-orange-500 h-full flex items-center justify-center text-[10px] text-white font-bold"
                                    style=(format!("width: {}", pct(data.score_bands.high)))
                                    title=(format!("High: {}", data.score_bands.high)) {}
                            }
                            @if data.score_bands.medium > 0 {
                                div class="bg-yellow-400 dark:bg-yellow-500 h-full flex items-center justify-center text-[10px] text-white font-bold"
                                    style=(format!("width: {}", pct(data.score_bands.medium)))
                                    title=(format!("Medium: {}", data.score_bands.medium)) {}
                            }
                            @if data.score_bands.low > 0 {
                                div class="bg-green-400 dark:bg-green-500 h-full flex items-center justify-center text-[10px] text-white font-bold"
                                    style=(format!("width: {}", pct(data.score_bands.low)))
                                    title=(format!("Low: {}", data.score_bands.low)) {}
                            }
                        }
                        div class="flex flex-wrap gap-2 text-xs mt-2" {
                            span class="badge badge-red" { "Critical: " (data.score_bands.critical) }
                            span class="badge badge-orange" { "High: " (data.score_bands.high) }
                            span class="badge badge-yellow" { "Medium: " (data.score_bands.medium) }
                            span class="badge badge-green" { "Low: " (data.score_bands.low) }
                        }
                    } @else {
                        p class="text-xs text-text-muted" { "No symbols scored yet." }
                    }
                }

                // Most stale symbol
                @if let Some(stale) = &data.most_stale {
                    div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-1" {
                        h3 class="text-sm font-semibold" { "Most Stale Symbol" }
                        div class="flex items-center gap-2" {
                            span class="symbol-link text-blue-600 dark:text-blue-400 hover:underline cursor-pointer font-mono text-sm"
                                data-symbol=(stale.qualified_name.as_str()) {
                                (stale.qualified_name.as_str())
                            }
                            span class="badge badge-red" { "Staleness: " (format!("{:.2}", stale.staleness_score)) }
                        }
                    }
                }

                // Error display
                @if let Some(error) = &data.last_error {
                    div class="rounded-xl border border-red-300/40 bg-red-50/40 dark:border-red-700/40 dark:bg-red-950/20 p-4" {
                        h3 class="text-sm font-semibold text-red-700 dark:text-red-400" { "Last Error" }
                        pre class="text-xs text-red-600 dark:text-red-300 mt-1 whitespace-pre-wrap" { (error) }
                    }
                }

                // CLI hint
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    h3 class="text-sm font-semibold" { "Run Scan" }
                    p class="text-xs text-text-secondary mt-1" { "To trigger a manual staleness scan, use the CLI:" }
                    code class="block mt-2 text-xs font-mono bg-surface-2/60 dark:bg-slate-800/60 p-2 rounded select-all" {
                        "aether continuous run-once"
                    }
                }
            }
        }
    })
}

fn metric_card(label: &str, value: &str) -> maud::Markup {
    html! {
        div class="rounded-lg border border-surface-3/30 bg-surface-0/60 dark:bg-slate-800/40 p-3 text-center" {
            div class="text-xl font-bold" { (value) }
            div class="text-xs text-text-muted mt-1" { (label) }
        }
    }
}

fn now_minus(timestamp: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (now - timestamp).max(0)
}
