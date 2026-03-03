use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::api::difficulty;
use crate::api::prompts::{self, PromptGoal};
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PromptsQuery {
    pub tab: Option<String>,
    pub goal: Option<String>,
    pub symbol: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PromptSearchFragmentQuery {
    pub q: Option<String>,
    pub tab: Option<String>,
    pub goal: Option<String>,
}

pub(crate) async fn prompts_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<PromptsQuery>,
) -> Html<String> {
    let tab = normalize_tab(query.tab.as_deref());
    let goal = query
        .goal
        .as_deref()
        .and_then(PromptGoal::from_str)
        .or(Some(PromptGoal::UnderstandComponent));
    let selected_symbol = query.symbol.clone();

    let generated_prompt = if tab == "build" {
        let shared = state.shared.clone();
        let symbol = selected_symbol.clone();
        support::run_blocking_with_timeout(move || {
            if let Some(goal) = goal {
                prompts::build_generated_prompt(shared.as_ref(), goal, symbol.as_deref())
            } else {
                Ok(None)
            }
        })
        .await
        .unwrap_or_default()
    } else {
        None
    };

    let difficulty_overview = if tab == "learn" {
        let shared = state.shared.clone();
        support::run_blocking_with_timeout(move || {
            let data = difficulty::build_difficulty_data(shared.as_ref())?;
            Ok::<difficulty::DifficultyData, String>(data)
        })
        .await
        .ok()
    } else {
        None
    };

    support::html_markup_response(html! {
        div class="space-y-4" data-page="prompts" {
            (support::explanation_header(
                "LLM Collaboration Suite",
                "Build prompts, generate specs, and learn how much context an LLM really needs for your code.",
                "Switch tabs to build a prompt, generate specs, inspect context, and learn prompt quality patterns.",
                "Template-composed prompting workflows powered by SIR + dependency graph + layer data."
            ))

            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                h1 class="text-xl font-semibold" { "💬 Prompts" }
                div class="flex flex-wrap items-center gap-2 text-sm" {
                    (tab_button("build", "💬 Build a Prompt", tab, &query))
                    (tab_button("spec", "📋 Generate Spec", tab, &query))
                    (tab_button("context", "🧠 Context Advisor", tab, &query))
                    (tab_button("learn", "🎓 Learn to Prompt", tab, &query))
                }
            }

            @if tab == "build" {
                (build_prompt_tab(goal, generated_prompt.as_ref(), &query))
            } @else if tab == "spec" {
                (spec_tab(&query))
            } @else if tab == "context" {
                (context_tab(&query))
            } @else {
                (learn_tab(&query, difficulty_overview.as_ref()))
            }
        }
    })
}

pub(crate) async fn prompt_search_fragment(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<PromptSearchFragmentQuery>,
) -> Html<String> {
    let q = query.q.unwrap_or_default();
    if q.trim().is_empty() {
        return support::html_markup_response(html! {
            div class="text-xs text-text-muted" { "Start typing to search symbols..." }
        });
    }

    let shared = state.shared.clone();
    let data = match support::run_blocking_with_timeout(move || {
        prompts::build_prompt_search_data(shared.as_ref(), q.as_str())
    })
    .await
    {
        Ok(data) => data,
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to search symbols", detail.as_str()))
            });
        }
    };

    if data.results.is_empty() {
        return support::html_markup_response(html! {
            div class="text-xs text-text-muted" { "No symbols matched." }
        });
    }

    let tab = normalize_tab(query.tab.as_deref());
    let goal = query.goal.unwrap_or_default();

    support::html_markup_response(html! {
        div class="space-y-2" {
            @for item in &data.results {
                button
                    class="w-full text-left rounded-md border border-surface-3/30 p-2 hover:bg-surface-3/20"
                    hx-get=(prompt_url(tab, Some(goal.as_str()), Some(item.qualified_name.as_str())))
                    hx-target="#main-content"
                    hx-push-url=(prompt_page_url(tab, Some(goal.as_str()), Some(item.qualified_name.as_str()))) {
                    div class="flex flex-wrap items-center justify-between gap-2" {
                        span class="font-mono text-xs" { (item.qualified_name.as_str()) }
                        span class={ "badge " (difficulty_badge_class(item.difficulty.label.as_str())) } {
                            (item.difficulty.emoji.as_str()) " " (item.difficulty.label.as_str())
                        }
                    }
                    div class="text-xs text-text-muted" { (item.kind.as_str()) " • " (item.file.as_str()) }
                    div class="text-xs text-text-secondary" { (item.intent.as_str()) }
                }
            }
        }
    })
}

fn build_prompt_tab(
    goal: Option<PromptGoal>,
    generated: Option<&crate::api::prompts::PromptGenerateData>,
    query: &PromptsQuery,
) -> maud::Markup {
    let selected_goal = goal.unwrap_or(PromptGoal::UnderstandComponent);

    html! {
        div class="space-y-4" {
            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                h2 class="text-base font-semibold" { "Step 1: Choose a Goal" }
                div class="grid gap-2 md:grid-cols-2 xl:grid-cols-4" {
                    (goal_card("understand_component", "🔍", "Understand a component", "What does [X] do and why?", "aether_explain", selected_goal, query))
                    (goal_card("understand_flow", "🔄", "Understand a flow", "How does [X] connect to [Y]?", "aether_dependencies", selected_goal, query))
                    (goal_card("find_related", "🔗", "Find related code", "What else is related to [X]?", "aether_search", selected_goal, query))
                    (goal_card("assess_risk", "⚡", "Assess change risk", "What breaks if I change [X]?", "aether_blast_radius", selected_goal, query))
                    (goal_card("debug", "🐛", "Debug a problem", "Why might [X] be failing?", "aether_explain + aether_dependencies", selected_goal, query))
                    (goal_card("plan_refactor", "🏗️", "Plan a refactor", "How should I restructure [X]?", "aether_blast_radius + aether_coupling", selected_goal, query))
                    (goal_card("health_check", "📊", "Health check", "What needs attention?", "aether_health", selected_goal, query))
                    (goal_card("understand_history", "📜", "Understand history", "Why was [X] built this way?", "aether_ask", selected_goal, query))
                }
            }

            @if selected_goal != PromptGoal::HealthCheck {
                section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                    h2 class="text-base font-semibold" { "Step 2: Select Target" }
                    input
                        type="text"
                        name="q"
                        placeholder="Search symbols..."
                        class="w-full rounded-md border border-surface-3/40 bg-surface-0/70 px-3 py-2 text-sm"
                        hx-get={"/dashboard/frag/prompts/search?tab=build&goal=" (selected_goal.as_str())}
                        hx-trigger="input changed delay:200ms"
                        hx-target="#prompt-search-results"
                        hx-include="this";
                    div id="prompt-search-results" class="space-y-2" {
                        @if let Some(symbol) = query.symbol.as_deref() {
                            div class="text-xs text-text-secondary" { "Selected: " (symbol) }
                        } @else {
                            div class="text-xs text-text-muted" { "Choose a symbol to generate a prompt." }
                        }
                    }
                }
            }

            @if let Some(generated) = generated {
                section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                    h2 class="text-base font-semibold" { "Step 3: Generated Prompt" }
                    div class="text-xs text-text-muted" { "MCP Tool: " (generated.mcp_tool.as_str()) }
                    pre class="rounded-lg border border-surface-3/30 bg-surface-0/70 p-3 text-sm whitespace-pre-wrap" {
                        (generated.prompt.as_str())
                    }
                    button
                        type="button"
                        class="px-3 py-2 rounded-md border border-surface-3/40 hover:bg-surface-3/20 text-sm"
                        data-copy-text=(generated.prompt.as_str())
                        onclick="aetherCopyText(this)" {
                        "Copy to Clipboard"
                    }

                    @if let Some(symbol) = generated.symbol.as_ref() {
                        div class="flex flex-wrap gap-2" {
                            button
                                class="px-2 py-1 text-xs rounded-md border border-surface-3/40 hover:bg-surface-3/20"
                                hx-get={"/dashboard/frag/decompose/" (percent_encode(symbol.qualified_name.as_str()))}
                                hx-target="#main-content"
                                hx-push-url={"/dashboard/decompose/" (percent_encode(symbol.qualified_name.as_str()))} {
                                "🔨 Build this component step by step"
                            }
                        }
                    }
                }
            }
        }
    }
}

fn spec_tab(query: &PromptsQuery) -> maud::Markup {
    html! {
        div class="space-y-4" {
            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                h2 class="text-base font-semibold" { "Generate Spec" }
                input
                    type="text"
                    name="q"
                    placeholder="Search symbols..."
                    class="w-full rounded-md border border-surface-3/40 bg-surface-0/70 px-3 py-2 text-sm"
                    hx-get="/dashboard/frag/prompts/search?tab=spec"
                    hx-trigger="input changed delay:200ms"
                    hx-target="#prompt-search-results";
                div id="prompt-search-results" class="space-y-2" {
                    @if let Some(symbol) = query.symbol.as_deref() {
                        div class="text-xs text-text-secondary" { "Selected: " (symbol) }
                    } @else {
                        div class="text-xs text-text-muted" { "Choose a symbol to generate its spec." }
                    }
                }
            }

            @if let Some(symbol) = query.symbol.as_deref() {
                div
                    hx-get={"/dashboard/frag/spec/" (percent_encode(symbol))}
                    hx-trigger="load"
                    hx-target="this" {
                    (support::html_empty_state("Loading spec...", None))
                }
            }
        }
    }
}

fn context_tab(query: &PromptsQuery) -> maud::Markup {
    html! {
        div class="space-y-4" {
            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                h2 class="text-base font-semibold" { "Context Advisor" }
                input
                    type="text"
                    name="q"
                    placeholder="Search symbols..."
                    class="w-full rounded-md border border-surface-3/40 bg-surface-0/70 px-3 py-2 text-sm"
                    hx-get="/dashboard/frag/prompts/search?tab=context"
                    hx-trigger="input changed delay:200ms"
                    hx-target="#prompt-search-results";
                div id="prompt-search-results" class="space-y-2" {
                    @if let Some(symbol) = query.symbol.as_deref() {
                        div class="text-xs text-text-secondary" { "Selected: " (symbol) }
                    } @else {
                        div class="text-xs text-text-muted" { "Choose a symbol to compute required context." }
                    }
                }
            }

            @if let Some(symbol) = query.symbol.as_deref() {
                div
                    hx-get={"/dashboard/frag/context/" (percent_encode(symbol))}
                    hx-trigger="load"
                    hx-target="this" {
                    (support::html_empty_state("Loading context advisor...", None))
                }
            }
        }
    }
}

fn learn_tab(
    query: &PromptsQuery,
    difficulty_data: Option<&difficulty::DifficultyData>,
) -> maud::Markup {
    html! {
        div class="space-y-4" {
            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                h2 class="text-base font-semibold" { "How to Prompt Effectively" }
                p class="text-sm text-text-secondary teaching-note" {
                    "Start with difficulty: easy components can take short prompts, while hard components need explicit control flow, edge cases, and dependency context."
                }
                @if let Some(difficulty_data) = difficulty_data {
                    p class="text-sm text-text-secondary" {
                        "🟢 Easy: " (difficulty_data.summary.easy.count) " | 🟡 Moderate: " (difficulty_data.summary.moderate.count)
                        " | 🔴 Hard: " (difficulty_data.summary.hard.count) " | ⛔ Very Hard: " (difficulty_data.summary.very_hard.count)
                    }
                }
            }

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                h3 class="text-sm font-semibold" { "Choose a Symbol to Learn" }
                input
                    type="text"
                    name="q"
                    placeholder="Search symbols..."
                    class="w-full rounded-md border border-surface-3/40 bg-surface-0/70 px-3 py-2 text-sm"
                    hx-get="/dashboard/frag/prompts/search?tab=learn"
                    hx-trigger="input changed delay:200ms"
                    hx-target="#prompt-search-results";
                div id="prompt-search-results" class="space-y-2" {
                    @if let Some(symbol) = query.symbol.as_deref() {
                        div class="text-xs text-text-secondary" { "Selected: " (symbol) }
                        div class="flex flex-wrap gap-2 mt-2" {
                            button
                                class="px-2 py-1 text-xs rounded-md border border-surface-3/40 hover:bg-surface-3/20"
                                hx-get={"/dashboard/frag/decompose/" (percent_encode(symbol))}
                                hx-target="#main-content"
                                hx-push-url={"/dashboard/decompose/" (percent_encode(symbol))} {
                                "🔨 Decomposer"
                            }
                            button
                                class="px-2 py-1 text-xs rounded-md border border-surface-3/40 hover:bg-surface-3/20"
                                hx-get={"/dashboard/frag/autopsy/" (percent_encode(symbol))}
                                hx-target="#main-content"
                                hx-push-url={"/dashboard/autopsy/" (percent_encode(symbol))} {
                                "🎓 Prompt Autopsy"
                            }
                            button
                                class="px-2 py-1 text-xs rounded-md border border-surface-3/40 hover:bg-surface-3/20"
                                hx-get={"/dashboard/frag/context/" (percent_encode(symbol))}
                                hx-target="#main-content"
                                hx-push-url={"/dashboard/context/" (percent_encode(symbol))} {
                                "🧠 Context"
                            }
                        }
                    } @else {
                        div class="text-xs text-text-muted" { "Select a symbol to open Decomposer and Prompt Autopsy." }
                    }
                }
            }
        }
    }
}

fn goal_card(
    goal: &str,
    icon: &str,
    title: &str,
    description: &str,
    tool: &str,
    selected_goal: PromptGoal,
    query: &PromptsQuery,
) -> maud::Markup {
    let active = selected_goal.as_str() == goal;
    html! {
        button
            class={
                "rounded-lg border p-3 text-left space-y-1 transition-colors "
                (if active {
                    "border-accent-cyan/60 bg-accent-cyan/10"
                } else {
                    "border-surface-3/40 hover:bg-surface-3/20"
                })
            }
            hx-get=(prompt_url("build", Some(goal), query.symbol.as_deref()))
            hx-target="#main-content"
            hx-push-url=(prompt_page_url("build", Some(goal), query.symbol.as_deref())) {
            div class="text-base" { (icon) " " (title) }
            p class="text-xs text-text-secondary" { (description) }
            p class="text-[11px] text-text-muted font-mono" { "Tool: " (tool) }
        }
    }
}

fn tab_button(tab: &str, label: &str, current_tab: &str, query: &PromptsQuery) -> maud::Markup {
    html! {
        button
            class={
                "rounded-md border px-3 py-2 "
                (if current_tab == tab {
                    "border-accent-cyan/60 bg-accent-cyan/10"
                } else {
                    "border-surface-3/40 hover:bg-surface-3/20"
                })
            }
            hx-get=(prompt_url(tab, query.goal.as_deref(), query.symbol.as_deref()))
            hx-target="#main-content"
            hx-push-url=(prompt_page_url(tab, query.goal.as_deref(), query.symbol.as_deref())) {
            (label)
        }
    }
}

fn normalize_tab(raw: Option<&str>) -> &'static str {
    match raw.unwrap_or("build") {
        "spec" => "spec",
        "context" => "context",
        "learn" => "learn",
        _ => "build",
    }
}

fn prompt_url(tab: &str, goal: Option<&str>, symbol: Option<&str>) -> String {
    let mut params = vec![format!("tab={}", percent_encode(tab))];
    if let Some(goal) = goal.map(str::trim).filter(|value| !value.is_empty()) {
        params.push(format!("goal={}", percent_encode(goal)));
    }
    if let Some(symbol) = symbol.map(str::trim).filter(|value| !value.is_empty()) {
        params.push(format!("symbol={}", percent_encode(symbol)));
    }
    format!("/dashboard/frag/prompts?{}", params.join("&"))
}

fn prompt_page_url(tab: &str, goal: Option<&str>, symbol: Option<&str>) -> String {
    let mut params = vec![format!("tab={}", percent_encode(tab))];
    if let Some(goal) = goal.map(str::trim).filter(|value| !value.is_empty()) {
        params.push(format!("goal={}", percent_encode(goal)));
    }
    if let Some(symbol) = symbol.map(str::trim).filter(|value| !value.is_empty()) {
        params.push(format!("symbol={}", percent_encode(symbol)));
    }
    format!("/dashboard/prompts?{}", params.join("&"))
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
