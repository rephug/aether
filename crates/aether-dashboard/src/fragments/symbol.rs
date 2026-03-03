use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Html;
use maud::html;

use crate::api::symbol::{self, SymbolDeepDiveBuild};
use crate::support::{self, DashboardState};

pub(crate) async fn symbol_fragment(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Html<String> {
    let shared = state.shared.clone();
    let build = match support::run_blocking_with_timeout(move || {
        symbol::build_symbol_deep_dive(shared.as_ref(), selector.as_str())
    })
    .await
    {
        Ok(Some(build)) => build,
        Ok(None) => {
            return support::html_markup_response(html! {
                (support::html_empty_state("Symbol not found", None))
            });
        }
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load symbol deep dive", detail.as_str()))
            });
        }
    };

    support::html_markup_response(render_symbol_deep_dive(&build))
}

fn render_symbol_deep_dive(build: &SymbolDeepDiveBuild) -> maud::Markup {
    let data = &build.data;

    html! {
        div class="space-y-4" data-page="symbol-deep-dive" {
            (support::explanation_header(
                "Symbol Deep Dive",
                "This page explains what the component does, where it fits, and what would be affected if you change it.",
                "Use this to understand dependencies, risks, and impact before editing.",
                "Narrative symbol report composed from SIR intent, graph centrality, and dependency structure."
            ))

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-6 space-y-3" {
                div class="flex flex-wrap items-start justify-between gap-3" {
                    div class="space-y-2" {
                        h1 class="text-2xl font-semibold" { (data.name.as_str()) }
                        div class="flex flex-wrap items-center gap-2 text-xs" {
                            span class={ "badge " (support::badge_class_for_kind(data.kind.as_str())) } { (data.kind.as_str()) }
                            span class="badge badge-muted" { (data.layer_icon.as_str()) " " (data.layer.as_str()) }
                        }
                        div class="text-xs text-text-muted" {
                            span class="font-semibold" { "File: " }
                            span class="file-link text-blue-600 hover:underline cursor-pointer font-mono"
                                data-path=(data.file.as_str()) {
                                (data.file.as_str())
                            }
                        }
                    }
                    @if !data.matched_by.is_empty() {
                        span class="badge badge-cyan" { "matched by " (data.matched_by.as_str()) }
                    }
                }
                p class="text-sm text-text-secondary" { (data.role.as_str()) }
                @if !data.alternatives.is_empty() {
                    div class="text-xs text-text-secondary space-y-1" {
                        div class="font-semibold text-text-primary" { "Other symbols with this name:" }
                        ul class="space-y-1" {
                            @for alt in &data.alternatives {
                                li {
                                    span class="symbol-link text-blue-600 hover:underline cursor-pointer"
                                        data-symbol=(alt.qualified_name.as_str()) {
                                        (alt.qualified_name.as_str())
                                    }
                                    " "
                                    span class="text-text-muted" { "(" (alt.layer.as_str()) ", " (alt.file.as_str()) ")" }
                                }
                            }
                        }
                    }
                }
            }

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-2" {
                h2 class="text-lg font-semibold" { "How It Fits" }
                p class="text-sm text-text-secondary" { (data.context.as_str()) }
                p class="text-sm text-text-secondary" { (data.centrality_narrative.as_str()) }
            }

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-2" {
                h2 class="text-lg font-semibold" { "How It Gets Used" }
                p class="text-sm text-text-secondary" { (data.creation_narrative.as_str()) }
            }

            section class="grid gap-4 xl:grid-cols-2" {
                article class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                    h3 class="text-base font-semibold" { "What Depends on This" }
                    p class="text-sm text-text-secondary" { (data.dependents.narrative.as_str()) }
                    @if data.dependents.by_layer.is_empty() {
                        (support::html_empty_state("No direct dependents", None))
                    } @else {
                        div class="space-y-2" {
                            @for layer_group in &data.dependents.by_layer {
                                div class="rounded-lg border border-surface-3/30 p-3 space-y-2" {
                                    div class="text-xs font-semibold uppercase tracking-wide text-text-muted" {
                                        (layer_group.layer.as_str())
                                    }
                                    div class="flex flex-wrap gap-2" {
                                        @for symbol_name in &layer_group.symbols {
                                            span class="symbol-link text-blue-600 hover:underline cursor-pointer text-sm"
                                                data-symbol=(symbol_name.as_str()) {
                                                (symbol_name.as_str())
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                article class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                    h3 class="text-base font-semibold" { "What This Depends On" }
                    p class="text-sm text-text-secondary" { (data.dependencies.narrative.as_str()) }
                    @if data.dependencies.items.is_empty() {
                        (support::html_empty_state("No direct dependencies", None))
                    } @else {
                        ul class="space-y-2" {
                            @for dep in &data.dependencies.items {
                                li class="rounded-lg border border-surface-3/30 p-3 text-sm space-y-1" {
                                    div {
                                        span class="symbol-link text-blue-600 hover:underline cursor-pointer"
                                            data-symbol=(dep.name.as_str()) {
                                            (dep.name.as_str())
                                        }
                                    }
                                    @if let Some(reason) = dep.reason.as_deref() {
                                        p class="text-xs text-text-secondary" { (reason) }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                h2 class="text-lg font-semibold" { "Side Effects & Risks" }

                div class="space-y-1" {
                    p class="text-sm text-text-secondary" { (data.side_effects.narrative.as_str()) }
                    @if !data.side_effects.items.is_empty() {
                        ul class="list-disc pl-5 text-xs text-text-secondary space-y-1" {
                            @for effect in &data.side_effects.items {
                                li { (effect.as_str()) }
                            }
                        }
                    }
                }

                div class="space-y-1" {
                    p class="text-sm text-text-secondary" { (data.error_modes.narrative.as_str()) }
                    @if !data.error_modes.items.is_empty() {
                        ul class="list-disc pl-5 text-xs text-text-secondary space-y-1" {
                            @for mode in &data.error_modes.items {
                                li { (mode.as_str()) }
                            }
                        }
                    }
                }

                div class="rounded-lg border border-surface-3/40 bg-surface-0/70 p-4 space-y-2" {
                    div class="flex items-center gap-2" {
                        span class={ "badge " (risk_badge_class(data.blast_radius.risk_level.as_str())) } {
                            (data.blast_radius.risk_level.as_str()) " risk"
                        }
                        span class="text-xs text-text-muted" {
                            (data.blast_radius.affected_symbols) " components / "
                            (data.blast_radius.affected_files) " files"
                        }
                    }
                    p class="text-sm text-text-secondary" { (data.blast_radius.narrative.as_str()) }
                }
            }

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                h2 class="text-lg font-semibold" { "LLM Difficulty" }
                div class="flex items-center gap-2" {
                    span class="text-2xl" { (data.difficulty.emoji.as_str()) }
                    span class={ "badge " (difficulty_badge_class(data.difficulty.label.as_str())) } {
                        (data.difficulty.label.as_str())
                    }
                    span class="text-xs text-text-muted" { (format!("score {:.1}/100", data.difficulty.score)) }
                }
                p class="text-sm text-text-secondary" { (data.difficulty.guidance.as_str()) }
                @if !data.difficulty.reasons.is_empty() {
                    ul class="list-disc pl-5 text-sm text-text-secondary space-y-1" {
                        @for reason in &data.difficulty.reasons {
                            li { (reason.as_str()) }
                        }
                    }
                }
                p class="text-sm text-blue-600 hover:underline" {
                    a
                        hx-get={"/dashboard/frag/autopsy/" (percent_encode(data.name.as_str()))}
                        hx-target="#main-content"
                        hx-push-url={"/dashboard/autopsy/" (percent_encode(data.name.as_str()))} {
                        "See the Prompt Advisor for guidance ->"
                    }
                }
            }

            section class="grid gap-3 md:grid-cols-4" {
                button
                    class="rounded-lg border border-surface-3/40 bg-surface-1 px-3 py-3 text-sm hover:bg-surface-3/20 text-left"
                    hx-get={"/dashboard/frag/spec/" (percent_encode(data.name.as_str()))}
                    hx-target="#main-content"
                    hx-push-url={"/dashboard/spec/" (percent_encode(data.name.as_str()))} {
                    "📋 Generate Spec"
                }
                button
                    class="rounded-lg border border-surface-3/40 bg-surface-1 px-3 py-3 text-sm hover:bg-surface-3/20 text-left"
                    hx-get={"/dashboard/frag/autopsy/" (percent_encode(data.name.as_str()))}
                    hx-target="#main-content"
                    hx-push-url={"/dashboard/autopsy/" (percent_encode(data.name.as_str()))} {
                    "🎓 Prompt Advisor"
                }
                button
                    class="rounded-lg border border-surface-3/40 bg-surface-1 px-3 py-3 text-sm hover:bg-surface-3/20 text-left"
                    hx-get={"/dashboard/frag/decompose/" (percent_encode(data.name.as_str()))}
                    hx-target="#main-content"
                    hx-push-url={"/dashboard/decompose/" (percent_encode(data.name.as_str()))} {
                    "🔨 See Build Steps"
                }
                a class="rounded-lg border border-surface-3/40 bg-surface-1 px-3 py-3 text-sm hover:bg-surface-3/20"
                    hx-get={"/dashboard/frag/flow?start=" (percent_encode(data.name.as_str()))}
                    hx-target="#main-content"
                    hx-push-url={"/dashboard/flow?start=" (percent_encode(data.name.as_str()))} {
                    "🔄 Trace Flow"
                }
            }
        }
    }
}

fn risk_badge_class(level: &str) -> &'static str {
    if level.eq_ignore_ascii_case("high") {
        "badge-red"
    } else if level.eq_ignore_ascii_case("medium") {
        "badge-yellow"
    } else {
        "badge-green"
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
