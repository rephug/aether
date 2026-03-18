use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::api::context_export;
use crate::support::{self, DashboardState};

pub(crate) async fn context_builder_fragment(
    State(state): State<Arc<DashboardState>>,
) -> Html<String> {
    let shared = state.shared.clone();
    let data = support::run_blocking_with_timeout(move || {
        context_export::load_context_export_data(shared.as_ref())
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(error = %err, "dashboard: failed to load context builder fragment data");
        context_export::ContextExportData {
            available_files: Vec::new(),
            available_presets: Vec::new(),
            formats: vec!["markdown".to_owned()],
            default_budget: 32_000,
        }
    });

    let file_count = data.available_files.len();

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Interactive Context Builder",
                "Build context documents by selecting files and intelligence layers. The preview updates live as you make selections.",
                "Assembles token-budgeted context from SIR, source, graph, coupling, health, drift, memory, and test layers with live preview.",
                "Direct store queries with greedy budget allocation using LAYER_SUGGESTIONS percentages and CHARS_PER_TOKEN=3.5 estimation."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Build Context for AI Chat" }
                    span class="intermediate-only" { "Interactive Context Builder" }
                    span class="expert-only" { "Context Builder" }
                }
                div class="flex items-center gap-2" {
                    span class="badge badge-cyan" { (file_count) " files indexed" }
                    div id="context-loading" class="htmx-indicator" {
                        span class="inline-block w-4 h-4 border-2 border-accent-cyan/30 border-t-accent-cyan rounded-full animate-spin" {}
                    }
                }
            }

            @if data.available_files.is_empty() {
                (support::html_empty_state(
                    "No symbols indexed yet",
                    Some("aetherd --index-once")
                ))
            } @else {
                // Budget bar (top)
                div id="context-budget-bar"
                    class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3" {
                    div class="flex items-center justify-between text-xs text-text-secondary mb-1" {
                        span { "Token Budget" }
                        span id="context-budget-label" class="font-mono" { "0 / 32K tokens" }
                    }
                    div class="w-full bg-surface-3/30 rounded-full h-2.5 dark:bg-slate-700/50" {
                        div id="context-budget-fill"
                            class="h-2.5 rounded-full bg-accent-green transition-all duration-300"
                            style="width: 0%" {}
                    }
                }

                // Two-panel layout
                div class="flex flex-col lg:flex-row gap-4" {
                    // Left panel: controls
                    div class="lg:w-80 flex-shrink-0 space-y-3" {
                        // File tree
                        div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3 space-y-2" {
                            h3 class="text-sm font-semibold flex items-center justify-between" {
                                span { "Files" }
                                button id="context-select-none"
                                    class="text-xs text-text-muted hover:text-text-secondary"
                                    onclick="window._ctxBuilder && window._ctxBuilder.clearAll()" {
                                    "Clear all"
                                }
                            }
                            div id="context-builder-tree"
                                class="max-h-[400px] overflow-y-auto text-xs font-mono space-y-0.5" {
                                div class="text-text-muted py-4 text-center" {
                                    "Loading file tree..."
                                }
                            }
                        }

                        // Layer toggles
                        div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3 space-y-2" {
                            h3 class="text-sm font-semibold" { "Intelligence Layers" }
                            div class="space-y-1" {
                                (layer_toggle("sir", "SIR", true, "Semantic intent, side effects, error modes"))
                                (layer_toggle("source", "Source Code", true, "Actual source with budget-aware truncation"))
                                (layer_toggle("graph", "Graph", true, "Dependency neighborhood (callers + dependencies)"))
                                (layer_toggle("coupling", "Coupling", false, "Co-change coupling from git history"))
                                (layer_toggle("health", "Health", false, "Health score and violations"))
                                (layer_toggle("drift", "Drift", false, "Semantic drift findings"))
                                (layer_toggle("memory", "Memory", false, "Relevant project notes"))
                                (layer_toggle("tests", "Tests", false, "Test intents and coverage"))
                            }
                        }

                        // Task input
                        div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3 space-y-2" {
                            label class="block text-sm font-semibold" { "Task Description" }
                            span class="beginner-only text-xs text-text-muted" {
                                "Describe what you're working on to help AETHER prioritize relevant context"
                            }
                            input id="context-task"
                                type="text"
                                placeholder="e.g. refactor error handling"
                                class="w-full p-2 text-sm border border-surface-3/50 rounded-lg bg-white/80 dark:bg-slate-800/80 dark:border-slate-600" {}
                        }

                        // Options row
                        div class="grid grid-cols-2 gap-2" {
                            // Depth
                            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3 space-y-1" {
                                label class="block text-xs text-text-muted uppercase tracking-wider" { "Depth" }
                                select id="context-depth"
                                    class="w-full p-1.5 text-sm border border-surface-3/50 rounded-lg bg-white/80 dark:bg-slate-800/80 dark:border-slate-600" {
                                    option value="0" { "0 (target only)" }
                                    option value="1" { "1 (immediate)" }
                                    option value="2" selected { "2 (standard)" }
                                    option value="3" { "3 (deep)" }
                                }
                            }

                            // Format
                            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3 space-y-1" {
                                label class="block text-xs text-text-muted uppercase tracking-wider" { "Format" }
                                select id="context-format"
                                    class="w-full p-1.5 text-sm border border-surface-3/50 rounded-lg bg-white/80 dark:bg-slate-800/80 dark:border-slate-600" {
                                    @for fmt in &data.formats {
                                        option value=(fmt) { (fmt) }
                                    }
                                }
                            }
                        }

                        // Budget slider
                        div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3 space-y-1" {
                            label class="block text-xs text-text-muted uppercase tracking-wider" { "Max Budget" }
                            input id="context-budget-slider"
                                type="range"
                                min="4000"
                                max="128000"
                                step="4000"
                                value=(data.default_budget.to_string())
                                class="w-full accent-cyan-500" {}
                            div id="context-budget-slider-val"
                                class="text-right text-xs font-mono text-text-secondary" {
                                (format_tokens(data.default_budget))
                            }
                        }

                        // Preset selector
                        div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3 space-y-1" {
                            label class="block text-xs text-text-muted uppercase tracking-wider" { "Preset" }
                            div class="flex flex-wrap gap-1" {
                                @for preset in &data.available_presets {
                                    button
                                        class="px-2 py-1 text-xs rounded border border-surface-3/50 hover:bg-surface-2/60 dark:hover:bg-slate-700/60 transition-colors"
                                        data-preset-name=(preset.name.as_str())
                                        data-preset-budget=(preset.budget.to_string())
                                        onclick="window._ctxBuilder && window._ctxBuilder.applyPreset(this.dataset)" {
                                        (preset.name.as_str())
                                    }
                                }
                            }
                        }

                        // Action buttons
                        div class="flex gap-2" {
                            button id="context-copy-btn"
                                class="flex-1 px-3 py-2 text-sm font-medium rounded-lg border border-accent-cyan/50 text-accent-cyan hover:bg-accent-cyan/10 transition-colors"
                                onclick="window._ctxBuilder && window._ctxBuilder.copyToClipboard()" {
                                "Copy"
                            }
                            button id="context-export-btn"
                                class="flex-1 px-3 py-2 text-sm font-medium rounded-lg border border-accent-purple/50 text-accent-purple hover:bg-accent-purple/10 transition-colors"
                                onclick="window._ctxBuilder && window._ctxBuilder.exportFile()" {
                                "Export"
                            }
                        }
                    }

                    // Right panel: preview
                    div class="flex-1 min-w-0 space-y-3" {
                        div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3" {
                            div class="flex items-center justify-between mb-2" {
                                h3 class="text-sm font-semibold" { "Preview" }
                                span id="context-target-count" class="text-xs text-text-muted" { "" }
                            }
                            pre id="context-preview"
                                class="text-xs font-mono bg-surface-2/40 dark:bg-slate-800/60 p-4 rounded-lg min-h-[400px] max-h-[600px] overflow-auto whitespace-pre-wrap break-words border border-surface-3/20" {
                                span class="text-text-muted" { "Select files from the tree to build context..." }
                            }
                        }

                        // Per-layer breakdown
                        div id="context-budget-breakdown"
                            class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-3" {
                            h3 class="text-sm font-semibold mb-2" { "Budget Breakdown" }
                            div id="context-layer-bars" class="space-y-1" {
                                div class="text-xs text-text-muted" { "No context built yet" }
                            }
                        }
                    }
                }
            }
        }
    })
}

fn layer_toggle(id: &str, label: &str, checked: bool, tooltip: &str) -> maud::Markup {
    let input_id = format!("layer-{id}");
    html! {
        label class="flex items-center gap-2 cursor-pointer group" title=(tooltip) {
            input
                id=(input_id)
                type="checkbox"
                data-layer=(id)
                class="rounded border-surface-3/50 text-accent-cyan focus:ring-accent-cyan/30"
                checked[checked] {}
            span class="text-xs group-hover:text-text-primary transition-colors" { (label) }
        }
    }
}

fn format_tokens(n: usize) -> String {
    if n >= 1000 {
        format!("{}K tokens", n / 1000)
    } else {
        format!("{n} tokens")
    }
}
