use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::api::presets;
use crate::support::{self, DashboardState};

pub(crate) async fn presets_fragment(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let workspace = state.shared.workspace.clone();
    let data = support::run_blocking_with_timeout(move || presets::load_presets_data(&workspace))
        .await
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, "dashboard: failed to load presets fragment data");
            presets::PresetsData {
                presets: Vec::new(),
                total: 0,
            }
        });

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Context Presets",
                "Presets are saved configurations for the context export command. Pick a preset instead of configuring options every time.",
                "Presets define token budget, graph depth, included layers, and output format. Stored as TOML in .aether/presets/.",
                "PresetConfig TOML files with budget, depth, include/exclude layer lists, format, and task templates."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Context Presets" }
                    span class="intermediate-only" { "Preset Library" }
                    span class="expert-only" { "Presets" }
                }
                span class="badge badge-cyan" { (data.total) " presets" }
            }

            // Preset cards
            @if data.presets.is_empty() {
                (support::html_empty_state(
                    "No presets available",
                    Some("aether preset create --name my-preset")
                ))
            } @else {
                div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3" {
                    @for preset in &data.presets {
                        div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                            div class="flex items-center justify-between" {
                                h3 class="text-sm font-semibold" { (preset.name.as_str()) }
                                @if preset.is_builtin {
                                    span class="badge badge-muted" { "built-in" }
                                } @else {
                                    span class="badge badge-purple" { "custom" }
                                }
                            }
                            p class="text-xs text-text-secondary" { (preset.description.as_str()) }

                            div class="flex flex-wrap gap-1.5 text-xs" {
                                span class="badge badge-cyan" {
                                    (format_tokens(preset.budget))
                                }
                                span class="badge badge-muted" {
                                    "depth " (preset.depth)
                                }
                                span class="badge badge-muted" {
                                    (preset.format.as_str())
                                }
                            }

                            @if !preset.include.is_empty() {
                                div class="flex flex-wrap gap-1 text-xs" {
                                    @for layer in &preset.include {
                                        span class="px-1.5 py-0.5 rounded bg-surface-2/60 dark:bg-slate-700/60 text-text-muted" {
                                            (layer)
                                        }
                                    }
                                }
                            }

                            div class="flex gap-2 mt-2" {
                                a href="#"
                                    class="text-xs text-blue-600 dark:text-blue-400 hover:underline"
                                    hx-get="/dashboard/frag/context-export"
                                    hx-target="#main-content"
                                    onclick="setActiveNav(this)" {
                                    "Use preset"
                                }
                                @if !preset.is_builtin {
                                    button
                                        class="text-xs text-red-500 hover:underline"
                                        hx-confirm=(format!("Delete preset '{}'?", preset.name))
                                        onclick=(format!(
                                            "fetch('/api/v1/presets/{}', {{method:'DELETE'}}).then(function(){{htmx.ajax('GET','/dashboard/frag/presets','#main-content')}})",
                                            support::percent_encode(&preset.name)
                                        )) {
                                        "Delete"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Create preset section
            div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                h3 class="text-sm font-semibold" { "Create Preset" }
                div class="grid grid-cols-1 md:grid-cols-2 gap-3" {
                    div class="space-y-1" {
                        label class="text-xs text-text-muted uppercase tracking-wider" { "Name" }
                        input
                            id="new-preset-name"
                            type="text"
                            placeholder="my-preset"
                            class="w-full p-2 text-sm border border-surface-3/50 rounded-lg bg-white/80 dark:bg-slate-800/80 dark:border-slate-600";
                    }
                    div class="space-y-1" {
                        label class="text-xs text-text-muted uppercase tracking-wider" { "Description" }
                        input
                            id="new-preset-desc"
                            type="text"
                            placeholder="What is this preset for?"
                            class="w-full p-2 text-sm border border-surface-3/50 rounded-lg bg-white/80 dark:bg-slate-800/80 dark:border-slate-600";
                    }
                }
                p class="text-xs text-text-secondary" {
                    "For advanced options (include/exclude layers, task templates), edit the generated TOML file in "
                    code { ".aether/presets/" }
                }
                button
                    class="px-3 py-1.5 text-xs rounded border border-surface-3/50 hover:bg-surface-2/60 dark:hover:bg-slate-700/60"
                    onclick="createPreset()" {
                    "Create"
                }

                script {
                    (maud::PreEscaped(r#"
                    function createPreset() {
                        var name = document.getElementById('new-preset-name').value.trim();
                        var desc = document.getElementById('new-preset-desc').value.trim();
                        if (!name) { alert('Please enter a preset name'); return; }
                        fetch('/api/v1/presets', {
                            method: 'POST',
                            headers: { 'Content-Type': 'application/json' },
                            body: JSON.stringify({ name: name, description: desc })
                        }).then(function() {
                            htmx.ajax('GET', '/dashboard/frag/presets', '#main-content');
                        });
                    }
                    "#))
                }
            }
        }
    })
}

fn format_tokens(n: usize) -> String {
    if n >= 1000 {
        format!("{}K", n / 1000)
    } else {
        format!("{n}")
    }
}
