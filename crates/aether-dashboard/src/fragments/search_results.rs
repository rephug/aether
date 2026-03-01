use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use aether_store::Store;

use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SearchFragmentQuery {
    pub q: Option<String>,
    pub mode: Option<String>,
    pub lang: Option<String>,
    pub risk: Option<String>,
    pub drift: Option<String>,
    pub has_tests: Option<bool>,
}

pub(crate) async fn search_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<SearchFragmentQuery>,
) -> Html<String> {
    let q = query.q.unwrap_or_default();
    let trimmed = q.trim().to_owned();
    let mode = query.mode.unwrap_or_else(|| "hybrid".to_owned());

    if trimmed.is_empty() {
        return support::html_markup_response(html! {
            div class="space-y-4" data-page="search" {
                h2 class="text-lg font-semibold" { "Smart Search" }
                (support::html_empty_state("Type in the search box to find symbols", Some("/ to focus search")))
            }
        });
    }

    let mut endpoint = format!("/api/v1/search?q={}&mode={}", trimmed, mode);
    if let Some(lang) = query.lang.as_deref().filter(|v| !v.trim().is_empty()) {
        endpoint.push_str(format!("&lang={}", lang).as_str());
    }
    if let Some(risk) = query.risk.as_deref().filter(|v| !v.trim().is_empty()) {
        endpoint.push_str(format!("&risk={}", risk).as_str());
    }
    if let Some(drift) = query.drift.as_deref().filter(|v| !v.trim().is_empty()) {
        endpoint.push_str(format!("&drift={}", drift).as_str());
    }
    if let Some(has_tests) = query.has_tests {
        endpoint.push_str(format!("&has_tests={has_tests}").as_str());
    }

    let results = state
        .shared
        .store
        .search_symbols(&trimmed, 50)
        .unwrap_or_default();

    support::html_markup_response(html! {
        div class="space-y-4" data-page="search" data-query=(trimmed.clone()) {
            div class="flex flex-wrap items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" { "Smart Search" }
                span class="badge badge-muted" id="search-result-count" { (results.len()) " results" }
            }

            div class="grid gap-4 xl:grid-cols-[220px_minmax(0,1fr)]" {
                aside class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3 text-xs" {
                    div class="space-y-2" {
                        div class="font-semibold text-text-primary" { "Mode" }
                        div class="flex gap-2" {
                            @for m in ["lexical", "semantic", "hybrid"] {
                                button
                                    class={"px-2 py-1 rounded-md border border-surface-3/50 hover:bg-surface-3/40 " (if mode.eq_ignore_ascii_case(m) { "bg-surface-3/40" } else { "" })}
                                    hx-get={"/dashboard/frag/search?q=" (trimmed.as_str()) "&mode=" (m)}
                                    hx-target="#main-content" {
                                    (m)
                                }
                            }
                        }
                    }

                    div class="space-y-2" {
                        div class="font-semibold text-text-primary" { "Filters" }
                        form id="search-filter-form" class="space-y-2"
                            hx-get="/dashboard/frag/search"
                            hx-target="#main-content"
                            hx-trigger="change"
                            hx-include="this" {
                            input type="hidden" name="q" value=(trimmed);
                            input type="hidden" name="mode" value=(mode.clone());

                            label class="block" {
                                span class="text-text-muted" { "Language" }
                                select name="lang" class="mt-1 w-full px-2 py-1 rounded-md bg-surface-0/60 border border-surface-3/50" {
                                    option value="" { "Any" }
                                    option value="rust" selected[query.lang.as_deref() == Some("rust")] { "Rust" }
                                    option value="python" selected[query.lang.as_deref() == Some("python")] { "Python" }
                                    option value="typescript" selected[query.lang.as_deref() == Some("typescript")] { "TypeScript" }
                                }
                            }

                            label class="block" {
                                span class="text-text-muted" { "Risk" }
                                select name="risk" class="mt-1 w-full px-2 py-1 rounded-md bg-surface-0/60 border border-surface-3/50" {
                                    option value="" { "Any" }
                                    option value="high" selected[query.risk.as_deref() == Some("high")] { "High" }
                                    option value="medium" selected[query.risk.as_deref() == Some("medium")] { "Medium" }
                                    option value="low" selected[query.risk.as_deref() == Some("low")] { "Low" }
                                }
                            }

                            label class="block" {
                                span class="text-text-muted" { "Drift" }
                                select name="drift" class="mt-1 w-full px-2 py-1 rounded-md bg-surface-0/60 border border-surface-3/50" {
                                    option value="" { "Any" }
                                    option value="drifting" selected[query.drift.as_deref() == Some("drifting")] { "Drifting" }
                                    option value="stable" selected[query.drift.as_deref() == Some("stable")] { "Stable" }
                                }
                            }

                            label class="inline-flex items-center gap-2" {
                                input type="checkbox" name="has_tests" value="true" checked[query.has_tests == Some(true)];
                                "Has tests"
                            }
                        }
                    }
                }

                section class="space-y-3" id="smart-search-results" data-endpoint=(endpoint) {
                    @if results.is_empty() {
                        (support::html_empty_state("No symbols matched your query", None))
                    } @else {
                        @for row in results {
                            @let sir_excerpt = support::sir_excerpt_for_symbol(state.shared.as_ref(), row.symbol_id.as_str());
                            article
                                class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 hover:border-accent-cyan/30 transition-colors cursor-pointer"
                                tabindex="0"
                                data-symbol-id=(row.symbol_id.clone())
                                hx-get={"/dashboard/frag/blast-radius?symbol_id=" (row.symbol_id)}
                                hx-target="#main-content" {
                                div class="flex flex-wrap items-start justify-between gap-3" {
                                    div class="space-y-1" {
                                        h3 class="font-mono text-sm text-text-primary" { (row.qualified_name.clone()) }
                                        div class="text-xs text-text-muted font-mono" { (support::normalized_display_path(row.file_path.as_str())) }
                                    }
                                    div class="flex flex-wrap gap-2" {
                                        span class={"badge " (support::badge_class_for_kind(row.kind.as_str()))} { (row.kind) }
                                        span class={"badge " (support::badge_class_for_language(row.language.as_str()))} { (row.language) }
                                    }
                                }

                                @if let Some(excerpt) = sir_excerpt {
                                    p class="mt-3 text-xs text-text-secondary" { (excerpt) }
                                }

                                div class="mt-3 flex flex-wrap gap-2 text-xs" {
                                    span class="badge badge-muted" { "Risk: loading" }
                                    span class="badge badge-muted" { "PageRank: loading" }
                                    span class="badge badge-muted" { "Drift: loading" }
                                    span class="badge badge-muted" { "Tests: loading" }
                                }

                                div class="mt-3 text-xs text-text-muted" { "Related: loading..." }
                            }
                        }
                    }
                }
            }
        }
    })
}
