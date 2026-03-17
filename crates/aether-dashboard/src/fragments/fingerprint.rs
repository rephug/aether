use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::api::fingerprint;
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct FingerprintFragmentQuery {
    pub symbol_id: Option<String>,
}

pub(crate) async fn fingerprint_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<FingerprintFragmentQuery>,
) -> Html<String> {
    let shared = state.shared.clone();
    let summary = support::run_blocking_with_timeout(move || {
        fingerprint::load_fingerprint_summary(shared.as_ref(), 30)
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(error = %err, "dashboard: failed to load fingerprint fragment data");
        fingerprint::FingerprintSummaryData {
            has_data: false,
            recent_changes: Vec::new(),
            total_recent: 0,
            trigger_breakdown: Default::default(),
            top_changed_symbols: Vec::new(),
        }
    });

    // If a symbol_id is provided, also load the timeline
    let timeline = if let Some(symbol_id) = &query.symbol_id {
        let shared = state.shared.clone();
        let sid = symbol_id.clone();
        support::run_blocking_with_timeout(move || {
            fingerprint::load_fingerprint_timeline(shared.as_ref(), &sid)
        })
        .await
        .ok()
    } else {
        None
    };

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Fingerprint Change History",
                "Every time AETHER regenerates its understanding of a symbol, it records what triggered the change.",
                "Tracks prompt hash changes per symbol with source/neighbor/config trigger flags and semantic delta.",
                "sir_fingerprint_history table with BLAKE3 prompt hashes, trigger decomposition, and delta_sem tracking."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Fingerprint Change History" }
                    span class="intermediate-only" { "Fingerprint History" }
                    span class="expert-only" { "Fingerprint History" }
                }
                span class="badge badge-cyan" { (summary.total_recent) " recent changes" }
            }

            @if !summary.has_data {
                (support::html_empty_state(
                    "No fingerprint history yet",
                    Some("aether batch run")
                ))
            } @else {
                // Trigger breakdown
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                    h3 class="text-sm font-semibold" { "Trigger Breakdown" }
                    div class="flex flex-wrap gap-2" {
                        @for (trigger, count) in &summary.trigger_breakdown {
                            span class=(format!("badge {}", trigger_badge_class(trigger))) {
                                (trigger) ": " (count)
                            }
                        }
                    }
                }

                // Top changed symbols
                @if !summary.top_changed_symbols.is_empty() {
                    div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                        h3 class="text-sm font-semibold" { "Most Frequently Changed" }
                        div class="space-y-1" {
                            @for sym in summary.top_changed_symbols.iter().take(10) {
                                div class="flex items-center justify-between text-sm" {
                                    a href="#"
                                        class="font-mono text-blue-600 dark:text-blue-400 hover:underline text-xs"
                                        hx-get=(format!("/dashboard/frag/fingerprint?symbol_id={}", support::percent_encode(&sym.symbol_id)))
                                        hx-target="#main-content" {
                                        (sym.symbol_name.as_str())
                                    }
                                    span class="badge badge-muted" { (sym.change_count) " changes" }
                                }
                            }
                        }
                    }
                }

                // Recent changes table
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                    h3 class="text-sm font-semibold" { "Recent Changes" }
                    table class="data-table" {
                        thead {
                            tr {
                                th { "Symbol" }
                                th { "Trigger" }
                                th { "Delta" }
                                th { "Model" }
                                th { "When" }
                            }
                        }
                        tbody {
                            @for change in summary.recent_changes.iter().take(20) {
                                tr {
                                    td {
                                        a href="#"
                                            class="font-mono text-xs text-blue-600 dark:text-blue-400 hover:underline"
                                            hx-get=(format!("/dashboard/frag/fingerprint?symbol_id={}", support::percent_encode(&change.symbol_id)))
                                            hx-target="#main-content" {
                                            (change.symbol_name.as_str())
                                        }
                                    }
                                    td {
                                        div class="flex flex-wrap gap-1" {
                                            @if change.source_changed {
                                                span class="badge badge-orange" { "source" }
                                            }
                                            @if change.neighbor_changed {
                                                span class="badge badge-purple" { "neighbor" }
                                            }
                                            @if change.config_changed {
                                                span class="badge badge-yellow" { "config" }
                                            }
                                        }
                                    }
                                    td class="font-mono text-xs" {
                                        @if let Some(delta) = change.delta_sem {
                                            (format!("{:.3}", delta))
                                        } @else {
                                            span class="text-text-muted" { "—" }
                                        }
                                    }
                                    td class="text-xs text-text-secondary" {
                                        @if let Some(model) = &change.generation_model {
                                            (model)
                                        } @else {
                                            "—"
                                        }
                                    }
                                    td class="text-xs text-text-muted whitespace-nowrap" {
                                        (support::format_age_seconds(Some(now_minus(change.timestamp)))) " ago"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Symbol timeline (if symbol_id is selected)
            @if let Some(timeline) = &timeline {
                div class="rounded-xl border border-blue-200/40 dark:border-blue-800/40 bg-blue-50/20 dark:bg-blue-950/10 p-4 space-y-3" {
                    h3 class="text-sm font-semibold" {
                        "Timeline for "
                        span class="font-mono text-blue-600 dark:text-blue-400" { (timeline.symbol_name.as_str()) }
                    }

                    @if timeline.entries.is_empty() {
                        p class="text-xs text-text-muted" { "No fingerprint changes recorded for this symbol." }
                    } @else {
                        div class="space-y-2" {
                            @for entry in &timeline.entries {
                                div class="rounded-lg border border-surface-3/30 bg-surface-0/60 dark:bg-slate-800/40 p-3 space-y-1" {
                                    div class="flex items-center justify-between" {
                                        div class="flex flex-wrap gap-1" {
                                            @if entry.source_changed {
                                                span class="badge badge-orange" { "source" }
                                            }
                                            @if entry.neighbor_changed {
                                                span class="badge badge-purple" { "neighbor" }
                                            }
                                            @if entry.config_changed {
                                                span class="badge badge-yellow" { "config" }
                                            }
                                        }
                                        span class="text-xs text-text-muted" {
                                            (support::format_age_seconds(Some(now_minus(entry.timestamp)))) " ago"
                                        }
                                    }
                                    div class="flex flex-wrap gap-2 text-xs" {
                                        @if let Some(delta) = entry.delta_sem {
                                            span class="font-mono" { "delta_sem: " (format!("{:.3}", delta)) }
                                        }
                                        @if let Some(model) = &entry.generation_model {
                                            span class="text-text-secondary" { "model: " (model) }
                                        }
                                        @if let Some(pass) = &entry.generation_pass {
                                            span class="text-text-secondary" { "pass: " (pass) }
                                        }
                                    }
                                    div class="text-[10px] font-mono text-text-muted truncate" {
                                        (entry.prompt_hash.as_str())
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Symbol search input
            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                h3 class="text-sm font-semibold" { "View Symbol Timeline" }
                div class="flex gap-2 mt-2" {
                    input
                        id="fp-symbol-input"
                        type="text"
                        placeholder="Enter symbol ID..."
                        class="flex-1 p-2 text-sm border border-surface-3/50 rounded-lg bg-white/80 dark:bg-slate-800/80 dark:border-slate-600";
                    button
                        class="px-3 py-1.5 text-xs rounded border border-surface-3/50 hover:bg-surface-2/60 dark:hover:bg-slate-700/60"
                        onclick="var sid = document.getElementById('fp-symbol-input').value.trim(); if(sid) htmx.ajax('GET', '/dashboard/frag/fingerprint?symbol_id=' + encodeURIComponent(sid), '#main-content');" {
                        "Load Timeline"
                    }
                }
            }
        }
    })
}

fn trigger_badge_class(trigger: &str) -> &'static str {
    match trigger {
        "source" => "badge-orange",
        "neighbor" => "badge-purple",
        "config" => "badge-yellow",
        _ => "badge-muted",
    }
}

fn now_minus(timestamp: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (now - timestamp).max(0)
}
