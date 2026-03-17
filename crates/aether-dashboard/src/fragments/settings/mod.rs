use std::sync::Arc;

use aether_config::load_workspace_config;
use axum::extract::{Path, State};
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) mod advanced;
pub(crate) mod batch;
pub(crate) mod continuous;
pub(crate) mod coupling;
pub(crate) mod dashboard_cfg;
pub(crate) mod drift;
pub(crate) mod embeddings;
pub(crate) mod generation;
pub(crate) mod health;
pub(crate) mod helpers;
pub(crate) mod indexing;
pub(crate) mod inference;
pub(crate) mod search;
pub(crate) mod watcher;

const TABS: &[(&str, &str)] = &[
    ("inference", "Inference"),
    ("embeddings", "Embeddings"),
    ("search", "Search"),
    ("indexing", "Indexing"),
    ("dashboard", "Dashboard"),
    ("generation", "Generation"),
    ("advanced", "Advanced"),
    ("coupling", "Coupling"),
    ("drift", "Drift"),
    ("health", "Health"),
    ("batch", "Batch"),
    ("watcher", "Watcher"),
    ("continuous", "Continuous"),
];

/// Main settings page — renders the tab bar and loads the first section via HTMX.
pub(crate) async fn settings_fragment(State(_state): State<Arc<DashboardState>>) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Configuration",
                "Change AETHER's settings here. Each tab covers a different part of the system. Changes are saved to your config.toml file.",
                "Visual config editor for all .aether/config.toml sections. Changes are written with comment preservation via toml_edit. Some settings require a restart.",
                "TOML config editor — reads/writes .aether/config.toml per-section. Uses toml_edit for comment-preserving merges. Hot-reload limited to next-use semantics."
            ))

            // Tab bar
            div class="flex flex-wrap gap-1 border-b border-surface-3/40 pb-1" id="settings-tabs" {
                @for (i, (key, label)) in TABS.iter().enumerate() {
                    button
                        class={ "settings-tab px-3 py-1.5 text-sm rounded-t-md transition-colors "
                            @if i == 0 { "bg-accent-cyan/15 text-accent-cyan border-b-2 border-accent-cyan" }
                            @else { "text-text-muted hover:text-text-secondary hover:bg-surface-3/20" }
                        }
                        hx-get=(format!("/dashboard/frag/settings/{key}"))
                        hx-target="#settings-content"
                        hx-swap="innerHTML"
                        onclick="setActiveSettingsTab(this)"
                    {
                        (label)
                    }
                }
            }

            // Section content — loaded via HTMX
            div id="settings-content"
                hx-get="/dashboard/frag/settings/inference"
                hx-trigger="load"
                hx-swap="innerHTML"
            {
                div class="flex items-center justify-center py-12 text-text-muted text-sm" {
                    "Loading settings…"
                }
            }

            // Inline script for tab switching
            script {
                (maud::PreEscaped(r#"
                function setActiveSettingsTab(el) {
                    var tabs = document.querySelectorAll('#settings-tabs .settings-tab');
                    for (var i = 0; i < tabs.length; i++) {
                        tabs[i].className = 'settings-tab px-3 py-1.5 text-sm rounded-t-md transition-colors text-text-muted hover:text-text-secondary hover:bg-surface-3/20';
                    }
                    el.className = 'settings-tab px-3 py-1.5 text-sm rounded-t-md transition-colors bg-accent-cyan/15 text-accent-cyan border-b-2 border-accent-cyan';
                }
                "#))
            }
        }
    })
}

/// Individual section fragment — dispatches to the correct section renderer.
pub(crate) async fn settings_section_fragment(
    State(state): State<Arc<DashboardState>>,
    Path(section): Path<String>,
) -> Html<String> {
    let workspace = state.shared.workspace.clone();
    let config = match load_workspace_config(&workspace) {
        Ok(c) => c,
        Err(err) => {
            return support::html_markup_response(html! {
                div class="rounded-md bg-accent-red/10 border border-accent-red/30 px-4 py-3 text-sm text-accent-red" {
                    "Failed to load config: " (err)
                }
            });
        }
    };

    let markup = match section.as_str() {
        "inference" => inference::render(&config),
        "embeddings" => embeddings::render(&config),
        "search" => search::render(&config),
        "indexing" => indexing::render(&config, &workspace),
        "dashboard" => dashboard_cfg::render(&config),
        "generation" => generation::render(&config),
        "advanced" => advanced::render(&config),
        "coupling" => coupling::render(&config),
        "drift" => drift::render(&config),
        "health" => health::render(&config),
        "batch" => batch::render(&config),
        "watcher" => watcher::render(&config),
        "continuous" => continuous::render(&config),
        _ => html! {
            div class="text-text-muted text-sm py-8 text-center" {
                "Unknown settings section: " (section)
            }
        },
    };

    support::html_markup_response(markup)
}
