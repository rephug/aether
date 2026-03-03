use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::api::anatomy;
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct AnatomyLayerQuery {
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct AnatomyFileQuery {
    pub path: Option<String>,
}

pub(crate) async fn anatomy_fragment(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let shared = state.shared.clone();
    let build = match support::run_blocking_with_timeout(move || {
        anatomy::load_anatomy_build(shared.as_ref())
    })
    .await
    {
        Ok(build) => build,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load anatomy", detail.as_str()))
            });
        }
    };

    support::html_markup_response(html! {
        div class="space-y-5" data-page="anatomy" {
            (support::explanation_header(
                "Project Anatomy",
                "Anatomy is the ingredients list for this codebase: layers, key actors, and how they connect.",
                "Use this page to map layers, central components, and file-level responsibilities.",
                "Layer decomposition, centrality actors, and aggregated dependency view."
            ))

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                div class="flex flex-wrap items-center justify-between gap-2" {
                    h2 class="text-xl font-semibold" { (build.data.project_name.as_str()) }
                    span class="badge badge-orange" {
                        (build.data.maturity.icon.as_str()) " "
                        (build.data.maturity.dominant_phase.as_str()) " Project"
                    }
                }
                p class="text-sm text-text-secondary" { (build.data.summary.as_str()) }
                p class="text-xs text-text-muted" { (build.data.maturity.description.as_str()) }
            }

            div class="space-y-3" {
                h3 class="text-base font-semibold" { "Tech Stack" }
                @if build.data.tech_stack.is_empty() {
                    (support::html_empty_state("No manifest dependencies detected", None))
                } @else {
                    div class="grid gap-3 md:grid-cols-2 xl:grid-cols-3" {
                        @for category in &build.data.tech_stack {
                            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                                h4 class="text-sm font-semibold" { (category.category.as_str()) }
                                ul class="space-y-1 text-xs text-text-secondary" {
                                    @for item in &category.items {
                                        li {
                                            span class="font-mono text-text-primary" { (item.name.as_str()) }
                                            " - "
                                            (item.description.as_str())
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div class="space-y-3" {
                div class="flex items-center justify-between gap-2" {
                    h3 class="text-base font-semibold" { "Project Layers" }
                    span class="badge badge-muted" { (build.data.layers.len()) " layers" }
                }

                @if build.data.layers.is_empty() {
                    (support::html_empty_state("No indexed layers found", Some("aether index")))
                } @else {
                    div class="space-y-2" {
                        @for layer in &build.data.layers {
                            div class="rounded-lg border border-surface-3/40 bg-surface-1/40 p-4" {
                                div class="flex flex-wrap items-center justify-between gap-2" {
                                    div class="space-y-1" {
                                        div class="text-sm font-semibold" {
                                            (layer.icon.as_str()) " " (layer.name.as_str())
                                        }
                                        p class="text-xs text-text-secondary" { (layer.description.as_str()) }
                                    }
                                    div class="flex items-center gap-2" {
                                        span class="badge badge-cyan" { (layer.total_symbol_count) " components" }
                                        button
                                            class="px-2 py-1 text-xs rounded-md border border-surface-3/50 hover:bg-surface-3/40"
                                            hx-get={"/dashboard/frag/anatomy/layer?name=" (percent_encode(layer.name.as_str()))}
                                            hx-target="#anatomy-layer-detail" {
                                            "Show files"
                                        }
                                    }
                                }
                                p class="mt-3 text-sm text-text-secondary" { (layer.narrative.as_str()) }
                            }
                        }
                    }
                }

                div id="anatomy-layer-detail" class="space-y-3" {}
                div id="anatomy-file-detail" class="space-y-3" {}
            }

            div class="space-y-3" {
                h3 class="text-base font-semibold" { "Key Actors" }
                @if build.data.key_actors.is_empty() {
                    (support::html_empty_state("No key actors available", Some("aether index")))
                } @else {
                    table class="data-table" {
                        thead {
                            tr {
                                th { "Name" }
                                th { "Layer" }
                                th { "Centrality" }
                                th { "Dependents" }
                                th { "Difficulty" }
                                th { "Description" }
                            }
                        }
                        tbody {
                            @for actor in &build.data.key_actors {
                                tr {
                                    td {
                                        span class="symbol-link text-blue-600 hover:underline cursor-pointer" data-symbol=(actor.name.as_str()) {
                                            (actor.name.as_str())
                                        }
                                        span class="text-xs text-text-muted ml-2" { "(" (actor.kind.as_str()) ")" }
                                    }
                                    td { (actor.layer.as_str()) }
                                    td class="font-mono" { (format!("{:.3}", actor.centrality)) }
                                    td class="font-mono" { (actor.dependents_count) }
                                    td {
                                        span class={ "badge " (difficulty_badge_class(actor.difficulty.label.as_str())) }
                                            data-tippy-content=(if actor.difficulty.reasons.is_empty() {
                                                actor.difficulty.guidance.clone()
                                            } else {
                                                actor.difficulty.reasons.join(" | ")
                                            }) {
                                            (actor.difficulty.emoji.as_str()) " " (actor.difficulty.label.as_str())
                                        }
                                    }
                                    td class="text-xs" {
                                        @if actor.description.trim().is_empty() {
                                            "No SIR intent summary available"
                                        } @else {
                                            (actor.description.as_str())
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div class="space-y-3" {
                h3 class="text-base font-semibold" { "Simplified Layer Graph" }
                div id="anatomy-layer-graph" class="chart-container min-h-[420px]" {}
            }
        }
    })
}

pub(crate) async fn anatomy_layer_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<AnatomyLayerQuery>,
) -> Html<String> {
    let Some(layer_name) = query
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return support::html_markup_response(html! {
            (support::html_error_state("Layer name is required", "Pass ?name=LayerName"))
        });
    };

    let shared = state.shared.clone();
    let build = match support::run_blocking_with_timeout(move || {
        anatomy::load_anatomy_build(shared.as_ref())
    })
    .await
    {
        Ok(build) => build,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load layer detail", detail.as_str()))
            });
        }
    };

    let key = layer_name.to_ascii_lowercase();
    let Some(detail) = build.layers_by_name.get(key.as_str()) else {
        return support::html_markup_response(html! {
            (support::html_error_state("Layer not found", layer_name))
        });
    };

    support::html_markup_response(html! {
        div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
            h4 class="text-sm font-semibold" {
                (detail.layer.icon.as_str()) " " (detail.layer.name.as_str()) " files"
            }
            p class="text-sm text-text-secondary" { (detail.layer.narrative.as_str()) }

            @if detail.layer.files.is_empty() {
                (support::html_empty_state("No files detected for this layer", None))
            } @else {
                div class="space-y-2" {
                    @for file in &detail.layer.files {
                        div class="rounded-lg border border-surface-3/40 bg-surface-0/80 p-3 space-y-2" {
                            div class="flex items-center justify-between gap-2" {
                                div class="file-link text-blue-600 hover:underline cursor-pointer font-mono text-xs text-text-primary"
                                    data-path=(file.path.as_str()) {
                                    (file.path.as_str())
                                }
                                button
                                    class="px-2 py-1 text-xs rounded-md border border-surface-3/50 hover:bg-surface-3/40"
                                    hx-get={"/dashboard/frag/anatomy/file?path=" (percent_encode(file.path.as_str()))}
                                    hx-target="#anatomy-file-detail" {
                                    "Show symbols"
                                }
                            }
                            p class="text-xs text-text-secondary" { (file.summary.as_str()) }
                            div class="text-[11px] text-text-muted" { (file.symbol_count) " components" }
                        }
                    }
                }
            }
        }
    })
}

pub(crate) async fn anatomy_file_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<AnatomyFileQuery>,
) -> Html<String> {
    let Some(path) = query
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return support::html_markup_response(html! {
            (support::html_error_state("File path is required", "Pass ?path=src/file.rs"))
        });
    };

    let shared = state.shared.clone();
    let build = match support::run_blocking_with_timeout(move || {
        anatomy::load_anatomy_build(shared.as_ref())
    })
    .await
    {
        Ok(build) => build,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load file detail", detail.as_str()))
            });
        }
    };

    let normalized = support::normalized_display_path(path);
    let Some(detail) = build.files_by_path.get(normalized.as_str()) else {
        return support::html_markup_response(html! {
            (support::html_error_state("File not found in anatomy view", normalized.as_str()))
        });
    };

    support::html_markup_response(html! {
        div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
            div class="flex items-center justify-between gap-2" {
                h4 class="file-link text-blue-600 hover:underline cursor-pointer text-sm font-semibold font-mono"
                    data-path=(detail.path.as_str()) {
                    (detail.path.as_str())
                }
                span class="badge badge-muted" { (detail.layer_name.as_str()) }
            }
            p class="text-sm text-text-secondary" { (detail.summary.as_str()) }

            @if detail.symbols.is_empty() {
                (support::html_empty_state("No symbols indexed for this file", None))
            } @else {
                table class="data-table" {
                    thead {
                        tr {
                            th { "Symbol" }
                            th { "Kind" }
                            th { "SIR Intent" }
                        }
                    }
                    tbody {
                        @for symbol in &detail.symbols {
                            tr {
                                td {
                                    span class="symbol-link text-blue-600 hover:underline cursor-pointer" data-symbol=(symbol.name.as_str()) {
                                        (symbol.name.as_str())
                                    }
                                    div class="text-[11px] text-text-muted font-mono" {
                                        (symbol.qualified_name.as_str())
                                    }
                                }
                                td { (symbol.kind.as_str()) }
                                td class="text-xs" {
                                    @if symbol.sir_intent.trim().is_empty() {
                                        "No SIR intent available"
                                    } @else {
                                        (symbol.sir_intent.as_str())
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

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(format!("{byte:02X}").as_str());
        }
    }
    out
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
