use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::api::task_context;
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TaskContextFragmentQuery {
    pub limit: Option<usize>,
}

pub(crate) async fn task_context_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<TaskContextFragmentQuery>,
) -> Html<String> {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let shared = state.shared.clone();
    let data = support::run_blocking_with_timeout(move || {
        task_context::load_task_context_data(shared.as_ref(), limit)
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(error = %err, "dashboard: failed to load task context fragment data");
        task_context::TaskContextData {
            has_history: false,
            entries: Vec::new(),
            total_entries: 0,
        }
    });

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Task Context Resolutions",
                "When you ask AETHER for context about a task, it finds the most relevant symbols and files. This page shows recent resolutions.",
                "Task-to-symbol ranking via Reciprocal Rank Fusion (RRF) + Personalized PageRank. Shows resolved context assemblies.",
                "RRF + PPR task context resolution history with budget tracking."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Task Context Resolutions" }
                    span class="intermediate-only" { "Task Context History" }
                    span class="expert-only" { "Task Context" }
                }
                span class="badge badge-cyan" { (data.total_entries) " resolutions" }
            }

            @if !data.has_history {
                (support::html_empty_state(
                    "No task context resolutions yet",
                    Some("aether context --mode task --task \"your task description\"")
                ))
            } @else {
                table class="data-table" {
                    thead {
                        tr {
                            th { "Task" }
                            th { "Branch" }
                            th { "Symbols" }
                            th { "Files" }
                            th { "Budget" }
                            th { "When" }
                        }
                    }
                    tbody {
                        @for entry in &data.entries {
                            tr {
                                td {
                                    div class="max-w-xs truncate text-sm" title=(entry.task_description.as_str()) {
                                        (entry.task_description.as_str())
                                    }
                                }
                                td {
                                    @if let Some(branch) = &entry.branch_name {
                                        span class="badge badge-purple" { (branch) }
                                    } @else {
                                        span class="text-xs text-text-muted" { "—" }
                                    }
                                }
                                td class="font-mono text-center" { (entry.symbol_count) }
                                td class="font-mono text-center" { (entry.file_count) }
                                td {
                                    div class="flex items-center gap-2" {
                                        div class="w-16 h-2 rounded-full bg-surface-3/40 dark:bg-slate-700 overflow-hidden" {
                                            div class="h-full rounded-full bg-accent-cyan"
                                                style=(format!("width: {}%", entry.budget_pct.round() as u64)) {}
                                        }
                                        span class="text-xs font-mono text-text-secondary" {
                                            (format!("{:.0}%", entry.budget_pct))
                                        }
                                    }
                                }
                                td class="text-xs text-text-muted whitespace-nowrap" {
                                    (support::format_age_seconds(Some(now_minus(entry.created_at)))) " ago"
                                }
                            }
                        }
                    }
                }
            }

            // CLI hint
            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                h3 class="text-sm font-semibold" { "Try a Task" }
                p class="text-xs text-text-secondary mt-1" {
                    "To see which symbols are relevant to a task, use the CLI:"
                }
                code class="block mt-2 text-xs font-mono bg-surface-2/60 dark:bg-slate-800/60 p-2 rounded select-all" {
                    "aether task-relevance --task \"describe your task here\""
                }
            }
        }
    })
}

fn now_minus(timestamp: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (now - timestamp).max(0)
}
