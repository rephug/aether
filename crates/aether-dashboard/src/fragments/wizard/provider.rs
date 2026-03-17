use std::collections::HashMap;

use maud::{Markup, html};

use super::{back_button, next_button};

/// Step 4: Inference provider selection with conditional fields.
pub(crate) fn render(params: &HashMap<String, String>) -> Markup {
    let workspace = params.get("workspace_path").cloned().unwrap_or_default();
    let provider = params.get("provider").cloned().unwrap_or_default();
    let enable_batch = params
        .get("enable_batch")
        .map(|v| v == "true")
        .unwrap_or(false);
    let enable_continuous = params
        .get("enable_continuous")
        .map(|v| v == "true")
        .unwrap_or(false);

    html! {
        div class="space-y-5" {
            h2 class="text-lg font-semibold text-text-primary dark:text-slate-100" {
                "Choose Inference Mode"
            }

            p class="text-sm text-text-secondary dark:text-slate-300" {
                "How should AETHER generate semantic summaries for your code?"
            }

            input type="hidden" name="workspace_path" value=(workspace) {}

            // Provider radio buttons
            div class="space-y-3" {
                (provider_option(
                    "cloud", "Cloud (Gemini Flash)", &provider,
                    "Best quality, requires API key. Costs ~$0.01 per file.",
                ))
                (provider_option(
                    "local", "Local (Ollama)", &provider,
                    "Fully offline, requires 8GB+ RAM. Good quality.",
                ))
                (provider_option(
                    "mock", "Mock (Testing)", &provider,
                    "No AI, instant results. Placeholder summaries only.",
                ))
            }

            // Conditional fields based on provider
            @if provider == "cloud" {
                div class="bg-surface-2/30 dark:bg-slate-700/30 rounded-lg p-3 space-y-2" {
                    label class="block text-sm text-text-secondary dark:text-slate-300" {
                        "API key environment variable"
                    }
                    input
                        type="text"
                        name="api_key_env"
                        value="GEMINI_API_KEY"
                        placeholder="GEMINI_API_KEY"
                        class="w-full px-3 py-1.5 text-sm rounded border border-surface-3/60 dark:border-slate-600 bg-surface-0 dark:bg-slate-900 text-text-primary dark:text-slate-100 focus:outline-none focus:ring-2 focus:ring-accent-cyan/50"
                    {}
                    p class="text-xs text-text-muted dark:text-slate-400" {
                        "Set this env var before launching AETHER. "
                        "Get a key at " a href="https://aistudio.google.com/apikey" target="_blank" class="text-accent-cyan hover:underline" { "Google AI Studio" } "."
                    }
                }
            }

            @if provider == "local" {
                div class="bg-surface-2/30 dark:bg-slate-700/30 rounded-lg p-3" {
                    p class="text-sm text-text-secondary dark:text-slate-300" {
                        "Make sure Ollama is running before starting indexing. "
                        "Install from "
                        a href="https://ollama.com" target="_blank" class="text-accent-cyan hover:underline" { "ollama.com" }
                        "."
                    }
                }
            }

            // Optional features
            div class="border-t border-surface-3/30 dark:border-slate-700 pt-4 space-y-2" {
                p class="text-xs text-text-muted dark:text-slate-400 uppercase tracking-wider" {
                    "Optional Features"
                }
                label class="flex items-center gap-2 text-sm text-text-secondary dark:text-slate-300 cursor-pointer" {
                    input
                        type="checkbox"
                        name="enable_batch"
                        value="true"
                        checked[enable_batch]
                        class="rounded border-surface-3 dark:border-slate-600 text-accent-cyan focus:ring-accent-cyan"
                    {}
                    "Enable batch pipeline (nightly reprocessing)"
                }
                label class="flex items-center gap-2 text-sm text-text-secondary dark:text-slate-300 cursor-pointer" {
                    input
                        type="checkbox"
                        name="enable_continuous"
                        value="true"
                        checked[enable_continuous]
                        class="rounded border-surface-3 dark:border-slate-600 text-accent-cyan focus:ring-accent-cyan"
                    {}
                    "Enable continuous drift monitor"
                }
            }

            // Navigation
            div class="flex justify-between pt-4" {
                (back_button(3, params))
                (next_button(5))
            }
        }
    }
}

/// Render a single provider radio option.
fn provider_option(value: &str, label: &str, selected: &str, description: &str) -> Markup {
    let is_selected = value == selected;
    html! {
        label class={
            "flex items-start gap-3 p-3 rounded-lg border cursor-pointer transition-colors "
            @if is_selected {
                "border-accent-cyan bg-accent-cyan/5 dark:bg-accent-cyan/10"
            } @else {
                "border-surface-3/50 dark:border-slate-600 hover:bg-surface-2/30 dark:hover:bg-slate-700/30"
            }
        } {
            input
                type="radio"
                name="provider"
                value=(value)
                checked[is_selected]
                class="mt-1 text-accent-cyan focus:ring-accent-cyan"
                hx-get="/dashboard/frag/wizard/step/4"
                hx-target="#wizard-content"
                hx-include="[name='workspace_path'], [name='enable_batch'], [name='enable_continuous']"
            {}
            div {
                div class="text-sm font-medium text-text-primary dark:text-slate-100" { (label) }
                div class="text-xs text-text-muted dark:text-slate-400 mt-0.5" { (description) }
            }
        }
    }
}
