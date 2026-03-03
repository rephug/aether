use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;

use crate::api::glossary::{self, GlossaryQuery};
use crate::support::{self, DashboardState};

pub(crate) async fn glossary_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<GlossaryQuery>,
) -> Html<String> {
    let shared = state.shared.clone();
    let query_for_build = query.clone();
    let data = match support::run_blocking_with_timeout(move || {
        glossary::build_glossary_data(shared.as_ref(), &query_for_build)
    })
    .await
    {
        Ok(data) => data,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load glossary", detail.as_str()))
            });
        }
    };

    let total_pages = if data.total == 0 {
        1
    } else {
        data.total.div_ceil(data.per_page)
    };

    support::html_markup_response(html! {
        div class="space-y-4" data-page="glossary" {
            (support::explanation_header(
                "Glossary",
                "This dictionary is generated from SIR intent so every component has a plain-language definition.",
                "Filter by layer and kind to narrow to the parts you care about.",
                "Auto-generated term catalog with layer/kind filtering and pagination."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" { "📚 Glossary" }
                span class="badge badge-cyan" { (data.total) " terms" }
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                input
                    type="text"
                    name="search"
                    value=(query.search.clone().unwrap_or_default())
                    placeholder="Search terms, files, or definitions..."
                    class="w-full rounded-md border border-surface-3/50 bg-surface-0/60 px-3 py-2 text-sm"
                    hx-get="/dashboard/frag/glossary"
                    hx-trigger="input changed delay:300ms"
                    hx-target="#main-content"
                    hx-push-url="true"
                    hx-include="#glossary-controls";

                div id="glossary-controls" class="grid gap-3 md:grid-cols-2" {
                    div class="space-y-2" {
                        div class="text-xs uppercase tracking-wider text-text-muted" { "Layer" }
                        div class="flex flex-wrap gap-2" {
                            @for layer in ["", "Interface", "Core Logic", "Data", "Wire Format", "Connectors", "Tests", "Utilities"] {
                                button
                                    class={
                                        "px-2 py-1 rounded-md border text-xs "
                                        (if query.layer.as_deref().unwrap_or_default().eq_ignore_ascii_case(layer) {
                                            "border-accent-cyan/60 bg-accent-cyan/10"
                                        } else {
                                            "border-surface-3/50 hover:bg-surface-3/30"
                                        })
                                    }
                                    hx-get=(build_query_url(
                                        query.search.as_deref(),
                                        (!layer.is_empty()).then_some(layer),
                                        query.kind.as_deref(),
                                        Some(1),
                                        Some(data.per_page),
                                    ))
                                    hx-target="#main-content"
                                    hx-push-url="true" {
                                    (if layer.is_empty() { "All" } else { layer })
                                }
                            }
                        }
                    }

                    div class="space-y-2" {
                        div class="text-xs uppercase tracking-wider text-text-muted" { "Kind" }
                        div class="flex flex-wrap gap-2" {
                            @for kind in ["", "struct", "enum", "trait", "function", "method", "module"] {
                                button
                                    class={
                                        "px-2 py-1 rounded-md border text-xs "
                                        (if query.kind.as_deref().unwrap_or_default().eq_ignore_ascii_case(kind) {
                                            "border-accent-cyan/60 bg-accent-cyan/10"
                                        } else {
                                            "border-surface-3/50 hover:bg-surface-3/30"
                                        })
                                    }
                                    hx-get=(build_query_url(
                                        query.search.as_deref(),
                                        query.layer.as_deref(),
                                        (!kind.is_empty()).then_some(kind),
                                        Some(1),
                                        Some(data.per_page),
                                    ))
                                    hx-target="#main-content"
                                    hx-push-url="true" {
                                    (if kind.is_empty() { "All" } else { kind })
                                }
                            }
                        }
                    }
                }
            }

            @if data.terms.is_empty() {
                (support::html_empty_state("No glossary terms matched this filter", None))
            } @else {
                div class="space-y-3" {
                    @for term in &data.terms {
                        article class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                            div class="flex flex-wrap items-start justify-between gap-3" {
                                div class="space-y-1" {
                                    h3 class="text-lg font-semibold" {
                                        span class="symbol-link text-blue-600 hover:underline cursor-pointer"
                                            data-symbol=(term.name.as_str()) {
                                            (term.name.as_str())
                                        }
                                    }
                                    div class="flex flex-wrap items-center gap-2" {
                                        span class="badge badge-cyan" { (term.kind.as_str()) }
                                        span class="badge badge-muted" { (term.layer_icon.as_str()) " " (term.layer.as_str()) }
                                        span class="badge badge-yellow" { (term.dependents_count) " dependents" }
                                        span
                                            class={ "badge " (difficulty_badge_class(term.difficulty.label.as_str())) }
                                            data-tippy-content=(difficulty_tooltip(term)) {
                                            (term.difficulty.emoji.as_str()) " " (term.difficulty.label.as_str())
                                        }
                                    }
                                    span class="file-link text-blue-600 hover:underline cursor-pointer font-mono text-xs"
                                        data-path=(term.file.as_str()) {
                                        (term.file.as_str())
                                    }
                                }
                                div class="flex items-center gap-2" {
                                    button
                                        class="px-2 py-1 text-xs rounded-md border border-surface-3/40 hover:bg-surface-3/20"
                                        hx-get={"/dashboard/frag/spec/" (support::percent_encode(term.name.as_str()))}
                                        hx-target="#main-content"
                                        hx-push-url={"/dashboard/spec/" (support::percent_encode(term.name.as_str()))} {
                                        "📋 Spec"
                                    }
                                    button
                                        class="px-2 py-1 text-xs rounded-md border border-surface-3/40 hover:bg-surface-3/20"
                                        hx-get={"/dashboard/frag/autopsy/" (support::percent_encode(term.name.as_str()))}
                                        hx-target="#main-content"
                                        hx-push-url={"/dashboard/autopsy/" (support::percent_encode(term.name.as_str()))} {
                                        "🎓 Advisor"
                                    }
                                }
                            }

                            p class="text-sm text-text-secondary" { (term.definition.as_str()) }

                            @if !term.related.is_empty() {
                                div class="text-xs text-text-secondary" {
                                    span class="font-semibold text-text-primary" { "Related: " }
                                    @for (idx, related) in term.related.iter().enumerate() {
                                        @if idx > 0 { " · " }
                                        span class="symbol-link text-blue-600 hover:underline cursor-pointer"
                                            data-symbol=(related.as_str()) {
                                            (related.as_str())
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            @if total_pages > 1 {
                div class="flex items-center justify-between gap-3 rounded-lg border border-surface-3/40 bg-surface-1/40 p-3 text-sm" {
                    span { "Page " (data.page) " of " (total_pages) }
                    div class="flex items-center gap-2" {
                        @if data.page > 1 {
                            button
                                class="px-3 py-1 rounded-md border border-surface-3/50 hover:bg-surface-3/30"
                                hx-get=(build_query_url(
                                    query.search.as_deref(),
                                    query.layer.as_deref(),
                                    query.kind.as_deref(),
                                    Some(data.page - 1),
                                    Some(data.per_page),
                                ))
                                hx-target="#main-content"
                                hx-push-url="true" {
                                "Previous"
                            }
                        }

                        @if data.page < total_pages {
                            button
                                class="px-3 py-1 rounded-md border border-surface-3/50 hover:bg-surface-3/30"
                                hx-get=(build_query_url(
                                    query.search.as_deref(),
                                    query.layer.as_deref(),
                                    query.kind.as_deref(),
                                    Some(data.page + 1),
                                    Some(data.per_page),
                                ))
                                hx-target="#main-content"
                                hx-push-url="true" {
                                "Next"
                            }
                        }
                    }
                }
            }
        }
    })
}

fn difficulty_tooltip(term: &crate::api::glossary::GlossaryTerm) -> String {
    if term.difficulty.reasons.is_empty() {
        term.difficulty.guidance.clone()
    } else {
        format!(
            "{} - {}",
            term.difficulty.reasons.join(" | "),
            term.difficulty.guidance
        )
    }
}

fn difficulty_badge_class(label: &str) -> &'static str {
    if label.eq_ignore_ascii_case("easy") {
        "badge-green"
    } else if label.eq_ignore_ascii_case("moderate") {
        "badge-yellow"
    } else {
        "badge-red"
    }
}

fn build_query_url(
    search: Option<&str>,
    layer: Option<&str>,
    kind: Option<&str>,
    page: Option<usize>,
    per_page: Option<usize>,
) -> String {
    let mut params = Vec::<String>::new();

    if let Some(search) = search.map(str::trim).filter(|value| !value.is_empty()) {
        params.push(format!("search={}", support::percent_encode(search)));
    }
    if let Some(layer) = layer.map(str::trim).filter(|value| !value.is_empty()) {
        params.push(format!("layer={}", support::percent_encode(layer)));
    }
    if let Some(kind) = kind.map(str::trim).filter(|value| !value.is_empty()) {
        params.push(format!("kind={}", support::percent_encode(kind)));
    }
    if let Some(page) = page {
        params.push(format!("page={page}"));
    }
    if let Some(per_page) = per_page {
        params.push(format!("per_page={per_page}"));
    }

    if params.is_empty() {
        "/dashboard/frag/glossary".to_owned()
    } else {
        format!("/dashboard/frag/glossary?{}", params.join("&"))
    }
}
