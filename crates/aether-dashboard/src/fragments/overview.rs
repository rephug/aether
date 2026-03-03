use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::{Markup, html};

use crate::support::{self, DashboardState};

pub(crate) async fn overview_fragment(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let shared = state.shared.clone();
    let overview = match support::run_async_with_timeout(move || async move {
        support::load_overview_data(shared.as_ref()).await
    })
    .await
    {
        Ok(data) => data,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to load overview", &detail))
            });
        }
    };
    let staleness = support::compute_staleness(state.shared.workspace.as_path());
    let age_text = support::format_age_seconds(staleness.index_age_seconds);
    let project_name = state
        .shared
        .workspace
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("this project")
        .to_owned();

    support::html_markup_response(html! {
        div class="space-y-5" {
            (support::explanation_header(
                "Welcome",
                "This dashboard translates technical graph and indexing signals into plain-language guidance so you can quickly understand the codebase.",
                "Use these cards to see project size, understanding coverage, and where to explore next.",
                "Overview metrics across symbols, files, SIR, drift, and coupling."
            ))

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-4" {
                h2 class="text-xl font-semibold" { "Welcome to " (project_name.clone()) }
                p class="text-sm text-text-secondary beginner-only" {
                    (project_name.clone()) " contains " (overview.total_symbols) " components across " (overview.total_files) " files. "
                    "AETHER has analyzed " (format!("{:.1}%", overview.sir_coverage_pct)) " of them and built a map of how they connect."
                }
                p class="text-sm text-text-secondary intermediate-only" {
                    (overview.total_symbols) " components in " (overview.total_files) " files, with "
                    (format!("{:.1}%", overview.sir_coverage_pct)) " understanding coverage."
                }
                p class="text-sm text-text-secondary expert-only" {
                    "Indexed symbols: " (overview.total_symbols) ", files: " (overview.total_files) ", SIR coverage: "
                    (format!("{:.1}%", overview.sir_coverage_pct)) "."
                }

                div class="grid gap-3 md:grid-cols-3" {
                    (feature_card("📖 Understand This Project", "/dashboard/frag/anatomy", "Open Anatomy"))
                    (coming_soon_card("🗺️ Take a Guided Tour", "/dashboard/frag/tour"))
                    (coming_soon_card("💬 Build a Question", "/dashboard/frag/prompts"))
                }
            }

            div
                id="overview-recent-changes"
                class="space-y-3"
                hx-get="/dashboard/frag/changes?since=24h&limit=20&embed=true"
                hx-trigger="load"
                hx-target="this" {
                (support::html_empty_state("Loading recent changes...", None))
            }

            div class="grid gap-4 md:grid-cols-2 xl:grid-cols-4" {
                (stat_card(
                    &overview.total_symbols.to_string(),
                    "Components",
                    "Symbols",
                    "A component is one indexed symbol such as a function, struct, enum, trait, or method."
                ))
                (stat_card(
                    &overview.total_files.to_string(),
                    "Source Files",
                    "Files Indexed",
                    "How many unique source files currently contribute indexed components."
                ))
                (stat_card(
                    &format!("{:.1}%", overview.sir_coverage_pct),
                    "Understanding Coverage",
                    "SIR Coverage",
                    "SIR is AETHER's structured understanding of what code does. Coverage shows how much of the codebase has that understanding."
                ))
                (stat_card(
                    &age_text,
                    "Analysis Age",
                    "Index Age",
                    "How long it has been since the latest indexed or analyzed symbol update."
                ))
            }

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                div class="flex flex-wrap items-center gap-2 text-xs" {
                    span class="badge badge-muted" { "Backend" }
                    span class={ "badge " (if overview.graph_backend == "surreal" { "badge-purple" } else { "badge-cyan" }) } { (overview.graph_backend) }
                    span class="badge badge-muted" { "Vector" }
                    span class={ "badge " (match overview.vector_status.as_str() { "available" => "badge-green", "disabled" => "badge-muted", _ => "badge-red" }) } { (overview.vector_status) }
                    span class="badge badge-yellow" { "Drift: " (overview.drift_count) }
                    span class="badge badge-orange" { "Coupling: " (overview.coupling_count) }
                    @if staleness.stale {
                        span class="badge badge-red" { "Stale" }
                    }
                }
            }

            div class="grid gap-5 xl:grid-cols-[minmax(0,1.2fr)_minmax(0,0.8fr)]" {
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    h2 class="text-sm font-semibold mb-3" {
                        "Language Breakdown "
                        (support::help_icon("Shows how your source files are distributed by language."))
                    }
                    @if overview.languages.is_empty() {
                        (support::html_empty_state("No indexed files yet", Some("aether index")))
                    } @else {
                        table class="data-table" {
                            thead {
                                tr {
                                    th { "Language" }
                                    th { "Files" }
                                    th { "Share" }
                                }
                            }
                            tbody {
                                @for (lang, count) in &overview.languages {
                                    tr {
                                        td {
                                            span class={ "badge " (support::badge_class_for_language(lang)) } { (lang) }
                                        }
                                        td class="font-mono" { (count) }
                                        td class="font-mono" {
                                            @let pct = if overview.total_files > 0 { (*count as f64 / overview.total_files as f64) * 100.0 } else { 0.0 };
                                            (format!("{pct:.1}%"))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    h2 class="text-sm font-semibold mb-3" {
                        "Overview Chart "
                        (support::help_icon("A visual summary of language distribution for the current index state."))
                    }
                    div id="overview-chart" class="min-h-[260px]" {
                        @if overview.languages.is_empty() {
                            (support::html_empty_state("No indexed files yet", Some("aether index")))
                        }
                    }
                }
            }
        }
    })
}

fn stat_card(value: &str, beginner_label: &str, expert_label: &str, tooltip: &str) -> Markup {
    html! {
        div class="stat-card" {
            div class="stat-value" { (value) }
            div class="stat-label" {
                (support::metric_label_with_tooltip(beginner_label, tooltip))
                span class="expert-only ml-1 text-[11px] normal-case tracking-normal text-text-muted" { "(" (expert_label) ")" }
            }
        }
    }
}

fn feature_card(title: &str, target: &str, label: &str) -> Markup {
    html! {
        a
            href="#"
            hx-get=(target)
            hx-target="#main-content"
            class="rounded-lg border border-accent-cyan/40 bg-surface-0/70 p-3 hover:border-accent-cyan/60 transition-colors" {
            div class="text-sm font-semibold text-text-primary" { (title) }
            div class="mt-1 text-xs text-text-secondary font-mono" { (target) }
            div class="mt-2 text-[11px] uppercase tracking-wider text-accent-cyan" { (label) }
        }
    }
}

fn coming_soon_card(title: &str, target: &str) -> Markup {
    html! {
        a
            href="#"
            hx-get="/dashboard/frag/overview"
            hx-target="#main-content"
            class="rounded-lg border border-surface-3/40 bg-surface-0/70 p-3 hover:border-accent-cyan/40 transition-colors" {
            div class="text-sm font-semibold text-text-primary" { (title) }
            div class="mt-1 text-xs text-text-secondary font-mono" { (target) }
            div class="mt-2 text-[11px] uppercase tracking-wider text-text-muted" { "Coming Soon" }
        }
    }
}
