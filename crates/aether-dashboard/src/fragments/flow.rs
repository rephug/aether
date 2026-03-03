use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;

use crate::api::catalog::load_symbol_catalog;
use crate::api::flow::{self, FlowBuildError, FlowData, FlowQuery};
use crate::support::{self, DashboardState};

pub(crate) async fn flow_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<FlowQuery>,
) -> Html<String> {
    let has_start = query
        .start
        .as_deref()
        .map(str::trim)
        .map(|value| !value.is_empty())
        .unwrap_or(false);

    let shared = state.shared.clone();
    let suggestions = support::run_blocking_with_timeout(move || flow_suggestions(shared.as_ref()))
        .await
        .unwrap_or_default();

    if !has_start {
        return support::html_markup_response(render_flow_builder(
            &query,
            suggestions.as_slice(),
            None,
        ));
    }

    let shared = state.shared.clone();
    let query_for_build = query.clone();
    let result = match support::run_blocking_with_timeout(move || {
        Ok(flow::build_flow_data(shared.as_ref(), &query_for_build))
    })
    .await
    {
        Ok(result) => result,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to trace flow", detail.as_str()))
            });
        }
    };

    match result {
        Ok(data) => support::html_markup_response(render_flow_page(
            &query,
            suggestions.as_slice(),
            Some(&data),
            None,
        )),
        Err(FlowBuildError::MissingStart) => {
            support::html_markup_response(render_flow_builder(&query, suggestions.as_slice(), None))
        }
        Err(FlowBuildError::NoConnection(message)) => support::html_markup_response(
            render_flow_page(&query, suggestions.as_slice(), None, Some(message)),
        ),
        Err(FlowBuildError::SymbolNotFound(name)) => {
            support::html_markup_response(render_flow_page(
                &query,
                suggestions.as_slice(),
                None,
                Some(format!("Symbol '{}' not found.", name)),
            ))
        }
        Err(FlowBuildError::Internal(err)) => support::html_markup_response(html! {
            (support::html_error_state("Failed to trace flow", err.as_str()))
        }),
    }
}

fn render_flow_builder(
    query: &FlowQuery,
    suggestions: &[(String, String)],
    error: Option<String>,
) -> maud::Markup {
    render_flow_page(query, suggestions, None, error)
}

fn render_flow_page(
    query: &FlowQuery,
    suggestions: &[(String, String)],
    data: Option<&FlowData>,
    error: Option<String>,
) -> maud::Markup {
    html! {
        div class="space-y-4" data-page="flow" {
            (support::explanation_header(
                "Flow Narrative",
                "Trace how data moves through the codebase step by step.",
                "Pick a start component and optionally an end component to get a narrated path.",
                "Dependency-path narrative timeline from entry point to downstream component."
            ))

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                h2 class="text-lg font-semibold" { "🔄 Trace Flow" }
                form class="grid gap-3 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] md:items-end"
                    hx-get="/dashboard/frag/flow"
                    hx-target="#main-content"
                    hx-push-url="true" {
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wide text-text-muted" { "Start from" }
                        input
                            type="text"
                            name="start"
                            value=(query.start.clone().unwrap_or_default())
                            placeholder="run, main, Command..."
                            class="w-full rounded-md border border-surface-3/50 bg-surface-0/60 px-3 py-2 text-sm";
                    }
                    label class="space-y-1" {
                        span class="text-xs uppercase tracking-wide text-text-muted" { "End at (optional)" }
                        input
                            type="text"
                            name="end"
                            value=(query.end.clone().unwrap_or_default())
                            placeholder="Db, parse, handler..."
                            class="w-full rounded-md border border-surface-3/50 bg-surface-0/60 px-3 py-2 text-sm";
                    }
                    button
                        type="submit"
                        class="px-4 py-2 rounded-md border border-surface-3/50 hover:bg-surface-3/30 text-sm" {
                        "Trace Flow"
                    }
                }

                @if !suggestions.is_empty() {
                    div class="text-xs text-text-secondary" {
                        span class="font-semibold text-text-primary" { "Try tracing from: " }
                        @for (idx, (name, desc)) in suggestions.iter().enumerate() {
                            @if idx > 0 { " · " }
                            a class="text-blue-600 hover:underline"
                                hx-get={"/dashboard/frag/flow?start=" (percent_encode(name.as_str()))}
                                hx-target="#main-content"
                                hx-push-url={"/dashboard/flow?start=" (percent_encode(name.as_str()))} {
                                (name.as_str()) " (" (desc.as_str()) ")"
                            }
                        }
                    }
                }
            }

            @if let Some(message) = error.as_deref() {
                div class="rounded-lg border border-accent-red/40 bg-accent-red/10 p-3 text-sm text-red-700" {
                    (message)
                }
            }

            @if let Some(flow) = data {
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    p class="text-sm text-text-secondary leading-relaxed" { (flow.summary.as_str()) }
                }

                div class="space-y-3" {
                    @for (idx, step) in flow.steps.iter().enumerate() {
                        article class={ "rounded-xl border-l-4 border border-surface-3/30 bg-surface-1/40 p-4 space-y-2 " (layer_border_class(step.layer.as_str())) } {
                            div class="flex flex-wrap items-center justify-between gap-2" {
                                div class="flex flex-wrap items-center gap-2" {
                                    span class="badge badge-cyan" { "Step " (step.number) }
                                    span class="symbol-link text-blue-600 hover:underline cursor-pointer text-lg font-semibold"
                                        data-symbol=(step.symbol.as_str()) {
                                        (step.symbol.as_str())
                                    }
                                    span class="badge badge-muted" { (step.layer_icon.as_str()) " " (step.layer.as_str()) }
                                }
                                span class="file-link text-blue-600 hover:underline cursor-pointer font-mono text-xs"
                                    data-path=(step.file.as_str()) {
                                    (step.file.as_str())
                                }
                            }
                            p class="text-sm text-text-secondary" { (step.narrative.as_str()) }
                            p class="text-xs italic text-text-muted" { (step.sir_intent.as_str()) }
                        }

                        @if idx + 1 < flow.steps.len() {
                            @if let Some(transition) = flow.steps[idx].transition.as_deref() {
                                div class="ml-4 border-l-2 border-surface-3/50 pl-4 py-1 text-xs text-text-secondary" {
                                    (transition)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn flow_suggestions(shared: &crate::state::SharedState) -> Result<Vec<(String, String)>, String> {
    let catalog = load_symbol_catalog(shared)?;
    let mut picks = catalog
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.layer_name == "Interface" && {
                let name = symbol.name.to_ascii_lowercase();
                let file = symbol.file_path.to_ascii_lowercase();
                name == "main"
                    || name == "run"
                    || name.contains("server")
                    || file.ends_with("/main.rs")
            }
        })
        .map(|symbol| {
            let desc = if symbol.name.eq_ignore_ascii_case("main") {
                "program start".to_owned()
            } else if symbol.name.eq_ignore_ascii_case("run") {
                "server entry".to_owned()
            } else {
                first_sentence_or_fallback(symbol.sir.intent.as_str())
            };
            (symbol.name.clone(), desc)
        })
        .collect::<Vec<_>>();

    picks.sort_by(|left, right| left.0.cmp(&right.0));
    picks.dedup_by(|left, right| left.0 == right.0);
    picks.truncate(4);

    if picks.is_empty() {
        picks.push(("main".to_owned(), "program start".to_owned()));
    }

    Ok(picks)
}

fn layer_border_class(layer: &str) -> &'static str {
    if layer.eq_ignore_ascii_case("interface") {
        "border-l-accent-cyan"
    } else if layer.eq_ignore_ascii_case("core logic") {
        "border-l-accent-purple"
    } else if layer.eq_ignore_ascii_case("data") {
        "border-l-accent-green"
    } else if layer.eq_ignore_ascii_case("wire format") {
        "border-l-accent-orange"
    } else if layer.eq_ignore_ascii_case("connectors") {
        "border-l-accent-yellow"
    } else {
        "border-l-surface-4"
    }
}

fn first_sentence_or_fallback(intent: &str) -> String {
    let first = crate::api::common::first_sentence(intent);
    if first.trim().is_empty() {
        "entry point".to_owned()
    } else {
        first
    }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(format!("{byte:02X}").as_str());
        }
    }
    out
}
