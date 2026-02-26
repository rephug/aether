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
}

pub(crate) async fn search_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<SearchFragmentQuery>,
) -> Html<String> {
    let q = query.q.unwrap_or_default();
    let trimmed = q.trim().to_owned();

    if trimmed.is_empty() {
        return support::html_markup_response(html! {
            div class="space-y-4" {
                h2 class="text-lg font-semibold" { "Search" }
                (support::html_empty_state("Type in the search box to find symbols", Some("/ to focus search")))
            }
        });
    }

    let results = match state.shared.store.search_symbols(&trimmed, 50) {
        Ok(rows) => rows,
        Err(err) => {
            return support::html_markup_response(html! {
                div class="space-y-4" {
                    h2 class="text-lg font-semibold" { "Search" }
                    (support::html_error_state("Search failed", &err.to_string()))
                }
            });
        }
    };

    support::html_markup_response(html! {
        div class="space-y-4" {
            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" { "Search Results" }
                span class="badge badge-muted" { (results.len()) " matches" }
            }

            @if results.is_empty() {
                (support::html_empty_state("No symbols matched your query", None))
            } @else {
                table class="data-table" {
                    thead {
                        tr {
                            th { "Symbol" }
                            th { "Kind" }
                            th { "Language" }
                            th { "File" }
                            th { "SIR" }
                        }
                    }
                    tbody {
                        @for row in results {
                            @let sir_exists = support::sir_exists_for_symbol(state.shared.as_ref(), &row.symbol_id);
                            @let excerpt = support::sir_excerpt_for_symbol(state.shared.as_ref(), &row.symbol_id);
                            tr class="clickable"
                                hx-get={ "/dashboard/frag/symbol/" (row.symbol_id) }
                                hx-target="#detail-panel" {
                                td {
                                    div class="font-medium" { (support::symbol_name_from_qualified(&row.qualified_name)) }
                                    div class="text-xs text-text-muted font-mono" { (row.qualified_name) }
                                    @if let Some(excerpt) = excerpt {
                                        div class="text-xs text-text-secondary mt-1" { (excerpt) }
                                    }
                                }
                                td { span class={ "badge " (support::badge_class_for_kind(&row.kind)) } { (row.kind) } }
                                td { span class={ "badge " (support::badge_class_for_language(&row.language)) } { (row.language) } }
                                td class="font-mono text-xs" { (support::normalized_display_path(&row.file_path)) }
                                td {
                                    @if sir_exists {
                                        span class="badge badge-green" { "Yes" }
                                    } @else {
                                        span class="badge badge-muted" { "No" }
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
