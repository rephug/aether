use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::api::context_export;
use crate::support::{self, DashboardState};

pub(crate) async fn context_export_fragment(
    State(state): State<Arc<DashboardState>>,
) -> Html<String> {
    let shared = state.shared.clone();
    let data = support::run_blocking_with_timeout(move || {
        context_export::load_context_export_data(shared.as_ref())
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(error = %err, "dashboard: failed to load context export fragment data");
        context_export::ContextExportData {
            available_files: Vec::new(),
            available_presets: Vec::new(),
            formats: vec!["markdown".to_owned()],
            default_budget: 32_000,
        }
    });

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Export Context for AI Agents",
                "Context export assembles relevant code, SIRs, and analysis into a single document for AI assistants.",
                "Builds a token-budgeted context document from source, SIR, graph, coupling, health, drift, memory, and test layers.",
                "Shared ExportDocument engine with layer budget allocation and RRF+PPR task ranking."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Export Context for AI Agents" }
                    span class="intermediate-only" { "Context Export Builder" }
                    span class="expert-only" { "Context Export" }
                }
                span class="badge badge-cyan" { (data.available_files.len()) " files indexed" }
            }

            @if data.available_files.is_empty() {
                (support::html_empty_state(
                    "No symbols indexed yet",
                    Some("aetherd --index-once")
                ))
            } @else {
                // Target selector
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                    h3 class="text-sm font-semibold" { "Target" }
                    div class="space-y-2" {
                        label class="block text-xs text-text-muted uppercase tracking-wider" { "File or symbol" }
                        input
                            id="ctx-target"
                            type="text"
                            list="ctx-file-list"
                            placeholder="e.g. src/lib.rs or MyStruct::method"
                            class="w-full p-2 text-sm border border-surface-3/50 rounded-lg bg-white/80 dark:bg-slate-800/80 dark:border-slate-600"
                            oninput="updateContextCommand()";
                        datalist id="ctx-file-list" {
                            @for file in &data.available_files {
                                option value=(file) {}
                            }
                        }
                    }
                }

                // Options row
                div class="grid grid-cols-1 md:grid-cols-3 gap-3" {
                    // Budget
                    div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                        label class="block text-xs text-text-muted uppercase tracking-wider" {
                            "Token Budget"
                        }
                        input
                            id="ctx-budget"
                            type="range"
                            min="4000"
                            max="128000"
                            step="4000"
                            value=(data.default_budget.to_string())
                            class="w-full"
                            oninput="updateContextCommand()";
                        div id="ctx-budget-value" class="text-right text-xs font-mono text-text-secondary" {
                            (format_tokens(data.default_budget))
                        }
                    }

                    // Format
                    div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                        label class="block text-xs text-text-muted uppercase tracking-wider" {
                            "Output Format"
                        }
                        select
                            id="ctx-format"
                            class="w-full p-2 text-sm border border-surface-3/50 rounded-lg bg-white/80 dark:bg-slate-800/80 dark:border-slate-600"
                            onchange="updateContextCommand()" {
                            @for fmt in &data.formats {
                                option value=(fmt) { (fmt) }
                            }
                        }
                    }

                    // Preset quick-select
                    div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                        label class="block text-xs text-text-muted uppercase tracking-wider" {
                            "Quick Preset"
                        }
                        div class="flex flex-wrap gap-1" {
                            @for preset in &data.available_presets {
                                button
                                    class="px-2 py-1 text-xs rounded border border-surface-3/50 hover:bg-surface-2/60 dark:hover:bg-slate-700/60"
                                    onclick=(format!("applyPreset('{}', {})", preset.name, preset.budget))
                                    title=(preset.description.as_str()) {
                                    (preset.name.as_str())
                                }
                            }
                        }
                    }
                }

                // Generated command
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                    h3 class="text-sm font-semibold" { "Generated Command" }
                    code
                        id="ctx-command"
                        class="block text-xs font-mono bg-surface-2/60 dark:bg-slate-800/60 p-3 rounded select-all whitespace-pre-wrap" {
                        "aether context --target <select a target> --budget 32000 --format markdown"
                    }
                    button
                        class="mt-2 px-3 py-1.5 text-xs rounded border border-surface-3/50 hover:bg-surface-2/60 dark:hover:bg-slate-700/60"
                        onclick="navigator.clipboard.writeText(document.getElementById('ctx-command').textContent)" {
                        "Copy Command"
                    }
                }

                // Inline JS for reactive command building
                script {
                    (maud::PreEscaped(r#"
                    function updateContextCommand() {
                        var target = document.getElementById('ctx-target').value || '<select a target>';
                        var budget = document.getElementById('ctx-budget').value;
                        var format = document.getElementById('ctx-format').value;
                        document.getElementById('ctx-budget-value').textContent = Number(budget).toLocaleString() + ' tokens';
                        var cmd = 'aether context --target ' + target + ' --budget ' + budget + ' --format ' + format;
                        document.getElementById('ctx-command').textContent = cmd;
                    }
                    function applyPreset(name, budget) {
                        document.getElementById('ctx-budget').value = budget;
                        updateContextCommand();
                    }
                    "#))
                }
            }
        }
    })
}

fn format_tokens(n: usize) -> String {
    if n >= 1000 {
        format!("{}K tokens", n / 1000)
    } else {
        format!("{n} tokens")
    }
}
