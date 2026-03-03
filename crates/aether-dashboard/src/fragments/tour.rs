use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Html;
use maud::{Markup, html};
use serde::Deserialize;

use crate::api::tour::{self, TourData, TourStop};
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TourQuery {
    pub stop: Option<usize>,
}

pub(crate) async fn tour_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<TourQuery>,
    headers: HeaderMap,
) -> Html<String> {
    let shared = state.shared.clone();
    let data =
        match support::run_blocking_with_timeout(move || tour::build_tour_data(shared.as_ref()))
            .await
        {
            Ok(data) => data,
            Err(err) => {
                let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
                return support::html_markup_response(html! {
                    (support::html_error_state("Failed to load guided tour", detail.as_str()))
                });
            }
        };

    let active_stop_index = selected_stop_index(&data, query.stop);
    let active_stop = data.stops.get(active_stop_index).cloned();

    let hx_target = headers
        .get("HX-Target")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if hx_target == "tour-content" {
        return support::html_markup_response(render_tour_content(
            active_stop.as_ref(),
            active_stop_index,
            data.stop_count,
        ));
    }

    support::html_markup_response(html! {
        div class="space-y-4" data-page="tour" {
            (support::explanation_header(
                "Guided Tour",
                "Walk the project like a museum tour: each stop explains a major area in plain English.",
                "Stops are generated from the layers and symbols currently indexed.",
                "Dynamic narrative tour derived from layer assignments and SIR data."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" { "🗺️ Guided Tour" }
                span class="badge badge-cyan" { (data.stop_count) " stops" }
            }

            @if !data.skipped_stops.is_empty() {
                div class="rounded-lg border border-surface-3/40 bg-surface-1/40 p-3 text-xs text-text-muted" {
                    "Skipped for this project: " (data.skipped_stops.join(", "))
                }
            }

            div class="grid gap-4 lg:grid-cols-[280px_minmax(0,1fr)]" {
                aside class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3 space-y-2" {
                    @for stop in &data.stops {
                        button
                            class={
                                "w-full rounded-lg border px-3 py-2 text-left text-sm transition-colors "
                                (if stop.number == active_stop_index + 1 {
                                    "border-accent-cyan/60 bg-accent-cyan/10"
                                } else {
                                    "border-surface-3/40 hover:bg-surface-3/20"
                                })
                            }
                            hx-get={"/dashboard/frag/tour?stop=" (stop.number)}
                            hx-target="#tour-content"
                            hx-push-url={"/dashboard/tour?stop=" (stop.number)} {
                            div class="flex items-center gap-2" {
                                span class="inline-flex h-6 w-6 items-center justify-center rounded-full bg-surface-3/50 text-xs font-semibold" {
                                    (stop.number)
                                }
                                div class="min-w-0" {
                                    div class="truncate font-medium" { (stop.title.as_str()) }
                                    div class="truncate text-xs text-text-muted" { (stop.subtitle.as_str()) }
                                }
                            }
                        }
                    }
                }

                div id="tour-content" class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    (render_tour_content(active_stop.as_ref(), active_stop_index, data.stop_count))
                }
            }
        }
    })
}

fn selected_stop_index(data: &TourData, selected_stop: Option<usize>) -> usize {
    if data.stops.is_empty() {
        return 0;
    }

    let requested = selected_stop.unwrap_or(1);
    requested.saturating_sub(1).min(data.stops.len() - 1)
}

fn render_tour_content(stop: Option<&TourStop>, stop_index: usize, stop_count: usize) -> Markup {
    let Some(stop) = stop else {
        return html! {
            (support::html_empty_state("No tour stops available", Some("aether index")))
        };
    };

    let previous = (stop_index > 0).then_some(stop_index);
    let next = (stop_index + 1 < stop_count).then_some(stop_index + 2);

    html! {
        div class="space-y-4" {
            div class="flex items-start justify-between gap-3" {
                div class="space-y-1" {
                    div class="flex items-center gap-2" {
                        span class="inline-flex h-8 w-8 items-center justify-center rounded-full bg-accent-cyan/15 text-sm font-semibold text-accent-cyan" {
                            (stop.number)
                        }
                        h3 class="text-xl font-semibold" { (stop.title.as_str()) }
                    }
                    p class="text-sm text-text-secondary" { (stop.subtitle.as_str()) }
                }
                span class="badge badge-muted" { (stop.layer.as_str()) }
            }

            div class="flex items-center gap-2 text-xs" {
                span class="badge badge-cyan" { (stop.symbol_count) " components" }
                span class="badge badge-muted" { (stop.file_count) " files" }
            }

            p class="text-sm text-text-secondary leading-relaxed" { (stop.description.as_str()) }

            div class="space-y-2" {
                h4 class="text-sm font-semibold" { "Components in this stop" }
                @if stop.symbols.is_empty() {
                    (support::html_empty_state("No symbols available for this stop", None))
                } @else {
                    ul class="space-y-2" {
                        @for symbol in &stop.symbols {
                            li class="rounded-lg border border-surface-3/40 bg-surface-0/70 p-3 space-y-1" {
                                div class="flex flex-wrap items-center justify-between gap-2" {
                                    span class="symbol-link text-blue-600 hover:underline cursor-pointer font-medium"
                                        data-symbol=(symbol.name.as_str()) {
                                        (symbol.name.as_str())
                                    }
                                    span class="file-link text-blue-600 hover:underline cursor-pointer font-mono text-xs"
                                        data-path=(symbol.file.as_str()) {
                                        (symbol.file.as_str())
                                    }
                                }
                                p class="text-xs text-text-secondary" { (symbol.sir_intent.as_str()) }
                            }
                        }
                    }
                }
            }

            div class="space-y-2" {
                div class="flex items-center gap-2" {
                    @for index in 0..stop_count {
                        div class={
                            "h-2 flex-1 rounded-full "
                            (if index <= stop_index { "bg-accent-cyan" } else { "bg-surface-3/40" })
                        } {}
                    }
                }
            }

            div class="flex items-center justify-between gap-2" {
                @if let Some(prev_number) = previous {
                    button
                        class="px-3 py-2 rounded-md border border-surface-3/50 hover:bg-surface-3/30 text-sm"
                        hx-get={"/dashboard/frag/tour?stop=" (prev_number)}
                        hx-target="#tour-content"
                        hx-push-url={"/dashboard/tour?stop=" (prev_number)} {
                        "Previous"
                    }
                } @else {
                    span class="text-xs text-text-muted" { "Start of tour" }
                }

                @if let Some(next_number) = next {
                    button
                        class="px-3 py-2 rounded-md border border-surface-3/50 hover:bg-surface-3/30 text-sm"
                        hx-get={"/dashboard/frag/tour?stop=" (next_number)}
                        hx-target="#tour-content"
                        hx-push-url={"/dashboard/tour?stop=" (next_number)} {
                        "Next"
                    }
                } @else {
                    span class="text-xs text-text-muted" { "End of tour" }
                }
            }
        }
    }
}
