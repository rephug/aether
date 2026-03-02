use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Html;
use maud::{Markup, html};

use aether_store::Store;

use crate::support::{self, DashboardState};

pub(crate) async fn symbol_detail_fragment(
    State(state): State<Arc<DashboardState>>,
    Path(symbol_id): Path<String>,
) -> Html<String> {
    let Some(symbol) = (match state.shared.store.get_symbol_record(&symbol_id) {
        Ok(symbol) => symbol,
        Err(err) => {
            return support::html_markup_response(
                html! { (support::html_error_state("Failed to load symbol", &err.to_string())) },
            );
        }
    }) else {
        return support::html_markup_response(
            html! { (support::html_empty_state("Symbol not found", None)) },
        );
    };

    let sir_meta = state.shared.store.get_sir_meta(&symbol.id).ok().flatten();
    let sir_fields = support::sir_key_values_for_symbol(state.shared.as_ref(), &symbol.id);
    let deps_shared = state.shared.clone();
    let dep_symbol_id = symbol.id.clone();
    let deps = support::run_async_with_timeout(move || async move {
        deps_shared
            .graph
            .get_dependencies(&dep_symbol_id)
            .await
            .map_err(|err| err.to_string())
    })
    .await
    .unwrap_or_default();

    let callers_shared = state.shared.clone();
    let qualified_name = symbol.qualified_name.clone();
    let callers = support::run_async_with_timeout(move || async move {
        callers_shared
            .graph
            .get_callers(&qualified_name)
            .await
            .map_err(|err| err.to_string())
    })
    .await
    .unwrap_or_default();

    support::html_markup_response(html! {
        div class="p-4 space-y-4" {
            div class="flex items-start justify-between gap-3" {
                div {
                    h2 class="text-base font-semibold" { (support::symbol_name_from_qualified(&symbol.qualified_name)) }
                    div class="text-xs text-text-muted font-mono break-all" { (symbol.qualified_name) }
                }
                button onclick="closeDetailPanel()"
                    class="px-2 py-1 rounded-md text-xs border border-surface-3/40 hover:bg-surface-3/40" {
                    "Close"
                }
            }

            div class="flex flex-wrap gap-2" {
                span class={ "badge " (support::badge_class_for_kind(&symbol.kind)) } { (symbol.kind) }
                span class={ "badge " (support::badge_class_for_language(&symbol.language)) } { (symbol.language) }
                @if let Some(meta) = &sir_meta {
                    span class={ "badge " (if meta.sir_status.eq_ignore_ascii_case("ready") { "badge-green" } else { "badge-yellow" }) } {
                        "SIR " (meta.sir_status)
                    }
                } @else {
                    span class="badge badge-muted" { "SIR missing" }
                }
            }

            div class="rounded-lg border border-surface-3/40 bg-surface-1/30 p-3 text-sm space-y-2" {
                div {
                    div class="text-xs uppercase tracking-wider text-text-muted mb-1" { "File" }
                    div class="font-mono text-xs break-all" { (support::normalized_display_path(&symbol.file_path)) }
                }
                div {
                    div class="text-xs uppercase tracking-wider text-text-muted mb-1" { "Symbol ID" }
                    div class="font-mono text-xs break-all" { (symbol.id) }
                }
                div {
                    div class="text-xs uppercase tracking-wider text-text-muted mb-1" { "Last Seen" }
                    div class="font-mono text-xs" { (symbol.last_seen_at) }
                }
            }

            div class="space-y-2" {
                h3 class="text-sm font-semibold" { "SIR" }
                @if sir_fields.is_empty() {
                    (support::html_empty_state("No SIR available for this symbol", Some("aether index")))
                } @else {
                    @for (key, value) in sir_fields {
                        div class="sir-block" {
                            span class="sir-field-label" { (titlecase(&key)) ":" }
                            span class="sir-field-value" { (value) }
                        }
                    }
                }
            }

            div class="space-y-3" {
                h3 class="text-sm font-semibold" { "Dependencies" }
                (symbol_list("None", &deps))
            }

            div class="space-y-3" {
                h3 class="text-sm font-semibold" { "Callers" }
                (symbol_list("None", &callers))
            }
        }
    })
}

fn titlecase(input: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for ch in input.chars() {
        if matches!(ch, '_' | '-') {
            out.push(' ');
            upper = true;
            continue;
        }
        if upper {
            for uc in ch.to_uppercase() {
                out.push(uc);
            }
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn symbol_list(empty: &str, items: &[aether_store::SymbolRecord]) -> Markup {
    if items.is_empty() {
        return html! { div class="text-xs text-text-muted" { (empty) } };
    }

    html! {
        table class="data-table" {
            thead {
                tr {
                    th { "Symbol" }
                    th { "Kind" }
                    th { "File" }
                }
            }
            tbody {
                @for item in items {
                    tr {
                        td {
                            div class="font-medium" { (support::symbol_name_from_qualified(&item.qualified_name)) }
                            div class="text-xs text-text-muted font-mono" { (&item.qualified_name) }
                        }
                        td { span class={ "badge " (support::badge_class_for_kind(&item.kind)) } { (&item.kind) } }
                        td class="font-mono text-xs" { (support::normalized_display_path(&item.file_path)) }
                    }
                }
            }
        }
    }
}
