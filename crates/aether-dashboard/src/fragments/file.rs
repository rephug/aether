use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Html;
use maud::html;

use crate::api::file::{self, FileDeepDiveData};
use crate::support::{self, DashboardState};

pub(crate) async fn file_fragment(
    State(state): State<Arc<DashboardState>>,
    Path(path): Path<String>,
) -> Html<String> {
    let shared = state.shared.clone();
    let data = match support::run_blocking_with_timeout(move || {
        file::build_file_deep_dive(shared.as_ref(), path.as_str())
    })
    .await
    {
        Ok(Some(data)) => data,
        Ok(None) => {
            return support::html_markup_response(html! {
                (support::html_empty_state("File not found", None))
            });
        }
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load file deep dive", detail.as_str()))
            });
        }
    };

    support::html_markup_response(render_file_deep_dive(&data))
}

fn render_file_deep_dive(data: &FileDeepDiveData) -> maud::Markup {
    html! {
        div class="space-y-4" data-page="file-deep-dive" {
            (support::explanation_header(
                "File Deep Dive",
                "This explains what this file is for, how its components work together, and how it connects to the rest of the project.",
                "Use this before editing a file to understand internal roles and downstream impact.",
                "Narrative file report composed from in-file symbol structure and project dependency graph."
            ))

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-6 space-y-3" {
                div class="flex flex-wrap items-center justify-between gap-3" {
                    h1 class="text-xl font-semibold font-mono break-all" { (data.path.as_str()) }
                    div class="flex flex-wrap items-center gap-2 text-xs" {
                        span class="badge badge-muted" { (data.layer_icon.as_str()) " " (data.layer.as_str()) }
                        span class="badge badge-cyan" { (data.symbol_count) " components" }
                    }
                }
                p class="text-sm text-text-secondary" { (data.summary.as_str()) }
            }

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-2" {
                h2 class="text-lg font-semibold" { "How This File Works" }
                p class="text-sm text-text-secondary" { (data.internal_narrative.as_str()) }
            }

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                h2 class="text-lg font-semibold" { "How This File Connects" }
                p class="text-sm text-text-secondary" { (data.external_narrative.as_str()) }

                div class="grid gap-3 md:grid-cols-2" {
                    div class="rounded-lg border border-surface-3/30 p-3 space-y-2" {
                        h3 class="text-sm font-semibold" { "Depended on by" }
                        @if data.connections_to_project.depended_on_by.is_empty() {
                            p class="text-xs text-text-muted" { "None" }
                        } @else {
                            ul class="space-y-1 text-xs" {
                                @for file_path in &data.connections_to_project.depended_on_by {
                                    li {
                                        span class="file-link text-blue-600 hover:underline cursor-pointer font-mono"
                                            data-path=(file_path.as_str()) {
                                            (file_path.as_str())
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div class="rounded-lg border border-surface-3/30 p-3 space-y-2" {
                        h3 class="text-sm font-semibold" { "Depends on" }
                        @if data.connections_to_project.depends_on.is_empty() {
                            p class="text-xs text-text-muted" { "None" }
                        } @else {
                            ul class="space-y-1 text-xs" {
                                @for dep_name in &data.connections_to_project.depends_on {
                                    li {
                                        span class="symbol-link text-blue-600 hover:underline cursor-pointer"
                                            data-symbol=(dep_name.as_str()) {
                                            (dep_name.as_str())
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                h2 class="text-lg font-semibold" { "All Components In This File" }
                @if data.symbols.is_empty() {
                    (support::html_empty_state("No symbols indexed for this file", None))
                } @else {
                    div class="space-y-3" {
                        @for symbol in &data.symbols {
                            article class="rounded-lg border border-surface-3/30 p-4 space-y-2" {
                                div class="flex flex-wrap items-center justify-between gap-2" {
                                    div class="flex flex-wrap items-center gap-2" {
                                        span class="symbol-link text-blue-600 hover:underline cursor-pointer text-lg font-semibold"
                                            data-symbol=(symbol.name.as_str()) {
                                            (symbol.name.as_str())
                                        }
                                        span class={ "badge " (support::badge_class_for_kind(symbol.kind.as_str())) } {
                                            (symbol.kind.as_str())
                                        }
                                        span class="badge badge-muted" {
                                            (symbol.role_in_file.as_str())
                                        }
                                        @if symbol.centrality > 0.0 {
                                            span class="badge badge-yellow" {
                                                "centrality " (format!("{:.3}", symbol.centrality))
                                            }
                                        }
                                    }
                                    span class="badge badge-cyan" { (symbol.dependents_count) " dependents" }
                                }
                                p class="text-sm text-text-secondary" { (symbol.sir_intent.as_str()) }
                                @if !symbol.internal_connections.is_empty() {
                                    div class="text-xs text-text-secondary" {
                                        span class="font-semibold text-text-primary" { "Internal connections: " }
                                        @for (idx, conn) in symbol.internal_connections.iter().enumerate() {
                                            @if idx > 0 { " · " }
                                            span class="symbol-link text-blue-600 hover:underline cursor-pointer"
                                                data-symbol=(conn.as_str()) {
                                                (conn.as_str())
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
