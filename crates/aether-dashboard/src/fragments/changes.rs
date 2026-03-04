use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use chrono::{DateTime, Utc};
use maud::html;
use serde::Deserialize;

use crate::api::changes;
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ChangesFragmentQuery {
    pub since: Option<String>,
    pub limit: Option<usize>,
    pub embed: Option<bool>,
}

pub(crate) async fn changes_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<ChangesFragmentQuery>,
) -> Html<String> {
    let since = query.since;
    let limit = query.limit;
    let embed = query.embed.unwrap_or(false);

    let shared = state.shared.clone();
    let data = match support::run_blocking_with_timeout(move || {
        changes::load_changes_data(shared.workspace.as_path(), since.as_deref(), limit)
    })
    .await
    {
        Ok(data) => data,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load recent changes", detail.as_str()))
            });
        }
    };

    let selected_period = data.period.clone();
    let selected_limit = limit.unwrap_or(20).clamp(1, 100);

    if embed {
        return support::html_markup_response(render_changes_content(
            &data,
            selected_period.as_str(),
            selected_limit,
            true,
        ));
    }

    support::html_markup_response(html! {
        div class="space-y-4" data-page="changes" {
            (support::explanation_header(
                "What Changed Recently",
                "This timeline summarizes what changed since you last checked in, with layer context and affected components.",
                "Use the time filters to track updates by file, understanding refreshes, and commit context.",
                "File-level change stream from mtimes, SIR refreshes, git history, and drift signals."
            ))
            h2 class="text-lg font-semibold" { "What Changed Recently" }
            (render_changes_content(&data, selected_period.as_str(), selected_limit, false))
        }
    })
}

fn render_changes_content(
    data: &changes::ChangesData,
    selected_period: &str,
    limit: usize,
    embed: bool,
) -> maud::Markup {
    let summary_text = format!(
        "{} files changed affecting {} components across {} layers",
        data.file_summary.files_changed,
        data.file_summary.symbols_affected,
        data.file_summary.layers_touched.len()
    );

    html! {
        div id="changes-content" class="space-y-3" {
            div class="flex flex-wrap items-center gap-2" {
                @for period in ["1h", "24h", "7d", "30d"] {
                    @let endpoint = changes_endpoint(period, limit, embed);
                    button
                        class={
                            "px-3 py-1.5 rounded-full text-xs border transition-colors "
                            (if period == selected_period {
                                "border-blue-500 bg-blue-100 text-blue-900 dark:bg-blue-900/30 dark:text-blue-200"
                            } else {
                                "border-surface-3/60 hover:bg-surface-3/30"
                            })
                        }
                        hx-get=(endpoint)
                        hx-target="#changes-content"
                        hx-swap="outerHTML" {
                        (period)
                    }
                }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                p class="text-sm text-text-secondary" { (summary_text) }
            }

            @if data.changes.is_empty() {
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 text-sm text-text-secondary" {
                    "No changes in the last " (data.period.as_str()) ". The codebase is stable. ✨"
                }
            } @else {
                div class="space-y-3" {
                    @for change in &data.changes {
                        @let when = relative_timestamp(change.timestamp.as_str());
                        @let symbol_count = change.symbols_affected.len();

                        article class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                            div class="flex flex-wrap items-center justify-between gap-2" {
                                div class="flex flex-wrap items-center gap-2" {
                                    span class="text-xs text-text-muted" { (when) }
                                    span class="badge badge-muted" {
                                        (change.layer_icon.as_str()) " " (change.layer.as_str())
                                    }
                                }
                                span class="file-link text-xs font-mono text-blue-600 hover:underline cursor-pointer"
                                    data-path=(change.file.as_str()) {
                                    (change.file.as_str())
                                }
                            }

                            p class="text-sm text-text-secondary" { (change.summary.as_str()) }

                            @if let Some(message) = &change.git_message {
                                blockquote class="border-l-2 border-surface-3/60 pl-3 text-xs text-text-secondary" {
                                    "\"" (message.as_str()) "\""
                                    @if let Some(author) = &change.git_author {
                                        span class="ml-2 text-text-muted" { "- " (author.as_str()) }
                                    }
                                }
                            }

                            @if symbol_count > 0 {
                                details class="rounded-md border border-surface-3/30 bg-surface-0/60 p-2" {
                                    summary class="cursor-pointer text-xs text-text-secondary" {
                                        "Affected components (" (symbol_count) ")"
                                    }
                                    div class="mt-2 flex flex-wrap gap-2" {
                                        @for symbol in &change.symbols_affected {
                                            span class="symbol-link text-xs px-2 py-1 rounded-md border border-surface-3/40 hover:bg-surface-3/30 cursor-pointer"
                                                data-symbol=(symbol.as_str()) {
                                                (symbol.as_str())
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn changes_endpoint(period: &str, limit: usize, embed: bool) -> String {
    let mut endpoint = format!(
        "/dashboard/frag/changes?since={}&limit={limit}",
        support::percent_encode(period)
    );
    if embed {
        endpoint.push_str("&embed=true");
    }
    endpoint
}

fn relative_timestamp(value: &str) -> String {
    let Ok(parsed) = DateTime::parse_from_rfc3339(value) else {
        return value.to_owned();
    };

    let ts_ms = parsed.with_timezone(&Utc).timestamp_millis();
    let now_ms = support::current_unix_timestamp().saturating_mul(1000);
    if now_ms <= ts_ms {
        return "just now".to_owned();
    }

    let age_seconds = now_ms.saturating_sub(ts_ms) / 1000;
    format!("{} ago", support::format_age_seconds(Some(age_seconds)))
}
