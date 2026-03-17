use std::collections::HashMap;

use maud::{Markup, html};

use super::{back_button, next_button};

/// Step 2: Workspace selection with Browse button and file detection.
pub(crate) fn render(params: &HashMap<String, String>) -> Markup {
    let workspace = params.get("workspace_path").cloned().unwrap_or_default();

    html! {
        div class="space-y-5" {
            h2 class="text-lg font-semibold text-text-primary dark:text-slate-100" {
                "Choose Your Workspace"
            }

            p class="text-sm text-text-secondary dark:text-slate-300" {
                "Select the root directory of the project you want AETHER to analyze."
            }

            // Workspace path input + Browse button
            div class="flex items-center gap-3" {
                input
                    type="text"
                    id="workspace-path-input"
                    name="workspace_path"
                    value=(workspace)
                    placeholder="/path/to/your/project"
                    class="flex-1 px-3 py-2 text-sm rounded-lg border border-surface-3/60 dark:border-slate-600 bg-surface-0 dark:bg-slate-900 text-text-primary dark:text-slate-100 focus:outline-none focus:ring-2 focus:ring-accent-cyan/50"
                    hx-get="/dashboard/frag/wizard/detect"
                    hx-trigger="change, keyup changed delay:500ms"
                    hx-target="#detection-results"
                    hx-include="this"
                {}

                button
                    type="button"
                    class="px-4 py-2 text-sm bg-surface-2 dark:bg-slate-700 border border-surface-3/60 dark:border-slate-600 rounded-lg hover:bg-surface-3/50 dark:hover:bg-slate-600 transition-colors whitespace-nowrap"
                    onclick="if(window.__TAURI__){window.__TAURI__.core.invoke('pick_directory').then(function(p){if(p){var el=document.getElementById('workspace-path-input');el.value=p;htmx.trigger(el,'change');}});}else{alert('File picker requires AETHER Desktop');}"
                {
                    "Browse\u{2026}"
                }
            }

            // Detection results (loaded via HTMX)
            div id="detection-results" {
                @if !workspace.is_empty() {
                    div
                        hx-get="/dashboard/frag/wizard/detect"
                        hx-trigger="load"
                        hx-target="#detection-results"
                        hx-vals={ "{\"workspace_path\":\"" (workspace) "\"}" }
                    {
                        div class="text-xs text-text-muted dark:text-slate-400 py-2" {
                            "Scanning..."
                        }
                    }
                }
            }

            // Navigation
            div class="flex justify-between pt-4" {
                (back_button(1, params))
                (next_button(3))
            }
        }
    }
}
